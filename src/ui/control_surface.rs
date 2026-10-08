//! Canonical Shared Control Surface Core (PROJECT-041 / J11.1b).
//!
//! Architectural direction:
//!
//! ```text
//! domain/core
//!    -> ui/control_surface
//!       -> platform render/host
//! ```
//!
//! # Responsibilities
//! - This presentation layer owns:
//!   - model grouping / capability grouping;
//!   - user-facing model labels and status lines;
//!   - explicit primary model action (`ModelPrimaryAction`);
//!   - enabled/disabled semantics;
//!   - download, install, selection, and retry presentation;
//!   - runtime state presentation;
//!   - language-selection presentation;
//!   - preload and idle-unload presentation;
//!   - local history presentation.
//!
//! - Platform renderers/hosts (Linux ksni, macOS status item, Windows tray,
//!   future Android/iOS keyboard hosts) are NOT allowed to independently infer
//!   product behavior. They render immutable projected [`ControlSurfaceState`]
//!   and emit explicit [`SurfaceAction`] variants.

use crate::actions::AppAction;
use crate::config::EcholetConfig;
use crate::models::download::DownloadStatus;
use crate::models::language::language_code_label;
use crate::models::registry::{LanguageTier, ModelRegistry, RegistryModelEntry};
use crossbeam_channel::Sender;
use std::collections::{HashMap, HashSet};

/// Platform-neutral runtime residency/activity state projected from `App`.
///
/// There is exactly one authority for this state (derived from the app's
/// recognizer residency and listening flag); platform UIs must not maintain a
/// second divergent state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeState {
    /// No active installed model exists.
    NoModel,
    /// An active installed model exists but its recognizer is not resident.
    Unloaded,
    /// The active model's recognizer is currently being created.
    Loading,
    /// The recognizer is resident and not listening.
    Ready,
    /// Actively capturing and decoding audio.
    Listening,
}

impl Default for RuntimeState {
    fn default() -> Self {
        RuntimeState::NoModel
    }
}

impl RuntimeState {
    /// Concise, human-readable status row label for a tray/menu.
    pub fn status_label(self) -> &'static str {
        match self {
            RuntimeState::NoModel => "Status: No model",
            RuntimeState::Unloaded => "Status: Unloaded",
            RuntimeState::Loading => "Status: Loading model…",
            RuntimeState::Ready => "Status: Ready",
            RuntimeState::Listening => "Status: Listening",
        }
    }

    pub fn is_listening(self) -> bool {
        matches!(self, RuntimeState::Listening)
    }
}

/// Single source of truth for deriving the runtime state.
pub fn project_runtime_state(
    has_active_model: bool,
    is_loaded: bool,
    is_loading: bool,
    is_listening: bool,
) -> RuntimeState {
    if is_listening {
        RuntimeState::Listening
    } else if !has_active_model {
        RuntimeState::NoModel
    } else if is_loading {
        RuntimeState::Loading
    } else if is_loaded {
        RuntimeState::Ready
    } else {
        RuntimeState::Unloaded
    }
}

/// Canonical platform-neutral UI action emitted by renderers/hosts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceAction {
    ToggleListening,
    DownloadModel(String),
    SelectModel(String),
    SelectLanguage {
        model_id: String,
        locale: Option<String>,
    },
    SetPreloadModelOnStartup(bool),
    SetModelIdleUnloadMinutes(Option<u32>),
    SetHistoryEnabled(bool),
    ToggleHistory,
    OpenHistoryFolder,
    Quit,
}

impl From<SurfaceAction> for AppAction {
    fn from(action: SurfaceAction) -> Self {
        match action {
            SurfaceAction::ToggleListening => AppAction::ToggleListening,
            SurfaceAction::DownloadModel(id) => AppAction::DownloadModel(id),
            SurfaceAction::SelectModel(id) => AppAction::SelectModel(id),
            SurfaceAction::SelectLanguage { model_id, locale } => {
                AppAction::SelectLanguage { model_id, locale }
            }
            SurfaceAction::SetPreloadModelOnStartup(val) => {
                AppAction::SetPreloadModelOnStartup(val)
            }
            SurfaceAction::SetModelIdleUnloadMinutes(val) => {
                AppAction::SetModelIdleUnloadMinutes(val)
            }
            SurfaceAction::SetHistoryEnabled(val) => AppAction::SetHistoryEnabled(val),
            SurfaceAction::ToggleHistory => AppAction::ToggleHistory,
            SurfaceAction::OpenHistoryFolder => AppAction::OpenHistoryFolder,
            SurfaceAction::Quit => AppAction::Quit,
        }
    }
}

/// Dispatches a platform-neutral [`SurfaceAction`] to the application event channel
/// through the canonical mapping boundary.
pub fn dispatch_surface_action(tx: &Sender<AppAction>, action: SurfaceAction) {
    let _ = tx.send(action.into());
}

/// Explicit primary action for a model row in the UI.
///
/// A renderer MUST NOT contain inference logic like `if installed => Select else Download`.
/// The shared presentation layer dictates the exact actionable primary action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelPrimaryAction {
    /// No actionable primary action (e.g. model is already active or in-progress download).
    None,
    /// Model is not installed; activating downloads it.
    Download,
    /// Model download previously failed; activating retries download.
    RetryDownload,
    /// Model is installed and inactive; activating selects it.
    Select,
}

impl ModelPrimaryAction {
    pub fn is_none(&self) -> bool {
        matches!(self, ModelPrimaryAction::None)
    }

    /// Converts this primary action into the corresponding platform-neutral [`SurfaceAction`],
    /// or `None` if this action is not executable.
    pub fn to_surface_action(&self, model_id: &str) -> Option<SurfaceAction> {
        match self {
            ModelPrimaryAction::None => None,
            ModelPrimaryAction::Download | ModelPrimaryAction::RetryDownload => {
                Some(SurfaceAction::DownloadModel(model_id.to_string()))
            }
            ModelPrimaryAction::Select => Some(SurfaceAction::SelectModel(model_id.to_string())),
        }
    }
}

/// Normalized download phase for UI presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadPhase {
    NotDownloading,
    Starting,
    Downloading,
    Verifying,
    Extracting,
    Installing,
    Completed,
    Failed,
}

/// Shared presentation representation for a model's download/install state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadPresentation {
    pub phase: DownloadPhase,
    /// Concise, user-facing label (e.g. `Downloading 45%`, `Starting download…`, `Download failed — Retry`).
    pub label: Option<String>,
    /// Normalized progress fraction in `[0.0, 1.0]` where deterministically known.
    pub progress_fraction: Option<String>,
    /// Progress percent in `0..=100`, capped at 100, where deterministically known.
    pub progress_percent: Option<u8>,
    /// Whether this download failed and can be retried.
    pub retryable: bool,
}

impl DownloadPresentation {
    pub fn from_status(status: &DownloadStatus) -> Self {
        match status {
            DownloadStatus::NotDownloading => Self {
                phase: DownloadPhase::NotDownloading,
                label: None,
                progress_fraction: None,
                progress_percent: None,
                retryable: false,
            },
            DownloadStatus::Starting => Self {
                phase: DownloadPhase::Starting,
                label: Some("Starting download…".to_string()),
                progress_fraction: None,
                progress_percent: None,
                retryable: false,
            },
            DownloadStatus::Downloading {
                downloaded_bytes,
                total_bytes,
            } => {
                let (label, fraction, percent) = match total_bytes.filter(|t| *t > 0) {
                    Some(total) => {
                        let pct = ((*downloaded_bytes).saturating_mul(100) / total).min(100) as u8;
                        let frac = format!(
                            "{:.2}",
                            ((*downloaded_bytes as f64) / (total as f64)).min(1.0)
                        );
                        (format!("Downloading {}%", pct), Some(frac), Some(pct))
                    }
                    None => (
                        format!("Downloading {}", format_bytes(*downloaded_bytes)),
                        None,
                        None,
                    ),
                };
                Self {
                    phase: DownloadPhase::Downloading,
                    label: Some(label),
                    progress_fraction: fraction,
                    progress_percent: percent,
                    retryable: false,
                }
            }
            DownloadStatus::Verifying => Self {
                phase: DownloadPhase::Verifying,
                label: Some("Verifying…".to_string()),
                progress_fraction: None,
                progress_percent: None,
                retryable: false,
            },
            DownloadStatus::Extracting => Self {
                phase: DownloadPhase::Extracting,
                label: Some("Extracting…".to_string()),
                progress_fraction: None,
                progress_percent: None,
                retryable: false,
            },
            DownloadStatus::Installing => Self {
                phase: DownloadPhase::Installing,
                label: Some("Installing…".to_string()),
                progress_fraction: None,
                progress_percent: None,
                retryable: false,
            },
            DownloadStatus::Completed => Self {
                phase: DownloadPhase::Completed,
                label: Some("Installed".to_string()),
                progress_fraction: Some("1.00".to_string()),
                progress_percent: Some(100),
                retryable: false,
            },
            DownloadStatus::Failed => Self {
                phase: DownloadPhase::Failed,
                label: Some("Download failed — Retry".to_string()),
                progress_fraction: None,
                progress_percent: None,
                retryable: true,
            },
        }
    }

    pub fn is_in_progress(&self) -> bool {
        matches!(
            self.phase,
            DownloadPhase::Starting
                | DownloadPhase::Downloading
                | DownloadPhase::Verifying
                | DownloadPhase::Extracting
                | DownloadPhase::Installing
        )
    }
}

/// A selectable language option in the platform UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageOptionPresentation {
    /// BCP-47 locale identifier, e.g. `ja-JP`.
    pub locale: String,
    /// Human-readable label, e.g. `Japanese (Japan)`.
    pub label: String,
    /// Runtime code passed to the model, e.g. `ja`.
    pub runtime_code: String,
    pub tier: LanguageTier,
}

/// Per-model language UI presentation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelLanguagePresentation {
    pub options: Vec<LanguageOptionPresentation>,
    /// Selected BCP-47 locale; `None` means Auto-detect.
    pub selected_locale: Option<String>,
}

impl ModelLanguagePresentation {
    pub fn is_empty(&self) -> bool {
        self.options.is_empty()
    }

    pub fn is_auto_selected(&self) -> bool {
        self.selected_locale.is_none()
    }

    pub fn transcription_ready(&self) -> impl Iterator<Item = &LanguageOptionPresentation> {
        self.options
            .iter()
            .filter(|o| o.tier == LanguageTier::TranscriptionReady)
    }

    pub fn broad_coverage(&self) -> impl Iterator<Item = &LanguageOptionPresentation> {
        self.options
            .iter()
            .filter(|o| o.tier == LanguageTier::BroadCoverage)
    }
}

/// Projected presentation state for a single model in the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPresentation {
    pub id: String,
    /// Short / display label from registry metadata.
    pub label: String,
    /// Release date shown under the name. Full upstream date when known, otherwise the version year.
    pub release_date: String,
    /// Verification status label, e.g. `Echolet Verified`.
    pub verification_label: String,
    pub is_verified: bool,
    pub selected: bool,
    pub installed: bool,
    pub download: DownloadPresentation,
    /// Explicit primary action decided by the shared presentation builder.
    pub primary_action: ModelPrimaryAction,
    /// Whether this row's primary action is enabled for user interaction.
    pub enabled: bool,
    pub language: ModelLanguagePresentation,
}

impl ModelPresentation {
    pub fn is_selected(&self) -> bool {
        self.selected
    }

    pub fn is_installed(&self) -> bool {
        self.installed
    }

    /// Resolves the executable [`SurfaceAction`] for this model row if actionable.
    pub fn surface_action(&self) -> Option<SurfaceAction> {
        if !self.enabled {
            None
        } else {
            self.primary_action.to_surface_action(&self.id)
        }
    }

    /// Concise, unambiguous model menu label including verification + state.
    pub fn menu_item_label(&self) -> String {
        let base = if self.selected {
            format!("✓ {} (Selected)", self.label)
        } else if let Some(dl) = &self.download.label {
            // For active downloading/failed states, show the progress label
            if self.download.phase != DownloadPhase::Completed {
                format!("{} — {}", self.label, dl)
            } else if self.installed {
                format!("{} — Installed", self.label)
            } else {
                format!("{} — Download", self.label)
            }
        } else if self.installed {
            format!("{} — Installed", self.label)
        } else {
            format!("{} — Download", self.label)
        };
        format!("{} · {}", base, self.verification_label)
    }
}

/// A presentation group of models with shared capability (e.g. Chinese + English, Multilingual).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelGroupPresentation {
    pub id: String,
    pub label: String,
    pub models: Vec<ModelPresentation>,
}

/// Complete canonical UI projection of model, runtime, and settings state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlSurfaceState {
    pub runtime_state: RuntimeState,
    pub model_groups: Vec<ModelGroupPresentation>,
    pub preload_on_startup: bool,
    pub idle_unload_minutes: Option<u32>,
    pub history_enabled: bool,
}

impl Default for ControlSurfaceState {
    fn default() -> Self {
        Self {
            runtime_state: RuntimeState::NoModel,
            model_groups: Vec::new(),
            preload_on_startup: false,
            idle_unload_minutes: Some(10),
            history_enabled: false,
        }
    }
}

impl ControlSurfaceState {
    /// Iterates through all projected models across all capability groups.
    pub fn all_models(&self) -> impl Iterator<Item = &ModelPresentation> {
        self.model_groups.iter().flat_map(|g| g.models.iter())
    }

    /// Returns the currently active/selected model, if any.
    pub fn selected_model(&self) -> Option<&ModelPresentation> {
        self.all_models().find(|m| m.selected)
    }

    /// Finds a model by ID across all capability groups.
    pub fn find_model(&self, id: &str) -> Option<&ModelPresentation> {
        self.all_models().find(|m| m.id == id)
    }

    /// The active model's language presentation, when that model supports language selection.
    pub fn active_language(&self) -> Option<&ModelLanguagePresentation> {
        let active = self.selected_model()?;
        if active.language.options.is_empty() {
            None
        } else {
            Some(&active.language)
        }
    }
}

/// Derives the capability group key and human-readable label from registry metadata.
///
/// Grouping rules:
/// - Derived entirely from registry metadata (`languages`, `language_options`), NEVER hardcoded model IDs.
/// - Broad multilingual models (> 2 languages or tiered multilingual options) -> `("multilingual", "Multilingual")`.
/// - Single-language models -> `("{code}", "{LanguageName}")` (e.g. `("en", "English")`).
/// - Bilingual models (e.g. `zh` + `en`) -> `("zh-en", "Chinese + English")`.
pub fn derive_capability_group(entry: &RegistryModelEntry) -> (String, String) {
    let supported_count = entry
        .language_options
        .as_ref()
        .map_or(0, |lo| lo.supported.len());
    let is_multilingual = entry.languages.len() > 2 || supported_count > 2;

    if is_multilingual {
        ("multilingual".to_string(), "Multilingual".to_string())
    } else if entry.languages.is_empty() {
        ("general".to_string(), "General".to_string())
    } else if entry.languages.len() == 1 {
        let code = &entry.languages[0];
        (code.clone(), language_code_label(code).to_string())
    } else if entry.languages.len() == 2 {
        let has_zh = entry.languages.iter().any(|c| c == "zh");
        let has_en = entry.languages.iter().any(|c| c == "en");
        if has_zh && has_en {
            ("zh-en".to_string(), "Chinese + English".to_string())
        } else {
            let mut sorted = entry.languages.clone();
            sorted.sort();
            let key = sorted.join("-");
            let label = sorted
                .iter()
                .map(|c| language_code_label(c))
                .collect::<Vec<_>>()
                .join(" + ");
            (key, label)
        }
    } else {
        ("multilingual".to_string(), "Multilingual".to_string())
    }
}

/// Day-level release date, plus the installed size when we know it.
fn model_release_subtitle(entry: &RegistryModelEntry) -> String {
    let date = entry
        .upstream_release_date
        .clone()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| entry.version.clone());
    match entry.installed_size_bytes {
        Some(bytes) => format!("{date} · {}", format_installed_size(bytes)),
        None => date,
    }
}

fn format_installed_size(bytes: u64) -> String {
    let mb = (bytes as f64) / (1024.0 * 1024.0);
    if mb >= 10.0 {
        format!("{:.0} MB", mb.round())
    } else {
        format!("{:.1} MB", mb)
    }
}

/// Single shared builder path that projects domain state into [`ControlSurfaceState`].
#[allow(clippy::too_many_arguments)]
pub fn build_control_surface_state(
    registry: &ModelRegistry,
    active_id: Option<&str>,
    installed: &HashSet<String>,
    downloading: &HashSet<String>,
    progress: &HashMap<String, DownloadStatus>,
    config: &EcholetConfig,
    runtime_state: RuntimeState,
    history_enabled: bool,
) -> ControlSurfaceState {
    let is_listening = runtime_state.is_listening();
    let mut groups: Vec<ModelGroupPresentation> = Vec::new();

    for entry in &registry.models {
        let is_installed = installed.contains(&entry.id);
        let is_selected = active_id == Some(entry.id.as_str());

        let download_status = progress.get(&entry.id).cloned().unwrap_or_else(|| {
            if downloading.contains(&entry.id) {
                DownloadStatus::Downloading {
                    downloaded_bytes: 0,
                    total_bytes: None,
                }
            } else {
                DownloadStatus::NotDownloading
            }
        });
        let download = DownloadPresentation::from_status(&download_status);

        // Derive explicit primary action and enabled state
        let (primary_action, enabled) = if is_selected {
            (ModelPrimaryAction::None, false)
        } else if download.is_in_progress() {
            (ModelPrimaryAction::None, false)
        } else if download.phase == DownloadPhase::Failed {
            (ModelPrimaryAction::RetryDownload, !is_listening)
        } else if is_installed {
            (ModelPrimaryAction::Select, !is_listening)
        } else {
            (ModelPrimaryAction::Download, !is_listening)
        };

        let options: Vec<LanguageOptionPresentation> = entry
            .supported_language_options()
            .iter()
            .map(|o| LanguageOptionPresentation {
                locale: o.locale.clone(),
                label: o.display_name.clone().unwrap_or_else(|| o.locale.clone()),
                runtime_code: o.runtime_code.clone(),
                tier: o.tier,
            })
            .collect();

        let selected_locale = config
            .language_preference(&entry.id)
            .filter(|pref| !pref.eq_ignore_ascii_case(EcholetConfig::LANGUAGE_AUTO))
            .map(|pref| pref.to_string());

        let language = ModelLanguagePresentation {
            options,
            selected_locale,
        };

        let model = ModelPresentation {
            id: entry.id.clone(),
            label: entry.display_title(),
            release_date: model_release_subtitle(entry),
            verification_label: entry.verification_status.label().to_string(),
            is_verified: entry.verification_status.is_verified(),
            selected: is_selected,
            installed: is_installed,
            download,
            primary_action,
            enabled,
            language,
        };

        let (group_id, group_label) = derive_capability_group(entry);
        if let Some(existing_group) = groups.iter_mut().find(|g| g.id == group_id) {
            existing_group.models.push(model);
        } else {
            groups.push(ModelGroupPresentation {
                id: group_id,
                label: group_label,
                models: vec![model],
            });
        }
    }

    ControlSurfaceState {
        runtime_state,
        model_groups: groups,
        preload_on_startup: config.preload_model_on_startup,
        idle_unload_minutes: config.model_idle_unload_minutes,
        history_enabled,
    }
}

/// Human-readable byte count with one decimal place, e.g. `12.3 MB`.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::download::DownloadStatus;
    use crate::models::registry::{
        ModelFilesConfig, ModelRuntimeConfig, ModelSource, VerificationStatus,
    };

    fn dummy_entry(id: &str, languages: Vec<&str>) -> RegistryModelEntry {
        RegistryModelEntry {
            id: id.to_string(),
            display_name: format!("Display {}", id),
            version: "1".to_string(),
            languages: languages.into_iter().map(String::from).collect(),
            family: "online-transducer".to_string(),
            source: ModelSource {
                bundled: false,
                url: None,
                sha256: None,
                repository: None,
                revision: None,
            },
            files: ModelFilesConfig {
                encoder: "enc.onnx".to_string(),
                decoder: "dec.onnx".to_string(),
                joiner: "join.onnx".to_string(),
                tokens: "tokens.txt".to_string(),
            },
            runtime: ModelRuntimeConfig {
                model_type: None,
                sample_rate: 16000,
                feature_dim: 80,
                num_threads: 1,
                provider: "cpu".to_string(),
                decoding_method: "greedy_search".to_string(),
                max_active_paths: 4,
            },
            download_size_bytes: None,
            installed_size_bytes: None,
            upstream_release_date: None,
            license: None,
            language_options: None,
            verification_status: VerificationStatus::EcholetVerified,
        }
    }

    #[test]
    fn test_projection_state_runtime_and_no_model() {
        assert_eq!(
            project_runtime_state(false, false, false, false),
            RuntimeState::NoModel
        );
        assert_eq!(
            project_runtime_state(true, false, false, false),
            RuntimeState::Unloaded
        );
        assert_eq!(
            project_runtime_state(true, false, true, false),
            RuntimeState::Loading
        );
        assert_eq!(
            project_runtime_state(true, true, false, false),
            RuntimeState::Ready
        );
        assert_eq!(
            project_runtime_state(true, true, true, false),
            RuntimeState::Loading
        );
        assert_eq!(
            project_runtime_state(true, true, false, true),
            RuntimeState::Listening
        );
        assert_eq!(
            project_runtime_state(true, true, true, true),
            RuntimeState::Listening
        );

        let default_state = ControlSurfaceState::default();
        assert_eq!(default_state.runtime_state, RuntimeState::NoModel);
        assert!(default_state.model_groups.is_empty());
        assert_eq!(default_state.selected_model(), None);
    }

    #[test]
    fn test_model_actionability_semantics() {
        let registry = ModelRegistry {
            schema_version: 2,
            default_model_id: "m1".to_string(),
            models: vec![
                dummy_entry("m1", vec!["en"]),
                dummy_entry("m2", vec!["en"]),
                dummy_entry("m3", vec!["en"]),
                dummy_entry("m4", vec!["en"]),
                dummy_entry("m5", vec!["en"]),
            ],
        };
        let mut installed = HashSet::new();
        installed.insert("m1".to_string());
        installed.insert("m2".to_string());

        let mut downloading = HashSet::new();
        downloading.insert("m3".to_string());

        let mut progress = HashMap::new();
        progress.insert("m4".to_string(), DownloadStatus::Failed);

        let config = EcholetConfig::default();

        // 1. Not listening
        let state = build_control_surface_state(
            &registry,
            Some("m1"),
            &installed,
            &downloading,
            &progress,
            &config,
            RuntimeState::Ready,
            false,
        );

        // m1: selected => None, disabled
        let m1 = state.find_model("m1").unwrap();
        assert_eq!(m1.primary_action, ModelPrimaryAction::None);
        assert!(!m1.enabled);
        assert_eq!(m1.surface_action(), None);

        // m2: installed, not selected, not listening => Select, enabled
        let m2 = state.find_model("m2").unwrap();
        assert_eq!(m2.primary_action, ModelPrimaryAction::Select);
        assert!(m2.enabled);
        assert_eq!(
            m2.surface_action(),
            Some(SurfaceAction::SelectModel("m2".to_string()))
        );

        // m3: in-progress downloading => None, disabled
        let m3 = state.find_model("m3").unwrap();
        assert_eq!(m3.primary_action, ModelPrimaryAction::None);
        assert!(!m3.enabled);
        assert_eq!(m3.surface_action(), None);

        // m4: failed download => RetryDownload, enabled
        let m4 = state.find_model("m4").unwrap();
        assert_eq!(m4.primary_action, ModelPrimaryAction::RetryDownload);
        assert!(m4.enabled);
        assert_eq!(
            m4.surface_action(),
            Some(SurfaceAction::DownloadModel("m4".to_string()))
        );

        // m5: uninstalled, idle => Download, enabled
        let m5 = state.find_model("m5").unwrap();
        assert_eq!(m5.primary_action, ModelPrimaryAction::Download);
        assert!(m5.enabled);
        assert_eq!(
            m5.surface_action(),
            Some(SurfaceAction::DownloadModel("m5".to_string()))
        );

        // 2. Listening prevents unsafe select and download
        let listening_state = build_control_surface_state(
            &registry,
            Some("m1"),
            &installed,
            &downloading,
            &progress,
            &config,
            RuntimeState::Listening,
            false,
        );
        let m2_list = listening_state.find_model("m2").unwrap();
        assert!(
            !m2_list.enabled,
            "switching model must be disabled while listening"
        );
        assert_eq!(m2_list.surface_action(), None);

        let m4_list = listening_state.find_model("m4").unwrap();
        assert!(
            !m4_list.enabled,
            "retrying download must be disabled while listening"
        );
        assert_eq!(m4_list.surface_action(), None);

        let m5_list = listening_state.find_model("m5").unwrap();
        assert!(
            !m5_list.enabled,
            "starting download must be disabled while listening"
        );
        assert_eq!(m5_list.surface_action(), None);
    }

    #[test]
    fn test_no_renderer_inference_action_is_explicit() {
        let registry = ModelRegistry {
            schema_version: 2,
            default_model_id: "m_inst".to_string(),
            models: vec![
                dummy_entry("m_inst", vec!["en"]),
                dummy_entry("m_uninst", vec!["en"]),
            ],
        };
        let mut installed = HashSet::new();
        installed.insert("m_inst".to_string());

        let state = build_control_surface_state(
            &registry,
            None,
            &installed,
            &HashSet::new(),
            &HashMap::new(),
            &EcholetConfig::default(),
            RuntimeState::Ready,
            false,
        );

        let m_inst = state.find_model("m_inst").unwrap();
        let m_uninst = state.find_model("m_uninst").unwrap();

        // The shared presentation output explicitly contains the action;
        // renderers do not test `is_installed ? select : download`.
        assert_eq!(m_inst.primary_action, ModelPrimaryAction::Select);
        assert_eq!(
            m_inst.surface_action(),
            Some(SurfaceAction::SelectModel("m_inst".to_string()))
        );

        assert_eq!(m_uninst.primary_action, ModelPrimaryAction::Download);
        assert_eq!(
            m_uninst.surface_action(),
            Some(SurfaceAction::DownloadModel("m_uninst".to_string()))
        );
    }

    #[test]
    fn test_grouping_generic_over_metadata_and_deterministic() {
        let registry = ModelRegistry {
            schema_version: 2,
            default_model_id: "synth-zh-en".to_string(),
            models: vec![
                dummy_entry("synth-zh-en", vec!["zh", "en"]),
                dummy_entry("synth-en-1", vec!["en"]),
                dummy_entry("synth-en-2", vec!["en"]),
                dummy_entry("synth-multi", vec!["en", "es", "fr", "de"]),
            ],
        };

        let state = build_control_surface_state(
            &registry,
            None,
            &HashSet::new(),
            &HashSet::new(),
            &HashMap::new(),
            &EcholetConfig::default(),
            RuntimeState::NoModel,
            false,
        );

        // Group ordering is strictly deterministic in appearance order
        assert_eq!(state.model_groups.len(), 3);
        assert_eq!(state.model_groups[0].id, "zh-en");
        assert_eq!(state.model_groups[0].label, "Chinese + English");
        assert_eq!(state.model_groups[0].models.len(), 1);

        assert_eq!(state.model_groups[1].id, "en");
        assert_eq!(state.model_groups[1].label, "English");
        assert_eq!(state.model_groups[1].models.len(), 2);
        assert_eq!(state.model_groups[1].models[0].id, "synth-en-1");
        assert_eq!(state.model_groups[1].models[1].id, "synth-en-2");

        assert_eq!(state.model_groups[2].id, "multilingual");
        assert_eq!(state.model_groups[2].label, "Multilingual");
        assert_eq!(state.model_groups[2].models.len(), 1);
    }

    #[test]
    fn test_download_presentation_rules() {
        // 1. Capped at 100%
        let dl_overflow = DownloadPresentation::from_status(&DownloadStatus::Downloading {
            downloaded_bytes: 200,
            total_bytes: Some(100),
        });
        assert_eq!(dl_overflow.progress_percent, Some(100));
        assert_eq!(dl_overflow.label.as_deref(), Some("Downloading 100%"));

        // 2. Unknown total gives meaningful progress text
        let dl_unknown = DownloadPresentation::from_status(&DownloadStatus::Downloading {
            downloaded_bytes: 5 * 1024 * 1024,
            total_bytes: None,
        });
        assert_eq!(dl_unknown.progress_percent, None);
        assert_eq!(dl_unknown.label.as_deref(), Some("Downloading 5.0 MB"));

        // 3. Failed is retryable
        let dl_failed = DownloadPresentation::from_status(&DownloadStatus::Failed);
        assert!(dl_failed.retryable);
        assert_eq!(dl_failed.phase, DownloadPhase::Failed);
        assert_eq!(dl_failed.label.as_deref(), Some("Download failed — Retry"));

        // 4. Completed is NOT rendered as Installing
        let dl_completed = DownloadPresentation::from_status(&DownloadStatus::Completed);
        assert_eq!(dl_completed.phase, DownloadPhase::Completed);
        assert_ne!(dl_completed.label.as_deref(), Some("Installing…"));
        assert_eq!(dl_completed.label.as_deref(), Some("Installed"));
    }

    #[test]
    fn test_surface_action_deterministic_mapping() {
        let actions = vec![
            (SurfaceAction::ToggleListening, AppAction::ToggleListening),
            (
                SurfaceAction::DownloadModel("m1".to_string()),
                AppAction::DownloadModel("m1".to_string()),
            ),
            (
                SurfaceAction::SelectModel("m1".to_string()),
                AppAction::SelectModel("m1".to_string()),
            ),
            (
                SurfaceAction::SelectLanguage {
                    model_id: "m1".to_string(),
                    locale: Some("ja-JP".to_string()),
                },
                AppAction::SelectLanguage {
                    model_id: "m1".to_string(),
                    locale: Some("ja-JP".to_string()),
                },
            ),
            (
                SurfaceAction::SetPreloadModelOnStartup(true),
                AppAction::SetPreloadModelOnStartup(true),
            ),
            (
                SurfaceAction::SetModelIdleUnloadMinutes(Some(10)),
                AppAction::SetModelIdleUnloadMinutes(Some(10)),
            ),
            (
                SurfaceAction::SetHistoryEnabled(true),
                AppAction::SetHistoryEnabled(true),
            ),
            (SurfaceAction::ToggleHistory, AppAction::ToggleHistory),
            (
                SurfaceAction::OpenHistoryFolder,
                AppAction::OpenHistoryFolder,
            ),
            (SurfaceAction::Quit, AppAction::Quit),
        ];

        for (surface, app) in actions {
            assert_eq!(AppAction::from(surface), app);
        }
    }
}
