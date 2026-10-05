pub mod download;
pub mod language;
pub mod manager;
pub mod manifest;
pub mod registry;

pub use crate::config::EcholetConfig;
pub use download::{
    download_and_install_model, download_and_install_model_with_progress, ArchiveFormat,
    DownloadStatus, InstallPhase, ProgressThrottle,
};
pub use manager::{InstalledModel, ModelManager};
pub use manifest::ModelManifest;
pub use registry::{
    LanguageTier, ModelLanguageOption, ModelLanguageOptions, ModelLicense, ModelRegistry,
    RegistryModelEntry, VerificationStatus, CURRENT_SCHEMA_VERSION,
};
