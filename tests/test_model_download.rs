use echolet::models::download::{ArchiveFormat, InstallPhase};
use echolet::models::manager::ModelManager;
use echolet::models::registry::{
    ModelFilesConfig, ModelRuntimeConfig, ModelSource, RegistryModelEntry, VerificationStatus,
};
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

// ---------------------------------------------------------------------------
// Deterministic local HTTP test server (no live internet required).
// ---------------------------------------------------------------------------

struct TestServer {
    addr: SocketAddr,
    hits: Arc<AtomicUsize>,
}

/// A fully-controlled HTTP response: `declared_len` is written as
/// `Content-Length` while `body` is what is actually sent. Setting
/// `declared_len > body.len()` plus connection close simulates a truncated
/// transfer that the client must observe as a read failure.
struct RawResponse {
    status: u16,
    declared_len: usize,
    body: Vec<u8>,
}

impl TestServer {
    fn start<F>(handler: F) -> Self
    where
        F: Fn(usize) -> (u16, Vec<u8>) + Send + 'static,
    {
        Self::start_raw(move |n| {
            let (status, body) = handler(n);
            RawResponse {
                status,
                declared_len: body.len(),
                body,
            }
        })
    }

    fn start_raw<F>(handler: F) -> Self
    where
        F: Fn(usize) -> RawResponse + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_thread = hits.clone();

        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = match stream {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let request_number = hits_thread.fetch_add(1, Ordering::SeqCst);
                // Drain the request headers; body is irrelevant for these tests.
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);

                let RawResponse {
                    status,
                    declared_len,
                    body,
                } = handler(request_number);
                let reason = match status {
                    200 => "OK",
                    404 => "Not Found",
                    500 => "Internal Server Error",
                    _ => "Error",
                };
                let header = format!(
                    "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                    status,
                    reason,
                    declared_len
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            }
        });

        TestServer { addr, hits }
    }

    fn serve(body: Vec<u8>) -> Self {
        Self::start(move |_| (200, body.clone()))
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_tmp(prefix: &str) -> PathBuf {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "echolet-dl-{}-{}-{}-{}",
        prefix,
        std::process::id(),
        now,
        counter
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

struct Fixture {
    tmp: PathBuf,
    bundled: PathBuf,
    user: PathBuf,
    config: PathBuf,
}

impl Fixture {
    fn new(prefix: &str) -> Self {
        let tmp = unique_tmp(prefix);
        let bundled = tmp.join("bundled");
        let user = tmp.join("user");
        let config = tmp.join("config.json");
        fs::create_dir_all(&bundled).unwrap();
        fs::create_dir_all(&user).unwrap();
        Self {
            tmp,
            bundled,
            user,
            config,
        }
    }

    fn manager(&self) -> ModelManager {
        ModelManager::new_with_paths(self.bundled.clone(), self.user.clone(), self.config.clone())
            .expect("ModelManager::new_with_paths")
    }

    fn user_model_dir(&self, id: &str) -> PathBuf {
        self.user.join(id)
    }

    fn user_dir_has_target(&self, id: &str) -> bool {
        self.user.join(id).exists()
    }

    fn has_staging_residue(&self) -> bool {
        has_entry_with_prefix(&self.user, ".echolet-staging-")
            || has_entry_with_prefix(&self.user, ".echolet-old-")
            || has_entry_with_prefix(&self.user, ".echolet-commit-")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.tmp);
    }
}

fn has_entry_with_prefix(dir: &Path, prefix: &str) -> bool {
    match fs::read_dir(dir) {
        Ok(entries) => entries.flatten().any(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.starts_with(prefix))
                .unwrap_or(false)
        }),
        Err(_) => false,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

const FILE_NAMES: [&str; 4] = [
    "encoder-480ms.onnx",
    "decoder-480ms.onnx",
    "joiner-480ms.onnx",
    "tokens.txt",
];

/// Builds the four canonical model files with a distinguishing tag embedded in
/// each file so replacement/preservation can be asserted.
fn model_files(tag: &str, root: Option<&str>) -> Vec<(String, Vec<u8>)> {
    FILE_NAMES
        .iter()
        .map(|name| {
            let path = match root {
                Some(root) => format!("{}/{}", root, name),
                None => (*name).to_string(),
            };
            (path, format!("{}::{}", name, tag).into_bytes())
        })
        .collect()
}

/// Deterministic, effectively incompressible bytes so a zstd archive stays
/// larger than the downloader's 256 KiB progress step.
fn incompressible_bytes(len: usize) -> Vec<u8> {
    let mut state: u32 = 0x1234_5678;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        out.push((state & 0xFF) as u8);
    }
    out
}

fn model_files_with_padding(tag: &str, pad: usize) -> Vec<(String, Vec<u8>)> {
    let mut files = model_files(tag, None);
    files.push(("pad.bin".to_string(), incompressible_bytes(pad)));
    files
}

fn build_tar_bytes(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut tar_buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_buf);
        for (name, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            builder
                .append_data(&mut header, name, content.as_slice())
                .unwrap();
        }
        builder.finish().unwrap();
    }
    tar_buf
}

fn build_tar_zst(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    zstd::stream::encode_all(build_tar_bytes(files).as_slice(), 3).unwrap()
}

fn build_tar_bz2(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
    encoder.write_all(&build_tar_bytes(files)).unwrap();
    encoder.finish().unwrap()
}

fn runtime_config() -> ModelRuntimeConfig {
    ModelRuntimeConfig {
        model_type: Some("zipformer2".into()),
        sample_rate: 16000,
        feature_dim: 80,
        num_threads: 1,
        provider: "cpu".into(),
        decoding_method: "greedy_search".into(),
        max_active_paths: 4,
    }
}

fn test_entry(id: &str, url: String, sha256: &str) -> RegistryModelEntry {
    RegistryModelEntry {
        id: id.to_string(),
        display_name: format!("Test Model {}", id),
        version: "2026".into(),
        languages: vec!["en".into()],
        family: "test".into(),
        source: ModelSource {
            bundled: false,
            url: Some(url),
            sha256: Some(sha256.to_string()),
            repository: Some("https://example.invalid/repo".into()),
            revision: Some("rev1".into()),
        },
        files: ModelFilesConfig {
            encoder: FILE_NAMES[0].into(),
            decoder: FILE_NAMES[1].into(),
            joiner: FILE_NAMES[2].into(),
            tokens: FILE_NAMES[3].into(),
        },
        runtime: runtime_config(),
        download_size_bytes: None,
        installed_size_bytes: None,
        upstream_release_date: None,
        license: None,
        verification_status: VerificationStatus::Experimental,
    }
}

fn register_entry(manager: &mut ModelManager, entry: RegistryModelEntry) {
    manager.registry.models.retain(|e| e.id != entry.id);
    manager.registry.models.push(entry);
}

fn assert_target_has_tag(dir: &Path, tag: &str) {
    let encoder = fs::read(dir.join(FILE_NAMES[0])).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&encoder),
        format!("{}::{}", FILE_NAMES[0], tag),
        "installed encoder content mismatch at {:?}",
        dir
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_tar_zst_happy_path_installs_and_registers() {
    let archive = build_tar_zst(&model_files("v1", Some("model-root")));
    let sha = sha256_hex(&archive);
    let server = TestServer::serve(archive);

    let fx = Fixture::new("zst-happy");
    let mut manager = fx.manager();
    let id = "test-zst-model";
    register_entry(
        &mut manager,
        test_entry(id, server.url("/model.tar.zst"), &sha),
    );

    let installed = manager
        .install_registry_model(id, None)
        .expect("zstd install must succeed");
    assert_eq!(installed.manifest.id, id);
    assert_eq!(installed.dir, fx.user_model_dir(id));

    let target = fx.user_model_dir(id);
    assert_target_has_tag(&target, "v1");
    assert!(
        target.join("model.json").exists(),
        "normalized manifest written"
    );
    assert!(
        manager.is_installed(id),
        "manager must register installed model"
    );
    assert!(
        manager.active_model_id.is_none(),
        "install must not auto-select a model"
    );
    assert!(
        !manager.downloading.contains(id),
        "downloading set must clear on success"
    );
    assert!(
        !fx.has_staging_residue(),
        "no staging residue after success"
    );
}

#[test]
fn test_tar_bz2_backward_compatible_installs() {
    let archive = build_tar_bz2(&model_files("bz2", None));
    let sha = sha256_hex(&archive);
    let server = TestServer::serve(archive);

    let fx = Fixture::new("bz2-happy");
    let mut manager = fx.manager();
    let id = "test-bz2-model";
    register_entry(
        &mut manager,
        test_entry(id, server.url("/model.tar.bz2"), &sha),
    );

    manager
        .install_registry_model(id, None)
        .expect("legacy tar.bz2 install must still succeed");
    assert_target_has_tag(&fx.user_model_dir(id), "bz2");
    assert!(!fx.has_staging_residue());
}

#[test]
fn test_bad_sha256_leaves_no_installed_target_or_residue() {
    let archive = build_tar_zst(&model_files("badsec", None));
    let wrong_sha = "0".repeat(64);
    let server = TestServer::serve(archive);

    let fx = Fixture::new("bad-sha");
    let mut manager = fx.manager();
    let id = "test-bad-sha";
    register_entry(
        &mut manager,
        test_entry(id, server.url("/m.tar.zst"), &wrong_sha),
    );

    let err = manager
        .install_registry_model(id, None)
        .expect_err("bad SHA must fail");
    assert!(err.contains("SHA256"), "unexpected error: {}", err);

    assert!(
        !fx.user_model_dir(id).exists(),
        "failed install must not create target"
    );
    assert!(
        !manager.installed.contains_key(id),
        "failed install must not register model"
    );
    assert!(
        !manager.downloading.contains(id),
        "downloading set must clear on error"
    );
    assert!(
        !fx.has_staging_residue(),
        "no staging residue after failure"
    );
}

#[test]
fn test_malformed_archive_missing_files_preserves_old_target() {
    let good = build_tar_zst(&model_files("good", Some("root")));
    let good_sha = sha256_hex(&good);
    let server_good = TestServer::serve(good);

    let fx = Fixture::new("malformed");
    let mut manager = fx.manager();
    let id = "test-malformed-model";

    register_entry(
        &mut manager,
        test_entry(id, server_good.url("/good.tar.zst"), &good_sha),
    );
    manager
        .install_registry_model(id, None)
        .expect("initial good install");

    let target = fx.user_model_dir(id);
    assert_target_has_tag(&target, "good");
    // Marker proves the old target directory identity is preserved.
    fs::write(target.join("marker.txt"), b"keep-me").unwrap();

    // Archive with encoder + decoder + tokens but missing joiner, correct SHA.
    // This is a valid layout that must fail required-file validation.
    let partial: Vec<(String, Vec<u8>)> = [0usize, 1, 3]
        .iter()
        .map(|i| {
            (
                FILE_NAMES[*i].to_string(),
                format!("{}::partial", FILE_NAMES[*i]).into_bytes(),
            )
        })
        .collect();
    let bad = build_tar_zst(&partial);
    let bad_sha = sha256_hex(&bad);
    let server_bad = TestServer::serve(bad);

    register_entry(
        &mut manager,
        test_entry(id, server_bad.url("/partial.tar.zst"), &bad_sha),
    );

    let err = manager
        .install_registry_model(id, None)
        .expect_err("malformed archive must fail validation");
    assert!(
        err.contains("missing") || err.contains("incomplete"),
        "unexpected validation error: {}",
        err
    );

    assert!(
        target.join("marker.txt").exists(),
        "old target must be intact"
    );
    assert_target_has_tag(&target, "good");
    assert!(
        !fx.has_staging_residue(),
        "no staging residue after failure"
    );
}

#[test]
fn test_failed_replacement_keeps_old_target_intact() {
    let good = build_tar_zst(&model_files("keep", Some("root")));
    let good_sha = sha256_hex(&good);
    let server_good = TestServer::serve(good.clone());

    let fx = Fixture::new("replace-fail");
    let mut manager = fx.manager();
    let id = "test-replace-model";

    register_entry(
        &mut manager,
        test_entry(id, server_good.url("/good.tar.zst"), &good_sha),
    );
    manager.install_registry_model(id, None).unwrap();

    let target = fx.user_model_dir(id);
    fs::write(target.join("marker.txt"), b"old-model").unwrap();

    // New archive with valid bytes but a declared checksum that cannot match.
    let server_bad = TestServer::serve(good);
    register_entry(
        &mut manager,
        test_entry(id, server_bad.url("/new.tar.zst"), &"f".repeat(64)),
    );

    let err = manager
        .install_registry_model(id, None)
        .expect_err("replacement with bad SHA must fail");
    assert!(err.contains("SHA256"), "unexpected error: {}", err);

    assert!(target.join("marker.txt").exists());
    assert_target_has_tag(&target, "keep");
    assert!(!fx.has_staging_residue());
}

#[test]
fn test_successful_replacement_is_atomic() {
    let old = build_tar_zst(&model_files("old", Some("root")));
    let old_sha = sha256_hex(&old);
    let server_old = TestServer::serve(old);

    let fx = Fixture::new("replace-ok");
    let mut manager = fx.manager();
    let id = "test-replace-ok";

    register_entry(
        &mut manager,
        test_entry(id, server_old.url("/old.tar.zst"), &old_sha),
    );
    manager.install_registry_model(id, None).unwrap();
    let target = fx.user_model_dir(id);
    assert_target_has_tag(&target, "old");
    fs::write(target.join("marker.txt"), b"old-marker").unwrap();

    let new = build_tar_zst(&model_files("new", Some("root")));
    let new_sha = sha256_hex(&new);
    let server_new = TestServer::serve(new);
    register_entry(
        &mut manager,
        test_entry(id, server_new.url("/new.tar.zst"), &new_sha),
    );

    manager
        .install_registry_model(id, None)
        .expect("replacement install must succeed");

    assert_target_has_tag(&target, "new");
    assert!(
        !target.join("marker.txt").exists(),
        "old target contents must be fully replaced"
    );
    assert!(!fx.has_staging_residue(), "staging/backup must be cleaned");
}

#[test]
fn test_progress_phases_are_monotonic_and_ordered() {
    let archive = build_tar_zst(&model_files("progress", None));
    let sha = sha256_hex(&archive);
    let server = TestServer::serve(archive);

    let fx = Fixture::new("progress");
    let mut manager = fx.manager();
    let id = "test-progress-model";
    register_entry(&mut manager, test_entry(id, server.url("/m.tar.zst"), &sha));

    let mut phases: Vec<InstallPhase> = Vec::new();
    manager
        .install_registry_model(id, Some(&mut |p| phases.push(p)))
        .expect("install with progress must succeed");

    assert_eq!(
        phases.first(),
        Some(&InstallPhase::Starting),
        "first phase must be Starting"
    );
    assert_eq!(
        phases.last(),
        Some(&InstallPhase::Completed),
        "last phase must be Completed"
    );

    let index_of = |needle: &InstallPhase| phases.iter().position(|p| p == needle);
    let verifying = index_of(&InstallPhase::Verifying).expect("Verifying phase");
    let extracting = index_of(&InstallPhase::Extracting).expect("Extracting phase");
    let installing = index_of(&InstallPhase::Installing).expect("Installing phase");
    assert!(verifying < extracting, "Verifying must precede Extracting");
    assert!(
        extracting < installing,
        "Extracting must precede Installing"
    );
    assert!(
        phases
            .iter()
            .any(|p| matches!(p, InstallPhase::Downloading { .. })),
        "at least one Downloading phase expected"
    );

    // Downloaded byte counts must be non-decreasing.
    let mut last = 0u64;
    for phase in &phases {
        if let InstallPhase::Downloading {
            downloaded_bytes, ..
        } = phase
        {
            assert!(
                *downloaded_bytes >= last,
                "downloaded bytes must be monotonic"
            );
            last = *downloaded_bytes;
        }
    }
}

#[test]
fn test_partial_transfer_retry_keeps_progress_monotonic() {
    // Archive must comfortably exceed one progress step (256 KiB) so the first
    // attempt emits at least one Downloading event before it fails.
    let files = model_files_with_padding("partial-retry", 400 * 1024);
    let archive = build_tar_zst(&files);
    assert!(
        archive.len() > 300 * 1024,
        "test archive too small to exercise incremental progress: {}",
        archive.len()
    );
    let declared = archive.len();
    let sha = sha256_hex(&archive);

    let full = archive.clone();
    let cut = 300 * 1024;
    assert!(cut < declared);
    // First request: declare the full length but send only a 300 KiB prefix and
    // close. The client reads >= one progress step, then hits a read error and
    // retries. Subsequent requests serve the full archive.
    let server = TestServer::start_raw(move |attempt| {
        if attempt == 0 {
            RawResponse {
                status: 200,
                declared_len: declared,
                body: full[..cut].to_vec(),
            }
        } else {
            RawResponse {
                status: 200,
                declared_len: full.len(),
                body: full.clone(),
            }
        }
    });

    let fx = Fixture::new("partial-retry");
    let mut manager = fx.manager();
    let id = "test-partial-retry";
    register_entry(&mut manager, test_entry(id, server.url("/m.tar.zst"), &sha));

    let mut phases: Vec<InstallPhase> = Vec::new();
    manager
        .install_registry_model(id, Some(&mut |p| phases.push(p)))
        .expect("retry after a partial transfer must succeed");

    assert!(server.hits() >= 2, "server must observe a retry");

    let mut last = 0u64;
    let mut downloading_events = 0usize;
    for phase in &phases {
        if let InstallPhase::Downloading {
            downloaded_bytes,
            total_bytes,
        } = phase
        {
            assert!(
                *downloaded_bytes >= last,
                "downloaded bytes must be monotonic across retries: {:?}",
                phases
            );
            last = *downloaded_bytes;
            downloading_events += 1;
            if let Some(total) = total_bytes {
                assert_eq!(
                    *total, declared as u64,
                    "total_bytes must stay the expected final object size"
                );
            }
        }
    }
    assert!(
        downloading_events > 0,
        "a partial transfer must emit progress before failing"
    );

    assert_target_has_tag(&fx.user_model_dir(id), "partial-retry");
    assert!(!fx.has_staging_residue());
}

#[test]
fn test_downloading_set_tracks_success_and_error() {
    let archive = build_tar_zst(&model_files("dl-state", None));
    let sha = sha256_hex(&archive);
    let server = TestServer::serve(archive);

    let fx = Fixture::new("dl-state");
    let mut manager = fx.manager();
    let ok_id = "test-dl-ok";
    register_entry(
        &mut manager,
        test_entry(ok_id, server.url("/ok.tar.zst"), &sha),
    );
    manager.install_registry_model(ok_id, None).unwrap();
    assert!(!manager.downloading.contains(ok_id));

    let err_id = "test-dl-err";
    register_entry(
        &mut manager,
        test_entry(err_id, server.url("/err.tar.zst"), &"a".repeat(64)),
    );
    assert!(manager.install_registry_model(err_id, None).is_err());
    assert!(
        !manager.downloading.contains(err_id),
        "downloading must clear on error"
    );
}

#[test]
fn test_duplicate_download_is_rejected() {
    let archive = build_tar_zst(&model_files("dup", None));
    let sha = sha256_hex(&archive);
    let server = TestServer::serve(archive);

    let fx = Fixture::new("duplicate");
    let mut manager = fx.manager();
    let id = "test-dup-model";
    register_entry(&mut manager, test_entry(id, server.url("/m.tar.zst"), &sha));

    // Simulate an in-flight download for the same model.
    manager.downloading.insert(id.to_string());

    let err = manager
        .install_registry_model(id, None)
        .expect_err("duplicate concurrent download must be rejected");
    assert!(
        err.contains("already downloading"),
        "unexpected error: {}",
        err
    );
}

#[test]
fn test_install_does_not_auto_select_and_preserves_existing_active() {
    let archive_a = build_tar_zst(&model_files("a", None));
    let sha_a = sha256_hex(&archive_a);
    let server_a = TestServer::serve(archive_a);

    let fx = Fixture::new("no-auto-select");
    let mut manager = fx.manager();
    let id_a = "test-no-select-a";
    register_entry(
        &mut manager,
        test_entry(id_a, server_a.url("/a.tar.zst"), &sha_a),
    );
    manager.install_registry_model(id_a, None).unwrap();
    assert!(
        manager.active_model_id.is_none(),
        "first install must not auto-select"
    );

    manager.set_active_model(id_a).expect("explicit select");
    assert_eq!(manager.active_model_id.as_deref(), Some(id_a));

    let archive_b = build_tar_zst(&model_files("b", None));
    let sha_b = sha256_hex(&archive_b);
    let server_b = TestServer::serve(archive_b);
    let id_b = "test-no-select-b";
    register_entry(
        &mut manager,
        test_entry(id_b, server_b.url("/b.tar.zst"), &sha_b),
    );
    manager.install_registry_model(id_b, None).unwrap();

    assert_eq!(
        manager.active_model_id.as_deref(),
        Some(id_a),
        "installing another model must not change the active selection"
    );
}

#[test]
fn test_manager_discovers_and_registers_new_model_after_success() {
    let archive = build_tar_zst(&model_files("discover", Some("root")));
    let sha = sha256_hex(&archive);
    let server = TestServer::serve(archive);

    let fx = Fixture::new("discover");
    let mut manager = fx.manager();
    let id = "test-discover-model";
    register_entry(&mut manager, test_entry(id, server.url("/m.tar.zst"), &sha));
    manager.install_registry_model(id, None).unwrap();

    assert!(manager.is_installed(id), "in-memory registration");

    // A fresh manager scanning the same user dir must discover the model.
    let fresh = fx.manager();
    assert!(
        fresh.is_installed(id),
        "fresh manager must discover installed model from disk"
    );
    assert_eq!(
        fresh.get_model(id).unwrap().manifest.display_name,
        format!("Test Model {}", id)
    );
}

#[test]
fn test_registry_download_metadata_matches_frozen_base_model_lock() {
    let registry: serde_json::Value =
        serde_json::from_str(include_str!("../models/registry.json")).unwrap();
    let lock: serde_json::Value =
        serde_json::from_str(include_str!("../models/base-model.lock.json")).unwrap();

    let default_id = registry["default_model_id"].as_str().unwrap();
    let entry = registry["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"].as_str() == Some(default_id))
        .expect("registry must contain the default X-ASR entry");

    assert_eq!(
        entry["source"]["url"].as_str().unwrap(),
        lock["url"].as_str().unwrap(),
        "registry source.url must match frozen base-model lock"
    );
    assert_eq!(
        entry["source"]["sha256"].as_str().unwrap(),
        lock["sha256"].as_str().unwrap(),
        "registry source.sha256 must match frozen base-model lock"
    );
    // Provenance must remain present.
    assert!(entry["source"]["repository"].as_str().is_some());
    assert!(entry["source"]["revision"].as_str().is_some());
}

#[test]
fn test_archive_format_dispatch_from_url() {
    assert_eq!(
        ArchiveFormat::from_url("https://x/model.tar.zst?token=abc"),
        Some(ArchiveFormat::TarZstd)
    );
    assert_eq!(
        ArchiveFormat::from_url("https://x/model.tar.bz2"),
        Some(ArchiveFormat::TarBzip2)
    );
    assert_eq!(
        ArchiveFormat::from_url("https://x/model.tar"),
        Some(ArchiveFormat::Tar)
    );
    assert_eq!(ArchiveFormat::from_url("https://x/model.zip"), None);
}

#[test]
fn test_transient_http_failure_is_retried_and_then_succeeds() {
    let archive = build_tar_zst(&model_files("retry", None));
    let sha = sha256_hex(&archive);
    let archive_for_server = archive.clone();
    let server = TestServer::start(move |attempt| {
        if attempt < 2 {
            (500, b"transient".to_vec())
        } else {
            (200, archive_for_server.clone())
        }
    });

    let fx = Fixture::new("retry");
    let mut manager = fx.manager();
    let id = "test-retry-model";
    register_entry(&mut manager, test_entry(id, server.url("/m.tar.zst"), &sha));

    manager
        .install_registry_model(id, None)
        .expect("transient failure should be retried to success");
    assert!(server.hits() >= 3, "server should observe retries");
    assert_target_has_tag(&fx.user_model_dir(id), "retry");
}

#[test]
fn test_non_success_http_status_returns_clear_error() {
    let server = TestServer::start(|_| (404, b"nope".to_vec()));

    let fx = Fixture::new("http-404");
    let mut manager = fx.manager();
    let id = "test-404-model";
    register_entry(
        &mut manager,
        test_entry(id, server.url("/missing.tar.zst"), &"b".repeat(64)),
    );

    let err = manager
        .install_registry_model(id, None)
        .expect_err("404 must fail");
    assert!(err.contains("HTTP"), "unexpected error: {}", err);
    assert!(!fx.user_dir_has_target(id));
    assert!(!fx.has_staging_residue());
}
