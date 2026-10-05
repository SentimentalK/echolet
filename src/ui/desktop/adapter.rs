//! Canonical Shared Desktop UI Adapter & View Model (PROJECT-041 / J11.1d).
//!
//! Architectural direction:
//! Domain/Core -> src/ui/control_surface.rs -> DesktopPanelViewModel -> EcholetPanel (Slint)
//!
//! Slint renders projected state and emits canonical intent only. Zero business logic
//! or product rules are inferred here; model hierarchy, actions, and capability groups
//! are preserved directly from canonical [`ControlSurfaceState`].

slint::include_modules!();

use crate::ui::control_surface::{
    ControlSurfaceState, DownloadPhase, DownloadPresentation, ModelGroupPresentation,
    ModelPresentation, ModelPrimaryAction, RuntimeState, SurfaceAction,
};
pub use slint::ComponentHandle;
use std::rc::Rc;

/// Fixed logical panel width specified by design contract.
pub const PANEL_WIDTH_PX: u32 = 380;

/// Default logical panel height specified by design contract.
pub const PANEL_HEIGHT_PX: u32 = 520;

/// Maximum allowable panel height before scrolling.
pub const PANEL_MAX_HEIGHT_PX: u32 = 640;

/// Plain-Rust view model for a single model row in the desktop panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopModelRowViewModel {
    pub id: String,
    pub group_id: String,
    pub group_label: String,
    pub label: String,
    pub verification_label: String,
    pub status_text: String,
    pub action_label: String,
    pub action_enabled: bool,
    pub selected: bool,
    pub is_in_progress: bool,
    pub progress_text: String,
    pub progress_percent: i32,
    pub primary_action: ModelPrimaryAction,
}

/// Plain-Rust view model for a hierarchical model capability group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopModelGroupViewModel {
    pub id: String,
    pub label: String,
    pub models: Vec<DesktopModelRowViewModel>,
}

/// Plain-Rust view model for a selectable language option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopLanguageOptionViewModel {
    pub locale: String,
    pub label: String,
    pub is_selected: bool,
}

/// Plain-Rust view model for the complete desktop production panel.
///
/// Fully testable in headless unit test environments without requiring a graphical
/// display or Slint event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopPanelViewModel {
    pub width: u32,
    pub height: u32,
    pub status_text: String,
    pub is_listening: bool,
    pub group_label: String,
    pub models: Vec<DesktopModelRowViewModel>,
    pub model_groups: Vec<DesktopModelGroupViewModel>,
    pub has_language_options: bool,
    pub language_model_id: String,
    pub current_language_label: String,
    pub language_options: Vec<DesktopLanguageOptionViewModel>,
    pub preload_on_startup: bool,
    pub idle_unload_minutes: Option<u32>,
    pub idle_unload_label: String,
    pub history_enabled: bool,
    pub footer_text: String,
    // Diagnostic compatibility fields
    pub diagnostic_state: String,
    pub diagnostic_count: u32,
}

impl DesktopPanelViewModel {
    /// Projects canonical [`ControlSurfaceState`] into [`DesktopPanelViewModel`].
    ///
    /// Preserves group and model ordering exactly as dictated by the shared core.
    /// Does NOT re-derive primary action semantics or grouping.
    pub fn from_control_surface(state: &ControlSurfaceState) -> Self {
        let status_text = state.runtime_state.status_label().to_string();
        let is_listening = state.runtime_state.is_listening();

        let mut model_groups = Vec::new();

        for group in &state.model_groups {
            let mut models = Vec::new();
            for model in &group.models {
                let status_text = if model.selected {
                    "Selected".to_string()
                } else if let Some(dl_label) = &model.download.label {
                    if model.download.phase != DownloadPhase::Completed {
                        dl_label.clone()
                    } else if model.installed {
                        "Installed".to_string()
                    } else {
                        "Download".to_string()
                    }
                } else if model.installed {
                    "Installed".to_string()
                } else {
                    "Download".to_string()
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
                let is_in_progress = model.download.is_in_progress();
                let progress_text = model.download.label.clone().unwrap_or_default();
                let progress_percent = model
                    .download
                    .progress_percent
                    .map(|p| p as i32)
                    .unwrap_or(-1);

                models.push(DesktopModelRowViewModel {
                    id: model.id.clone(),
                    group_id: group.id.clone(),
                    group_label: group.label.clone(),
                    label: model.label.clone(),
                    verification_label: model.verification_label.clone(),
                    status_text,
                    action_label,
                    action_enabled,
                    selected: model.selected,
                    is_in_progress,
                    progress_text,
                    progress_percent,
                    primary_action: model.primary_action,
                });
            }

            model_groups.push(DesktopModelGroupViewModel {
                id: group.id.clone(),
                label: group.label.clone(),
                models,
            });
        }

        // Language options from active model
        let (has_language_options, language_model_id, current_language_label, language_options) =
            if let Some(active) = state.selected_model() {
                if !active.language.options.is_empty() {
                    let mut opts = Vec::new();
                    // Auto option is always represented first
                    opts.push(DesktopLanguageOptionViewModel {
                        locale: "".to_string(),
                        label: "Auto".to_string(),
                        is_selected: active.language.is_auto_selected(),
                    });

                    let mut current_label = "Auto".to_string();
                    for opt in &active.language.options {
                        let is_sel = active.language.selected_locale.as_deref() == Some(&opt.locale);
                        if is_sel {
                            current_label = opt.label.clone();
                        }
                        opts.push(DesktopLanguageOptionViewModel {
                            locale: opt.locale.clone(),
                            label: opt.label.clone(),
                            is_selected: is_sel,
                        });
                    }

                    (true, active.id.clone(), current_label, opts)
                } else {
                    (false, String::new(), "Auto".to_string(), Vec::new())
                }
            } else {
                (false, String::new(), "Auto".to_string(), Vec::new())
            };

        let idle_unload_label = match state.idle_unload_minutes {
            Some(0) => "Immediate".to_string(),
            Some(1) => "1 minute".to_string(),
            Some(5) => "5 minutes".to_string(),
            Some(10) => "10 minutes".to_string(),
            Some(30) => "30 minutes".to_string(),
            None => "Never".to_string(),
            Some(n) => format!("{} minutes", n),
        };

        let all_models: Vec<DesktopModelRowViewModel> = model_groups
            .iter()
            .flat_map(|g| g.models.clone())
            .collect();
        let group_label = model_groups
            .first()
            .map(|g| g.label.clone())
            .unwrap_or_else(|| "Models".to_string());

        Self {
            width: PANEL_WIDTH_PX,
            height: PANEL_HEIGHT_PX,
            status_text,
            is_listening,
            group_label,
            models: all_models,
            model_groups,
            has_language_options,
            language_model_id,
            current_language_label,
            language_options,
            preload_on_startup: state.preload_on_startup,
            idle_unload_minutes: state.idle_unload_minutes,
            idle_unload_label,
            history_enabled: state.history_enabled,
            footer_text: "Echolet · Slint 1.18.1 · 380px".to_string(),
            diagnostic_state: "Initial".to_string(),
            diagnostic_count: 0,
        }
    }

    /// Resolves the canonical [`SurfaceAction`] for a clicked model without re-inferring logic.
    pub fn resolve_surface_action(&self, model_id: &str) -> Option<SurfaceAction> {
        for group in &self.model_groups {
            for model in &group.models {
                if model.id == model_id {
                    if !model.action_enabled {
                        return None;
                    }
                    return model.primary_action.to_surface_action(&model.id);
                }
            }
        }
        None
    }

    /// Flattened model iterator for backward compatibility with spike tests.
    pub fn models(&self) -> Vec<DesktopModelRowViewModel> {
        self.model_groups
            .iter()
            .flat_map(|g| g.models.clone())
            .collect()
    }

    /// First group label for backward compatibility with spike tests.
    pub fn group_label(&self) -> String {
        self.model_groups
            .first()
            .map(|g| g.label.clone())
            .unwrap_or_else(|| "Models".to_string())
    }

    /// Toggles diagnostic test state and increments click count.
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
        panel.set_is_listening(vm.is_listening);
        panel.set_preload_on_startup(vm.preload_on_startup);
        panel.set_idle_unload_minutes(vm.idle_unload_minutes.map(|m| m as i32).unwrap_or(-1));
        panel.set_idle_unload_label(vm.idle_unload_label.as_str().into());
        panel.set_history_enabled(vm.history_enabled);
        panel.set_footer_text(vm.footer_text.as_str().into());

        // Language options
        panel.set_has_language_options(vm.has_language_options);
        panel.set_language_model_id(vm.language_model_id.as_str().into());
        panel.set_current_language_label(vm.current_language_label.as_str().into());
        let lang_opts: Vec<SlintLanguageOptionData> = vm
            .language_options
            .iter()
            .map(|opt| SlintLanguageOptionData {
                locale: opt.locale.as_str().into(),
                label: opt.label.as_str().into(),
                is_selected: opt.is_selected,
            })
            .collect();
        panel.set_language_options(Rc::new(slint::VecModel::from(lang_opts)).into());

        // Hierarchical Model Groups
        let groups: Vec<SlintModelGroupData> = vm
            .model_groups
            .iter()
            .map(|g| {
                let rows: Vec<SlintModelRowData> = g
                    .models
                    .iter()
                    .map(|m| SlintModelRowData {
                        id: m.id.as_str().into(),
                        label: m.label.as_str().into(),
                        verification_label: m.verification_label.as_str().into(),
                        status_text: m.status_text.as_str().into(),
                        action_label: m.action_label.as_str().into(),
                        action_enabled: m.action_enabled,
                        selected: m.selected,
                        is_in_progress: m.is_in_progress,
                        progress_text: m.progress_text.as_str().into(),
                        progress_percent: m.progress_percent,
                    })
                    .collect();

                SlintModelGroupData {
                    id: g.id.as_str().into(),
                    label: g.label.as_str().into(),
                    models: Rc::new(slint::VecModel::from(rows)).into(),
                }
            })
            .collect();

        panel.set_model_groups(Rc::new(slint::VecModel::from(groups)).into());
    }

    /// Builds a deterministic, representative [`ControlSurfaceState`] fixture for tests.
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
                        download: DownloadPresentation::from_status(
                            &crate::models::download::DownloadStatus::Completed,
                        ),
                        primary_action: ModelPrimaryAction::None,
                        enabled: true,
                        language: Default::default(),
                    }],
                },
                ModelGroupPresentation {
                    id: "multilingual".to_string(),
                    label: "Multilingual".to_string(),
                    models: vec![ModelPresentation {
                        id: "echolet-whisper-large-v3".to_string(),
                        label: "Echolet Whisper Large v3 Turbo".to_string(),
                        verification_label: "Community".to_string(),
                        is_verified: false,
                        selected: false,
                        installed: false,
                        download: DownloadPresentation::from_status(
                            &crate::models::download::DownloadStatus::NotDownloading,
                        ),
                        primary_action: ModelPrimaryAction::Download,
                        enabled: true,
                        language: Default::default(),
                    }],
                },
            ],
            preload_on_startup: true,
            idle_unload_minutes: Some(10),
            history_enabled: true,
        }
    }

    /// Fixture with multiple models in a group to verify hierarchical non-flattened layout.
    pub fn create_multi_model_group_fixture() -> ControlSurfaceState {
        ControlSurfaceState {
            runtime_state: RuntimeState::Ready,
            model_groups: vec![
                ModelGroupPresentation {
                    id: "bilingual-zh-en".to_string(),
                    label: "Chinese + English".to_string(),
                    models: vec![ModelPresentation {
                        id: "echolet-nemotron-3.5".to_string(),
                        label: "X-ASR · 2026".to_string(),
                        verification_label: "Echolet Verified".to_string(),
                        is_verified: true,
                        selected: true,
                        installed: true,
                        download: DownloadPresentation::from_status(
                            &crate::models::download::DownloadStatus::Completed,
                        ),
                        primary_action: ModelPrimaryAction::None,
                        enabled: true,
                        language: Default::default(),
                    }],
                },
                ModelGroupPresentation {
                    id: "english".to_string(),
                    label: "English".to_string(),
                    models: vec![
                        ModelPresentation {
                            id: "english-small".to_string(),
                            label: "English Streaming 2023 (Small)".to_string(),
                            verification_label: "Echolet Verified".to_string(),
                            is_verified: true,
                            selected: false,
                            installed: true,
                            download: DownloadPresentation::from_status(
                                &crate::models::download::DownloadStatus::Completed,
                            ),
                            primary_action: ModelPrimaryAction::Select,
                            enabled: true,
                            language: Default::default(),
                        },
                        ModelPresentation {
                            id: "english-large".to_string(),
                            label: "Nemotron Speech EN 0.6B (Large)".to_string(),
                            verification_label: "Candidate".to_string(),
                            is_verified: false,
                            selected: false,
                            installed: false,
                            download: DownloadPresentation::from_status(
                                &crate::models::download::DownloadStatus::NotDownloading,
                            ),
                            primary_action: ModelPrimaryAction::Download,
                            enabled: true,
                            language: Default::default(),
                        },
                    ],
                },
            ],
            preload_on_startup: false,
            idle_unload_minutes: Some(5),
            history_enabled: true,
        }
    }

    /// Long names fixture for testing elision without widening 380px panel width.
    pub fn create_spike_fixture_long_names() -> ControlSurfaceState {
        ControlSurfaceState {
            runtime_state: RuntimeState::Ready,
            model_groups: vec![ModelGroupPresentation {
                id: "bilingual-zh-en-extremely-long-capability-group-name-testing-overflow"
                    .to_string(),
                label: "Chinese + English Extremely Long Descriptive Capability Group Title That Must Not Expand Panel Width Beyond 380 Pixels"
                    .to_string(),
                models: vec![ModelPresentation {
                    id: "model-with-an-absurdly-long-identifier-and-display-name-for-ui-boundary-testing"
                        .to_string(),
                    label: "Echolet Ultra-High-Performance Streaming Speech Ingestion Model Specification With Extreme Characters [2026 Edition]"
                        .to_string(),
                    verification_label: "Echolet Verified Rigorous Evaluation".to_string(),
                    is_verified: true,
                    selected: true,
                    installed: true,
                    download: DownloadPresentation::from_status(
                        &crate::models::download::DownloadStatus::Completed,
                    ),
                    primary_action: ModelPrimaryAction::None,
                    enabled: true,
                    language: Default::default(),
                }],
            }],
            preload_on_startup: true,
            idle_unload_minutes: Some(30),
            history_enabled: false,
        }
    }
}
