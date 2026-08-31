pub mod download;
pub mod manifest;
pub mod manager;
pub mod registry;

pub use download::download_and_install_model;
pub use manifest::ModelManifest;
pub use manager::{EcholetConfig, InstalledModel, ModelManager};
pub use registry::{ModelRegistry, RegistryModelEntry};
