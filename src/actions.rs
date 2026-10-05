use crate::models::download::DownloadStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppAction {
    ToggleListening,
    StartListening,
    StopListening,
    Quit,
    /// Start downloading/installing an uninstalled registry model. This never
    /// changes the active model; the user must select it explicitly afterwards.
    DownloadModel(String),
    /// Switch the active model. Only has an effect for an already-installed
    /// model; it never triggers a download.
    SelectModel(String),
    /// Incremental download/install progress for a model.
    ModelDownloadProgress {
        model_id: String,
        status: DownloadStatus,
    },
    ModelInstalled {
        model_id: String,
        success: bool,
        error: Option<String>,
    },
    /// Set the forced language for a model. `locale == None` means Auto.
    SelectLanguage {
        model_id: String,
        locale: Option<String>,
    },
    SetPreloadModelOnStartup(bool),
    /// `None` means "Never"; `Some(0)` means "Immediate".
    SetModelIdleUnloadMinutes(Option<u32>),
    ToggleHistory,
    OpenHistoryFolder,
}
