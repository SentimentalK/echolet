#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppAction {
    ToggleListening,
    StartListening,
    StopListening,
    Quit,
}
