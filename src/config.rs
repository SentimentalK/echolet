use crate::paths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EcholetConfig {
    #[serde(default = "default_selected_model")]
    pub selected_model: String,
    #[serde(default)]
    pub history_enabled: bool,
    #[serde(default = "default_preload_model_on_startup")]
    pub preload_model_on_startup: bool,
    #[serde(default = "default_model_idle_unload_minutes")]
    pub model_idle_unload_minutes: Option<u32>,
    /// Per-model forced-language preference: model id -> BCP-47 locale or
    /// [`EcholetConfig::LANGUAGE_AUTO`]. Stored per model (never a single global
    /// locale) so switching models cannot carry an invalid locale across.
    /// Defaults to empty for backward-compatible parsing of legacy configs.
    #[serde(default)]
    pub model_language_preferences: BTreeMap<String, String>,
}

fn default_selected_model() -> String {
    "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1".to_string()
}

fn default_preload_model_on_startup() -> bool {
    false
}

fn default_model_idle_unload_minutes() -> Option<u32> {
    Some(10)
}

impl Default for EcholetConfig {
    fn default() -> Self {
        Self {
            selected_model: default_selected_model(),
            history_enabled: false,
            preload_model_on_startup: default_preload_model_on_startup(),
            model_idle_unload_minutes: default_model_idle_unload_minutes(),
            model_language_preferences: BTreeMap::new(),
        }
    }
}

impl EcholetConfig {
    /// Sentinel stored when the user chooses Auto-detect for a model.
    pub const LANGUAGE_AUTO: &'static str = "auto";

    pub fn language_preference(&self, model_id: &str) -> Option<&str> {
        self.model_language_preferences
            .get(model_id)
            .map(String::as_str)
    }

    /// Stores the preference for one model. `None` stores the explicit Auto
    /// sentinel so a user's "Auto" choice is remembered without becoming a
    /// global setting.
    pub fn set_language_preference(&mut self, model_id: &str, locale: Option<&str>) {
        let value = locale
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or(Self::LANGUAGE_AUTO)
            .to_string();
        self.model_language_preferences
            .insert(model_id.to_string(), value);
    }

    pub fn language_preference_is_auto(&self, model_id: &str) -> bool {
        matches!(
            self.language_preference(model_id),
            None | Some(EcholetConfig::LANGUAGE_AUTO)
        )
    }

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
        fs::write(path, content).map_err(|e| format!("Failed to write config {:?}: {}", path, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_legacy_config_missing_fields_defaults() {
        let legacy_json = r#"{"selected_model": "custom-model", "history_enabled": true}"#;
        let config: EcholetConfig = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(config.selected_model, "custom-model");
        assert!(config.history_enabled);
        assert_eq!(config.preload_model_on_startup, false);
        assert_eq!(config.model_idle_unload_minutes, Some(10));
    }

    #[test]
    fn test_legacy_config_without_language_preferences_parses() {
        // A pre-J11 config has no model_language_preferences field at all and
        // must still parse, defaulting to an empty per-model map.
        let legacy_json = r#"{
            "selected_model": "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1",
            "history_enabled": false,
            "preload_model_on_startup": true,
            "model_idle_unload_minutes": 5
        }"#;
        let config: EcholetConfig = serde_json::from_str(legacy_json).unwrap();
        assert!(config.model_language_preferences.is_empty());
        assert!(config.language_preference_is_auto("some-model"));
    }

    #[test]
    fn test_per_model_language_preference_round_trip() {
        let mut config = EcholetConfig::default();
        config.set_language_preference("model-a", Some("ja-JP"));
        config.set_language_preference("model-b", None); // explicit Auto
        let json = serde_json::to_string(&config).unwrap();
        let back: EcholetConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.language_preference("model-a"), Some("ja-JP"));
        assert_eq!(
            back.language_preference("model-b"),
            Some(EcholetConfig::LANGUAGE_AUTO)
        );
        assert!(!back.language_preference_is_auto("model-a"));
        assert!(back.language_preference_is_auto("model-b"));
        // A model with no stored preference is also Auto.
        assert!(back.language_preference_is_auto("model-c"));
    }

    #[test]
    fn test_config_serialize_deserialize_matrix() {
        // Case 1: preload=false, idle=Some(0)
        let c1 = EcholetConfig {
            selected_model: default_selected_model(),
            history_enabled: false,
            preload_model_on_startup: false,
            model_idle_unload_minutes: Some(0),
            model_language_preferences: BTreeMap::new(),
        };
        let s1 = serde_json::to_string(&c1).unwrap();
        let d1: EcholetConfig = serde_json::from_str(&s1).unwrap();
        assert_eq!(c1, d1);

        // Case 2: preload=true, idle=Some(10)
        let c2 = EcholetConfig {
            selected_model: default_selected_model(),
            history_enabled: true,
            preload_model_on_startup: true,
            model_idle_unload_minutes: Some(10),
            model_language_preferences: BTreeMap::new(),
        };
        let s2 = serde_json::to_string(&c2).unwrap();
        let d2: EcholetConfig = serde_json::from_str(&s2).unwrap();
        assert_eq!(c2, d2);

        // Case 3: preload=false, idle=None (null)
        let c3 = EcholetConfig {
            selected_model: default_selected_model(),
            history_enabled: false,
            preload_model_on_startup: false,
            model_idle_unload_minutes: None,
            model_language_preferences: BTreeMap::new(),
        };
        let s3 = serde_json::to_string(&c3).unwrap();
        assert!(s3.contains("\"model_idle_unload_minutes\":null"));
        let d3: EcholetConfig = serde_json::from_str(&s3).unwrap();
        assert_eq!(c3, d3);

        // Case 4: preload=true, idle=Some(30)
        let c4 = EcholetConfig {
            selected_model: default_selected_model(),
            history_enabled: false,
            preload_model_on_startup: true,
            model_idle_unload_minutes: Some(30),
            model_language_preferences: BTreeMap::new(),
        };
        let s4 = serde_json::to_string(&c4).unwrap();
        let d4: EcholetConfig = serde_json::from_str(&s4).unwrap();
        assert_eq!(c4, d4);
    }
}
