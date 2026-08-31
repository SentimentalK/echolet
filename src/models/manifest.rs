use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelManifest {
    pub id: String,
    pub display_name: String,
    pub version: String,
    pub language: String,
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
            language: "zh-en".into(),
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
        }
    }
}

impl ModelManifest {
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
            fs::create_dir_all(parent).map_err(|e| {
                format!("Failed to create parent dir for {:?}: {}", path, e)
            })?;
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
