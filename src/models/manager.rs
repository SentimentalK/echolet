use crate::models::manifest::ModelManifest;
use crate::models::registry::ModelRegistry;
use crate::paths;
use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EcholetConfig {
    pub selected_model: String,
}

pub struct ModelManager {
    pub registry: ModelRegistry,
    pub installed: HashMap<String, InstalledModel>,
    pub active_model_id: String,
    pub downloading: HashSet<String>,
    pub bundled_models_dir: PathBuf,
    pub user_models_dir: PathBuf,
    pub config_path: PathBuf,
}

impl ModelManager {
    pub fn new() -> Result<Self, String> {
        let res_root = paths::resource_root();
        let registry_path = res_root.join("models/registry.json");

        let registry = if registry_path.exists() {
            ModelRegistry::from_file(&registry_path)?
        } else {
            // Fallback to embedded default registry if file missing
            let default_str = include_str!("../../models/registry.json");
            ModelRegistry::from_str(default_str)?
        };

        let bundled_models_dir = res_root.join("models");
        let user_models_dir = paths::user_models_dir();
        let config_path = paths::user_config_path();

        let mut manager = Self {
            registry,
            installed: HashMap::new(),
            active_model_id: String::new(),
            downloading: HashSet::new(),
            bundled_models_dir,
            user_models_dir,
            config_path,
        };

        manager.discover_installed();

        // Resolve active model: Config -> Default bundled -> First available
        let saved_config = manager.load_config();
        let chosen_id = saved_config
            .and_then(|c| {
                if manager.installed.contains_key(&c.selected_model) {
                    Some(c.selected_model)
                } else {
                    None
                }
            })
            .or_else(|| {
                let default_id = manager.registry.default_model_id.clone();
                if manager.installed.contains_key(&default_id) {
                    Some(default_id)
                } else {
                    None
                }
            })
            .or_else(|| manager.installed.keys().next().cloned())
            .ok_or_else(|| {
                format!(
                    "No installed models found in bundled dir {:?} or user dir {:?}",
                    manager.bundled_models_dir, manager.user_models_dir
                )
            })?;

        manager.active_model_id = chosen_id;
        Ok(manager)
    }

    pub fn discover_installed(&mut self) {
        self.installed.clear();

        // 1. Scan bundled models directory (<resource_root>/models)
        self.scan_directory(&self.bundled_models_dir.clone(), true);

        // 2. Scan user models directory (~/.local/share/echolet/models)
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
                    if reg.id == dir_name || dir_name.starts_with(&reg.language) || dir_name.contains("bilingual") {
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

    pub fn get_active_model(&self) -> Result<&InstalledModel, String> {
        self.installed
            .get(&self.active_model_id)
            .ok_or_else(|| format!("Active model '{}' is not installed", self.active_model_id))
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
        self.active_model_id = model_id.to_string();
        let _ = self.save_config();
        Ok(self.installed.get(model_id).unwrap())
    }

    pub fn load_config(&self) -> Option<EcholetConfig> {
        if self.config_path.exists() {
            if let Ok(content) = fs::read_to_string(&self.config_path) {
                if let Ok(config) = serde_json::from_str::<EcholetConfig>(&content) {
                    return Some(config);
                }
            }
        }
        None
    }

    pub fn save_config(&self) -> Result<(), String> {
        if let Some(parent) = self.config_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let config = EcholetConfig {
            selected_model: self.active_model_id.clone(),
        };
        let content = serde_json::to_string_pretty(&config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        fs::write(&self.config_path, content)
            .map_err(|e| format!("Failed to write config {:?}: {}", self.config_path, e))
    }
}
