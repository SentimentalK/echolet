//! Shared UI product and presentation layer for Echolet.
//!
//! Architectural direction:
//! domain/core -> ui/control_surface -> platform render/host
//!
//! Platform renderers/hosts (Linux ksni, macOS status item, Windows tray,
//! and future mobile keyboard shells) must not independently infer product behavior.
//! They render immutable projected [`ControlSurfaceState`] and emit explicit [`SurfaceAction`].

pub mod control_surface;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub mod desktop;

pub use control_surface::{
    build_control_surface_state, format_bytes, project_runtime_state,
    ControlSurfaceState, DownloadPhase, DownloadPresentation, LanguageOptionPresentation,
    ModelGroupPresentation, ModelLanguagePresentation, ModelPresentation, ModelPrimaryAction,
    RuntimeState, SurfaceAction,
};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub use control_surface::dispatch_surface_action;
