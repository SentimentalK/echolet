use crate::models::language::{self, deserialize_languages};
use crate::models::manifest::ModelManifest;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Current canonical registry schema version emitted by this build.
pub const CURRENT_SCHEMA_VERSION: u32 = 2;

/// Legacy registry schema version still accepted by the parser.
pub const LEGACY_SCHEMA_VERSION: u32 = 1;

fn default_schema_version() -> u32 {
    LEGACY_SCHEMA_VERSION
}

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

/// Structured license metadata.
///
/// At minimum an SPDX identifier or a human-readable name is expected; the URL
/// is optional. Individual fields are optional so a partially known license
/// never makes a model unusable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ModelLicense {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spdx: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Typed product verification status.
///
/// The default ([`VerificationStatus::Experimental`]) is deliberately the most
/// conservative non-Verified state: legacy input that omits
/// `verification_status` must never be treated as [`VerificationStatus::EcholetVerified`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum VerificationStatus {
    #[serde(rename = "Echolet Verified")]
    EcholetVerified,
    #[serde(rename = "Community")]
    Community,
    #[serde(rename = "Experimental")]
    #[default]
    Experimental,
}

impl VerificationStatus {
    /// Whether this status represents the fully product-verified state.
    pub fn is_verified(self) -> bool {
        matches!(self, VerificationStatus::EcholetVerified)
    }

    /// Canonical product label for this status.
    pub fn label(self) -> &'static str {
        match self {
            VerificationStatus::EcholetVerified => "Echolet Verified",
            VerificationStatus::Community => "Community",
            VerificationStatus::Experimental => "Experimental",
        }
    }
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
    /// Canonical plural language list. Legacy `"language": "zh-en"` input is
    /// accepted through the shared [`deserialize_languages`] seam and
    /// normalized centrally; there is no competing in-memory `language` field.
    #[serde(alias = "language", deserialize_with = "deserialize_languages")]
    pub languages: Vec<String>,
    pub family: String,
    pub source: ModelSource,
    pub files: ModelFilesConfig,
    pub runtime: ModelRuntimeConfig,
    /// Length in bytes of the immutable source archive at `source.url`, as
    /// advertised by the release host. `None` when not authoritatively known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_size_bytes: Option<u64>,
    /// Deterministic installed footprint: the sum of the four canonical
    /// installed model-pack files (`files.encoder`, `files.decoder`,
    /// `files.joiner`, `files.tokens`), excluding the generated `model.json`.
    /// `None` when not authoritatively known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_release_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<ModelLicense>,
    #[serde(default)]
    pub verification_status: VerificationStatus,
}

impl RegistryModelEntry {
    pub fn display_title(&self) -> String {
        format!("{} — {}", self.display_name, self.version)
    }

    /// Whether this model supports every code in `code` (which itself may be a
    /// legacy composite spec such as `"zh-en"`).
    pub fn supports_language(&self, code: &str) -> bool {
        let requested = language::normalize_language_spec(code);
        if requested.is_empty() {
            return false;
        }
        requested
            .iter()
            .all(|req| self.languages.iter().any(|lang| lang == req))
    }

    /// First (primary) canonical language code, if any.
    pub fn primary_language(&self) -> Option<&str> {
        self.languages.first().map(String::as_str)
    }

    /// Stable canonical language key (for example `"zh-en"`).
    pub fn language_key(&self) -> String {
        language::language_key(&self.languages)
    }

    /// Human-readable language label (for example `"Chinese + English"`).
    pub fn language_label(&self) -> String {
        language::language_label(&self.languages)
    }

    /// Heuristic match for an on-disk install directory when no `model.json` is
    /// present. Matching is based on the explicit model id or the normalized
    /// language codes, never on ad-hoc string joining.
    pub fn matches_install_dir(&self, dir_name: &str) -> bool {
        if dir_name == self.id {
            return true;
        }
        if self.languages.is_empty() {
            return false;
        }
        let tokens: Vec<String> = dir_name
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|token| !token.is_empty())
            .map(|token| token.to_ascii_lowercase())
            .collect();
        self.languages
            .iter()
            .all(|lang| tokens.iter().any(|token| token == lang))
    }

    pub fn to_manifest(&self) -> ModelManifest {
        ModelManifest {
            id: self.id.clone(),
            display_name: self.display_name.clone(),
            version: self.version.clone(),
            languages: self.languages.clone(),
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
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub default_model_id: String,
    pub models: Vec<RegistryModelEntry>,
}

impl ModelRegistry {
    /// Parses a registry document, accepting legacy v1 and canonical v2 shapes.
    ///
    /// Schema versions newer than [`CURRENT_SCHEMA_VERSION`] fail clearly rather
    /// than being silently misinterpreted.
    pub fn from_str(content: &str) -> Result<Self, String> {
        let registry: ModelRegistry = serde_json::from_str(content)
            .map_err(|e| format!("Failed to parse registry JSON: {}", e))?;

        if registry.schema_version > CURRENT_SCHEMA_VERSION {
            return Err(format!(
                "Unsupported registry schema_version {}: this build supports schema versions up to {}. Please upgrade Echolet.",
                registry.schema_version, CURRENT_SCHEMA_VERSION
            ));
        }

        Ok(registry)
    }

    pub fn from_file(path: &Path) -> Result<Self, String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read registry from {:?}: {}", path, e))?;
        Self::from_str(&content)
    }

    /// Serializes this registry as canonical v2, upgrading the schema version
    /// and emitting plural `languages` and typed metadata.
    pub fn to_canonical_string(&self) -> Result<String, String> {
        let mut canonical = self.clone();
        canonical.schema_version = CURRENT_SCHEMA_VERSION;
        serde_json::to_string_pretty(&canonical)
            .map_err(|e| format!("Failed to serialize registry: {}", e))
    }

    pub fn get_model(&self, id: &str) -> Option<&RegistryModelEntry> {
        self.models.iter().find(|m| m.id == id)
    }

    pub fn default_entry(&self) -> Option<&RegistryModelEntry> {
        self.get_model(&self.default_model_id)
            .or_else(|| self.models.first())
    }
}
