pub mod installer;
pub mod language;
pub mod manifest;
pub mod progress;
pub mod registry;

// Desktop-only model acquisition and installation management.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub mod download;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub mod manager;

pub use crate::config::EcholetConfig;
pub use installer::{install_model_from_archive, sha256_file};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub use download::{download_and_install_model, download_and_install_model_with_progress};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub use manager::{InstalledModel, ModelManager};
pub use manifest::ModelManifest;
pub use progress::{ArchiveFormat, DownloadStatus, InstallPhase, ProgressThrottle};
pub use registry::{
    LanguageTier, ModelLanguageOption, ModelLanguageOptions, ModelLicense, ModelRegistry,
    RegistryModelEntry, VerificationStatus, CURRENT_SCHEMA_VERSION,
};
