//! Compatibility shim for legacy platform view consumers.
//!
//! Canonical semantic authority has moved to [`crate::ui::control_surface`].
//! This module remains only as an incremental compatibility layer re-exporting
//! the canonical types and helpers.

use crate::config::EcholetConfig;
use crate::models::download::DownloadStatus;
use crate::models::registry::ModelRegistry;
use std::collections::{HashMap, HashSet};

pub use crate::ui::control_surface::{
    build_control_surface_state, dispatch_surface_action, format_bytes, project_runtime_state,
    ControlSurfaceState, ControlSurfaceState as PlatformView, DownloadPhase, DownloadPresentation,
    LanguageOptionPresentation, LanguageOptionPresentation as LanguageOptionView,
    ModelGroupPresentation, ModelLanguagePresentation,
    ModelLanguagePresentation as ModelLanguageView, ModelPresentation,
    ModelPresentation as PlatformModelItem, ModelPrimaryAction, RuntimeState, SurfaceAction,
};

/// Backward-compatible builder that delegates to canonical [`build_control_surface_state`].
pub fn build_view(
    registry: &ModelRegistry,
    active_id: Option<&str>,
    installed: &HashSet<String>,
    downloading: &HashSet<String>,
    progress: &HashMap<String, DownloadStatus>,
    config: &EcholetConfig,
    runtime_state: RuntimeState,
) -> PlatformView {
    build_control_surface_state(
        registry,
        active_id,
        installed,
        downloading,
        progress,
        config,
        runtime_state,
        config.history_enabled,
    )
}

/// Concise label for an in-flight or failed download, projected through canonical
/// [`DownloadPresentation`].
pub fn download_status_label(status: &DownloadStatus) -> Option<String> {
    DownloadPresentation::from_status(status).label
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatibility_shim_delegates_truthfully() {
        assert_eq!(
            project_runtime_state(true, true, false, true),
            RuntimeState::Listening
        );
        assert_eq!(
            download_status_label(&DownloadStatus::Failed).as_deref(),
            Some("Download failed — Retry")
        );
        assert_eq!(format_bytes(1024), "1.0 KB");
    }
}
