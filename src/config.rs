use crate::paths;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EcholetConfig {
    #[serde(default = "default_selected_model")]
    pub selected_model: String,
    #[serde(default)]
    pub history_enabled: bool,
}

fn default_selected_model() -> String {
    "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1".to_string()
}

impl Default for EcholetConfig {
    fn default() -> Self {
        Self {
            selected_model: default_selected_model(),
            history_enabled: false,
        }
    }
}

impl EcholetConfig {
    pub fn load() -> Self {
        Self::load_from(&paths::user_config_path())
    }

    pub fn load_from(path: &Path) -> Self {
        if path.exists() {
            if let Ok(content) = fs::read_to_string(path) {
                if let Ok(config) = serde_json::from_str::<EcholetConfig>(&content) {
                    return config;
                }
            }
        }

        // Backward compatibility fallback to legacy paths if primary ~/.echolet/config.json doesn't exist
        for legacy_path in paths::legacy_config_paths() {
            if legacy_path.exists() {
                if let Ok(content) = fs::read_to_string(&legacy_path) {
                    if let Ok(config) = serde_json::from_str::<EcholetConfig>(&content) {
                        return config;
                    }
                }
            }
        }

        Self::default()
    }

    pub fn save(&self) -> Result<(), String> {
        Self::save_to(self, &paths::user_config_path())
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        fs::write(path, content)
            .map_err(|e| format!("Failed to write config {:?}: {}", path, e))
    }
}
