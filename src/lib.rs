// Platform-neutral core: compiled for every target, including Android.
pub mod asr;
pub mod capture;
pub mod config;
pub mod diff;
pub mod ffi;
pub mod ios_ipc;
pub mod models;
pub mod paths;
pub mod session;

// Desktop-only application/UI/platform layers. These pull in cpal, Slint &&
// OS-specific integrations, none of which may enter the Android or iOS builds.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub mod actions;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub mod app;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub mod audio;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub mod beep;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub mod diagnostics;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub mod history;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub mod log;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub mod platform;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub mod state;
pub mod ui;
