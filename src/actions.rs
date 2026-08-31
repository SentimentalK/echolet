#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppAction {
    ToggleListening,
    StartListening,
    StopListening,
    Quit,
    SelectModel(String),
    ModelInstalled {
        model_id: String,
        success: bool,
        error: Option<String>,
    },
    ToggleHistory,
    OpenHistoryFolder,
}
