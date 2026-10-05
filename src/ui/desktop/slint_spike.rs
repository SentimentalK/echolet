//! Slint desktop UI adapter and view model (PROJECT-041 / J11.1c Spike).
//!
//! Architectural direction:
//! Domain/Core -> ControlSurfaceState -> DesktopPanelViewModel -> EcholetPanel (Slint)
//!
//! Slint renders state and emits callback intent only. Zero business logic or product
//! rules are inferred here; model actions and capability groups are preserved directly
//! from canonical [`ControlSurfaceState`].

slint::include_modules!();

use crate::ui::control_surface::{
    ControlSurfaceState, DownloadPhase, DownloadPresentation, ModelGroupPresentation,
    ModelPresentation, ModelPrimaryAction, RuntimeState, SurfaceAction,
};
pub use slint::ComponentHandle;
use std::rc::Rc;

/// Fixed logical panel width specified by design contract.
pub const PANEL_WIDTH_PX: u32 = 380;

/// Fixed logical panel height specified by design contract.
pub const PANEL_HEIGHT_PX: u32 = 240;

/// Plain-Rust view model for a single model row in the desktop panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopModelRowViewModel {
    pub id: String,
    pub group_id: String,
    pub group_label: String,
    pub label: String,
    pub status_text: String,
    pub action_label: String,
    pub action_enabled: bool,
    pub selected: bool,
    pub primary_action: ModelPrimaryAction,
}

/// Plain-Rust view model for the complete desktop spike panel.
///
/// Designed to be completely testable in headless unit test environments
/// without requiring a graphical display or Slint event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopPanelViewModel {
    pub width: u32,
    pub height: u32,
    pub status_text: String,
    pub group_label: String,
    pub models: Vec<DesktopModelRowViewModel>,
    pub diagnostic_state: String,
    pub diagnostic_count: u32,
    pub footer_text: String,
    // Typed verification of optional control surface settings fields
    pub preload_on_startup: bool,
    pub idle_unload_minutes: Option<u32>,
    pub history_enabled: bool,
}

impl DesktopPanelViewModel {
    /// Projects canonical [`ControlSurfaceState`] into [`DesktopPanelViewModel`].
    ///
    /// Preserves group and model ordering exactly as dictated by the shared core.
    /// Does NOT re-derive primary action semantics or grouping.
    pub fn from_control_surface(state: &ControlSurfaceState) -> Self {
        let status_text = state.runtime_state.status_label().to_string();

        let first_group_label = state
            .model_groups
            .first()
            .map(|g| g.label.clone())
            .unwrap_or_else(|| "Models".to_string());

        let mut models = Vec::new();
        for group in &state.model_groups {
            for model in &group.models {
                let status_text = if model.selected {
                    format!("Selected · {}", model.verification_label)
                } else if let Some(dl_label) = &model.download.label {
                    if model.download.phase != DownloadPhase::Completed {
                        format!("{} · {}", dl_label, model.verification_label)
                    } else if model.installed {
                        format!("Installed · {}", model.verification_label)
                    } else {
                        format!("Download · {}", model.verification_label)
                    }
                } else if model.installed {
                    format!("Installed · {}", model.verification_label)
                } else {
                    format!("Download · {}", model.verification_label)
                };

                let action_label = match model.primary_action {
                    ModelPrimaryAction::None => {
                        if model.selected {
                            "Selected".to_string()
                        } else {
                            "".to_string()
                        }
                    }
                    ModelPrimaryAction::Download => "Download".to_string(),
                    ModelPrimaryAction::RetryDownload => "Retry".to_string(),
                    ModelPrimaryAction::Select => "Select".to_string(),
                };

                let action_enabled = model.enabled && !model.primary_action.is_none();

                models.push(DesktopModelRowViewModel {
                    id: model.id.clone(),
                    group_id: group.id.clone(),
                    group_label: group.label.clone(),
                    label: model.label.clone(),
                    status_text,
                    action_label,
                    action_enabled,
                    selected: model.selected,
                    primary_action: model.primary_action,
                });
            }
        }

        Self {
            width: PANEL_WIDTH_PX,
            height: PANEL_HEIGHT_PX,
            status_text,
            group_label: first_group_label,
            models,
            diagnostic_state: "Initial".to_string(),
            diagnostic_count: 0,
            footer_text: "Slint 1.18.1 · Software Renderer · 380x240".to_string(),
            preload_on_startup: state.preload_on_startup,
            idle_unload_minutes: state.idle_unload_minutes,
            history_enabled: state.history_enabled,
        }
    }

    /// Resolves the canonical [`SurfaceAction`] for a clicked model without re-inferring logic.
    pub fn resolve_surface_action(&self, model_id: &str) -> Option<SurfaceAction> {
        let model = self.models.iter().find(|m| m.id == model_id)?;
        if !model.action_enabled {
            return None;
        }
        model.primary_action.to_surface_action(&model.id)
    }

    /// Toggles diagnostic test state and increments click count.
    ///
    /// This state is spike-only diagnostic verification and does not mutate production core.
    pub fn trigger_diagnostic_toggle(&mut self) {
        self.diagnostic_count = self.diagnostic_count.saturating_add(1);
        self.diagnostic_state = if self.diagnostic_state == "Active" {
            "Toggled".to_string()
        } else {
            "Active".to_string()
        };
    }
}

/// Adapter between canonical data types and the Slint component instance.
pub struct SlintControlSurfaceAdapter;

impl SlintControlSurfaceAdapter {
    /// Applies view model values to a running [`EcholetPanel`] Slint component instance.
    pub fn apply_to_panel(panel: &EcholetPanel, vm: &DesktopPanelViewModel) {
        panel.set_status_text(vm.status_text.as_str().into());
        panel.set_group_label(vm.group_label.as_str().into());
        panel.set_diagnostic_count(vm.diagnostic_count as i32);
        panel.set_diagnostic_state(vm.diagnostic_state.as_str().into());
        panel.set_footer_text(vm.footer_text.as_str().into());

        let rows: Vec<SlintModelRow> = vm
            .models
            .iter()
            .map(|m| SlintModelRow {
                id: m.id.as_str().into(),
                label: m.label.as_str().into(),
                status_text: m.status_text.as_str().into(),
                action_label: m.action_label.as_str().into(),
                action_enabled: m.action_enabled,
                selected: m.selected,
            })
            .collect();

        let model_rc = Rc::new(slint::VecModel::from(rows));
        panel.set_models(model_rc.into());
    }

    /// Builds a deterministic, representative [`ControlSurfaceState`] fixture.
    ///
    /// Does not require microphone access, network downloads, or runtime model weights.
    pub fn create_spike_fixture() -> ControlSurfaceState {
        ControlSurfaceState {
            runtime_state: RuntimeState::Ready,
            model_groups: vec![
                ModelGroupPresentation {
                    id: "bilingual-zh-en".to_string(),
                    label: "Chinese + English".to_string(),
                    models: vec![ModelPresentation {
                        id: "echolet-nemotron-3.5".to_string(),
                        label: "Echolet Nemotron 3.5 Bilingual".to_string(),
                        verification_label: "Echolet Verified".to_string(),
                        is_verified: true,
                        selected: true,
                        installed: true,
                        download: DownloadPresentation {
                            phase: DownloadPhase::Completed,
                            label: Some("Installed".to_string()),
                            progress_fraction: Some("1.00".to_string()),
                            progress_percent: Some(100),
                            retryable: false,
                        },
                        primary_action: ModelPrimaryAction::None,
                        enabled: true,
                        language: Default::default(),
                    }],
                },
                ModelGroupPresentation {
                    id: "multilingual".to_string(),
                    label: "Broad Multilingual".to_string(),
                    models: vec![ModelPresentation {
                        id: "echolet-multilingual-v2".to_string(),
                        label: "Echolet Multilingual 8-Language Pack".to_string(),
                        verification_label: "Echolet Verified".to_string(),
                        is_verified: true,
                        selected: false,
                        installed: false,
                        download: DownloadPresentation {
                            phase: DownloadPhase::NotDownloading,
                            label: None,
                            progress_fraction: None,
                            progress_percent: None,
                            retryable: false,
                        },
                        primary_action: ModelPrimaryAction::Download,
                        enabled: true,
                        language: Default::default(),
                    }],
                },
            ],
            preload_on_startup: true,
            idle_unload_minutes: Some(10),
            history_enabled: false,
        }
    }

    /// Builds a fixture with extremely long model and group names to prove fixed width invariants.
    pub fn create_spike_fixture_long_names() -> ControlSurfaceState {
        ControlSurfaceState {
            runtime_state: RuntimeState::Listening,
            model_groups: vec![ModelGroupPresentation {
                id: "extremely-long-group-identifier-for-layout-testing".to_string(),
                label: "Supercalifragilisticexpialidocious Ultra Wide Language Group With Unbounded Name Length 1234567890".to_string(),
                models: vec![ModelPresentation {
                    id: "very-long-model-identifier-1234567890".to_string(),
                    label: "Super Extended Ultra Long Model Name Description That Should Never Expand The Fixed 380px Window Boundary".to_string(),
                    verification_label: "Echolet Verified Ultra Long Badge".to_string(),
                    is_verified: true,
                    selected: false,
                    installed: true,
                    download: DownloadPresentation {
                        phase: DownloadPhase::Completed,
                        label: Some("Installed".to_string()),
                        progress_fraction: Some("1.00".to_string()),
                        progress_percent: Some(100),
                        retryable: false,
                    },
                    primary_action: ModelPrimaryAction::Select,
                    enabled: true,
                    language: Default::default(),
                }],
            }],
            preload_on_startup: false,
            idle_unload_minutes: None,
            history_enabled: true,
        }
    }
}
