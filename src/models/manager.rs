use crate::config::EcholetConfig;
use crate::models::download::{download_and_install_model_with_progress, ProgressCallback};
use crate::models::manifest::ModelManifest;
use crate::models::registry::ModelRegistry;
use crate::paths;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct InstalledModel {
    pub id: String,
    pub dir: PathBuf,
    pub manifest: ModelManifest,
    pub is_bundled: bool,
}

pub struct ModelManager {
    pub registry: ModelRegistry,
    pub installed: HashMap<String, InstalledModel>,
    pub active_model_id: Option<String>,
    pub downloading: HashSet<String>,
    pub bundled_models_dir: PathBuf,
    pub user_models_dir: PathBuf,
    pub config_path: PathBuf,
}

impl ModelManager {
    pub fn new() -> Result<Self, String> {
        let res_root = paths::resource_root();
        Self::new_with_paths(
            res_root.join("models"),
            paths::user_models_dir(),
            paths::user_config_path(),
        )
    }

    pub fn new_with_paths(
        bundled_models_dir: PathBuf,
        user_models_dir: PathBuf,
        config_path: PathBuf,
    ) -> Result<Self, String> {
        let registry_path = bundled_models_dir.join("registry.json");
        let registry = if registry_path.exists() {
            ModelRegistry::from_file(&registry_path)?
        } else {
            // Fallback to embedded default registry if file missing
            let default_str = include_str!("../../models/registry.json");
            ModelRegistry::from_str(default_str)?
        };

        let mut manager = Self {
            registry,
            installed: HashMap::new(),
            active_model_id: None,
            downloading: HashSet::new(),
            bundled_models_dir,
            user_models_dir,
            config_path,
        };

        manager.discover_installed();
        manager.resolve_active_model();

        Ok(manager)
    }

    /// Resolves active model: saved selected model if installed -> registry default if installed -> first installed.
    /// If no models are installed, leaves active model absent (None).
    pub fn resolve_active_model(&mut self) -> Option<String> {
        let saved_config = self.load_config();
        let chosen_id = saved_config
            .and_then(|c| {
                if self.installed.contains_key(&c.selected_model) {
                    Some(c.selected_model)
                } else {
                    None
                }
            })
            .or_else(|| {
                let default_id = self.registry.default_model_id.clone();
                if self.installed.contains_key(&default_id) {
                    Some(default_id)
                } else {
                    None
                }
            })
            .or_else(|| self.installed.keys().next().cloned());

        self.active_model_id = chosen_id.clone();
        chosen_id
    }

    pub fn discover_installed(&mut self) {
        self.installed.clear();

        // 1. Scan bundled models directory (<resource_root>/models)
        self.scan_directory(&self.bundled_models_dir.clone(), true);

        // 2. Scan user models directory (~/.echolet/models)
        // User models override or complement bundled models
        self.scan_directory(&self.user_models_dir.clone(), false);
    }

    fn scan_directory(&mut self, dir: &Path, is_bundled: bool) {
        if !dir.exists() {
            return;
        }

        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                // Check for model.json
                let manifest_path = path.join("model.json");
                if manifest_path.exists() {
                    if let Ok(manifest) = ModelManifest::from_file(&manifest_path) {
                        if manifest.validate_files(&path).is_ok() {
                            self.installed.insert(
                                manifest.id.clone(),
                                InstalledModel {
                                    id: manifest.id.clone(),
                                    dir: path.clone(),
                                    manifest,
                                    is_bundled,
                                },
                            );
                            continue;
                        }
                    }
                }

                // If model.json is absent, match against registry entries
                let dir_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                for reg in &self.registry.models {
                    if reg.id == dir_name
                        || dir_name.starts_with(&reg.language)
                        || dir_name.contains("bilingual")
                    {
                        let manifest = reg.to_manifest();
                        if manifest.validate_files(&path).is_ok() {
                            self.installed.insert(
                                reg.id.clone(),
                                InstalledModel {
                                    id: reg.id.clone(),
                                    dir: path.clone(),
                                    manifest,
                                    is_bundled,
                                },
                            );
                            break;
                        }
                    }
                }
            }
        }
    }

    pub fn active_model_id(&self) -> Option<&str> {
        self.active_model_id.as_deref()
    }

    pub fn get_active_model(&self) -> Result<&InstalledModel, String> {
        let active_id = self
            .active_model_id
            .as_deref()
            .ok_or_else(|| "No active model installed".to_string())?;
        self.installed
            .get(active_id)
            .ok_or_else(|| format!("Active model '{}' is not installed", active_id))
    }

    pub fn get_model(&self, id: &str) -> Option<&InstalledModel> {
        self.installed.get(id)
    }

    pub fn is_installed(&self, id: &str) -> bool {
        self.installed.contains_key(id)
    }

    pub fn get_user_install_dir(&self, model_id: &str) -> PathBuf {
        self.user_models_dir.join(model_id)
    }

    /// Productized API: downloads and atomically installs a registry model by ID.
    ///
    /// The manager exclusively owns the `downloading` state for this model:
    /// * a duplicate concurrent download for the same ID is rejected;
    /// * the ID is inserted before any work and removed on every exit path.
    ///
    /// On success the freshly installed model is registered from its actual
    /// on-disk manifest. The active model is intentionally **not** changed; the
    /// caller must explicitly call [`ModelManager::set_active_model`].
    pub fn install_registry_model(
        &mut self,
        model_id: &str,
        progress: Option<ProgressCallback<'_>>,
    ) -> Result<InstalledModel, String> {
        if self.downloading.contains(model_id) {
            return Err(format!("Model '{}' is already downloading", model_id));
        }

        let entry = self
            .registry
            .get_model(model_id)
            .cloned()
            .ok_or_else(|| format!("Model '{}' not found in registry", model_id))?;

        let target_dir = self.get_user_install_dir(model_id);
        self.downloading.insert(model_id.to_string());

        let result = download_and_install_model_with_progress(&entry, &target_dir, progress);

        // Always clear downloading state, regardless of outcome.
        self.downloading.remove(model_id);

        let downloaded_manifest = result?;

        // Prefer the manifest actually written on disk, falling back to the one
        // produced by the downloader.
        let manifest =
            ModelManifest::from_file(&target_dir.join("model.json")).unwrap_or(downloaded_manifest);
        let installed = InstalledModel {
            id: manifest.id.clone(),
            dir: target_dir,
            manifest,
            is_bundled: false,
        };
        self.installed
            .insert(installed.id.clone(), installed.clone());
        Ok(installed)
    }

    pub fn register_installed(&mut self, manifest: ModelManifest, dir: PathBuf) {
        let id = manifest.id.clone();
        self.downloading.remove(&id);
        self.installed.insert(
            id.clone(),
            InstalledModel {
                id,
                dir,
                manifest,
                is_bundled: false,
            },
        );
    }

    pub fn set_active_model(&mut self, model_id: &str) -> Result<&InstalledModel, String> {
        if !self.installed.contains_key(model_id) {
            return Err(format!("Model '{}' is not installed", model_id));
        }
        self.active_model_id = Some(model_id.to_string());
        let _ = self.save_config();
        Ok(self.installed.get(model_id).unwrap())
    }

    pub fn load_config(&self) -> Option<EcholetConfig> {
        if self.config_path.exists() {
            Some(EcholetConfig::load_from(&self.config_path))
        } else {
            None
        }
    }

    pub fn save_config(&self) -> Result<(), String> {
        let mut config = EcholetConfig::load_from(&self.config_path);
        if let Some(ref active) = self.active_model_id {
            config.selected_model = active.clone();
        }
        config.save_to(&self.config_path)
    }
}
