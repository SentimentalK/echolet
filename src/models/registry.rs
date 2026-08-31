use crate::models::manifest::ModelManifest;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelSource {
    #[serde(default)]
    pub bundled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelFilesConfig {
    pub encoder: String,
    pub decoder: String,
    pub joiner: String,
    pub tokens: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelRuntimeConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_type: Option<String>,
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    #[serde(default = "default_feature_dim")]
    pub feature_dim: i32,
    #[serde(default = "default_num_threads")]
    pub num_threads: i32,
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_decoding_method")]
    pub decoding_method: String,
    #[serde(default = "default_max_active_paths")]
    pub max_active_paths: i32,
}

fn default_sample_rate() -> u32 {
    16000
}
fn default_feature_dim() -> i32 {
    80
}
fn default_num_threads() -> i32 {
    1
}
fn default_provider() -> String {
    "cpu".into()
}
fn default_decoding_method() -> String {
    "greedy_search".into()
}
fn default_max_active_paths() -> i32 {
    4
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistryModelEntry {
    pub id: String,
    pub display_name: String,
    pub version: String,
    pub language: String,
    pub family: String,
    pub source: ModelSource,
    pub files: ModelFilesConfig,
    pub runtime: ModelRuntimeConfig,
}

impl RegistryModelEntry {
    pub fn display_title(&self) -> String {
        format!("{} — {}", self.display_name, self.version)
    }

    pub fn to_manifest(&self) -> ModelManifest {
        ModelManifest {
            id: self.id.clone(),
            display_name: self.display_name.clone(),
            version: self.version.clone(),
            language: self.language.clone(),
            family: self.family.clone(),
            encoder: self.files.encoder.clone(),
            decoder: self.files.decoder.clone(),
            joiner: self.files.joiner.clone(),
            tokens: self.files.tokens.clone(),
            model_type: self.runtime.model_type.clone(),
            sample_rate: self.runtime.sample_rate,
            feature_dim: self.runtime.feature_dim,
            num_threads: self.runtime.num_threads,
            provider: self.runtime.provider.clone(),
            decoding_method: self.runtime.decoding_method.clone(),
            max_active_paths: self.runtime.max_active_paths,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelRegistry {
    pub schema_version: u32,
    pub default_model_id: String,
    pub models: Vec<RegistryModelEntry>,
}

impl ModelRegistry {
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read registry from {:?}: {}", path, e))?;
        Self::from_str(&content)
    }

    pub fn from_str(content: &str) -> Result<Self, String> {
        serde_json::from_str(content)
            .map_err(|e| format!("Failed to parse registry JSON: {}", e))
    }

    pub fn get_model(&self, id: &str) -> Option<&RegistryModelEntry> {
        self.models.iter().find(|m| m.id == id)
    }

    pub fn default_entry(&self) -> Option<&RegistryModelEntry> {
        self.get_model(&self.default_model_id)
            .or_else(|| self.models.first())
    }
}
