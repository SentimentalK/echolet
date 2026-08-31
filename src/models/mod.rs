pub mod download;
pub mod manifest;
pub mod manager;
pub mod registry;

pub use crate::config::EcholetConfig;
pub use download::download_and_install_model;
pub use manifest::ModelManifest;
pub use manager::{InstalledModel, ModelManager};
pub use registry::{ModelRegistry, RegistryModelEntry};
