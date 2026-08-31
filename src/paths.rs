use crate::models::manifest::ModelManifest;
use std::env;
use std::path::{Path, PathBuf};

/// Resolves the application ResourceRoot directory (read-only product bundle).
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

/// Resolves the consolidated Echolet user data directory (~/.echolet).
/// Override via ECHOLET_USER_HOME or ECHOLET_HOME for tests / isolated environments.
pub fn echolet_home_dir() -> PathBuf {
    if let Ok(env_home) = env::var("ECHOLET_USER_HOME") {
        return PathBuf::from(env_home);
    }
    if let Ok(env_home) = env::var("ECHOLET_HOME") {
        return PathBuf::from(env_home);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from(env::var("HOME").unwrap_or_else(|_| ".".into())))
        .join(".echolet")
}

/// Consolidated user configuration path: ~/.echolet/config.json
pub fn user_config_path() -> PathBuf {
    echolet_home_dir().join("config.json")
}

/// User-downloaded models directory: ~/.echolet/models
pub fn user_models_dir() -> PathBuf {
    echolet_home_dir().join("models")
}

/// Local transcript history directory: ~/.echolet/history
pub fn history_dir() -> PathBuf {
    echolet_home_dir().join("history")
}

/// Backward compatibility search paths for migrating legacy configurations.
pub fn legacy_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(cfg) = dirs::config_dir() {
        paths.push(cfg.join("echolet/config.json"));
    }
    if let Some(data) = dirs::data_local_dir() {
        paths.push(data.join("echolet/config.json"));
    }
    paths
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
