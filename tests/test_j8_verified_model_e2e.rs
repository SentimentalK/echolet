//! J8 opt-in, reproducible end-to-end verification of the canonical Echolet
//! Verified X-ASR (zh/en, 480ms) model pack.
//!
//! This harness proves the full product path against the *immutable* Echolet
//! GitHub Release asset: a brand-new user starts from a clean NO_MODEL state,
//! loads the shipped v2 registry, installs the frozen model through the J6
//! product downloader (`ModelManager::install_registry_model`), confirms it is
//! selectable but not auto-selected, explicitly selects it, loads the real
//! sherpa-onnx recognizer/stream from the fresh install, and runs real local
//! ASR against the repo-authoritative `0.wav` sample.
//!
//! Ordinary CI must NOT touch the network, so the real run is gated behind an
//! explicit environment variable. Run it with:
//!
//! ```sh
//! ECHOLET_J8_E2E=1 cargo test --test test_j8_verified_model_e2e -- --nocapture
//! ```
//!
//! Optional knobs:
//! * `ECHOLET_J8_TEST_WAV=/path/to/0.wav` reuses a local sample instead of
//!   downloading it; its SHA256 is still verified against the repo authority.
//!
//! There is deliberately **no** local-rebuild fallback: the install proof only
//! counts when it comes from the immutable Release URL in
//! `models/base-model.lock.json`. The product downloader has no rebuild path,
//! so asserting the entry's URL/SHA against the lock (done in
//! `tests/test_model_registry.rs`) plus this real run is the complete proof.

use echolet::asr::OnlineRecognizer;
use echolet::models::manager::ModelManager;
use echolet::models::registry::{ModelRegistry, VerificationStatus};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Repo-authoritative known test WAV (English + Chinese speech).
const TEST_WAV_URL: &str = "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20/resolve/main/test_wavs/0.wav";
/// SHA256 authority for [`TEST_WAV_URL`], matching the release asset scripts.
const TEST_WAV_SHA256: &str = "7d93384ca14702cc584a7a33fe2fed92e89e708549161cb12ea38c916882103b";

fn e2e_enabled() -> bool {
    matches!(
        std::env::var("ECHOLET_J8_E2E").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn unique_tmp_root() -> PathBuf {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("echolet-j8-e2e-{}-{}", std::process::id(), now))
}

/// Resolves the known test WAV: uses `ECHOLET_J8_TEST_WAV` if set, otherwise
/// downloads it from the repo authority. Either way the SHA256 is verified.
fn acquire_test_wav(root: &Path) -> Result<PathBuf, String> {
    let bytes = if let Ok(local) = std::env::var("ECHOLET_J8_TEST_WAV") {
        eprintln!("[J8] Reading local test WAV from {}", local);
        fs::read(&local)
            .map_err(|e| format!("Failed to read ECHOLET_J8_TEST_WAV {}: {}", local, e))?
    } else {
        eprintln!("[J8] Downloading known test WAV from {}", TEST_WAV_URL);
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(120))
            .build();
        let response = agent
            .get(TEST_WAV_URL)
            .call()
            .map_err(|e| format!("Failed to download test WAV: {}", e))?;
        let mut buf = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut buf)
            .map_err(|e| format!("Failed to read test WAV body: {}", e))?;
        buf
    };

    let observed = sha256_hex(&bytes);
    if observed != TEST_WAV_SHA256 {
        return Err(format!(
            "Test WAV SHA256 mismatch.\nExpected: {}\nGot:      {}",
            TEST_WAV_SHA256, observed
        ));
    }

    let path = root.join("0.wav");
    fs::write(&path, &bytes).map_err(|e| format!("Failed to write test WAV: {}", e))?;
    Ok(path)
}

fn decode_wav_as_f32_mono16k(path: &Path) -> Result<Vec<f32>, String> {
    let bytes = fs::read(path).map_err(|e| format!("Failed to read wav {:?}: {}", path, e))?;
    if bytes.len() <= 44 {
        return Err(format!(
            "WAV {:?} is too short to contain PCM samples",
            path
        ));
    }
    Ok(bytes[44..]
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
        .collect())
}

fn is_sane_transcript(text: &str) -> bool {
    let trimmed = text.trim();
    let visible = trimmed.chars().filter(|c| !c.is_whitespace()).count();
    visible >= 2
        && trimmed
            .chars()
            .any(|c| c.is_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(&c))
}

/// Sum of the four canonical installed model-pack files. This is the
/// deterministic definition used for `installed_size_bytes` in the registry.
fn installed_pack_size(
    model_dir: &Path,
    manifest: &echolet::models::manifest::ModelManifest,
) -> u64 {
    [
        &manifest.encoder,
        &manifest.decoder,
        &manifest.joiner,
        &manifest.tokens,
    ]
    .iter()
    .filter_map(|name| fs::metadata(model_dir.join(name)).ok())
    .map(|m| m.len())
    .sum()
}

#[test]
fn test_j8_e2e_verified_frozen_model() {
    if !e2e_enabled() {
        eprintln!("[J8] ECHOLET_J8_E2E is not set; skipping real-network verification (CI-safe).");
        return;
    }

    let result = run_e2e();
    if let Err(err) = &result {
        eprintln!("[J8] FAILED: {}", err);
    }
    assert!(
        result.is_ok(),
        "J8 end-to-end verification failed: {:?}",
        result.err()
    );
}

fn run_e2e() -> Result<(), String> {
    // ---- 2. Load the shipped v2 registry (and repo authorities) ----
    let registry = ModelRegistry::from_str(include_str!("../models/registry.json"))
        .map_err(|e| format!("shipped registry must parse: {}", e))?;
    let base: serde_json::Value = serde_json::from_str(include_str!("../models/base-model.json"))
        .map_err(|e| format!("base-model.json must parse: {}", e))?;
    let lock: serde_json::Value =
        serde_json::from_str(include_str!("../models/base-model.lock.json"))
            .map_err(|e| format!("base-model.lock.json must parse: {}", e))?;

    let entry = registry
        .default_entry()
        .ok_or("shipped registry has no default entry")?
        .clone();
    let model_id = entry.id.clone();

    // ---- 3. Confirm the frozen identity and the promoted Verified status ----
    if entry.source.url.as_deref() != lock["url"].as_str() {
        return Err(format!(
            "registry source.url ({:?}) != lock url ({:?})",
            entry.source.url, lock["url"]
        ));
    }
    if entry.source.sha256.as_deref() != lock["sha256"].as_str() {
        return Err(format!(
            "registry source.sha256 ({:?}) != lock sha256 ({:?})",
            entry.source.sha256, lock["sha256"]
        ));
    }
    if entry.source.revision.as_deref() != base["upstream_revision"].as_str() {
        return Err(format!(
            "registry source.revision ({:?}) != base upstream_revision ({:?})",
            entry.source.revision, base["upstream_revision"]
        ));
    }
    if entry.verification_status != VerificationStatus::EcholetVerified {
        return Err(format!(
            "canonical entry must be Echolet Verified after J8, got {:?}",
            entry.verification_status
        ));
    }

    eprintln!(
        "[J8] archive_url={}",
        entry.source.url.as_deref().unwrap_or("")
    );
    eprintln!(
        "[J8] expected_sha={}",
        entry.source.sha256.as_deref().unwrap_or("")
    );

    // ---- 1. Clean temporary user home with NO installed models ----
    let root = unique_tmp_root();
    let bundled = root.join("bundled");
    let user = root.join("user");
    let config = root.join("config.json");
    fs::create_dir_all(&bundled).map_err(|e| format!("mkdir bundled: {}", e))?;
    fs::create_dir_all(&user).map_err(|e| format!("mkdir user: {}", e))?;
    fs::write(
        bundled.join("registry.json"),
        include_str!("../models/registry.json"),
    )
    .map_err(|e| format!("write bundled registry: {}", e))?;

    let cleanup = |root: &Path| {
        let _ = fs::remove_dir_all(root);
    };

    let run = (|| -> Result<(), String> {
        let mut manager =
            ModelManager::new_with_paths(bundled.clone(), user.clone(), config.clone())?;
        if !manager.installed.is_empty() {
            return Err(format!(
                "expected clean NO_MODEL state, found {:?}",
                manager.installed.keys().collect::<Vec<_>>()
            ));
        }
        if manager.active_model_id().is_some() {
            return Err("expected no active model in clean state".to_string());
        }

        // ---- 4/5/6. Install through the J6 product downloader (real network) ----
        let mut advertised_total: Option<u64> = None;
        let mut observed_downloaded: u64 = 0;
        {
            let mut on_progress = |phase: echolet::models::download::InstallPhase| {
                if let echolet::models::download::InstallPhase::Downloading {
                    downloaded_bytes,
                    total_bytes,
                } = phase
                {
                    observed_downloaded = observed_downloaded.max(downloaded_bytes);
                    if let Some(total) = total_bytes {
                        advertised_total = Some(total);
                    }
                }
            };
            manager
                .install_registry_model(&model_id, Some(&mut on_progress))
                .map_err(|e| format!("product install failed: {}", e))?;
        }

        // ---- 7. Discoverable, required files + manifest validate ----
        if !manager.is_installed(&model_id) {
            return Err("installed model not registered in manager".to_string());
        }
        let installed = manager
            .get_model(&model_id)
            .ok_or("installed model missing from manager")?
            .clone();
        installed
            .manifest
            .validate_files(&installed.dir)
            .map_err(|e| format!("installed manifest validation failed: {}", e))?;
        if !installed.dir.join("model.json").exists() {
            return Err("installed model.json missing".to_string());
        }

        // ---- 8. Install must NOT auto-select ----
        if manager.active_model_id().is_some() {
            return Err(format!(
                "install must not auto-select, but active model is {:?}",
                manager.active_model_id()
            ));
        }

        // ---- 9. Explicit selection ----
        manager
            .set_active_model(&model_id)
            .map_err(|e| format!("explicit selection failed: {}", e))?;
        if manager.active_model_id() != Some(model_id.as_str()) {
            return Err("explicit selection did not take effect".to_string());
        }

        let download_size = advertised_total.unwrap_or(observed_downloaded);
        let installed_size = installed_pack_size(&installed.dir, &installed.manifest);
        eprintln!("[J8] downloaded_bytes={}", observed_downloaded);
        eprintln!("[J8] download_size_bytes={}", download_size);
        eprintln!("[J8] installed_size_bytes={}", installed_size);
        eprintln!("[J8] installed_path={}", installed.dir.display());

        // The shipped size metadata must match the real immutable archive and
        // the real installed pack; this keeps registry metadata authoritative.
        if entry.download_size_bytes != Some(download_size) {
            return Err(format!(
                "registry download_size_bytes {:?} != observed {}",
                entry.download_size_bytes, download_size
            ));
        }
        if entry.installed_size_bytes != Some(installed_size) {
            return Err(format!(
                "registry installed_size_bytes {:?} != observed {}",
                entry.installed_size_bytes, installed_size
            ));
        }

        // ---- 10/11/12. Real recognizer/stream + real inference ----
        let wav_path = acquire_test_wav(&root)?;
        let samples = decode_wav_as_f32_mono16k(&wav_path)?;

        let recognizer = Arc::new(
            OnlineRecognizer::from_manifest(&installed.dir, &installed.manifest)
                .map_err(|e| format!("recognizer init failed: {}", e))?,
        );
        let stream = recognizer
            .create_stream()
            .map_err(|e| format!("stream creation failed: {}", e))?;

        // Stream in 0.2s chunks, then flush with tail silence.
        for chunk in samples.chunks(3200) {
            stream.accept_waveform(16000, chunk);
            stream.decode_all_ready();
        }
        let tail = vec![0.0f32; 4800];
        stream.accept_waveform(16000, &tail);
        stream.decode_all_ready();

        let transcript = stream.get_result();
        eprintln!("[J8] wav_path={}", wav_path.display());
        eprintln!("[J8] wav_sha={}", TEST_WAV_SHA256);
        eprintln!("[J8] transcript={}", transcript);

        if !is_sane_transcript(&transcript) {
            return Err(format!(
                "inference produced an empty/insane transcript: {:?}",
                transcript
            ));
        }

        eprintln!("[J8] PASS: fresh NO_MODEL -> install -> select -> load -> real ASR verified");
        Ok(())
    })();

    // ---- 13. Always clean temporary user state ----
    cleanup(&root);
    run
}
