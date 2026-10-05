//! Desktop UI layer for Echolet (PROJECT-041 / J11.1d).
//!
//! Provides the single shared cross-platform Slint renderer, adapter, controller,
//! and host seams across Windows, macOS, and Linux.

pub mod adapter;
pub mod controller;
pub mod host;

pub use adapter::*;
pub use controller::*;
pub use host::*;
