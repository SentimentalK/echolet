use crate::config::EcholetConfig;
use crate::models::download::DownloadStatus;
use crate::models::registry::{LanguageTier, ModelRegistry};
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
    } else if is_loaded {
        RuntimeState::Ready
    } else if is_loading {
        RuntimeState::Loading
    } else {
        RuntimeState::Unloaded
    }
}

/// A selectable language option in the platform UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageOptionView {
    /// BCP-47 locale identifier, e.g. `ja-JP`. Kept verbatim.
    pub locale: String,
    /// Human-readable label, e.g. `Japanese (Japan)`.
    pub label: String,
    /// Runtime code passed to the model, e.g. `ja`.
    pub runtime_code: String,
    pub tier: LanguageTier,
}

/// Per-model language UI projection. Empty `options` means the model has no
/// forced-language support (for example X-ASR) and the Language menu must be
/// hidden.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelLanguageView {
    pub options: Vec<LanguageOptionView>,
    /// Selected BCP-47 locale; `None` means Auto-detect.
    pub selected_locale: Option<String>,
}

impl ModelLanguageView {
    pub fn transcription_ready(&self) -> impl Iterator<Item = &LanguageOptionView> {
        self.options
            .iter()
            .filter(|o| o.tier == LanguageTier::TranscriptionReady)
    }

    pub fn broad_coverage(&self) -> impl Iterator<Item = &LanguageOptionView> {
        self.options
            .iter()
            .filter(|o| o.tier == LanguageTier::BroadCoverage)
    }
}

/// A single model row in the platform UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformModelItem {
    pub id: String,
    /// Human-readable model label (from registry metadata, never hardcoded).
    pub label: String,
    /// Verification status label, e.g. `Echolet Verified`.
    pub verification_label: String,
    pub is_verified: bool,
    pub is_selected: bool,
    pub is_installed: bool,
    pub download: DownloadStatus,
    pub language: ModelLanguageView,
}

/// Complete platform-neutral projection of model/runtime/settings state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformView {
    pub runtime_state: RuntimeState,
    pub models: Vec<PlatformModelItem>,
    pub preload_on_startup: bool,
    pub idle_unload_minutes: Option<u32>,
}

impl Default for PlatformView {
    fn default() -> Self {
        Self {
            runtime_state: RuntimeState::NoModel,
            models: Vec::new(),
            preload_on_startup: false,
            idle_unload_minutes: Some(10),
        }
    }
}

impl PlatformView {
    pub fn selected_model(&self) -> Option<&PlatformModelItem> {
        self.models.iter().find(|m| m.is_selected)
    }

    /// The active model's language view, when that model supports language
    /// selection. Returns `None` for single-language models such as X-ASR.
    pub fn active_language(&self) -> Option<&ModelLanguageView> {
        let active = self.selected_model()?;
        if active.language.options.is_empty() {
            None
        } else {
            Some(&active.language)
        }
    }
}

/// Builds the platform view from registry + manager + config state. This is the
/// one function that turns domain state into UI state, so the tray and the
/// tests observe identical semantics.
#[allow(clippy::too_many_arguments)]
pub fn build_view(
    registry: &ModelRegistry,
    active_id: Option<&str>,
    installed: &HashSet<String>,
    downloading: &HashSet<String>,
    progress: &HashMap<String, DownloadStatus>,
    config: &EcholetConfig,
    runtime_state: RuntimeState,
) -> PlatformView {
    let mut models = Vec::with_capacity(registry.models.len());
    for entry in &registry.models {
        let is_installed = installed.contains(&entry.id);
        let download = progress.get(&entry.id).cloned().unwrap_or_else(|| {
            if downloading.contains(&entry.id) {
                DownloadStatus::Downloading {
                    downloaded_bytes: 0,
                    total_bytes: None,
                }
            } else {
                DownloadStatus::NotDownloading
            }
        });

        let options: Vec<LanguageOptionView> = entry
            .supported_language_options()
            .iter()
            .map(|o| LanguageOptionView {
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

        models.push(PlatformModelItem {
            id: entry.id.clone(),
            label: entry.display_title(),
            verification_label: entry.verification_status.label().to_string(),
            is_verified: entry.verification_status.is_verified(),
            is_selected: active_id == Some(entry.id.as_str()),
            is_installed,
            download,
            language: ModelLanguageView {
                options,
                selected_locale,
            },
        });
    }

    PlatformView {
        runtime_state,
        models,
        preload_on_startup: config.preload_model_on_startup,
        idle_unload_minutes: config.model_idle_unload_minutes,
    }
}

/// Concise label for an in-flight or failed download. Returns `None` when no
/// download state should be shown.
pub fn download_status_label(status: &DownloadStatus) -> Option<String> {
    match status {
        DownloadStatus::NotDownloading => None,
        DownloadStatus::Starting => Some("Starting download…".to_string()),
        DownloadStatus::Downloading {
            downloaded_bytes,
            total_bytes,
        } => Some(match total_bytes.filter(|t| *t > 0) {
            Some(total) => {
                let percent = (*downloaded_bytes).saturating_mul(100) / total;
                format!("Downloading {}%", percent.min(100))
            }
            None => format!("Downloading {}", format_bytes(*downloaded_bytes)),
        }),
        DownloadStatus::Verifying => Some("Verifying…".to_string()),
        DownloadStatus::Extracting => Some("Extracting…".to_string()),
        DownloadStatus::Installing => Some("Installing…".to_string()),
        DownloadStatus::Completed => Some("Installing…".to_string()),
        DownloadStatus::Failed => Some("Download failed — Retry".to_string()),
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
    use crate::models::registry::ModelRegistry;

    fn registry() -> ModelRegistry {
        ModelRegistry::from_str(include_str!("../../models/registry.json"))
            .expect("shipped registry must parse")
    }

    #[test]
    fn runtime_state_projection_precedence() {
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
            project_runtime_state(true, true, false, true),
            RuntimeState::Listening
        );
        // Listening wins even if a stale loading flag lingers.
        assert_eq!(
            project_runtime_state(true, true, true, true),
            RuntimeState::Listening
        );
    }

    #[test]
    fn download_status_labels_are_actionable() {
        assert_eq!(download_status_label(&DownloadStatus::NotDownloading), None);
        assert_eq!(
            download_status_label(&DownloadStatus::Downloading {
                downloaded_bytes: 512,
                total_bytes: Some(1024),
            })
            .as_deref(),
            Some("Downloading 50%")
        );
        assert!(download_status_label(&DownloadStatus::Downloading {
            downloaded_bytes: 2 * 1024 * 1024,
            total_bytes: None,
        })
        .unwrap()
        .starts_with("Downloading "));
        assert_eq!(
            download_status_label(&DownloadStatus::Failed).as_deref(),
            Some("Download failed — Retry")
        );
    }

    #[test]
    fn build_view_marks_download_state_and_language_selection() {
        let registry = registry();
        let nemotron = "echolet-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11-r1";
        let mut config = EcholetConfig::default();
        config.set_language_preference(nemotron, Some("ja-JP"));
        let mut installed = HashSet::new();
        installed.insert(nemotron.to_string());
        let mut downloading = HashSet::new();
        downloading.insert("other".to_string());
        let mut progress = HashMap::new();
        progress.insert(
            "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1".to_string(),
            DownloadStatus::Downloading {
                downloaded_bytes: 1,
                total_bytes: Some(4),
            },
        );

        let view = build_view(
            &registry,
            Some(nemotron),
            &installed,
            &downloading,
            &progress,
            &config,
            RuntimeState::Ready,
        );

        let active = view.selected_model().unwrap();
        assert_eq!(active.id, nemotron);
        assert_eq!(active.language.selected_locale.as_deref(), Some("ja-JP"));
        assert_eq!(active.language.transcription_ready().count(), 19);
        assert_eq!(active.language.broad_coverage().count(), 13);

        // X-ASR download projection + no language options.
        let xasr = view
            .models
            .iter()
            .find(|m| m.id.starts_with("echolet-xasr"))
            .unwrap();
        assert_eq!(
            xasr.download,
            DownloadStatus::Downloading {
                downloaded_bytes: 1,
                total_bytes: Some(4),
            }
        );
        assert!(xasr.language.options.is_empty());
    }

    #[test]
    fn format_bytes_scales_units() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
