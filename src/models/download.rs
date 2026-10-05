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
/// Mobile UI without pulling UI dependencies into the core crate.
///
/// Byte progress is **globally monotonic across retries**:
/// `downloaded_bytes` is the cumulative number of bytes transferred for this
/// install, including bytes transferred by attempts that later failed. After a
/// partial transfer is retried it therefore continues from the previous value
/// instead of resetting to zero, and it may exceed `total_bytes` if one or more
/// retries occurred. `total_bytes` is the expected final object size (the
/// server's advertised `Content-Length`), or `None` when the server did not
/// advertise one; UI code should treat it as a target rather than a hard cap.
/// Consumers can divide `downloaded_bytes` by `total_bytes` only under the
/// assumption of no retries.
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

/// Coarse, platform-neutral projection of the download/install lifecycle for UI
/// surfaces. Unlike [`InstallPhase`] this is cheap to clone and carries the
/// terminal `Failed` state a tray/status row needs; it is the single shape the
/// app thread stores and the platform UI renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadStatus {
    /// No download is in flight for this model.
    NotDownloading,
    Starting,
    Downloading {
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    Verifying,
    Extracting,
    Installing,
    Completed,
    /// The download/install ended in a failure and may be retried.
    Failed,
}

impl DownloadStatus {
    /// Projects a raw downloader phase into the UI status type.
    pub fn from_phase(phase: &InstallPhase) -> Self {
        match phase {
            InstallPhase::Starting => DownloadStatus::Starting,
            InstallPhase::Downloading {
                downloaded_bytes,
                total_bytes,
            } => DownloadStatus::Downloading {
                downloaded_bytes: *downloaded_bytes,
                total_bytes: *total_bytes,
            },
            InstallPhase::Verifying => DownloadStatus::Verifying,
            InstallPhase::Extracting => DownloadStatus::Extracting,
            InstallPhase::Installing => DownloadStatus::Installing,
            InstallPhase::Completed => DownloadStatus::Completed,
        }
    }

    /// Whether a download is actively in progress (used to disable conflicting
    /// UI actions such as selecting another model or starting a duplicate
    /// download).
    pub fn is_in_progress(&self) -> bool {
        matches!(
            self,
            DownloadStatus::Starting
                | DownloadStatus::Downloading { .. }
                | DownloadStatus::Verifying
                | DownloadStatus::Extracting
                | DownloadStatus::Installing
        )
    }
}

/// Coalesces raw downloader progress into bounded UI updates.
///
/// The downloader already emits `Downloading` at most once per
/// [`PROGRESS_BYTE_STEP`] plus the terminal event of each attempt; this filter
/// further reduces that to at most one update per whole percent (when the total
/// size is known) or per fixed byte bucket (when it is not), and passes through
/// every genuine phase transition exactly once. It is deliberately pure and
/// deterministic so the coalescing contract can be unit tested without a
/// network or clock.
#[derive(Debug, Default)]
pub struct ProgressThrottle {
    last: Option<DownloadStatus>,
}

/// Granularity of coalesced progress when the total size is unknown.
const PROGRESS_COALESCE_UNKNOWN_BYTES: u64 = 1024 * 1024;

impl ProgressThrottle {
    pub fn new() -> Self {
        Self { last: None }
    }

    /// Returns `true` when `status` should be projected to the UI, updating the
    /// internal last-emitted marker only when it does.
    pub fn should_emit(&mut self, status: DownloadStatus) -> bool {
        let emit = match &self.last {
            None => true,
            Some(prev) => progress_identity(prev) != progress_identity(&status),
        };
        if emit {
            self.last = Some(status);
        }
        emit
    }
}

/// A comparable identity for progress coalescing. `Downloading` collapses to a
/// percent (known total) or a coarse byte bucket (unknown total), so two
/// adjacent byte-level events within the same bucket compare equal.
fn progress_identity(status: &DownloadStatus) -> (u8, u64, Option<u8>) {
    match status {
        DownloadStatus::Downloading {
            downloaded_bytes,
            total_bytes,
        } => {
            let percent = total_bytes
                .filter(|t| *t > 0)
                .map(|t| (downloaded_bytes.saturating_mul(100) / t).min(100) as u8);
            let bucket = match percent {
                Some(_) => 0,
                None => downloaded_bytes / PROGRESS_COALESCE_UNKNOWN_BYTES,
            };
            (1, bucket, percent)
        }
        DownloadStatus::NotDownloading => (0, 0, None),
        DownloadStatus::Starting => (2, 0, None),
        DownloadStatus::Verifying => (3, 0, None),
        DownloadStatus::Extracting => (4, 0, None),
        DownloadStatus::Installing => (5, 0, None),
        DownloadStatus::Completed => (6, 0, None),
        DownloadStatus::Failed => (7, 0, None),
    }
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

/// Failure classification for a single download attempt.
///
/// Only [`DownloadError::Transient`] failures (HTTP transport / response-body
/// read errors) are retried. Local filesystem failures are surfaced directly so
/// that a broken disk, permissions problem, or full filesystem is not masked by
/// two pointless network retries.
enum DownloadError {
    /// Network/transport or response-body read failure; safe to retry.
    Transient(String),
    /// Local filesystem failure (create/write/flush); not retryable.
    Local(String),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::Transient(msg) | DownloadError::Local(msg) => f.write_str(msg),
        }
    }
}

/// Runs the download loop with a bounded retry policy. Only transient failures
/// (connection/transport errors and retryable HTTP statuses) are retried.
///
/// Byte progress is cumulative across attempts so the public
/// [`InstallPhase::Downloading`] contract stays monotonic even when an attempt
/// fails after emitting progress and is retried.
fn download_with_retry(
    source_url: &str,
    archive_path: &Path,
    progress: &mut Option<ProgressCallback<'_>>,
) -> Result<(), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(600))
        .build();

    let mut attempt: u32 = 0;
    // Cumulative bytes transferred across every attempt. Handed to
    // `stream_response_to_file` so a retry cannot regress reported progress.
    let mut cumulative_downloaded: u64 = 0;
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
                    &mut cumulative_downloaded,
                    progress,
                ) {
                    Ok(()) => return Ok(()),
                    // Local disk errors must not be retried as if they were network errors.
                    Err(DownloadError::Local(err)) => return Err(err),
                    Err(DownloadError::Transient(err)) if attempt < MAX_DOWNLOAD_ATTEMPTS => {
                        eprintln!(
                            "[Model] Download attempt {} failed ({}); retrying...",
                            attempt, err
                        );
                        backoff(attempt);
                    }
                    Err(DownloadError::Transient(err)) => return Err(err),
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

/// Streams one HTTP response body into `archive_path`.
///
/// `cumulative_downloaded` is owned by the retry loop and carried across
/// attempts; this function adds the bytes it transfers to it and emits
/// `Downloading` events from the cumulative total, so a retried partial
/// transfer never reports a smaller byte count. Each attempt truncates and
/// rewrites `archive_path` from scratch, but the progress counter is not reset.
fn stream_response_to_file(
    mut reader: Box<dyn Read + Send + Sync + 'static>,
    archive_path: &Path,
    total_bytes: Option<u64>,
    cumulative_downloaded: &mut u64,
    progress: &mut Option<ProgressCallback<'_>>,
) -> Result<(), DownloadError> {
    let mut file = File::create(archive_path).map_err(|e| {
        DownloadError::Local(format!(
            "Failed to create archive file {:?}: {}",
            archive_path, e
        ))
    })?;

    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    // Emit every PROGRESS_BYTE_STEP of *cumulative* progress. Seeding from the
    // current cumulative value means a retry does not immediately re-emit (or
    // regress) the last event of the previous attempt.
    let mut last_emitted: u64 = *cumulative_downloaded;

    loop {
        let bytes_read = reader
            .read(&mut buffer)
            .map_err(|e| DownloadError::Transient(format!("Error reading HTTP stream: {}", e)))?;
        if bytes_read == 0 {
            break;
        }
        file.write_all(&buffer[..bytes_read])
            .map_err(|e| DownloadError::Local(format!("Error writing archive file: {}", e)))?;
        hasher.update(&buffer[..bytes_read]);
        *cumulative_downloaded += bytes_read as u64;
        if *cumulative_downloaded - last_emitted >= PROGRESS_BYTE_STEP {
            last_emitted = *cumulative_downloaded;
            emit_progress(
                progress,
                InstallPhase::Downloading {
                    downloaded_bytes: *cumulative_downloaded,
                    total_bytes,
                },
            );
        }
    }
    file.flush()
        .map_err(|e| DownloadError::Local(format!("Failed to flush archive: {}", e)))?;

    emit_progress(
        progress,
        InstallPhase::Downloading {
            downloaded_bytes: *cumulative_downloaded,
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

/// Filesystem operations used by the commit/swap step.
///
/// This is a deliberately tiny seam: production uses [`RealCommitFs`] while
/// tests inject failures for the rename/copy stages without a heavyweight
/// filesystem abstraction. Keeping it this small avoids leaking test-only
/// concepts into the public API.
trait CommitFs {
    fn rename(&self, from: &Path, to: &Path) -> Result<(), String>;
    fn copy_dir(&self, from: &Path, to: &Path) -> Result<(), String>;
}

/// Production [`CommitFs`] backed by the real filesystem.
struct RealCommitFs;

impl CommitFs for RealCommitFs {
    fn rename(&self, from: &Path, to: &Path) -> Result<(), String> {
        rename_dir(from, to)
    }

    fn copy_dir(&self, from: &Path, to: &Path) -> Result<(), String> {
        copy_dir_all(from, to)
    }
}

/// Commits validated `content_dir` to `target_dir` with a safe swap:
/// the previous model is moved aside first, the new model is renamed into
/// place, and the previous model is restored if the rename fails.
fn commit_install(
    content_dir: &Path,
    target_dir: &Path,
    staging_root: &Path,
) -> Result<(), String> {
    commit_install_with(content_dir, target_dir, staging_root, &RealCommitFs)
}

/// Testable implementation of [`commit_install`] with injectable filesystem
/// operations.
///
/// Failure-safety contract:
/// * if the new commit fails and the old target can be restored, the returned
///   error reports the install failure and the old model is back in place;
/// * if the restore itself also fails, the returned error is an explicit
///   high-severity error reporting **both** failures, naming the backup path,
///   and the backup directory is left on disk for manual recovery (it is never
///   deleted on restore failure).
fn commit_install_with(
    content_dir: &Path,
    target_dir: &Path,
    staging_root: &Path,
    fs_ops: &dyn CommitFs,
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
        fs_ops.rename(target_dir, &backup_dir).map_err(|e| {
            format!(
                "Failed to move existing model {:?} aside: {}",
                target_dir, e
            )
        })?;
        backup = Some(backup_dir);
    }

    // Fast path: same-filesystem rename of the fully staged, validated content.
    let commit_result = match fs_ops.rename(content_dir, target_dir) {
        Ok(()) => Ok(()),
        Err(rename_err) => {
            // Cross-device fallback: copy into a temporary sibling, then rename.
            let temp_commit = parent.join(format!(".echolet-commit-{}", nonce));
            let _ = fs::remove_dir_all(&temp_commit);
            match fs_ops
                .copy_dir(content_dir, &temp_commit)
                .and_then(|_| fs_ops.rename(&temp_commit, target_dir))
            {
                Ok(()) => Ok(()),
                Err(copy_err) => {
                    let _ = fs::remove_dir_all(&temp_commit);
                    Err(format!(
                        "rename error: {}; copy error: {}",
                        rename_err, copy_err
                    ))
                }
            }
        }
    };

    match commit_result {
        Ok(()) => {
            // Replacement succeeded: the old model is no longer needed.
            if let Some(backup_dir) = backup {
                let _ = fs::remove_dir_all(backup_dir);
            }
            Ok(())
        }
        Err(commit_err) => {
            // Commit failed after the old target was moved aside. Try to put it back.
            if let Some(backup_dir) = backup {
                match fs_ops.rename(&backup_dir, target_dir) {
                    Ok(()) => Err(format!(
                        "Failed to install model into {:?} ({}); previous model was restored",
                        target_dir, commit_err
                    )),
                    Err(restore_err) => Err(format!(
                        "CRITICAL: failed to install model into {:?} AND failed to restore the previous model from backup {:?}. Install error: {}. Restore error: {}. The previous model is preserved at the backup path for manual recovery.",
                        target_dir, backup_dir, commit_err, restore_err
                    )),
                }
            } else {
                Err(format!(
                    "Failed to install model into {:?} ({})",
                    target_dir, commit_err
                ))
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

    use std::net::TcpListener;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use std::thread;

    fn unique_tmp_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("echolet-{}-{}", prefix, unique_nonce()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(path: &Path, contents: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn has_prefix(dir: &Path, prefix: &str) -> bool {
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

    fn find_prefix(dir: &Path, prefix: &str) -> Option<PathBuf> {
        fs::read_dir(dir).ok()?.flatten().find_map(|e| {
            let name = e.file_name();
            if name.to_str().map(|n| n.starts_with(prefix)) == Some(true) {
                Some(e.path())
            } else {
                None
            }
        })
    }

    /// Injectable [`CommitFs`] for forcing failures in the commit/swap stages.
    struct ControlledFs {
        content_dir: PathBuf,
        fail_commit_rename: bool,
        fail_copy: bool,
        fail_restore_rename: bool,
    }

    impl CommitFs for ControlledFs {
        fn rename(&self, from: &Path, to: &Path) -> Result<(), String> {
            if self.fail_commit_rename && from == self.content_dir {
                return Err("simulated commit rename failure".to_string());
            }
            let is_backup = from
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(".echolet-old-"))
                .unwrap_or(false);
            if self.fail_restore_rename && is_backup {
                return Err("simulated restore rename failure".to_string());
            }
            RealCommitFs.rename(from, to)
        }

        fn copy_dir(&self, from: &Path, to: &Path) -> Result<(), String> {
            if self.fail_copy {
                return Err("simulated copy failure".to_string());
            }
            RealCommitFs.copy_dir(from, to)
        }
    }

    #[test]
    fn commit_failure_restores_old_target_and_cleans_temp() {
        let tmp = unique_tmp_dir("commit-restore");
        let parent = tmp.join("models");
        fs::create_dir_all(&parent).unwrap();
        let target = parent.join("m");
        let content = tmp.join("staged-content");
        let staging = tmp.join(".echolet-staging-x");
        write_file(&target.join("model.txt"), b"old");
        write_file(&content.join("model.txt"), b"new");

        let ops = ControlledFs {
            content_dir: content.clone(),
            fail_commit_rename: true,
            fail_copy: true,
            fail_restore_rename: false,
        };
        let err = commit_install_with(&content, &target, &staging, &ops).unwrap_err();
        assert!(
            err.contains("previous model was restored"),
            "unexpected error: {}",
            err
        );
        assert_eq!(fs::read(target.join("model.txt")).unwrap(), b"old");
        assert!(
            !has_prefix(&parent, ".echolet-old-"),
            "backup must be consumed by a successful restore"
        );
        assert!(
            !has_prefix(&parent, ".echolet-commit-"),
            "temp commit dir must be cleaned"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn restore_failure_is_reported_and_backup_preserved() {
        let tmp = unique_tmp_dir("commit-restore-fail");
        let parent = tmp.join("models");
        fs::create_dir_all(&parent).unwrap();
        let target = parent.join("m");
        let content = tmp.join("staged-content");
        let staging = tmp.join(".echolet-staging-x");
        write_file(&target.join("model.txt"), b"old");
        write_file(&content.join("model.txt"), b"new");

        let ops = ControlledFs {
            content_dir: content.clone(),
            fail_commit_rename: true,
            fail_copy: true,
            fail_restore_rename: true,
        };
        let err = commit_install_with(&content, &target, &staging, &ops).unwrap_err();
        assert!(err.contains("CRITICAL"), "unexpected error: {}", err);
        assert!(
            err.contains("restore") && err.contains("simulated restore rename failure"),
            "restore failure must be surfaced: {}",
            err
        );
        assert!(
            !target.exists(),
            "restore failed so the target must not be claimed as intact"
        );
        let backup = find_prefix(&parent, ".echolet-old-").expect("backup must be preserved");
        assert_eq!(fs::read(backup.join("model.txt")).unwrap(), b"old");
        // The path is formatted with `{:?}`, which escapes separators on
        // Windows, so assert on the separator-free backup directory name.
        let backup_name = backup.file_name().unwrap().to_str().unwrap();
        assert!(
            err.contains(backup_name),
            "error must name the preserved backup path ({}): {}",
            backup_name,
            err
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn successful_commit_cleans_backup_and_leaves_siblings_untouched() {
        let tmp = unique_tmp_dir("commit-ok");
        let parent = tmp.join("models");
        fs::create_dir_all(&parent).unwrap();
        let target = parent.join("m");
        let content = tmp.join("staged-content");
        let staging = tmp.join(".echolet-staging-x");
        let sibling = parent.join("unrelated");
        write_file(&target.join("model.txt"), b"old");
        write_file(&content.join("model.txt"), b"new");
        write_file(&sibling.join("keep.txt"), b"keep");

        let ops = ControlledFs {
            content_dir: content.clone(),
            fail_commit_rename: false,
            fail_copy: false,
            fail_restore_rename: false,
        };
        commit_install_with(&content, &target, &staging, &ops).expect("commit must succeed");

        assert_eq!(fs::read(target.join("model.txt")).unwrap(), b"new");
        assert!(!has_prefix(&parent, ".echolet-old-"));
        assert!(
            !has_prefix(&parent, ".echolet-commit-"),
            "temp commit dir must be cleaned"
        );
        assert_eq!(
            fs::read(sibling.join("keep.txt")).unwrap(),
            b"keep",
            "unrelated sibling directory must not be touched"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn local_file_creation_failure_is_classified_local() {
        let archive = std::env::temp_dir().join(format!(
            "echolet-missing-parent-{}/archive.download",
            unique_nonce()
        ));
        let mut progress: Option<ProgressCallback<'_>> = None;
        let reader: Box<dyn Read + Send + Sync + 'static> =
            Box::new(std::io::Cursor::new(vec![1u8, 2, 3]));
        let mut cumulative = 0u64;
        let err =
            stream_response_to_file(reader, &archive, Some(3), &mut cumulative, &mut progress)
                .unwrap_err();
        assert!(
            matches!(err, DownloadError::Local(_)),
            "filesystem error must be classified Local"
        );
    }

    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "simulated reset",
            ))
        }
    }

    #[test]
    fn stream_read_failure_is_classified_transient() {
        let tmp = unique_tmp_dir("transient-classify");
        let archive = tmp.join("archive.download");
        let mut progress: Option<ProgressCallback<'_>> = None;
        let reader: Box<dyn Read + Send + Sync + 'static> = Box::new(FailingReader);
        let mut cumulative = 0u64;
        let err = stream_response_to_file(reader, &archive, None, &mut cumulative, &mut progress)
            .unwrap_err();
        assert!(
            matches!(err, DownloadError::Transient(_)),
            "stream read error must stay retryable"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Minimal HTTP server used to prove local filesystem errors are not retried.
    fn tiny_http_server(body: Vec<u8>) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_thread = hits.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = match stream {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                hits_thread.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            }
        });
        (format!("http://{}/m.tar.zst", addr), hits)
    }

    #[test]
    fn progress_throttle_emits_phase_changes_and_one_per_percent() {
        let mut t = ProgressThrottle::new();
        assert!(t.should_emit(DownloadStatus::Starting));
        assert!(!t.should_emit(DownloadStatus::Starting));

        // 10% -> emit, 12% -> emit, 12% again -> coalesced away.
        assert!(t.should_emit(DownloadStatus::Downloading {
            downloaded_bytes: 100,
            total_bytes: Some(1000),
        }));
        assert!(t.should_emit(DownloadStatus::Downloading {
            downloaded_bytes: 125,
            total_bytes: Some(1000),
        }));
        assert!(!t.should_emit(DownloadStatus::Downloading {
            downloaded_bytes: 129,
            total_bytes: Some(1000),
        }));

        assert!(t.should_emit(DownloadStatus::Verifying));
        assert!(!t.should_emit(DownloadStatus::Verifying));
        assert!(t.should_emit(DownloadStatus::Extracting));
        assert!(t.should_emit(DownloadStatus::Installing));
        assert!(t.should_emit(DownloadStatus::Completed));
    }

    #[test]
    fn progress_throttle_coalesces_unknown_total_by_byte_bucket() {
        let mut t = ProgressThrottle::new();
        assert!(t.should_emit(DownloadStatus::Downloading {
            downloaded_bytes: 0,
            total_bytes: None,
        }));
        // Same 1 MiB bucket -> no update.
        assert!(!t.should_emit(DownloadStatus::Downloading {
            downloaded_bytes: 100 * 1024,
            total_bytes: None,
        }));
        // Crosses into the next bucket -> update.
        assert!(t.should_emit(DownloadStatus::Downloading {
            downloaded_bytes: 2 * 1024 * 1024,
            total_bytes: None,
        }));
    }

    #[test]
    fn download_status_maps_install_phases() {
        assert_eq!(
            DownloadStatus::from_phase(&InstallPhase::Starting),
            DownloadStatus::Starting
        );
        assert_eq!(
            DownloadStatus::from_phase(&InstallPhase::Downloading {
                downloaded_bytes: 5,
                total_bytes: Some(10),
            }),
            DownloadStatus::Downloading {
                downloaded_bytes: 5,
                total_bytes: Some(10),
            }
        );
        assert!(!DownloadStatus::Completed.is_in_progress());
        assert!(!DownloadStatus::Failed.is_in_progress());
        assert!(DownloadStatus::Verifying.is_in_progress());
    }

    #[test]
    fn local_filesystem_errors_are_not_retried() {
        let (url, hits) = tiny_http_server(vec![0u8; 8]);
        // Parent directory does not exist, so `File::create` fails locally.
        let bad_archive = std::env::temp_dir().join(format!(
            "echolet-missing-{}/archive.download",
            unique_nonce()
        ));
        let mut progress: Option<ProgressCallback<'_>> = None;
        let err = download_with_retry(&url, &bad_archive, &mut progress).unwrap_err();
        assert!(
            err.contains("Failed to create archive file"),
            "unexpected error: {}",
            err
        );
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "a local filesystem error must not be retried"
        );
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
