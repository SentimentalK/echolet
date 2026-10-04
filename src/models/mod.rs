pub mod download;
pub mod manager;
pub mod manifest;
pub mod registry;

pub use crate::config::EcholetConfig;
pub use download::{
    download_and_install_model, download_and_install_model_with_progress, ArchiveFormat,
    InstallPhase,
};
pub use manager::{InstalledModel, ModelManager};
pub use manifest::ModelManifest;
pub use registry::{ModelRegistry, RegistryModelEntry};
