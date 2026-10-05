use crate::actions::AppAction;
use crossbeam_channel::Sender;
use std::path::Path;

pub mod view;

pub use view::{
    build_view, download_status_label, format_bytes, project_runtime_state, LanguageOptionView,
    ModelLanguageView, PlatformModelItem, PlatformView, RuntimeState,
};

pub trait TextInjector: Send + Sync {
    fn apply_diff(&self, backspaces: usize, new_suffix: &str);
}

pub trait PlatformHandle: Send + Sync {
    fn set_listening(&self, _listening: bool) {}
    fn shutdown(&self) {}
    /// Projects the complete platform-neutral model/runtime/settings view.
    ///
    /// Windows/macOS use only the model name today; the richer fields keep the
    /// Linux tray a pure function of this projection.
    fn update_models(&self, _view: &PlatformView) {}
    fn update_history_state(&self, _enabled: bool) {}
    fn open_history_folder(&self, _history_dir: &Path) {}
}

pub struct PlatformRuntime {
    pub injector: Box<dyn TextInjector>,
    pub handle: Box<dyn PlatformHandle>,
    pub _resources: Box<dyn std::any::Any + Send + Sync>,
}

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "macos")]
pub mod macos;

pub fn handle_subcommand(args: &[String]) -> Result<bool, Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    {
        linux::handle_subcommand(args)
    }
    #[cfg(target_os = "windows")]
    {
        windows::handle_subcommand(args)
    }
    #[cfg(target_os = "macos")]
    {
        macos::handle_subcommand(args)
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        Ok(false)
    }
}

pub fn init(
    action_tx: Sender<AppAction>,
) -> Result<Option<PlatformRuntime>, Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    {
        linux::init(action_tx)
    }
    #[cfg(target_os = "windows")]
    {
        windows::init(action_tx)
    }
    #[cfg(target_os = "macos")]
    {
        macos::init(action_tx)
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        Err("Unsupported operating system".into())
    }
}
