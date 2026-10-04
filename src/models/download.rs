use crate::models::manifest::ModelManifest;
use crate::models::registry::RegistryModelEntry;
use bzip2::read::BzDecoder;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tar::Archive;

/// Bounded automatic retry policy for transient network / HTTP failures.
const MAX_DOWNLOAD_ATTEMPTS: u32 = 3;
/// Linear backoff base; attempt N sleeps `N * RETRY_BACKOFF_BASE_MS`.
const RETRY_BACKOFF_BASE_MS: u64 = 250;
/// Emit a `Downloading` progress event at most once per this many bytes.
const PROGRESS_BYTE_STEP: u64 = 256 * 1024;

static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Platform-neutral progress contract for a model download/install.
///
/// This is intentionally small so it can be projected onto a future Desktop /
/// Mobile UI without pulling UI dependencies into the core crate. Bytes are
/// reported monotonically; `total_bytes` is `None` when the server did not
/// advertise a `Content-Length`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallPhase {
    Starting,
    Downloading {
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    Verifying,
    Extracting,
    Installing,
    Completed,
}

/// Callback type accepted by the install APIs.
pub type ProgressCallback<'a> = &'a mut dyn FnMut(InstallPhase);

/// Tar-based archive formats the downloader can install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    TarZstd,
    TarBzip2,
    Tar,
}

impl ArchiveFormat {
    /// Best-effort format hint derived from a URL path (query/fragment ignored).
    pub fn from_url(url: &str) -> Option<Self> {
        let path = url
            .split(['?', '#'])
            .next()
            .unwrap_or(url)
            .to_ascii_lowercase();
        if path.ends_with(".tar.zst") || path.ends_with(".tar.zstd") || path.ends_with(".tzst") {
            Some(ArchiveFormat::TarZstd)
        } else if path.ends_with(".tar.bz2") || path.ends_with(".tbz2") || path.ends_with(".tbz") {
            Some(ArchiveFormat::TarBzip2)
        } else if path.ends_with(".tar") {
            Some(ArchiveFormat::Tar)
        } else {
            None
        }
    }
}

/// Downloads and installs a registry model archive into `target_dir`.
///
/// Convenience wrapper around [`download_and_install_model_with_progress`] for
/// callers that do not need progress events.
pub fn download_and_install_model(
    entry: &RegistryModelEntry,
    target_dir: &Path,
) -> Result<ModelManifest, String> {
    download_and_install_model_with_progress(entry, target_dir, None)
}

/// Downloads, verifies, extracts, validates, and atomically installs a registry
/// model archive into `target_dir`.
///
/// Guarantees:
/// * the archive SHA256 is verified before any extraction;
/// * extraction is path-traversal safe;
/// * a valid existing model at `target_dir` is never deleted before the new
///   archive is fully staged and validated;
/// * on any failure the previously installed model is left intact and staging
///   residue is cleaned up.
pub fn download_and_install_model_with_progress(
    entry: &RegistryModelEntry,
    target_dir: &Path,
    mut progress: Option<ProgressCallback<'_>>,
) -> Result<ModelManifest, String> {
    let source_url = entry.source.url.as_deref().ok_or_else(|| {
        format!(
            "Model {} is bundled or does not have a downloadable archive URL",
            entry.id
        )
    })?;
    let expected_sha256 = entry.source.sha256.as_deref().ok_or_else(|| {
        format!(
            "Model {} does not have an archive SHA256 checksum",
            entry.id
        )
    })?;

    // Stage everything under the *same parent* as the final target so the last
    // commit step can be an atomic rename on the same filesystem.
    let target_parent = target_dir.parent().ok_or_else(|| {
        format!(
            "Install target {:?} has no parent directory to stage into",
            target_dir
        )
    })?;
    fs::create_dir_all(target_parent).map_err(|e| {
        format!(
            "Failed to create model directory {:?}: {}",
            target_parent, e
        )
    })?;

    let staging_root = target_parent.join(format!(
        ".echolet-staging-{}-{}",
        sanitize_component(&entry.id),
        unique_nonce()
    ));
    fs::create_dir_all(&staging_root)
        .map_err(|e| format!("Failed to create staging dir {:?}: {}", staging_root, e))?;

    // Removes the whole staging tree on every exit path (success or failure).
    let _guard = StagingGuard(staging_root.clone());

    let archive_path = staging_root.join("archive.download");
    let extract_dir = staging_root.join("extracted");

    emit_progress(&mut progress, InstallPhase::Starting);

    download_with_retry(source_url, &archive_path, &mut progress)?;

    // ---- Verify checksum before touching the filesystem outside staging ----
    emit_progress(&mut progress, InstallPhase::Verifying);
    let calculated_sha256 = sha256_file(&archive_path)?;
    if !calculated_sha256.eq_ignore_ascii_case(expected_sha256) {
        return Err(format!(
            "SHA256 checksum mismatch for {}.\nExpected: {}\nGot:      {}",
            entry.id, expected_sha256, calculated_sha256
        ));
    }

    // ---- Extract (path-traversal safe) ----
    emit_progress(&mut progress, InstallPhase::Extracting);
    fs::create_dir_all(&extract_dir)
        .map_err(|e| format!("Failed to create extract dir {:?}: {}", extract_dir, e))?;
    let format = detect_archive_format(&archive_path)?;
    extract_archive(&archive_path, format, &extract_dir)?;

    // ---- Locate and validate model content ----
    let model_content_dir = find_model_content_dir(&extract_dir, entry)?;
    let manifest = entry.to_manifest();
    manifest.validate_files(&model_content_dir)?;

    let manifest_path = model_content_dir.join("model.json");
    manifest.save_to_file(&manifest_path)?;

    // ---- Atomic commit ----
    emit_progress(&mut progress, InstallPhase::Installing);
    commit_install(&model_content_dir, target_dir, &staging_root)?;

    emit_progress(&mut progress, InstallPhase::Completed);
    println!(
        "[Model] Successfully installed {} to {:?}",
        entry.display_title(),
        target_dir
    );
    Ok(manifest)
}

/// Runs the download loop with a bounded retry policy. Only transient failures
/// (connection/transport errors and retryable HTTP statuses) are retried.
fn download_with_retry(
    source_url: &str,
    archive_path: &Path,
    progress: &mut Option<ProgressCallback<'_>>,
) -> Result<(), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(600))
        .build();

    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        match agent.get(source_url).call() {
            Ok(response) => {
                let total_bytes = response
                    .header("Content-Length")
                    .and_then(|v| v.trim().parse::<u64>().ok());
                match stream_response_to_file(
                    response.into_reader(),
                    archive_path,
                    total_bytes,
                    progress,
                ) {
                    Ok(()) => return Ok(()),
                    Err(err) if attempt < MAX_DOWNLOAD_ATTEMPTS => {
                        eprintln!(
                            "[Model] Download attempt {} failed ({}); retrying...",
                            attempt, err
                        );
                        backoff(attempt);
                    }
                    Err(err) => return Err(err),
                }
            }
            Err(err) => {
                let retryable = is_retryable_http_error(&err);
                if retryable && attempt < MAX_DOWNLOAD_ATTEMPTS {
                    eprintln!(
                        "[Model] Download attempt {} failed ({}); retrying...",
                        attempt, err
                    );
                    backoff(attempt);
                    continue;
                }
                return Err(format!("HTTP download failed for {}: {}", source_url, err));
            }
        }
    }
}

fn stream_response_to_file(
    mut reader: Box<dyn Read + Send + Sync + 'static>,
    archive_path: &Path,
    total_bytes: Option<u64>,
    progress: &mut Option<ProgressCallback<'_>>,
) -> Result<(), String> {
    let mut file = File::create(archive_path)
        .map_err(|e| format!("Failed to create archive file {:?}: {}", archive_path, e))?;

    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut downloaded: u64 = 0;
    let mut last_emitted: u64 = 0;

    loop {
        let bytes_read = reader
            .read(&mut buffer)
            .map_err(|e| format!("Error reading HTTP stream: {}", e))?;
        if bytes_read == 0 {
            break;
        }
        file.write_all(&buffer[..bytes_read])
            .map_err(|e| format!("Error writing archive file: {}", e))?;
        hasher.update(&buffer[..bytes_read]);
        downloaded += bytes_read as u64;
        if downloaded - last_emitted >= PROGRESS_BYTE_STEP {
            last_emitted = downloaded;
            emit_progress(
                progress,
                InstallPhase::Downloading {
                    downloaded_bytes: downloaded,
                    total_bytes,
                },
            );
        }
    }
    file.flush()
        .map_err(|e| format!("Failed to flush archive: {}", e))?;

    emit_progress(
        progress,
        InstallPhase::Downloading {
            downloaded_bytes: downloaded,
            total_bytes,
        },
    );
    Ok(())
}

fn is_retryable_http_error(err: &ureq::Error) -> bool {
    match err {
        ureq::Error::Status(code, _) => *code == 408 || *code == 429 || (500..=599).contains(code),
        ureq::Error::Transport(_) => true,
    }
}

fn backoff(attempt: u32) {
    let sleep_ms = RETRY_BACKOFF_BASE_MS.saturating_mul(attempt as u64);
    std::thread::sleep(Duration::from_millis(sleep_ms));
}

/// Deterministically detects the compression wrapper by magic bytes. This avoids
/// tying correctness to a hardcoded temporary file name.
fn detect_archive_format(path: &Path) -> Result<ArchiveFormat, String> {
    let mut file =
        File::open(path).map_err(|e| format!("Failed to open downloaded archive: {}", e))?;
    let mut header = [0u8; 262];
    let mut filled = 0usize;
    while filled < header.len() {
        let n = file
            .read(&mut header[filled..])
            .map_err(|e| format!("Failed to read archive header: {}", e))?;
        if n == 0 {
            break;
        }
        filled += n;
    }

    if filled >= 4 && header[..4] == [0x28, 0xB5, 0x2F, 0xFD] {
        return Ok(ArchiveFormat::TarZstd);
    }
    if filled >= 3 && &header[..3] == b"BZh" {
        return Ok(ArchiveFormat::TarBzip2);
    }
    if filled >= 262 && &header[257..262] == b"ustar" {
        return Ok(ArchiveFormat::Tar);
    }

    Err(format!(
        "Unrecognized or unsupported archive format for {:?} (expected .tar.zst, .tar.bz2 or .tar)",
        path
    ))
}

fn extract_archive(archive_path: &Path, format: ArchiveFormat, dest: &Path) -> Result<(), String> {
    let file = File::open(archive_path)
        .map_err(|e| format!("Failed to open downloaded archive: {}", e))?;

    match format {
        ArchiveFormat::TarZstd => {
            let decoder = zstd::stream::read::Decoder::new(file)
                .map_err(|e| format!("Failed to initialize zstd decoder: {}", e))?;
            safe_unpack(Archive::new(decoder), dest)
        }
        ArchiveFormat::TarBzip2 => {
            let decoder = BzDecoder::new(file);
            safe_unpack(Archive::new(decoder), dest)
        }
        ArchiveFormat::Tar => safe_unpack(Archive::new(file), dest),
    }
}

/// Extracts a tar stream, rejecting absolute paths and `..` components and
/// skipping symlinks / hardlinks so nothing can escape `dest`.
fn safe_unpack<R: Read>(mut archive: Archive<R>, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| format!("Failed to create extract root: {}", e))?;
    let canonical_dest = dest
        .canonicalize()
        .map_err(|e| format!("Failed to canonicalize extract root {:?}: {}", dest, e))?;

    for entry_res in archive
        .entries()
        .map_err(|e| format!("Failed to read archive entries: {}", e))?
    {
        let mut entry = entry_res.map_err(|e| format!("Failed to read archive entry: {}", e))?;
        let path = entry
            .path()
            .map_err(|e| format!("Archive entry has an invalid path: {}", e))?
            .to_path_buf();

        // Reject anything that could escape the extraction root.
        if path.is_absolute() {
            return Err(format!(
                "Refusing to extract absolute path from archive: {:?}",
                path
            ));
        }
        for component in path.components() {
            match component {
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(format!(
                        "Refusing to extract path traversal entry from archive: {:?}",
                        path
                    ));
                }
                _ => {}
            }
        }

        let out_path = canonical_dest.join(&path);
        let entry_type = entry.header().entry_type();

        if entry_type.is_dir() {
            fs::create_dir_all(&out_path)
                .map_err(|e| format!("Failed to create directory {:?}: {}", out_path, e))?;
        } else if entry_type.is_file() {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create directory {:?}: {}", parent, e))?;
            }
            let mut out_file = File::create(&out_path)
                .map_err(|e| format!("Failed to create file {:?}: {}", out_path, e))?;
            std::io::copy(&mut entry, &mut out_file)
                .map_err(|e| format!("Failed to extract file {:?}: {}", out_path, e))?;
        }
        // Symlinks, hardlinks, devices and other special entries are skipped.
    }

    Ok(())
}

/// Finds the directory inside `base` that contains the model files. Supports
/// archives with files at the root or nested under a single top-level folder.
fn find_model_content_dir(base: &Path, entry: &RegistryModelEntry) -> Result<PathBuf, String> {
    if base.join(&entry.files.tokens).exists() && base.join(&entry.files.encoder).exists() {
        return Ok(base.to_path_buf());
    }

    if let Ok(entries) = fs::read_dir(base) {
        for entry_res in entries.flatten() {
            let path = entry_res.path();
            if path.is_dir()
                && path.join(&entry.files.tokens).exists()
                && path.join(&entry.files.encoder).exists()
            {
                return Ok(path);
            }
        }
    }

    Err(format!(
        "Could not find model files ({}, {}) inside extracted archive at {:?}",
        entry.files.tokens, entry.files.encoder, base
    ))
}

/// Commits validated `content_dir` to `target_dir` with a safe swap:
/// the previous model is moved aside first, the new model is renamed into
/// place, and the previous model is restored if the rename fails.
fn commit_install(
    content_dir: &Path,
    target_dir: &Path,
    staging_root: &Path,
) -> Result<(), String> {
    let parent = target_dir
        .parent()
        .ok_or_else(|| format!("Install target {:?} has no parent directory", target_dir))?;
    let nonce = sanitize_component(
        staging_root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("stage"),
    );

    // Move an existing target aside so it can be restored on failure.
    let mut backup: Option<PathBuf> = None;
    if target_dir.exists() {
        let backup_dir = parent.join(format!(".echolet-old-{}", nonce));
        let _ = fs::remove_dir_all(&backup_dir);
        fs::rename(target_dir, &backup_dir).map_err(|e| {
            format!(
                "Failed to move existing model {:?} aside: {}",
                target_dir, e
            )
        })?;
        backup = Some(backup_dir);
    }

    match rename_dir(content_dir, target_dir) {
        Ok(()) => {
            if let Some(backup_dir) = backup {
                let _ = fs::remove_dir_all(backup_dir);
            }
            Ok(())
        }
        Err(rename_err) => {
            // Cross-device fallback: copy into a temporary sibling, then rename.
            let temp_commit = parent.join(format!(".echolet-commit-{}", nonce));
            let _ = fs::remove_dir_all(&temp_commit);
            let copy_result = copy_dir_all(content_dir, &temp_commit).and_then(|_| {
                fs::rename(&temp_commit, target_dir)
                    .map_err(|e| format!("Failed to commit model to {:?}: {}", target_dir, e))
            });

            match copy_result {
                Ok(()) => {
                    if let Some(backup_dir) = backup {
                        let _ = fs::remove_dir_all(backup_dir);
                    }
                    Ok(())
                }
                Err(copy_err) => {
                    let _ = fs::remove_dir_all(&temp_commit);
                    if let Some(backup_dir) = backup {
                        let _ = fs::rename(&backup_dir, target_dir);
                    }
                    Err(format!(
                        "Failed to install model into {:?} (rename error: {}; copy error: {})",
                        target_dir, rename_err, copy_err
                    ))
                }
            }
        }
    }
}

/// Thin rename wrapper used for the fast same-filesystem commit path.
fn rename_dir(src: &Path, dst: &Path) -> Result<(), String> {
    fs::rename(src, dst).map_err(|e| format!("rename {:?} -> {:?} failed: {}", src, dst, e))
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("Failed to create dir {:?}: {}", dst, e))?;
    for entry in fs::read_dir(src).map_err(|e| format!("Failed to read dir {:?}: {}", src, e))? {
        let entry = entry.map_err(|e| format!("Error reading entry: {}", e))?;
        let ty = entry
            .file_type()
            .map_err(|e| format!("Error getting file type: {}", e))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)
                .map_err(|e| format!("Failed to copy {:?} to {:?}: {}", src_path, dst_path, e))?;
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file =
        File::open(path).map_err(|e| format!("Failed to open archive for hashing: {}", e))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| format!("Error reading archive for hashing: {}", e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn emit_progress(progress: &mut Option<ProgressCallback<'_>>, phase: InstallPhase) {
    if let Some(callback) = progress.as_deref_mut() {
        callback(phase);
    }
}

fn sanitize_component(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn unique_nonce() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{}-{}", std::process::id(), now, counter)
}

struct StagingGuard(PathBuf);

impl Drop for StagingGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a tar containing a single entry with a raw (potentially unsafe)
    /// name that the safe `Builder::append_data` path would refuse to create.
    fn malicious_tar_bytes(entry_name: &str) -> Vec<u8> {
        let data = b"pwned";
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        {
            let old = header.as_old_mut();
            let bytes = entry_name.as_bytes();
            let n = bytes.len().min(old.name.len());
            old.name[..n].copy_from_slice(&bytes[..n]);
        }
        header.set_cksum();

        let mut tar_buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_buf);
            builder.append(&header, data.as_slice()).unwrap();
            builder.finish().unwrap();
        }
        tar_buf
    }

    #[test]
    fn rejects_path_traversal_entries() {
        let tmp = std::env::temp_dir().join(format!(
            "echolet-traversal-{}-{}",
            std::process::id(),
            unique_nonce()
        ));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let archive_path = tmp.join("evil.tar");
        fs::write(&archive_path, malicious_tar_bytes("../evil.txt")).unwrap();

        let dest = tmp.join("extract");
        let err = extract_archive(&archive_path, ArchiveFormat::Tar, &dest).unwrap_err();
        assert!(
            err.contains("traversal") || err.contains("absolute"),
            "unexpected error: {}",
            err
        );
        assert!(
            !tmp.join("evil.txt").exists(),
            "traversal entry must not escape the extraction root"
        );

        let _ = fs::remove_dir_all(&tmp);
    }
}
