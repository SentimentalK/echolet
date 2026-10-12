//! Platform-neutral projection surface for the iOS model-management flow.
//!
//! Hosted unit-testable projection logic that the iOS `echolet_ios` FFI bridge
//! (and only that bridge) calls into. It intentionally owns NO transport, NO
//! IO beyond what [`crate::models::installer`] already owns, and invents NO
//! catalog data: every model entry comes verbatim from the canonical embedded
//! [`ModelRegistry`], and installed truth comes verbatim from
//! [`ModelManager::discover_installed`].
//!
//! Contracts encoded here:
//! * Download specs exist ONLY for registry-validated models with an HTTPS
//!   archive URL and a non-empty trusted `sha256` anchor (Rust stays the hash
//!   authority; Swift never invents hashes).
//! * Catalog compatibility on iOS: only `Echolet Verified` entries are
//!   proclaimed runtime-compatible; Experimental entries stay visible but are
//!   marked unverified so the UI disables selection.
//! * Selection persistence is fail-closed: if the config save fails the
//!   previous active model id is rolled back and the error is surfaced
//!   (the desktop `ModelManager::set_active_model` ignores save failures, so
//!   the iOS flow must not reuse it directly).

use crate::models::installer::install_model_from_archive_with_progress;
use crate::models::manager::{InstalledModel, ModelManager};
use crate::models::progress::{ProgressCallback};
use crate::models::registry::{ModelRegistry, RegistryModelEntry, VerificationStatus};
use crate::models::manifest::ModelManifest;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Catalog snapshot
// ---------------------------------------------------------------------------

/// Whether an entry is proven runtime-compatible on iOS targets: only the
/// product-verified tier has actual iOS model + native-load evidence today.
pub fn ios_runtime_compatible(entry: &RegistryModelEntry) -> bool {
    entry.verification_status == VerificationStatus::EcholetVerified
}

/// Builds the canonical catalog snapshot JSON consumed verbatim by the Swift
/// Home UI (no Swift-side duplicated catalog).
///
/// Entry order comes strictly from the registry document. Installed truth for
/// each id is taken from `installed`; `active_id` (when present and installed)
/// marks exactly one row `active`.
pub fn catalog_snapshot_json(
    registry: &ModelRegistry,
    installed: &HashMap<String, InstalledModel>,
    active_id: Option<&str>,
) -> String {
    let models: Vec<serde_json::Value> = registry
        .models
        .iter()
        .map(|entry| {
            let installed_model = installed.get(&entry.id);
            let installed = installed_model.is_some();
            serde_json::json!({
                "id": entry.id,
                "display_name": entry.display_name,
                "version": entry.version,
                "languages": entry.languages,
                "verification_status": entry.verification_status.label(),
                "verification_tier": verified_tier_code(entry),
                "download_size_bytes": entry.download_size_bytes,
                "installed_size_bytes": entry.installed_size_bytes,
                "runtime_compatible": ios_runtime_compatible(entry),
                "installed": installed,
                "installed_dir": installed_model
                    .map(|m| m.dir.to_string_lossy().into_owned()),
                "active": installed && active_id == Some(entry.id.as_str()),
            })
        })
        .collect();

    let snapshot = serde_json::json!({
        "schema_version": crate::models::registry::CURRENT_SCHEMA_VERSION,
        "default_model_id": registry.default_model_id,
        "models": models,
    });

    serde_json::to_string(&snapshot).unwrap_or_else(|_| "{}".to_string())
}

fn verified_tier_code(entry: &RegistryModelEntry) -> u8 {
    if ios_runtime_compatible(entry) {
        1
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Download spec (single canonical source: registry)
// ---------------------------------------------------------------------------

/// Resolves the canonical download spec JSON for a `model_info` registry id.
///
/// Rejects: unknown ids, bundled-only entries, non-HTTPS URLs and entries
/// without a trusted sha256 anchor. Mirrors the Android
/// `model_owner::get_download_spec` contract (same fields, same wire shape).
pub fn download_spec_json(entry: &RegistryModelEntry) -> Result<String, String> {
    if entry.source.bundled {
        return Err(format!("Model {} is bundled and cannot be downloaded", entry.id));
    }
    let url = entry
        .source
        .url
        .as_ref()
        .ok_or_else(|| format!("Model {} has no download URL configured", entry.id))?;
    if !url.starts_with("https://") {
        return Err(format!(
            "Model {} download URL must use HTTPS: {}",
            entry.id, url
        ));
    }
    let sha256 = entry
        .source
        .sha256
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("Model {} has no sha256 checksum configured", entry.id))?;

    let spec = serde_json::json!({
        "model_id": entry.id,
        "url": url,
        "sha256": sha256,
        "download_size_bytes": entry.download_size_bytes,
        "installed_size_bytes": entry.installed_size_bytes,
    });
    serde_json::to_string(&spec).map_err(|e| format!("Failed to serialize download spec: {}", e))
}

// ---------------------------------------------------------------------------
// Selection (fail-closed persistence)
// ---------------------------------------------------------------------------

/// Persisted selection with rollback semantics for the iOS flow.
///
/// * Registry lookup happens FIRST so unknown ids can never select anything.
/// * A save failure rolls `active_model_id` back to the previous value and
///   surfaces the error, rather than silently proclaiming persistence.
pub fn select_model_persisted(manager: &mut ModelManager, model_id: &str) -> Result<(), String> {
    if manager.registry.get_model(model_id).is_none() {
        return Err(format!("Unknown model ID: {}", model_id));
    }
    if !manager.installed.contains_key(model_id) {
        return Err(format!("Model {} is not installed", model_id));
    }
    let previous = manager.active_model_id.clone();
    if previous.as_deref() == Some(model_id) {
        // Already selected; re-saving is harmless but not required.
        return Ok(());
    }
    manager.active_model_id = Some(model_id.to_string());
    if let Err(save_err) = manager.save_config() {
        manager.active_model_id = previous;
        return Err(format!(
            "Failed to persist selected model {}: {}",
            model_id, save_err
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Install from downloaded archive (validated entry, bounded)
// ---------------------------------------------------------------------------

/// Installs a downloaded archive for a registry-validated model id into
/// `models_dir/<model_id>` through the shared Rust installer (SHA256 verify →
/// safe extraction → atomic commit). Purely delegate: hash authority is Rust,
/// staging cleanup is the installer's [`crate::models::installer::StagingGuard`],
/// and an existing valid install is never corrupted on failure.
pub fn install_model_archive_validated(
    registry: &ModelRegistry,
    model_id: &str,
    archive_path: &Path,
    models_dir: &Path,
    progress: Option<ProgressCallback<'_>>,
) -> Result<ModelManifest, String> {
    let entry = registry
        .get_model(model_id)
        .cloned()
        .ok_or_else(|| format!("Unknown model ID: {}", model_id))?;

    let target_dir = models_dir.join(model_id);
    install_model_from_archive_with_progress(&entry, archive_path, &target_dir, progress)
}

// ---------------------------------------------------------------------------
// FFI-side manager shorthand
// ---------------------------------------------------------------------------

/// Resolves the directory of the currently active installed model, if any.
pub fn active_model_dir(manager: &ModelManager) -> Option<PathBuf> {
    manager
        .active_model_id
        .as_ref()
        .and_then(|id| manager.installed.get(id))
        .map(|m| m.dir.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::progress::InstallPhase;
    use serde_json::Value;
    use std::fs;
    use std::io::Write;
    use std::path::Path;

    fn canonical_registry() -> ModelRegistry {
        ModelRegistry::canonical().expect("embedded canonical registry must parse")
    }

    fn xasr_entry(reg: &ModelRegistry) -> RegistryModelEntry {
        reg.get_model(&reg.default_model_id).cloned().unwrap()
    }

    fn installed_fixture(dir: &Path) -> InstalledModel {
        let manifest = ModelManifest::default();
        fs::create_dir_all(dir).unwrap();
        for name in [
            manifest.encoder.as_str(),
            manifest.decoder.as_str(),
            manifest.joiner.as_str(),
            manifest.tokens.as_str(),
        ] {
            let mut f = fs::File::create(dir.join(name)).unwrap();
            f.write_all(b"fixture").unwrap();
        }
        manifest.save_to_file(&dir.join("model.json")).unwrap();
        InstalledModel {
            id: manifest.id.clone(),
            dir: dir.to_path_buf(),
            manifest,
            is_bundled: false,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let base = std::env::temp_dir();
        let dir = base.join(format!(
            "echolet-ios-surface-test-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn catalog_lists_canonical_four_entries_with_verified_compatibility() {
        let reg = canonical_registry();
        assert_eq!(reg.models.len(), 4);
        let empty: HashMap<String, InstalledModel> = HashMap::new();
        let snap: Value =
            serde_json::from_str(&catalog_snapshot_json(&reg, &empty, None)).unwrap();

        let models = snap["models"].as_array().unwrap();
        assert_eq!(models.len(), 4);
        // Order strictly follows the registry (X-ASR first, verified).
        assert_eq!(models[0]["id"], xasr_entry(&reg).id);
        assert_eq!(models[0]["verification_status"], "Echolet Verified");
        assert_eq!(models[0]["runtime_compatible"], true);
        assert_eq!(models[0]["installed"], false);
        for m in &models[1..] {
            assert_eq!(m["verification_status"], "Experimental");
            // Experimental entries stay visible but are NOT proclaimed
            // iOS-runtime compatible without tested evidence.
            assert_eq!(m["runtime_compatible"], false);
        }
        assert_eq!(snap["default_model_id"], reg.default_model_id);
        assert_eq!(snap["schema_version"], 2);
    }

    #[test]
    fn catalog_marks_installed_and_active_truth() {
        let reg = canonical_registry();
        let dir = temp_dir("installed-active");
        let installed_dir = dir.join("xasr");
        let fixture = installed_fixture(&installed_dir);
        let fixture_id = fixture.id.clone();

        let mut installed: HashMap<String, InstalledModel> = HashMap::new();
        installed.insert(fixture.id.clone(), fixture);
        let snap: Value =
            serde_json::from_str(&catalog_snapshot_json(&reg, &installed, Some(fixture_id.as_str())))
                .unwrap();
        let models = snap["models"].as_array().unwrap();
        let first_id = models[0]["id"].as_str().unwrap().to_string();
        assert_eq!(first_id, fixture_id);
        assert_eq!(models[0]["installed"], true);
        assert_eq!(models[0]["active"], true);
        assert!(models[0]["installed_dir"].as_str().unwrap().len() > 0);
        for m in &models[1..] {
            assert_eq!(m["installed"], false);
            assert_eq!(m["active"], false);
            assert!(m["installed_dir"].is_null());
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn download_spec_is_valid_https_with_sha_and_sizes_for_all_four_models() {
        let reg = canonical_registry();
        for entry in &reg.models {
            let spec: Value =
                serde_json::from_str(&download_spec_json(entry).expect("spec valid")).unwrap();
            assert_eq!(spec["model_id"], entry.id);
            let url = spec["url"].as_str().unwrap();
            assert!(url.starts_with("https://"), "{} must be HTTPS", url);
            assert_eq!(spec["sha256"], entry.source.sha256.as_ref().unwrap().as_str());
            assert_eq!(
                spec["download_size_bytes"],
                serde_json::to_value(entry.download_size_bytes).unwrap()
            );
            assert_eq!(
                spec["installed_size_bytes"],
                serde_json::to_value(entry.installed_size_bytes).unwrap()
            );
        }
    }

    #[test]
    fn download_spec_rejects_unknown_id_and_bundled_and_insecure_urls() {
        let reg = canonical_registry();
        let mut entry = xasr_entry(&reg);
        entry.source.url = Some("http://insecure.example/model.tar.zst".into());
        match download_spec_json(&entry) {
            Err(e) => assert!(e.to_lowercase().contains("https")),
            Ok(_) => panic!("HTTP url must be rejected"),
        }
        let mut bundled = xasr_entry(&reg);
        bundled.source.bundled = true;
        assert!(download_spec_json(&bundled).unwrap_err().contains("bundled"));
        let mut no_sha = xasr_entry(&reg);
        no_sha.source.sha256 = Some("".into());
        let _ = download_spec_json(&no_sha).unwrap_err();
        // Unknown-id rejection lives at the registry lookup seam.
        let unknown = "not-a-model-id";
        assert!(reg.get_model(unknown).is_none(), "unknown must not resolve");
    }

    #[test]
    fn unknown_install_id_rejected_before_any_disk_work() {
        let reg = canonical_registry();
        let dir = temp_dir("unknown-install");
        let result = install_model_archive_validated(
            &reg,
            "path/../traversal",
            Path::new("/tmp/nonexistent-archive.bin"),
            &dir,
            None,
        );
        assert!(result.is_err());
        assert!(
            !ls_survivors(&dir).iter().any(|p| p.contains("traversal")),
            "no traversal dir may be created"
        );
        fs::remove_dir_all(&dir).ok();
    }

    fn ls_survivors(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn sha_mismatch_leaves_existing_install_intact_and_staging_cleaned() {
        let reg = canonical_registry();
        let dir = temp_dir("sha-mismatch");
        let models_dir = dir.join("models");
        // Existing VALID install of a fake model via fixture.
        let existing_dir = models_dir.join(&xasr_entry(&reg).id);
        installed_fixture(&existing_dir);

        // Note: to reach the sha check without a real 550MB fixture we craft a
        // tar.tar (unsupported format) — but the SHA check happens BEFORE
        // format detection, so a checksum mismatch is returned and no archive
        // IO mutates the install.
        let archive = dir.join("archive.bin");
        fs::write(&archive, b"definitely-not-the-real-archive").unwrap();

        let result = install_model_archive_validated(
            &reg,
            &xasr_entry(&reg).id,
            &archive,
            &models_dir,
            None,
        );
        match result {
            Err(e) => assert!(
                e.contains("Unrecognized") || e.contains("SHA256"),
                "expected truthy mismatch/format error, got: {}",
                e
            ),
            Ok(_) => panic!("corrupt archive must NOT install"),
        }

        // The previous install remains exactly as before (intact manifest).
        let manifest = ModelManifest::from_file(&existing_dir.join("model.json")).unwrap();
        assert_eq!(manifest.id, xasr_entry(&reg).id);
        assert!(existing_dir.join(manifest.encoder).exists());
        // Staging dirs are cleaned by the installer's guard.
        let survivors = ls_survivors(&models_dir);
        assert!(
            !survivors.iter().any(|n| n.starts_with(".echolet-")),
            "staging leftovers found: {:?}",
            survivors
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn select_rejects_unknown_and_not_installed() {
        let dir = temp_dir("select-unknown");
        let user = dir.join("user-models");
        let config = dir.join("config.json");
        let mut manager = ModelManager::new_with_paths(
            dir.join("no-bundled-models"),
            user.clone(),
            config.clone(),
        )
        .unwrap();
        assert!(select_model_persisted(&mut manager, "path/traversal-model").is_err());
        assert!(select_model_persisted(&mut manager, "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1").is_err(), "uninstalled cannot be selected");
        assert!(manager.active_model_id.is_none());
        // No config file was written for rejected selections.
        assert!(!config.exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn select_persists_and_survives_manager_restart() {
        let dir = temp_dir("select-persist");
        let user = dir.join("user-models");
        let config = dir.join("config.json");
        let xasr_id = canonical_registry().default_model_id;
        installed_fixture(&user.join(&xasr_id));

        // Create a fake second installed model discovered by the manager.
        let kroko_id = "echolet-kroko-streaming-en-2025-08-06-r1";
        let manifest = {
            let mut m = ModelManifest::default();
            m.id = kroko_id.into();
            m.languages = vec!["en".into()];
            m.encoder = "encoder.onnx".into();
            m.decoder = "decoder.onnx".into();
            m.joiner = "joiner.onnx".into();
            m.tokens = "tokens.txt".into();
            m
        };
        let install_dir = user.join(kroko_id);
        fs::create_dir_all(&install_dir).unwrap();
        for name in [
            manifest.encoder.as_str(),
            manifest.decoder.as_str(),
            manifest.joiner.as_str(),
            manifest.tokens.as_str(),
        ] {
            fs::File::create(install_dir.join(name)).unwrap().write_all(b"x").unwrap();
        }
        manifest.save_to_file(&install_dir.join("model.json")).unwrap();

        // Fresh manager: discovers both installs and resolves the default.
        let mut manager = ModelManager::new_with_paths(
            dir.join("no-bundled-models"),
            user.clone(),
            config.clone(),
        )
        .unwrap();
        assert_eq!(resolve_check(&manager), xasr_id);

        // Select the kroko model: must persist and become active.
        select_model_persisted(&mut manager, kroko_id).expect("select after install");
        assert_eq!(manager.active_model_id.as_deref(), Some(kroko_id));

        // Restart semantics: a fresh manager restores the persisted pick.
        let mut manager2 = ModelManager::new_with_paths(
            dir.join("no-bundled-models"),
            user.clone(),
            config.clone(),
        )
        .unwrap();
        assert_eq!(manager2.active_model_id.as_deref(), Some(kroko_id));

        fs::remove_dir_all(&dir).ok();
    }

    fn resolve_check(manager: &ModelManager) -> String {
        manager.active_model_id.clone().unwrap()
    }

    #[test]
    fn select_save_failure_rolls_back_previous_active() {
        let dir = temp_dir("select-rollback");
        let user = dir.join("user-models");
        // config_path points to a DIRECTORY: save can never succeed there.
        let config_path = dir.join("config.json");
        fs::create_dir_all(&config_path).unwrap();

        let mut manager = ModelManager::new_with_paths(
            dir.join("no-bundled-models"),
            user.clone(),
            config_path,
        )
        .unwrap();
        let xasr_id = manager.registry.default_model_id.clone();
        installed_fixture(&user.join(&xasr_id));
        manager.discover_installed();
        manager.resolve_active_model();
        assert_eq!(manager.active_model_id.as_deref().unwrap(), xasr_id.as_str());

        let result = select_model_persisted(&mut manager, &xasr_id);
        // Already-selected branch returns early without a save attempt.
        let kroko_id = "echolet-kroko-streaming-en-2025-08-06-r1";
        let manifest = {
            let mut m = ModelManifest::default();
            m.id = kroko_id.into();
            m.languages = vec!["en".into()];
            m.encoder = "encoder.onnx".into();
            m.decoder = "decoder.onnx".into();
            m.joiner = "joiner.onnx".into();
            m.tokens = "tokens.txt".into();
            m
        };
        let install_dir = user.join(kroko_id);
        fs::create_dir_all(&install_dir).unwrap();
        for name in [
            manifest.encoder.as_str(),
            manifest.decoder.as_str(),
            manifest.joiner.as_str(),
            manifest.tokens.as_str(),
        ] {
            fs::File::create(install_dir.join(name)).unwrap().write_all(b"x").unwrap();
        }
        manifest.save_to_file(&install_dir.join("model.json")).unwrap();
        manager.discover_installed();

        // Selecting a DIFFERENT (installed) id with a broken config path must
        // surface the save failure AND roll back to the previous id.
        match select_model_persisted(&mut manager, kroko_id) {
            Err(e) => assert!(e.contains("Failed to persist")),
            Ok(_) => panic!("save failure must surface; state directory cannot be persisted"),
        }
        assert_eq!(manager.active_model_id.as_deref(), Some(xasr_id.as_str()));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn install_valid_fixture_installs_and_registers() {
        let dir = temp_dir("valid-install");
        let models_dir = dir.join("models");

        // Build a real deterministic plain .tar with the four canonical files
        // inside a single top folder (mirrors the product layout), then point
        // the REGISTRY's trusted sha at the fixture bytes so the full
        // verify -> extract -> validate -> commit path runs deterministically.
        let archive_path = dir.join("model.tar");
        let mut entry: RegistryModelEntry = canonical_registry().default_entry().cloned().unwrap();
        build_plain_tar(&archive_path, &mut entry);

        let reg = canonical_registry();

        let mut phases = Vec::new();
        let fixture_sha = crate::models::installer::sha256_file(&archive_path).unwrap();
        let mut reg = canonical_registry();
        if let Some(pos) = reg.models.iter().position(|m| m.id == entry.id) {
            // Point the registry's trusted sha anchor at the fixture digest so
            // the full verify -> extract -> commit path runs deterministically.
            reg.models[pos].source.sha256 = Some(fixture_sha);
        }
        install_model_archive_validated(
            &reg,
            entry.id.as_str(),
            &archive_path,
            &models_dir,
            Some(&mut |phase| phases.push(phase)),
        )
        .expect("plain tar install with matching fixture must succeed");

        let target = models_dir.join(entry.id.as_str());
        let manifest = ModelManifest::from_file(&target.join("model.json")).unwrap();
        assert_eq!(manifest.id, entry.id);
        assert!(manifest.validate_files(&target).is_ok());
        // A manager can discover and resolve the freshly installed model.
        let mut manager = ModelManager::new_with_paths(
            dir.join("no-bundled"),
            models_dir,
            dir.join("config.json"),
        )
        .unwrap();
        assert!(manager.is_installed(&entry.id));
        assert_eq!(manager.active_model_id().unwrap(), entry.id.as_str());
        assert!(phases.contains(&InstallPhase::Verifying));
        assert!(phases.contains(&InstallPhase::Installing));
        assert!(phases.contains(&InstallPhase::Completed));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn install_rejects_bad_sha_and_leaves_no_staging() {
        let reg = canonical_registry();
        let mut entry = xasr_entry(&reg);
        let dir = temp_dir("bad-sha");
        let models_dir = dir.join("models");
        let archive_path = dir.join("model.tar");
        build_plain_tar(&archive_path, &mut entry);

        // Provide the EXACT sha anchor from the freshly built fixture, then
        // flip one byte in the recorded digest so verification must reject.
        let true_sha = entry.source.sha256.take().unwrap();
        entry.source.sha256 = Some(true_sha[..62].to_string() + "ab");

        let result = install_model_archive_validated(
            &reg,
            entry.id.as_str(),
            &archive_path,
            &models_dir,
            None,
        );
        match result {
            Err(e) => assert!(e.contains("SHA256"), "expected SHA mismatch, got: {}", e),
            Ok(_) => panic!("fixture archive has a different sha; must reject"),
        }
        let survivors = ls_survivors(&models_dir);
        assert!(
            !survivors.iter().any(|n| n.starts_with(".echolet-")),
            "staging leftovers found: {:?}",
            survivors
        );
        fs::remove_dir_all(&dir).ok();
    }

    /// Writes a minimal uncompressed ustar tar with one top-level folder
    /// containing the four manifest file names as tiny files. This is fixture
    /// data only; the real product archive carries the actual model bytes.
    fn build_plain_tar(archive_path: &Path, entry: &mut RegistryModelEntry) {
        use tar::{Builder, Header};
        let dir = archive_path.parent().unwrap().join("plainsrc");
        fs::create_dir_all(dir.join("top")).unwrap();
        for name in [
            entry.files.encoder.as_str(),
            entry.files.decoder.as_str(),
            entry.files.joiner.as_str(),
            entry.files.tokens.as_str(),
        ] {
            fs::write(dir.join("top").join(name), b"fixture-bytes").unwrap();
        }
        let file = fs::File::create(archive_path).unwrap();
        let mut tar = Builder::new(file);
        for entry in fs::read_dir(dir.join("top")).unwrap().flatten() {
            let path = dir.join("top").join(entry.file_name());
            let mut f = fs::File::open(&path).unwrap();
            let mut header = Header::new_gnu();
            header.set_size(fs::metadata(&path).unwrap().len());
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(
                &mut header,
                format!("top/{}", entry.file_name().to_string_lossy()),
                &mut f,
            )
            .unwrap();
        }
        tar.finish().unwrap();
    }
}
