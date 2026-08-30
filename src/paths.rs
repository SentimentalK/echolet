use std::env;
use std::path::{Path, PathBuf};

pub struct ModelManifest {
    pub id: String,
    pub display_name: String,
    pub version: String,
    pub directory: String,
    pub encoder: String,
    pub decoder: String,
    pub joiner: String,
    pub tokens: String,
}

impl Default for ModelManifest {
    fn default() -> Self {
        Self {
            id: "bilingual-zh-en".into(),
            display_name: "Chinese + English".into(),
            version: "2023-02-16".into(),
            directory: "models/bilingual-zh-en".into(),
            encoder: "encoder-epoch-99-avg-1.int8.onnx".into(),
            decoder: "decoder-epoch-99-avg-1.onnx".into(),
            joiner: "joiner-epoch-99-avg-1.int8.onnx".into(),
            tokens: "tokens.txt".into(),
        }
    }
}

/// Resolves the application ResourceRoot directory.
/// Priority:
/// 1. ECHOLET_RESOURCE_ROOT environment variable (if set and exists)
/// 2. Directory containing current executable (Production Bundle layout)
/// 3. .local-runtime in workspace / current working directory (Development layout)
pub fn resource_root() -> PathBuf {
    if let Ok(env_root) = env::var("ECHOLET_RESOURCE_ROOT") {
        let path = PathBuf::from(env_root);
        if path.exists() {
            return path;
        }
    }

    if let Ok(exe_path) = env::current_exe() {
        if let Some(app_dir) = exe_path.parent() {
            // Check production bundle layout (resource_root is next to binary)
            if app_dir.join("models").exists() {
                return app_dir.to_path_buf();
            }

            // Check development target layout (target/release/../../.local-runtime)
            let dev_staging = app_dir.join("../../.local-runtime");
            if dev_staging.exists() {
                return dev_staging.canonicalize().unwrap_or(dev_staging);
            }
        }
    }

    // Check current working directory for .local-runtime
    let cwd_local = PathBuf::from(".local-runtime");
    if cwd_local.exists() {
        return cwd_local.canonicalize().unwrap_or(cwd_local);
    }

    // Default fallback to executable directory
    env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn runtime_lib_dir() -> PathBuf {
    resource_root().join("runtime/lib")
}

pub fn model_root() -> PathBuf {
    resource_root().join("models")
}

pub fn default_model_dir() -> PathBuf {
    resource_root().join("models/bilingual-zh-en")
}

/// Validates that the model bundle contains all required files.
pub fn validate_model_bundle(model_dir: &Path) -> Result<(), String> {
    if !model_dir.exists() {
        return Err(format!(
            "[Error] Model directory not found: {:?}\nEnsure the bundle has 'models/bilingual-zh-en' or set ECHOLET_RESOURCE_ROOT.",
            model_dir
        ));
    }

    let manifest = ModelManifest::default();
    let required_files = [
        (&manifest.encoder, "Encoder ONNX model"),
        (&manifest.decoder, "Decoder ONNX model"),
        (&manifest.joiner, "Joiner ONNX model"),
        (&manifest.tokens, "Tokens vocabulary file"),
    ];

    for (file_name, desc) in required_files {
        let file_path = model_dir.join(file_name);
        if !file_path.exists() {
            return Err(format!(
                "[Error] Model bundle incomplete: missing {} ({:?})\nExpected at: {:?}",
                desc, file_name, file_path
            ));
        }
    }

    Ok(())
}
