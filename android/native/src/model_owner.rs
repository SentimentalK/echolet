use echolet::config::EcholetConfig;
use echolet::models::installer::{install_model_from_archive, sanitize_component, unique_nonce};
use echolet::models::progress::DownloadStatus;
use echolet::models::registry::{ModelRegistry, RegistryModelEntry, CURRENT_SCHEMA_VERSION};
use echolet::ui::control_surface::{
    build_control_surface_state, ControlSurfaceState, ModelGroupPresentation, RuntimeState,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The legacy model directory fixture on Pixel9a / Android.
pub const LEGACY_MODEL_DIR_NAME: &str = "bilingual-zh-en";

/// Snapshot schema returned across JNI to Kotlin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSnapshot {
    pub schema_version: u32,
    pub selected_model_id: String,
    pub selected_model_dir: Option<String>,
    pub runtime_state: RuntimeState,
    pub model_groups: Vec<ModelGroupPresentation>,
}

/// Global model owner singleton for Android JNI.
static MODEL_OWNER: OnceLock<Mutex<AndroidModelOwner>> = OnceLock::new();

pub fn shared_model_owner() -> &'static Mutex<AndroidModelOwner> {
    MODEL_OWNER.get_or_init(|| Mutex::new(AndroidModelOwner::empty()))
}

/// Typed download specification returned to Kotlin download transport.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelDownloadSpec {
    pub model_id: String,
    pub url: String,
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_size_bytes: Option<u64>,
}

/// Parses PascalCase download phase wire string from Android/Kotlin into typed [`DownloadStatus`].
/// Rejects unknown phase strings strictly instead of resetting silently to NotDownloading.
pub fn parse_download_status(
    phase: &str,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
) -> Result<DownloadStatus, String> {
    match phase {
        "Starting" => Ok(DownloadStatus::Starting),
        "Downloading" => Ok(DownloadStatus::Downloading {
            downloaded_bytes,
            total_bytes,
        }),
        "Verifying" => Ok(DownloadStatus::Verifying),
        "Extracting" => Ok(DownloadStatus::Extracting),
        "Installing" => Ok(DownloadStatus::Installing),
        "Completed" => Ok(DownloadStatus::Completed),
        "Failed" => Ok(DownloadStatus::Failed),
        unknown => Err(format!(
            "Unknown download phase '{}'; expected PascalCase Starting, Downloading, Verifying, Extracting, Installing, Completed, Failed",
            unknown
        )),
    }
}

/// Platform-neutral model owner for Android.
pub struct AndroidModelOwner {
    models_dir: Option<PathBuf>,
    files_dir: Option<PathBuf>,
    registry: ModelRegistry,
    selected_model_id: Option<String>,
    progress: HashMap<String, DownloadStatus>,
    in_flight_installs: HashSet<String>,
}

impl AndroidModelOwner {
    pub fn empty() -> Self {
        let registry = ModelRegistry::canonical().expect("canonical registry must load");
        Self {
            models_dir: None,
            files_dir: None,
            selected_model_id: Some(registry.default_model_id.clone()),
            registry,
            progress: HashMap::new(),
            in_flight_installs: HashSet::new(),
        }
    }

    pub fn new(models_dir: PathBuf, files_dir: PathBuf) -> Self {
        let mut owner = Self::empty();
        owner.initialize(models_dir, files_dir);
        owner
    }

    pub fn initialize(&mut self, models_dir: PathBuf, files_dir: PathBuf) {
        let _ = fs::create_dir_all(&models_dir);
        let _ = fs::create_dir_all(&files_dir);
        self.models_dir = Some(models_dir.clone());
        self.files_dir = Some(files_dir.clone());

        // Clean up any abandoned staging residue from killed processes when no install is active
        if self.in_flight_installs.is_empty() {
            self.clean_abandoned_staging(&models_dir);
        }

        // Restore persisted selected model if valid
        self.restore_selected_model(&files_dir);
    }

    pub fn registry(&self) -> &ModelRegistry {
        &self.registry
    }

    pub fn selected_model_id(&self) -> &str {
        self.selected_model_id
            .as_deref()
            .unwrap_or(&self.registry.default_model_id)
    }

    /// Resolves on-disk path for `model_id`.
    ///
    /// Legacy compatibility: for the default X-ASR model, `models/bilingual-zh-en`
    /// is resolved in situ without copying or moving!
    pub fn resolve_model_dir(&self, model_id: &str) -> Option<PathBuf> {
        let models_dir = self.models_dir.as_ref()?;
        let entry = self.registry.get_model(model_id)?;

        // If this is the default X-ASR model, check legacy directory first
        if model_id == self.registry.default_model_id {
            let legacy_path = models_dir.join(LEGACY_MODEL_DIR_NAME);
            if self.is_model_dir_valid(entry, &legacy_path) {
                return Some(legacy_path);
            }
        }

        // Standard registry-ID directory
        let standard_path = models_dir.join(model_id);
        if self.is_model_dir_valid(entry, &standard_path) {
            Some(standard_path)
        } else {
            None
        }
    }

    /// Path of the currently selected model if installed and valid.
    pub fn current_model_dir(&self) -> Option<PathBuf> {
        let id = self.selected_model_id();
        self.resolve_model_dir(id)
    }

    /// Verifies canonical manifest & the 4 required runtime files.
    pub fn is_model_dir_valid(&self, entry: &RegistryModelEntry, dir: &Path) -> bool {
        if !dir.is_dir() {
            return false;
        }
        if !dir.join("model.json").exists() {
            return false;
        }
        let manifest = entry.to_manifest();
        manifest.validate_files(dir).is_ok()
    }

    /// Set of model IDs currently installed and valid.
    pub fn installed_model_ids(&self) -> HashSet<String> {
        let mut installed = HashSet::new();
        for entry in &self.registry.models {
            if self.resolve_model_dir(&entry.id).is_some() {
                installed.insert(entry.id.clone());
            }
        }
        installed
    }

    /// Selects a model by ID.
    ///
    /// Fails if:
    /// - `model_id` is unknown in the canonical registry
    /// - model is not installed or invalid
    /// - `is_session_active` is true (voice dictation in progress)
    pub fn select_model(&mut self, model_id: &str, is_session_active: bool) -> Result<(), String> {
        if is_session_active {
            return Err("Cannot switch models while dictation is listening or active".to_string());
        }

        let entry = self
            .registry
            .get_model(model_id)
            .ok_or_else(|| format!("Unknown model ID: {}", model_id))?;

        if self.resolve_model_dir(&entry.id).is_none() {
            return Err(format!("Model {} is not installed", model_id));
        }

        // Persist atomically to files_dir/selected_model.txt
        if let Some(files_dir) = &self.files_dir {
            self.persist_selected_model_atomic(files_dir, model_id)?;
        }

        self.selected_model_id = Some(model_id.to_string());
        Ok(())
    }

    /// Gets canonical download specification for a catalog model.
    pub fn get_download_spec(&self, model_id: &str) -> Result<ModelDownloadSpec, String> {
        let entry = self
            .registry
            .get_model(model_id)
            .ok_or_else(|| format!("Unknown model ID: {}", model_id))?;

        if entry.source.bundled {
            return Err(format!("Model {} is bundled and cannot be downloaded", model_id));
        }

        let url = entry
            .source
            .url
            .as_ref()
            .ok_or_else(|| format!("Model {} has no download URL configured", model_id))?;

        if !url.starts_with("https://") {
            return Err(format!("Model {} download URL must use HTTPS: {}", model_id, url));
        }

        let sha256 = entry
            .source
            .sha256
            .as_ref()
            .ok_or_else(|| format!("Model {} has no sha256 checksum configured", model_id))?;

        if sha256.trim().is_empty() {
            return Err(format!("Model {} has empty sha256 checksum", model_id));
        }

        Ok(ModelDownloadSpec {
            model_id: model_id.to_string(),
            url: url.clone(),
            sha256: sha256.clone(),
            download_size_bytes: entry.download_size_bytes,
            installed_size_bytes: entry.installed_size_bytes,
        })
    }

    /// Reserves an in-flight install under short lock.
    pub fn begin_install(
        &mut self,
        model_id: &str,
    ) -> Result<(RegistryModelEntry, PathBuf), String> {
        if self.in_flight_installs.contains(model_id) {
            return Err(format!("Install already in flight for model {}", model_id));
        }

        let entry = self
            .registry
            .get_model(model_id)
            .cloned()
            .ok_or_else(|| format!("Unknown model ID: {}", model_id))?;

        let models_dir = self
            .models_dir
            .clone()
            .ok_or_else(|| "Models directory not initialized".to_string())?;

        self.in_flight_installs.insert(model_id.to_string());
        self.progress
            .insert(model_id.to_string(), DownloadStatus::Installing);

        Ok((entry, models_dir))
    }

    /// Finishes an in-flight install on success under short lock.
    pub fn complete_install(&mut self, model_id: &str) {
        self.in_flight_installs.remove(model_id);
        self.progress
            .insert(model_id.to_string(), DownloadStatus::Completed);
    }

    /// Finishes an in-flight install on failure under short lock.
    pub fn fail_install(&mut self, model_id: &str) {
        self.in_flight_installs.remove(model_id);
        self.progress
            .insert(model_id.to_string(), DownloadStatus::Failed);
    }

    pub fn is_install_in_flight(&self, model_id: &str) -> bool {
        self.in_flight_installs.contains(model_id)
    }

    /// Installs a model from an already-staged archive file synchronously.
    pub fn install_from_archive(
        &mut self,
        model_id: &str,
        archive_path: &Path,
    ) -> Result<(), String> {
        let (entry, models_dir) = self.begin_install(model_id)?;
        let target_dir = models_dir.join(model_id);

        let res = install_model_from_archive(&entry, archive_path, &target_dir);
        match res {
            Ok(_) => {
                self.complete_install(model_id);
                Ok(())
            }
            Err(e) => {
                self.fail_install(model_id);
                Err(e)
            }
        }
    }

    pub fn set_download_progress(&mut self, model_id: &str, status: DownloadStatus) {
        self.progress.insert(model_id.to_string(), status);
    }

    /// Produces the complete canonical UI projection state.
    pub fn build_surface_state(&self, runtime_state: RuntimeState) -> ControlSurfaceState {
        let installed = self.installed_model_ids();
        let downloading: HashSet<String> = self
            .progress
            .iter()
            .filter(|(_, s)| s.is_in_progress())
            .map(|(id, _)| id.clone())
            .collect();

        let config = EcholetConfig {
            selected_model: self.selected_model_id().to_string(),
            ..Default::default()
        };

        build_control_surface_state(
            &self.registry,
            self.selected_model_id.as_deref(),
            &installed,
            &downloading,
            &self.progress,
            &config,
            runtime_state,
            false,
        )
    }

    /// Builds the compact JSON snapshot for Kotlin.
    pub fn build_snapshot_json(&self, runtime_state: RuntimeState) -> Result<String, String> {
        let surface_state = self.build_surface_state(runtime_state);
        let selected_dir = self.current_model_dir().map(|p| p.to_string_lossy().into_owned());

        let snapshot = ModelSnapshot {
            schema_version: CURRENT_SCHEMA_VERSION,
            selected_model_id: self.selected_model_id().to_string(),
            selected_model_dir: selected_dir,
            runtime_state,
            model_groups: surface_state.model_groups,
        };

        serde_json::to_string(&snapshot)
            .map_err(|e| format!("Failed to serialize model snapshot: {}", e))
    }

    fn persist_selected_model_atomic(&self, files_dir: &Path, model_id: &str) -> Result<(), String> {
        let target_file = files_dir.join("selected_model.txt");
        let temp_file = files_dir.join(format!(
            ".selected_model-{}-{}.tmp",
            sanitize_component(model_id),
            unique_nonce()
        ));

        fs::write(&temp_file, model_id.as_bytes())
            .map_err(|e| format!("Failed to write temp selected model file: {}", e))?;

        fs::rename(&temp_file, &target_file)
            .map_err(|e| format!("Failed to atomically rename selected model file: {}", e))?;

        Ok(())
    }

    fn restore_selected_model(&mut self, files_dir: &Path) {
        let selected_file = files_dir.join("selected_model.txt");
        if let Ok(content) = fs::read_to_string(&selected_file) {
            let candidate = content.trim();
            if self.registry.get_model(candidate).is_some() {
                self.selected_model_id = Some(candidate.to_string());
                return;
            }
        }

        // Initial default: default model id if not already selected
        if self.selected_model_id.is_none() {
            self.selected_model_id = Some(self.registry.default_model_id.clone());
        }
    }

    pub fn list_recoverable_backups(&self) -> Vec<PathBuf> {
        let mut backups = Vec::new();
        if let Some(models_dir) = &self.models_dir {
            if let Ok(entries) = fs::read_dir(models_dir) {
                for entry in entries.flatten() {
                    let name_str = entry.file_name().to_string_lossy().into_owned();
                    if name_str.starts_with(".echolet-old-") && entry.path().is_dir() {
                        backups.push(entry.path());
                    }
                }
            }
        }
        backups
    }

    fn clean_abandoned_staging(&self, models_dir: &Path) {
        if !self.in_flight_installs.is_empty() {
            return;
        }
        if let Ok(entries) = fs::read_dir(models_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                // Never delete .echolet-old-* backups: they represent manual recovery
                // backups when a previous installation replacement failed.
                if name_str.starts_with(".echolet-staging-")
                    || name_str.starts_with(".echolet-commit-")
                    || name_str.ends_with(".part")
                    || name_str.ends_with(".download")
                {
                    let path = entry.path();
                    if path.is_dir() {
                        let _ = fs::remove_dir_all(&path);
                    } else {
                        let _ = fs::remove_file(&path);
                    }
                }
            }
        }
    }
}

/// RAII guard ensuring in-flight install cleanup even on panic or error.
pub struct InFlightGuard<'a>(pub &'a str);

impl<'a> Drop for InFlightGuard<'a> {
    fn drop(&mut self) {
        let mutex = shared_model_owner();
        let mut guard = mutex.lock().unwrap_or_else(|p| p.into_inner());
        guard.in_flight_installs.remove(self.0);
    }
}

/// Installs a model from archive outside the global model owner lock.
/// Short locks are acquired ONLY to reserve the install, update progress,
/// and commit the final status. Concurrent calls to snapshot/control-surface
/// are never blocked by SHA256 hashing or extraction.
pub fn perform_install_from_archive(
    model_id: &str,
    archive_path: &Path,
) -> Result<(), String> {
    let (entry, models_dir) = {
        let mutex = shared_model_owner();
        let mut guard = mutex.lock().unwrap_or_else(|p| p.into_inner());
        guard.begin_install(model_id)?
    };

    let _guard = InFlightGuard(model_id);
    let target_dir = models_dir.join(model_id);

    // Prevent overwriting assets while an active dictation session is using them
    let session_active_with_model = {
        let runtime_mutex = crate::runtime::shared_runtime();
        let runtime_guard = runtime_mutex.lock().unwrap_or_else(|p| p.into_inner());
        let active = runtime_guard.session_active();
        drop(runtime_guard);

        let owner_guard = shared_model_owner().lock().unwrap_or_else(|p| p.into_inner());
        active && owner_guard.current_model_dir() == Some(target_dir.clone())
    };
    if session_active_with_model {
        let mutex = shared_model_owner();
        let mut guard = mutex.lock().unwrap_or_else(|p| p.into_inner());
        guard.fail_install(model_id);
        return Err(format!(
            "Cannot replace model {} while dictation session is actively using it",
            model_id
        ));
    }

    let mut cb = |phase: echolet::models::progress::InstallPhase| {
        let mutex = shared_model_owner();
        let mut guard = mutex.lock().unwrap_or_else(|p| p.into_inner());
        guard.set_download_progress(model_id, DownloadStatus::from_phase(&phase));
    };
    let progress_cb: Option<echolet::models::progress::ProgressCallback<'_>> = Some(&mut cb);

    let result = echolet::models::installer::install_model_from_archive_with_progress(
        &entry,
        archive_path,
        &target_dir,
        progress_cb,
    );

    let mutex = shared_model_owner();
    let mut guard = mutex.lock().unwrap_or_else(|p| p.into_inner());
    match result {
        Ok(_) => {
            guard.complete_install(model_id);
            Ok(())
        }
        Err(e) => {
            guard.fail_install(model_id);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echolet::models::registry::VerificationStatus;
    use echolet::ui::control_surface::ModelPrimaryAction;

    fn temp_test_dirs(prefix: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "echolet-android-test-{}-{}-{}",
            prefix,
            std::process::id(),
            unique_nonce()
        ));
        let models = root.join("models");
        let files = root.join("files");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&models).unwrap();
        fs::create_dir_all(&files).unwrap();
        (models, files)
    }

    fn create_valid_model_dir(dir: &Path, entry: &RegistryModelEntry) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("model.json"), "{}").unwrap();
        fs::write(dir.join(&entry.files.encoder), b"encoder").unwrap();
        fs::write(dir.join(&entry.files.decoder), b"decoder").unwrap();
        fs::write(dir.join(&entry.files.joiner), b"joiner").unwrap();
        fs::write(dir.join(&entry.files.tokens), b"tokens").unwrap();
    }

    #[test]
    fn test_canonical_catalog_groups_and_verification() {
        let owner = AndroidModelOwner::empty();
        let registry = owner.registry();
        assert_eq!(registry.models.len(), 4, "Registry must contain exactly 4 models");

        let default_entry = registry.get_model(&registry.default_model_id).unwrap();
        assert_eq!(
            default_entry.verification_status,
            VerificationStatus::EcholetVerified,
            "Default X-ASR model must be Echolet Verified"
        );

        for entry in &registry.models {
            if entry.id != registry.default_model_id {
                assert_eq!(
                    entry.verification_status,
                    VerificationStatus::Experimental,
                    "Model {} must have Experimental status",
                    entry.id
                );
            }
        }
    }

    #[test]
    fn test_legacy_bilingual_zh_en_recognized_in_situ() {
        let (models_dir, files_dir) = temp_test_dirs("legacy-insitu");
        let legacy_dir = models_dir.join(LEGACY_MODEL_DIR_NAME);
        let registry = ModelRegistry::canonical().unwrap();
        let xasr_entry = registry.get_model(&registry.default_model_id).unwrap();

        // Create legacy model in situ (simulating existing 600MB model on Pixel9a)
        create_valid_model_dir(&legacy_dir, xasr_entry);

        let owner = AndroidModelOwner::new(models_dir.clone(), files_dir);
        let installed = owner.installed_model_ids();
        assert!(
            installed.contains(&registry.default_model_id),
            "Legacy directory must resolve as installed default X-ASR model"
        );

        let resolved = owner.current_model_dir().expect("Current model dir must resolve");
        assert_eq!(
            resolved, legacy_dir,
            "Must resolve legacy directory in situ without moving or copying"
        );
        assert!(legacy_dir.exists(), "Legacy directory must remain in original place");
    }

    #[test]
    fn test_selection_persistence_and_reopen() {
        let (models_dir, files_dir) = temp_test_dirs("select-persist");
        let registry = ModelRegistry::canonical().unwrap();
        let xasr_entry = registry.get_model(&registry.default_model_id).unwrap();
        let kroko_entry = registry.get_model("echolet-kroko-streaming-en-2025-08-06-r1").unwrap();

        // Install both models
        create_valid_model_dir(&models_dir.join(LEGACY_MODEL_DIR_NAME), xasr_entry);
        create_valid_model_dir(&models_dir.join(&kroko_entry.id), kroko_entry);

        let mut owner = AndroidModelOwner::new(models_dir.clone(), files_dir.clone());
        assert_eq!(owner.selected_model_id(), xasr_entry.id);

        // Select Kroko while idle
        assert!(owner.select_model(&kroko_entry.id, false).is_ok());
        assert_eq!(owner.selected_model_id(), kroko_entry.id);

        // Reopen owner from disk
        let reopened = AndroidModelOwner::new(models_dir, files_dir);
        assert_eq!(
            reopened.selected_model_id(),
            kroko_entry.id,
            "Selection must persist across reopen"
        );
    }

    #[test]
    fn test_selection_rejected_when_session_active_or_uninstalled() {
        let (models_dir, files_dir) = temp_test_dirs("select-reject");
        let registry = ModelRegistry::canonical().unwrap();
        let xasr_entry = registry.get_model(&registry.default_model_id).unwrap();
        let kroko_entry = registry.get_model("echolet-kroko-streaming-en-2025-08-06-r1").unwrap();

        create_valid_model_dir(&models_dir.join(LEGACY_MODEL_DIR_NAME), xasr_entry);
        create_valid_model_dir(&models_dir.join(&kroko_entry.id), kroko_entry);

        let mut owner = AndroidModelOwner::new(models_dir, files_dir);

        // Active dictation session blocks model selection
        let res_active = owner.select_model(&kroko_entry.id, true);
        assert!(res_active.is_err(), "Must reject model switch when session is active");
        assert_eq!(owner.selected_model_id(), xasr_entry.id, "Previous selection must remain");

        // Uninstalled model blocks selection
        let uninstalled_id = "echolet-nemotron-speech-streaming-en-0.6b-560ms-int8-2026-04-25-r1";
        let res_uninstalled = owner.select_model(uninstalled_id, false);
        assert!(res_uninstalled.is_err(), "Must reject uninstalled model");
        assert_eq!(owner.selected_model_id(), xasr_entry.id, "Previous selection must remain");
    }

    #[test]
    fn test_download_status_and_actions_disabled_while_listening() {
        let (models_dir, files_dir) = temp_test_dirs("listening-actions");
        let registry = ModelRegistry::canonical().unwrap();
        let xasr_entry = registry.get_model(&registry.default_model_id).unwrap();
        create_valid_model_dir(&models_dir.join(LEGACY_MODEL_DIR_NAME), xasr_entry);

        let owner = AndroidModelOwner::new(models_dir, files_dir);

        // State while Listening
        let surface_listening = owner.build_surface_state(RuntimeState::Listening);
        assert_eq!(surface_listening.runtime_state, RuntimeState::Listening);

        for model in surface_listening.all_models() {
            if !model.selected {
                assert!(
                    !model.enabled,
                    "Non-selected model action must be disabled while Listening"
                );
            }
        }

        // State while Ready
        let surface_ready = owner.build_surface_state(RuntimeState::Ready);
        assert_eq!(surface_ready.runtime_state, RuntimeState::Ready);
        let uninstalled_model = surface_ready
            .all_models()
            .find(|m| !m.installed)
            .expect("Must have uninstalled model");
        assert_eq!(uninstalled_model.primary_action, ModelPrimaryAction::Download);
        assert!(uninstalled_model.enabled, "Download action must be enabled while Ready");
    }

    #[test]
    fn test_abandoned_staging_cleanup_preserves_old_backups() {
        let (models_dir, files_dir) = temp_test_dirs("clean-staging");
        let registry = ModelRegistry::canonical().unwrap();
        let xasr_entry = registry.get_model(&registry.default_model_id).unwrap();
        create_valid_model_dir(&models_dir.join(LEGACY_MODEL_DIR_NAME), xasr_entry);

        // Add abandoned staging residue
        let stage_dir = models_dir.join(".echolet-staging-12345");
        fs::create_dir_all(&stage_dir).unwrap();
        let old_dir = models_dir.join(".echolet-old-12345");
        fs::create_dir_all(&old_dir).unwrap();
        fs::write(old_dir.join("model.json"), b"{}").unwrap();
        let part_file = models_dir.join("partial.part");
        fs::write(&part_file, b"partial").unwrap();

        let owner = AndroidModelOwner::new(models_dir.clone(), files_dir);

        assert!(!stage_dir.exists(), "Staging dir must be cleaned");
        assert!(old_dir.exists(), "Old backup dir must be PRESERVED for manual recovery");
        assert!(old_dir.join("model.json").exists(), "Backup contents must survive");
        assert!(!part_file.exists(), "Partial file must be cleaned");
        assert!(
            models_dir.join(LEGACY_MODEL_DIR_NAME).exists(),
            "Committed model dir must not be touched"
        );
        let backups = owner.list_recoverable_backups();
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0], old_dir);
    }

    #[test]
    fn test_parse_download_status_wire_contract() {
        // Valid PascalCase phases
        assert_eq!(
            parse_download_status("Starting", 0, None).unwrap(),
            DownloadStatus::Starting
        );
        assert_eq!(
            parse_download_status("Downloading", 0, Some(100)).unwrap(),
            DownloadStatus::Downloading {
                downloaded_bytes: 0,
                total_bytes: Some(100),
            }
        );
        assert_eq!(
            parse_download_status("Downloading", 45, Some(100)).unwrap(),
            DownloadStatus::Downloading {
                downloaded_bytes: 45,
                total_bytes: Some(100),
            }
        );
        assert_eq!(
            parse_download_status("Downloading", 100, Some(100)).unwrap(),
            DownloadStatus::Downloading {
                downloaded_bytes: 100,
                total_bytes: Some(100),
            }
        );
        assert_eq!(
            parse_download_status("Verifying", 0, None).unwrap(),
            DownloadStatus::Verifying
        );
        assert_eq!(
            parse_download_status("Extracting", 0, None).unwrap(),
            DownloadStatus::Extracting
        );
        assert_eq!(
            parse_download_status("Installing", 0, None).unwrap(),
            DownloadStatus::Installing
        );
        assert_eq!(
            parse_download_status("Completed", 0, None).unwrap(),
            DownloadStatus::Completed
        );
        assert_eq!(
            parse_download_status("Failed", 0, None).unwrap(),
            DownloadStatus::Failed
        );

        // Unknown / lowercase phases MUST fail closed
        assert!(parse_download_status("starting", 0, None).is_err());
        assert!(parse_download_status("downloading", 45, Some(100)).is_err());
        assert!(parse_download_status("verifying", 0, None).is_err());
        assert!(parse_download_status("extracting", 0, None).is_err());
        assert!(parse_download_status("installing", 0, None).is_err());
        assert!(parse_download_status("completed", 0, None).is_err());
        assert!(parse_download_status("failed", 0, None).is_err());
        assert!(parse_download_status("bogus", 0, None).is_err());

        // Failed status must yield RetryDownload in UI control surface
        let (models_dir, files_dir) = temp_test_dirs("retry-action");
        let mut owner = AndroidModelOwner::new(models_dir, files_dir);
        let kroko_id = "echolet-kroko-streaming-en-2025-08-06-r1";
        owner.set_download_progress(kroko_id, DownloadStatus::Failed);

        let surface = owner.build_surface_state(RuntimeState::Ready);
        let kroko_model = surface
            .all_models()
            .find(|m| m.id == kroko_id)
            .expect("Kroko model must be in surface");
        assert_eq!(
            kroko_model.primary_action,
            ModelPrimaryAction::RetryDownload,
            "Failed download must produce RetryDownload action"
        );
        assert!(kroko_model.enabled, "RetryDownload must be enabled while Ready");
    }

    #[test]
    fn test_canonical_download_spec_lookup() {
        let owner = AndroidModelOwner::empty();
        let registry = owner.registry();

        // All 4 shipped models have valid HTTPS download URLs and SHA256 checksums
        for entry in &registry.models {
            let spec = owner
                .get_download_spec(&entry.id)
                .unwrap_or_else(|e| panic!("Model {} must have valid download spec: {}", entry.id, e));
            assert_eq!(spec.model_id, entry.id);
            assert!(
                spec.url.starts_with("https://"),
                "URL for {} must be HTTPS: {}",
                entry.id,
                spec.url
            );
            assert_eq!(Some(&spec.url), entry.source.url.as_ref());
            assert_eq!(Some(&spec.sha256), entry.source.sha256.as_ref());
            assert!(!spec.sha256.is_empty());
            assert_eq!(spec.download_size_bytes, entry.download_size_bytes);
            assert_eq!(spec.installed_size_bytes, entry.installed_size_bytes);
        }

        // Unknown model ID is strictly rejected
        let unknown = owner.get_download_spec("non-existent-model-id");
        assert!(unknown.is_err());
    }

    #[test]
    fn test_in_flight_install_reservation_and_duplicate_rejection() {
        let (models_dir, files_dir) = temp_test_dirs("inflight-dup");
        let mut owner = AndroidModelOwner::new(models_dir, files_dir);
        let kroko_id = "echolet-kroko-streaming-en-2025-08-06-r1";

        assert!(!owner.is_install_in_flight(kroko_id));
        let begin1 = owner.begin_install(kroko_id);
        assert!(begin1.is_ok(), "First install reservation must succeed");
        assert!(owner.is_install_in_flight(kroko_id));

        // Duplicate install for same model must be rejected
        let begin2 = owner.begin_install(kroko_id);
        assert!(begin2.is_err(), "Duplicate install must be rejected");

        // Failure cleans reservation
        owner.fail_install(kroko_id);
        assert!(!owner.is_install_in_flight(kroko_id));
        assert_eq!(owner.progress.get(kroko_id), Some(&DownloadStatus::Failed));

        // Can reserve again after failure
        let begin3 = owner.begin_install(kroko_id);
        assert!(begin3.is_ok(), "Retry install reservation must succeed");
        owner.complete_install(kroko_id);
        assert!(!owner.is_install_in_flight(kroko_id));
        assert_eq!(owner.progress.get(kroko_id), Some(&DownloadStatus::Completed));
    }

    #[test]
    fn test_concurrent_snapshot_does_not_block_during_in_flight_install() {
        let (models_dir, files_dir) = temp_test_dirs("concurrent-snap");
        let mut owner = AndroidModelOwner::new(models_dir, files_dir);
        let kroko_id = "echolet-kroko-streaming-en-2025-08-06-r1";

        // Reserve install and set progress
        owner.begin_install(kroko_id).unwrap();
        owner.set_download_progress(kroko_id, DownloadStatus::Installing);

        // Building snapshot completes immediately and reflects Installing status
        let start = std::time::Instant::now();
        let json = owner.build_snapshot_json(RuntimeState::Ready).unwrap();
        let duration = start.elapsed();
        assert!(
            duration < std::time::Duration::from_millis(50),
            "Snapshot build took {:?}, must complete within tight bound",
            duration
        );

        assert!(json.contains("Installing"));
        assert!(json.contains(kroko_id));
    }

    #[test]
    fn test_service_recreation_during_in_flight_install_preserves_staging_and_cleans_afterwards() {
        let (models_dir, files_dir) = temp_test_dirs("recreate-staging");
        let registry = ModelRegistry::canonical().unwrap();
        let xasr_entry = registry.get_model(&registry.default_model_id).unwrap();
        create_valid_model_dir(&models_dir.join(LEGACY_MODEL_DIR_NAME), xasr_entry);

        let mut owner = AndroidModelOwner::new(models_dir.clone(), files_dir.clone());
        assert_eq!(owner.selected_model_id(), xasr_entry.id);

        let kroko_id = "echolet-kroko-streaming-en-2025-08-06-r1";
        // 1. Reserve install
        owner.begin_install(kroko_id).expect("begin_install should succeed");
        assert!(owner.is_install_in_flight(kroko_id));

        // 2. Create .echolet-staging-X, .echolet-commit-X, and .echolet-old-X under models
        let staging_dir = models_dir.join(".echolet-staging-active-123");
        let commit_dir = models_dir.join(".echolet-commit-active-123");
        let old_dir = models_dir.join(".echolet-old-backup-123");
        fs::create_dir_all(&staging_dir).unwrap();
        fs::write(staging_dir.join("temp.bin"), b"staging-data").unwrap();
        fs::create_dir_all(&commit_dir).unwrap();
        fs::write(commit_dir.join("temp.bin"), b"commit-data").unwrap();
        fs::create_dir_all(&old_dir).unwrap();
        fs::write(old_dir.join("model.json"), b"{}").unwrap();

        // 3. Call owner.initialize on same instance (simulating IME service recreation)
        owner.initialize(models_dir.clone(), files_dir.clone());

        // 4. Assert both staging and commit survive, and selected model persists
        assert!(staging_dir.exists(), "Active staging must survive service recreation");
        assert!(commit_dir.exists(), "Active commit must survive service recreation");
        assert!(old_dir.exists(), "Recovery backup must survive");
        assert_eq!(owner.selected_model_id(), xasr_entry.id, "Selected model must persist");
        assert!(owner.is_install_in_flight(kroko_id), "In-flight reservation must be retained");

        // 5. Finish / release reservation
        owner.complete_install(kroko_id);
        assert!(!owner.is_install_in_flight(kroko_id));

        // 6. Next init cleans abandoned staging but still preserves .echolet-old backup
        owner.initialize(models_dir.clone(), files_dir.clone());
        assert!(!staging_dir.exists(), "Staging must now be cleaned once install completes");
        assert!(!commit_dir.exists(), "Commit dir must now be cleaned once install completes");
        assert!(old_dir.exists(), "Recovery backup must still be preserved");
        assert_eq!(owner.selected_model_id(), xasr_entry.id);
    }

    #[test]
    fn test_missing_default_model_returns_download_action_and_selected_after_install() {
        let (models_dir, files_dir) = temp_test_dirs("missing-default");
        let registry = ModelRegistry::canonical().unwrap();
        let xasr_id = &registry.default_model_id;
        let xasr_entry = registry.get_model(xasr_id).unwrap();

        // Fresh owner with empty models directory (default X-ASR not installed)
        let owner = AndroidModelOwner::new(models_dir.clone(), files_dir.clone());
        assert_eq!(owner.selected_model_id(), xasr_id);
        assert!(!owner.installed_model_ids().contains(xasr_id));
        assert!(owner.current_model_dir().is_none());

        // 1. Snapshot JSON when idle / not listening
        let snapshot_json = owner.build_snapshot_json(RuntimeState::Ready).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&snapshot_json).unwrap();
        assert_eq!(parsed["selected_model_id"], *xasr_id);
        assert!(parsed["selected_model_dir"].is_null());
        assert_eq!(parsed["runtime_state"], "Ready");

        let groups = parsed["model_groups"].as_array().unwrap();
        let mut found_xasr = None;
        for g in groups {
            for m in g["models"].as_array().unwrap() {
                if m["id"] == *xasr_id {
                    found_xasr = Some(m.clone());
                    break;
                }
            }
        }
        let xasr_val = found_xasr.expect("Default X-ASR model must be present in snapshot");
        assert_eq!(xasr_val["selected"], true);
        assert_eq!(xasr_val["installed"], false);
        assert_eq!(xasr_val["primary_action"], "Download");
        assert_eq!(xasr_val["enabled"], true);

        // 2. Snapshot JSON when Listening: primary_action is Download but enabled is false
        let listening_snapshot_json = owner.build_snapshot_json(RuntimeState::Listening).unwrap();
        let parsed_listening: serde_json::Value = serde_json::from_str(&listening_snapshot_json).unwrap();
        assert_eq!(parsed_listening["runtime_state"], "Listening");
        let mut found_xasr_listening = None;
        for g in parsed_listening["model_groups"].as_array().unwrap() {
            for m in g["models"].as_array().unwrap() {
                if m["id"] == *xasr_id {
                    found_xasr_listening = Some(m.clone());
                    break;
                }
            }
        }
        let xasr_val_listening = found_xasr_listening.unwrap();
        assert_eq!(xasr_val_listening["selected"], true);
        assert_eq!(xasr_val_listening["installed"], false);
        assert_eq!(xasr_val_listening["primary_action"], "Download");
        assert_eq!(xasr_val_listening["enabled"], false);

        // 3. Install default model fixture and verify transition to installed/Selected
        create_valid_model_dir(&models_dir.join(LEGACY_MODEL_DIR_NAME), xasr_entry);
        let owner_installed = AndroidModelOwner::new(models_dir, files_dir);
        assert!(owner_installed.installed_model_ids().contains(xasr_id));
        assert!(owner_installed.current_model_dir().is_some());

        let installed_json = owner_installed.build_snapshot_json(RuntimeState::Ready).unwrap();
        let parsed_installed: serde_json::Value = serde_json::from_str(&installed_json).unwrap();
        assert_eq!(parsed_installed["selected_model_id"], *xasr_id);
        assert!(parsed_installed["selected_model_dir"].is_string());

        let mut found_xasr_installed = None;
        for g in parsed_installed["model_groups"].as_array().unwrap() {
            for m in g["models"].as_array().unwrap() {
                if m["id"] == *xasr_id {
                    found_xasr_installed = Some(m.clone());
                    break;
                }
            }
        }
        let xasr_installed_val = found_xasr_installed.unwrap();
        assert_eq!(xasr_installed_val["selected"], true);
        assert_eq!(xasr_installed_val["installed"], true);
        assert_eq!(xasr_installed_val["primary_action"], "None");
        assert_eq!(xasr_installed_val["enabled"], false);
    }
}
