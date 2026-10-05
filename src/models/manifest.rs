use crate::models::language::{self, deserialize_languages};
use crate::models::registry::{ModelLanguageOption, ModelLanguageOptions};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelManifest {
    pub id: String,
    pub display_name: String,
    pub version: String,
    /// Canonical plural language list. Legacy on-disk manifests that stored a
    /// single `"language"` string still load through the shared normalization
    /// seam; there is no competing in-memory `language` field.
    #[serde(alias = "language", deserialize_with = "deserialize_languages")]
    pub languages: Vec<String>,
    pub family: String,

    pub encoder: String,
    pub decoder: String,
    pub joiner: String,
    pub tokens: String,

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

    /// Typed tiered locale metadata for multilingual models (propagated from
    /// the registry). `None` for single-language models such as X-ASR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_options: Option<ModelLanguageOptions>,
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

impl Default for ModelManifest {
    fn default() -> Self {
        Self {
            id: "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1".into(),
            display_name: "Chinese + English (X-ASR / 480ms)".into(),
            version: "2026".into(),
            languages: vec!["zh".into(), "en".into()],
            family: "online-transducer".into(),
            encoder: "encoder-480ms.onnx".into(),
            decoder: "decoder-480ms.onnx".into(),
            joiner: "joiner-480ms.onnx".into(),
            tokens: "tokens.txt".into(),
            model_type: Some("zipformer2".into()),
            sample_rate: 16000,
            feature_dim: 80,
            num_threads: 1,
            provider: "cpu".into(),
            decoding_method: "greedy_search".into(),
            max_active_paths: 4,
            language_options: None,
        }
    }
}

impl ModelManifest {
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

    /// Runtime-selectable language options (out-of-box only). Empty for
    /// single-language models. Mirrors
    /// [`crate::models::registry::RegistryModelEntry::supported_language_options`].
    pub fn supported_language_options(&self) -> &[ModelLanguageOption] {
        self.language_options
            .as_ref()
            .map(|o| o.supported.as_slice())
            .unwrap_or(&[])
    }

    /// Validates a requested language selection against this manifest's typed
    /// metadata. See
    /// [`crate::models::registry::ModelLanguageOptions::validate_selection`].
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

    pub fn from_file(path: &Path) -> Result<Self, String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read model manifest {:?}: {}", path, e))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse model manifest {:?}: {}", path, e))
    }

    pub fn save_to_file(&self, path: &Path) -> Result<(), String> {
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize model manifest: {}", e))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create parent dir for {:?}: {}", path, e))?;
        }
        fs::write(path, content)
            .map_err(|e| format!("Failed to write model manifest to {:?}: {}", path, e))
    }

    pub fn validate_files(&self, model_dir: &Path) -> Result<(), String> {
        if !model_dir.exists() {
            return Err(format!("Model directory does not exist: {:?}", model_dir));
        }

        let required = [
            (&self.encoder, "Encoder ONNX model"),
            (&self.decoder, "Decoder ONNX model"),
            (&self.joiner, "Joiner ONNX model"),
            (&self.tokens, "Tokens vocabulary file"),
        ];

        for (filename, desc) in required {
            let p = model_dir.join(filename);
            if !p.exists() {
                return Err(format!(
                    "Model bundle incomplete: missing {} ({:?}) at {:?}",
                    desc, filename, p
                ));
            }
        }

        Ok(())
    }
}
