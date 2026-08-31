use crate::models::manifest::ModelManifest;
use std::env;
use std::path::{Path, PathBuf};

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

pub fn bundled_models_dir() -> PathBuf {
    resource_root().join("models")
}

pub fn user_data_dir() -> PathBuf {
    if let Ok(env_data) = env::var("ECHOLET_USER_DATA_DIR") {
        return PathBuf::from(env_data);
    }
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from(env::var("HOME").unwrap_or_else(|_| ".".into())).join(".local/share"))
        .join("echolet")
}

pub fn user_models_dir() -> PathBuf {
    user_data_dir().join("models")
}

pub fn user_config_dir() -> PathBuf {
    if let Ok(env_cfg) = env::var("ECHOLET_USER_CONFIG_DIR") {
        return PathBuf::from(env_cfg);
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from(env::var("HOME").unwrap_or_else(|_| ".".into())).join(".config"))
        .join("echolet")
}

pub fn user_config_path() -> PathBuf {
    user_config_dir().join("config.json")
}

pub fn default_model_dir() -> PathBuf {
    let bundled = bundled_models_dir();
    let modern = bundled.join("zh-en-small-2023-02-16");
    if modern.exists() {
        return modern;
    }
    bundled.join("bilingual-zh-en")
}

pub fn validate_model_bundle(model_dir: &Path) -> Result<(), String> {
    let manifest = if model_dir.join("model.json").exists() {
        ModelManifest::from_file(&model_dir.join("model.json"))?
    } else {
        ModelManifest::default()
    };
    manifest.validate_files(model_dir)
}
