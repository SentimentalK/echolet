use crate::models::manifest::ModelManifest;
use crate::models::registry::RegistryModelEntry;
use bzip2::read::BzDecoder;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tar::Archive;

pub fn download_and_install_model(
    entry: &RegistryModelEntry,
    target_dir: &Path,
) -> Result<ModelManifest, String> {
    let tmp_parent = std::env::temp_dir().join("echolet-downloads");
    fs::create_dir_all(&tmp_parent).map_err(|e| format!("Failed to create temp dir: {}", e))?;

    let unique_id = format!("{}-{}", entry.id, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis());
    let tmp_dir = tmp_parent.join(&unique_id);
    fs::create_dir_all(&tmp_dir).map_err(|e| format!("Failed to create temp dir: {}", e))?;

    // Safe auto-cleanup on exit
    struct TempDirGuard(PathBuf);
    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _guard = TempDirGuard(tmp_dir.clone());

    let archive_path = tmp_dir.join("model_archive.tar.bz2");

    println!("[Model] Downloading {} from {}...", entry.display_title(), entry.source.url);

    // 1. Download file and calculate SHA256 simultaneously
    let response = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .get(&entry.source.url)
        .call()
        .map_err(|e| format!("HTTP download failed for {}: {}", entry.source.url, e))?;

    let mut reader = response.into_reader();
    let mut file = File::create(&archive_path)
        .map_err(|e| format!("Failed to create archive file {:?}: {}", archive_path, e))?;

    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];

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
    }
    file.flush().map_err(|e| format!("Failed to flush archive: {}", e))?;

    // 2. Verify SHA256 checksum
    let calculated_sha256 = hex::encode(hasher.finalize());
    println!("[Model] Download finished. Calculated SHA256: {}", calculated_sha256);

    if !calculated_sha256.eq_ignore_ascii_case(&entry.source.sha256) {
        return Err(format!(
            "SHA256 checksum mismatch for {}.\nExpected: {}\nGot:      {}",
            entry.id, entry.source.sha256, calculated_sha256
        ));
    }

    println!("[Model] SHA256 verification passed!");

    // 3. Extract tar.bz2 archive
    println!("[Model] Extracting model archive...");
    let archive_file = File::open(&archive_path)
        .map_err(|e| format!("Failed to open downloaded archive: {}", e))?;
    let bz_decoder = BzDecoder::new(archive_file);
    let mut archive = Archive::new(bz_decoder);

    let extract_dir = tmp_dir.join("extracted");
    fs::create_dir_all(&extract_dir)
        .map_err(|e| format!("Failed to create extract dir: {}", e))?;
    archive
        .unpack(&extract_dir)
        .map_err(|e| format!("Failed to unpack tar.bz2 archive: {}", e))?;

    // 4. Locate model directory inside extract_dir
    let model_content_dir = find_model_content_dir(&extract_dir, entry)?;

    // 5. Generate and write normalized model.json
    let manifest = entry.to_manifest();
    manifest.validate_files(&model_content_dir)?;

    let manifest_path = model_content_dir.join("model.json");
    manifest.save_to_file(&manifest_path)?;

    // 6. Atomically install model into target_dir
    if let Some(parent) = target_dir.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create parent dir {:?}: {}", parent, e))?;
    }

    if target_dir.exists() {
        let _ = fs::remove_dir_all(target_dir);
    }

    // Attempt atomic rename, fallback to copy if cross-device
    if let Err(_) = fs::rename(&model_content_dir, target_dir) {
        copy_dir_all(&model_content_dir, target_dir)?;
    }

    println!("[Model] Successfully installed {} to {:?}", entry.display_title(), target_dir);
    Ok(manifest)
}

fn find_model_content_dir(base: &Path, entry: &RegistryModelEntry) -> Result<PathBuf, String> {
    // Check if files exist directly in base
    if base.join(&entry.files.tokens).exists() && base.join(&entry.files.encoder).exists() {
        return Ok(base.to_path_buf());
    }

    // Check subdirectories
    if let Ok(entries) = fs::read_dir(base) {
        for entry_res in entries.flatten() {
            let path = entry_res.path();
            if path.is_dir() {
                if path.join(&entry.files.tokens).exists() && path.join(&entry.files.encoder).exists() {
                    return Ok(path);
                }
            }
        }
    }

    Err(format!(
        "Could not find model files ({}, {}) inside extracted archive at {:?}",
        entry.files.tokens, entry.files.encoder, base
    ))
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("Failed to create dir {:?}: {}", dst, e))?;
    for entry in fs::read_dir(src).map_err(|e| format!("Failed to read dir {:?}: {}", src, e))? {
        let entry = entry.map_err(|e| format!("Error reading entry: {}", e))?;
        let ty = entry.file_type().map_err(|e| format!("Error getting file type: {}", e))?;
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
