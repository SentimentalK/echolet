#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    pub listening: bool,
    pub running: bool,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            listening: false,
            running: true,
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
