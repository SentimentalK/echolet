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

/// Product quality/confidence tier for a single language-locale.
///
/// This is a *typed* product concept and is deliberately separate from the
/// coarse base-language `languages` list, which the legacy hyphen-splitting
/// normalizer owns. BCP-47 locale values (for example `ja-JP`) must never be
/// pushed through that normalizer; they live here instead.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum LanguageTier {
    /// Highest-accuracy ASR, ready out of the box.
    #[serde(rename = "TranscriptionReady")]
    TranscriptionReady,
    /// Produces ASR out of the box at clearly lower / less-established quality.
    #[serde(rename = "BroadCoverage")]
    BroadCoverage,
    /// Tokenizer-supported but requires fine-tuning; NOT product-ready.
    #[serde(rename = "AdaptationReady")]
    AdaptationReady,
}

impl LanguageTier {
    pub fn label(self) -> &'static str {
        match self {
            LanguageTier::TranscriptionReady => "TranscriptionReady",
            LanguageTier::BroadCoverage => "BroadCoverage",
            LanguageTier::AdaptationReady => "AdaptationReady",
        }
    }

    /// Whether a locale in this tier may be offered as a normal selectable
    /// option. Adaptation-ready locales must never be selectable.
    pub fn is_selectable(self) -> bool {
        matches!(
            self,
            LanguageTier::TranscriptionReady | LanguageTier::BroadCoverage
        )
    }
}

/// A single locale understood by a multilingual model, with the runtime code
/// Sherpa actually consumes on the stream.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelLanguageOption {
    /// BCP-47 locale identifier, for example `ja-JP`. Kept verbatim.
    pub locale: String,
    /// Runtime code passed to the model (for example `ja`, or `auto`).
    pub runtime_code: String,
    pub tier: LanguageTier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// Typed, tiered language metadata for a multilingual model.
///
/// Only [`ModelLanguageOptions::supported`] may be exposed as runtime-selectable.
/// [`ModelLanguageOptions::adaptation_ready`] is provenance/catalog metadata and
/// must never be returned by the normal selection API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ModelLanguageOptions {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported: Vec<ModelLanguageOption>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adaptation_ready: Vec<ModelLanguageOption>,
}

impl ModelLanguageOptions {
    pub fn transcription_ready_count(&self) -> usize {
        self.supported
            .iter()
            .filter(|o| o.tier == LanguageTier::TranscriptionReady)
            .count()
    }

    pub fn broad_coverage_count(&self) -> usize {
        self.supported
            .iter()
            .filter(|o| o.tier == LanguageTier::BroadCoverage)
            .count()
    }

    pub fn adaptation_ready_count(&self) -> usize {
        self.adaptation_ready.len()
    }

    /// Case-insensitive lookup over selectable options by BCP-47 locale or by
    /// runtime code. This intentionally does NOT use the legacy language
    /// normalizer, so `ja-JP` is never split into `ja` + `jp`.
    pub fn find_supported(&self, query: &str) -> Option<&ModelLanguageOption> {
        let q = query.trim();
        if q.is_empty() {
            return None;
        }
        self.supported
            .iter()
            .find(|o| o.locale.eq_ignore_ascii_case(q) || o.runtime_code.eq_ignore_ascii_case(q))
    }

    /// Case-insensitive lookup over adaptation-ready (non-selectable) options.
    pub fn find_adaptation_ready(&self, query: &str) -> Option<&ModelLanguageOption> {
        let q = query.trim();
        if q.is_empty() {
            return None;
        }
        self.adaptation_ready
            .iter()
            .find(|o| o.locale.eq_ignore_ascii_case(q) || o.runtime_code.eq_ignore_ascii_case(q))
    }

    /// Shared selection contract used by both the registry entry and the
    /// on-disk manifest so there is exactly one language-selection authority.
    ///
    /// `None`/empty means auto-detect (always valid). Adaptation-ready locales
    /// are rejected explicitly. `model_id` is only used for error messages.
    pub fn validate_selection<'a>(
        &'a self,
        model_id: &str,
        selection: Option<&str>,
    ) -> Result<Option<&'a ModelLanguageOption>, String> {
        let Some(query) = selection.map(str::trim).filter(|q| !q.is_empty()) else {
            return Ok(None);
        };
        // The runtime documents "auto" as the explicit auto-detect sentinel.
        if query.eq_ignore_ascii_case("auto") {
            return Ok(None);
        }
        if let Some(opt) = self.find_supported(query) {
            return Ok(Some(opt));
        }
        if let Some(opt) = self.find_adaptation_ready(query) {
            return Err(format!(
                "Language '{}' is adaptation-ready for model '{}' and is not a supported \
                 Echolet language; it requires a separately fine-tuned model.",
                opt.locale, model_id
            ));
        }
        Err(format!(
            "Language '{}' is not a supported language option for model '{}'",
            query, model_id
        ))
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
    /// Typed tiered locale metadata for multilingual models. `None` for
    /// single-language models such as the X-ASR baseline; the coarse
    /// `languages` list remains the only language authority for those.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_options: Option<ModelLanguageOptions>,
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

    /// Runtime-selectable language options for this model.
    ///
    /// Returns only the out-of-box options ([`LanguageTier::TranscriptionReady`]
    /// and [`LanguageTier::BroadCoverage`]). Adaptation-ready locales are never
    /// returned here; use [`RegistryModelEntry::adaptation_ready_locales`] when
    /// catalog/provenance data is needed.
    pub fn supported_language_options(&self) -> &[ModelLanguageOption] {
        self.language_options
            .as_ref()
            .map(|o| o.supported.as_slice())
            .unwrap_or(&[])
    }

    /// Non-selectable, adaptation-only locales (catalog/provenance only).
    pub fn adaptation_ready_locales(&self) -> &[ModelLanguageOption] {
        self.language_options
            .as_ref()
            .map(|o| o.adaptation_ready.as_slice())
            .unwrap_or(&[])
    }

    /// Validates a requested language selection against this model's typed
    /// metadata.
    ///
    /// * `None` or an empty string means auto-detect and is always valid.
    /// * A supported locale/runtime code returns the resolved option.
    /// * An adaptation-ready locale is explicitly rejected so it can never be
    ///   treated as a normal supported language.
    /// * Anything else is rejected.
    ///
    /// Models without typed metadata (for example X-ASR) reject any forced
    /// language, preserving their simple base-language behavior.
    pub fn validate_language_selection(
        &self,
        selection: Option<&str>,
    ) -> Result<Option<&ModelLanguageOption>, String> {
        match &self.language_options {
            Some(opts) => opts.validate_selection(&self.id, selection),
            None => {
                let query = selection
                    .map(str::trim)
                    .filter(|q| !q.is_empty() && !q.eq_ignore_ascii_case("auto"));
                match query {
                    None => Ok(None),
                    Some(q) => Err(format!(
                        "Language '{}' is not a supported language option for model '{}'",
                        q, self.id
                    )),
                }
            }
        }
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
            language_options: self.language_options.clone(),
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
