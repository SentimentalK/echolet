pub mod language;
pub mod manifest;
pub mod registry;

// Desktop-only model acquisition and installation management. The Android
// bridge consumes staged model directories directly and performs no downloads.
#[cfg(not(target_os = "android"))]
pub mod download;
#[cfg(not(target_os = "android"))]
pub mod manager;

pub use crate::config::EcholetConfig;
#[cfg(not(target_os = "android"))]
pub use download::{
    download_and_install_model, download_and_install_model_with_progress, ArchiveFormat,
    DownloadStatus, InstallPhase, ProgressThrottle,
};
#[cfg(not(target_os = "android"))]
pub use manager::{InstalledModel, ModelManager};
pub use manifest::ModelManifest;
pub use registry::{
    LanguageTier, ModelLanguageOption, ModelLanguageOptions, ModelLicense, ModelRegistry,
    RegistryModelEntry, VerificationStatus, CURRENT_SCHEMA_VERSION,
};
