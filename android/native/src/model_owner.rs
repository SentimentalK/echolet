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

/// Platform-neutral model owner for Android.
pub struct AndroidModelOwner {
    models_dir: Option<PathBuf>,
    files_dir: Option<PathBuf>,
    registry: ModelRegistry,
    selected_model_id: Option<String>,
    progress: HashMap<String, DownloadStatus>,
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

        // Clean up any abandoned staging residue from killed processes
        self.clean_abandoned_staging(&models_dir);

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

    /// Installs a model from an already-staged archive file.
    ///
    /// Verifies SHA256 against catalog, unpacks safely without path traversal,
    /// validates manifest/model files, and commits atomically.
    ///
    /// IMPORTANT: Complete install does NOT auto-select.
    pub fn install_from_archive(
        &mut self,
        model_id: &str,
        archive_path: &Path,
    ) -> Result<(), String> {
        let entry = self
            .registry
            .get_model(model_id)
            .ok_or_else(|| format!("Unknown model ID: {}", model_id))?;

        let models_dir = self
            .models_dir
            .as_ref()
            .ok_or_else(|| "Models directory not initialized".to_string())?;

        let target_dir = models_dir.join(model_id);

        self.progress
            .insert(model_id.to_string(), DownloadStatus::Installing);

        match install_model_from_archive(entry, archive_path, &target_dir) {
            Ok(_) => {
                self.progress
                    .insert(model_id.to_string(), DownloadStatus::Completed);
                Ok(())
            }
            Err(e) => {
                self.progress
                    .insert(model_id.to_string(), DownloadStatus::Failed);
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
            if self.registry.get_model(candidate).is_some()
                && self.resolve_model_dir(candidate).is_some()
            {
                self.selected_model_id = Some(candidate.to_string());
                return;
            }
        }

        // Fall back to default model if installed, otherwise keep default model id
        if self.resolve_model_dir(&self.registry.default_model_id).is_some() {
            self.selected_model_id = Some(self.registry.default_model_id.clone());
        }
    }

    fn clean_abandoned_staging(&self, models_dir: &Path) {
        if let Ok(entries) = fs::read_dir(models_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with(".echolet-staging-")
                    || name_str.starts_with(".echolet-old-")
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
    fn test_abandoned_staging_cleanup() {
        let (models_dir, files_dir) = temp_test_dirs("clean-staging");
        let registry = ModelRegistry::canonical().unwrap();
        let xasr_entry = registry.get_model(&registry.default_model_id).unwrap();
        create_valid_model_dir(&models_dir.join(LEGACY_MODEL_DIR_NAME), xasr_entry);

        // Add abandoned staging residue
        let stage_dir = models_dir.join(".echolet-staging-12345");
        fs::create_dir_all(&stage_dir).unwrap();
        let old_dir = models_dir.join(".echolet-old-12345");
        fs::create_dir_all(&old_dir).unwrap();
        let part_file = models_dir.join("partial.part");
        fs::write(&part_file, b"partial").unwrap();

        let _owner = AndroidModelOwner::new(models_dir.clone(), files_dir);

        assert!(!stage_dir.exists(), "Staging dir must be cleaned");
        assert!(!old_dir.exists(), "Old backup dir must be cleaned");
        assert!(!part_file.exists(), "Partial file must be cleaned");
        assert!(
            models_dir.join(LEGACY_MODEL_DIR_NAME).exists(),
            "Committed model dir must not be touched"
        );
    }
}
