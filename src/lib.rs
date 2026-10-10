// Platform-neutral core: compiled for every target, including Android.
pub mod asr;
pub mod capture;
pub mod config;
pub mod diff;
pub mod ffi;
pub mod models;
pub mod paths;
pub mod session;

// Desktop-only application/UI/platform layers. These pull in cpal, Slint &&
// OS-specific integrations, none of which may enter the Android build.
#[cfg(not(target_os = "android"))]
pub mod actions;
#[cfg(not(target_os = "android"))]
pub mod app;
#[cfg(not(target_os = "android"))]
pub mod audio;
#[cfg(not(target_os = "android"))]
pub mod beep;
#[cfg(not(target_os = "android"))]
pub mod diagnostics;
#[cfg(not(target_os = "android"))]
pub mod history;
#[cfg(not(target_os = "android"))]
pub mod log;
#[cfg(not(target_os = "android"))]
pub mod platform;
#[cfg(not(target_os = "android"))]
pub mod state;
pub mod ui;
