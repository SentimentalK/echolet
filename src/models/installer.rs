use crate::models::manifest::ModelManifest;
use crate::models::progress::{ArchiveFormat, InstallPhase, ProgressCallback};
use crate::models::registry::RegistryModelEntry;
use bzip2::read::BzDecoder;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tar::Archive;

static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct StagingGuard(pub PathBuf);

impl Drop for StagingGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn sanitize_component(input: &str) -> String {
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

pub fn unique_nonce() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{}-{}", std::process::id(), now, counter)
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
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

/// Deterministically detects the compression wrapper by magic bytes. This avoids
/// tying correctness to a hardcoded temporary file name.
pub fn detect_archive_format(path: &Path) -> Result<ArchiveFormat, String> {
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

pub fn extract_archive(archive_path: &Path, format: ArchiveFormat, dest: &Path) -> Result<(), String> {
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
pub fn safe_unpack<R: Read>(mut archive: Archive<R>, dest: &Path) -> Result<(), String> {
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
pub fn find_model_content_dir(base: &Path, entry: &RegistryModelEntry) -> Result<PathBuf, String> {
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

pub trait CommitFs {
    fn rename(&self, from: &Path, to: &Path) -> Result<(), String>;
    fn copy_dir(&self, from: &Path, to: &Path) -> Result<(), String>;
}

pub struct RealCommitFs;

impl CommitFs for RealCommitFs {
    fn rename(&self, from: &Path, to: &Path) -> Result<(), String> {
        rename_dir(from, to)
    }

    fn copy_dir(&self, from: &Path, to: &Path) -> Result<(), String> {
        copy_dir_all(from, to)
    }
}

pub fn commit_install(
    content_dir: &Path,
    target_dir: &Path,
    staging_root: &Path,
) -> Result<(), String> {
    commit_install_with(content_dir, target_dir, staging_root, &RealCommitFs)
}

pub fn commit_install_with(
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

pub fn rename_dir(src: &Path, dst: &Path) -> Result<(), String> {
    fs::rename(src, dst).map_err(|e| format!("rename {:?} -> {:?} failed: {}", src, dst, e))
}

pub fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), String> {
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

pub fn emit_progress(progress: &mut Option<ProgressCallback<'_>>, phase: InstallPhase) {
    if let Some(callback) = progress.as_deref_mut() {
        callback(phase);
    }
}

/// Verifies, extracts, validates, and atomically installs an archive file that has
/// already been staged on disk (e.g. via HTTP download).
pub fn install_model_from_archive_with_progress(
    entry: &RegistryModelEntry,
    archive_path: &Path,
    target_dir: &Path,
    mut progress: Option<ProgressCallback<'_>>,
) -> Result<ModelManifest, String> {
    let expected_sha256 = entry.source.sha256.as_deref().ok_or_else(|| {
        format!(
            "Model {} does not have an archive SHA256 checksum",
            entry.id
        )
    })?;

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

    let _guard = StagingGuard(staging_root.clone());
    let extract_dir = staging_root.join("extracted");

    // ---- Verify checksum ----
    emit_progress(&mut progress, InstallPhase::Verifying);
    let calculated_sha256 = sha256_file(archive_path)?;
    if !calculated_sha256.eq_ignore_ascii_case(expected_sha256) {
        return Err(format!(
            "SHA256 checksum mismatch for {}.\nExpected: {}\nGot:      {}",
            entry.id, expected_sha256, calculated_sha256
        ));
    }

    // ---- Extract ----
    emit_progress(&mut progress, InstallPhase::Extracting);
    fs::create_dir_all(&extract_dir)
        .map_err(|e| format!("Failed to create extract dir {:?}: {}", extract_dir, e))?;
    let format = detect_archive_format(archive_path)?;
    extract_archive(archive_path, format, &extract_dir)?;

    // ---- Validate model content ----
    let model_content_dir = find_model_content_dir(&extract_dir, entry)?;
    let manifest = entry.to_manifest();
    manifest.validate_files(&model_content_dir)?;

    let manifest_path = model_content_dir.join("model.json");
    manifest.save_to_file(&manifest_path)?;

    // ---- Atomic commit ----
    emit_progress(&mut progress, InstallPhase::Installing);
    commit_install(&model_content_dir, target_dir, &staging_root)?;

    emit_progress(&mut progress, InstallPhase::Completed);
    Ok(manifest)
}

pub fn install_model_from_archive(
    entry: &RegistryModelEntry,
    archive_path: &Path,
    target_dir: &Path,
) -> Result<ModelManifest, String> {
    install_model_from_archive_with_progress(entry, archive_path, target_dir, None)
}
