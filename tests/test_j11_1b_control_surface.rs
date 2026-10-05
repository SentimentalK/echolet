//! PROJECT-041 / J11.1b — Shared Control Surface Core architectural proof tests.
//!
//! Verifies the shared UI product/presentation contract:
//! A. Projection/state: neutral startup, no-model, exact runtime state mapping
//! B. Model actionability: explicit primary actions and listening guard
//! C. No renderer inference: renderer does not decide `installed ? select : download`
//! D. Capability grouping: generic over metadata, deterministic, no hard-coded model IDs
//! E. Language presentation: auto None, tier representation, adaptation-only excluded
//! F. Download presentation: progress formatting, <=100% cap, retryability, completed != installing
//! G. Action mapping: deterministic 1-to-1 SurfaceAction -> AppAction mapping

use echolet::actions::AppAction;
use echolet::config::EcholetConfig;
use echolet::models::download::DownloadStatus;
use echolet::models::registry::{
    LanguageTier, ModelFilesConfig, ModelLanguageOption, ModelLanguageOptions, ModelRegistry,
    ModelRuntimeConfig, ModelSource, RegistryModelEntry, VerificationStatus,
};
use echolet::ui::control_surface::{
    build_control_surface_state, derive_capability_group, dispatch_surface_action, format_bytes,
    project_runtime_state, ControlSurfaceState, DownloadPhase, DownloadPresentation,
    ModelPrimaryAction, RuntimeState, SurfaceAction,
};
use std::collections::{HashMap, HashSet};

fn synthetic_entry(
    id: &str,
    display_name: &str,
    languages: Vec<&str>,
    language_options: Option<ModelLanguageOptions>,
) -> RegistryModelEntry {
    RegistryModelEntry {
        id: id.to_string(),
        display_name: display_name.to_string(),
        version: "1.0".to_string(),
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
        language_options,
        verification_status: VerificationStatus::EcholetVerified,
    }
}

// =========================================================================
// A. Projection / state
// =========================================================================

#[test]
fn test_a_projection_state_no_model_and_truthful_runtime_states() {
    // 1. Runtime states map exactly according to single authority
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
    // Listening takes precedence even if loading flag is lingering
    assert_eq!(
        project_runtime_state(true, true, true, true),
        RuntimeState::Listening
    );

    // 2. No-model state produces valid ControlSurfaceState
    assert_eq!(
        ControlSurfaceState::default().runtime_state,
        RuntimeState::NoModel
    );
    let registry = ModelRegistry {
        schema_version: 2,
        default_model_id: "test-m1".to_string(),
        models: vec![synthetic_entry("test-m1", "Test Model 1", vec!["en"], None)],
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

    assert_eq!(state.runtime_state, RuntimeState::NoModel);
    assert_eq!(state.selected_model(), None);
    assert!(state.all_models().all(|m| !m.installed));
    assert!(state.all_models().all(|m| !m.selected));
    assert!(!state.preload_on_startup);
    assert_eq!(state.idle_unload_minutes, Some(10));
    assert!(!state.history_enabled);
}

// =========================================================================
// B. Model actionability & C. No renderer inference
// =========================================================================

#[test]
fn test_b_and_c_model_actionability_and_no_renderer_inference() {
    let registry = ModelRegistry {
        schema_version: 2,
        default_model_id: "m_sel".to_string(),
        models: vec![
            synthetic_entry("m_sel", "Selected Model", vec!["en"], None),
            synthetic_entry("m_inst", "Installed Inactive", vec!["en"], None),
            synthetic_entry("m_uninst", "Uninstalled Model", vec!["en"], None),
            synthetic_entry("m_dl", "Downloading Model", vec!["en"], None),
            synthetic_entry("m_failed", "Failed Model", vec!["en"], None),
        ],
    };

    let mut installed = HashSet::new();
    installed.insert("m_sel".to_string());
    installed.insert("m_inst".to_string());

    let mut downloading = HashSet::new();
    downloading.insert("m_dl".to_string());

    let mut progress = HashMap::new();
    progress.insert("m_failed".to_string(), DownloadStatus::Failed);

    let config = EcholetConfig::default();

    // 1. Ready state (not listening)
    let ready_state = build_control_surface_state(
        &registry,
        Some("m_sel"),
        &installed,
        &downloading,
        &progress,
        &config,
        RuntimeState::Ready,
        false,
    );

    // selected => None, disabled
    let m_sel = ready_state.find_model("m_sel").unwrap();
    assert_eq!(m_sel.primary_action, ModelPrimaryAction::None);
    assert!(!m_sel.enabled);
    assert_eq!(m_sel.surface_action(), None);

    // installed + not selected + not listening => Select, enabled
    let m_inst = ready_state.find_model("m_inst").unwrap();
    assert_eq!(m_inst.primary_action, ModelPrimaryAction::Select);
    assert!(m_inst.enabled);
    assert_eq!(
        m_inst.surface_action(),
        Some(SurfaceAction::SelectModel("m_inst".to_string()))
    );

    // uninstalled + idle => Download, enabled
    let m_uninst = ready_state.find_model("m_uninst").unwrap();
    assert_eq!(m_uninst.primary_action, ModelPrimaryAction::Download);
    assert!(m_uninst.enabled);
    assert_eq!(
        m_uninst.surface_action(),
        Some(SurfaceAction::DownloadModel("m_uninst".to_string()))
    );

    // downloading => no actionable primary action (None, disabled)
    let m_dl = ready_state.find_model("m_dl").unwrap();
    assert_eq!(m_dl.primary_action, ModelPrimaryAction::None);
    assert!(!m_dl.enabled);
    assert_eq!(m_dl.surface_action(), None);

    // failed download => RetryDownload, enabled
    let m_failed = ready_state.find_model("m_failed").unwrap();
    assert_eq!(m_failed.primary_action, ModelPrimaryAction::RetryDownload);
    assert!(m_failed.enabled);
    assert_eq!(
        m_failed.surface_action(),
        Some(SurfaceAction::DownloadModel("m_failed".to_string()))
    );

    // 2. Listening state prevents unsafe model select/download
    let listening_state = build_control_surface_state(
        &registry,
        Some("m_sel"),
        &installed,
        &downloading,
        &progress,
        &config,
        RuntimeState::Listening,
        false,
    );

    let m_inst_listening = listening_state.find_model("m_inst").unwrap();
    assert!(!m_inst_listening.enabled);
    assert_eq!(m_inst_listening.surface_action(), None);

    let m_uninst_listening = listening_state.find_model("m_uninst").unwrap();
    assert!(!m_uninst_listening.enabled);
    assert_eq!(m_uninst_listening.surface_action(), None);

    let m_failed_listening = listening_state.find_model("m_failed").unwrap();
    assert!(!m_failed_listening.enabled);
    assert_eq!(m_failed_listening.surface_action(), None);

    // Renderer proof: renderer does NOT inspect is_installed to determine action.
    // The shared presentation object provides the action explicitly.
    assert_eq!(
        m_inst.primary_action.to_surface_action(&m_inst.id),
        Some(SurfaceAction::SelectModel("m_inst".to_string()))
    );
    assert_eq!(
        m_uninst.primary_action.to_surface_action(&m_uninst.id),
        Some(SurfaceAction::DownloadModel("m_uninst".to_string()))
    );
}

// =========================================================================
// D. Grouping
// =========================================================================

#[test]
fn test_d_capability_grouping_generic_and_deterministic() {
    let registry = ModelRegistry {
        schema_version: 2,
        default_model_id: "synth-bilingual".to_string(),
        models: vec![
            synthetic_entry("synth-bilingual", "Bilingual zh-en", vec!["zh", "en"], None),
            synthetic_entry("synth-en-alpha", "English Alpha", vec!["en"], None),
            synthetic_entry("synth-en-beta", "English Beta", vec!["en"], None),
            synthetic_entry(
                "synth-multi",
                "Multilingual 10L",
                vec!["en", "es", "fr", "de", "it"],
                None,
            ),
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

    // 1. zh+en -> one bilingual capability group
    assert_eq!(state.model_groups[0].id, "zh-en");
    assert_eq!(state.model_groups[0].label, "Chinese + English");
    assert_eq!(state.model_groups[0].models.len(), 1);

    // 2. en -> English group; multiple models with same capability land in same group
    assert_eq!(state.model_groups[1].id, "en");
    assert_eq!(state.model_groups[1].label, "English");
    assert_eq!(state.model_groups[1].models.len(), 2);
    assert_eq!(state.model_groups[1].models[0].id, "synth-en-alpha");
    assert_eq!(state.model_groups[1].models[1].id, "synth-en-beta");

    // 3. broad multilingual -> Multilingual group
    assert_eq!(state.model_groups[2].id, "multilingual");
    assert_eq!(state.model_groups[2].label, "Multilingual");
    assert_eq!(state.model_groups[2].models.len(), 1);
    assert_eq!(state.model_groups[2].models[0].id, "synth-multi");

    // 4. Group ordering is strictly deterministic (appearance order)
    assert_eq!(state.model_groups.len(), 3);

    // 5. Test derive_capability_group directly on metadata
    let entry_bilingual_rev = synthetic_entry("bilingual-rev", "Rev", vec!["en", "zh"], None);
    let (group_id, group_label) = derive_capability_group(&entry_bilingual_rev);
    assert_eq!(group_id, "zh-en");
    assert_eq!(group_label, "Chinese + English");
}

// =========================================================================
// E. Language presentation
// =========================================================================

#[test]
fn test_e_language_presentation_semantics() {
    let lang_options = ModelLanguageOptions {
        supported: vec![
            ModelLanguageOption {
                locale: "ja-JP".to_string(),
                runtime_code: "ja".to_string(),
                tier: LanguageTier::TranscriptionReady,
                display_name: Some("Japanese (Japan)".to_string()),
            },
            ModelLanguageOption {
                locale: "es-ES".to_string(),
                runtime_code: "es".to_string(),
                tier: LanguageTier::BroadCoverage,
                display_name: Some("Spanish (Spain)".to_string()),
            },
        ],
        adaptation_ready: vec![ModelLanguageOption {
            locale: "el-GR".to_string(),
            runtime_code: "el".to_string(),
            tier: LanguageTier::AdaptationReady,
            display_name: Some("Greek (Greece)".to_string()),
        }],
    };

    let registry = ModelRegistry {
        schema_version: 2,
        default_model_id: "m_lang".to_string(),
        models: vec![synthetic_entry(
            "m_lang",
            "Language Model",
            vec!["en", "ja", "es"],
            Some(lang_options),
        )],
    };

    let mut config = EcholetConfig::default();
    config.set_language_preference("m_lang", None); // Auto selection

    let state = build_control_surface_state(
        &registry,
        Some("m_lang"),
        &HashSet::new(),
        &HashSet::new(),
        &HashMap::new(),
        &config,
        RuntimeState::Ready,
        false,
    );

    let active_lang = state.active_language().unwrap();
    // Auto remains None
    assert!(active_lang.is_auto_selected());
    assert_eq!(active_lang.selected_locale, None);

    // TranscriptionReady and BroadCoverage are present
    assert_eq!(active_lang.transcription_ready().count(), 1);
    assert_eq!(
        active_lang.transcription_ready().next().unwrap().locale,
        "ja-JP"
    );
    assert_eq!(active_lang.broad_coverage().count(), 1);
    assert_eq!(active_lang.broad_coverage().next().unwrap().locale, "es-ES");

    // Adaptation-ready options remain excluded
    assert!(active_lang.options.iter().all(|o| o.locale != "el-GR"));
}

// =========================================================================
// F. Download presentation
// =========================================================================

#[test]
fn test_f_download_presentation_rules() {
    // 1. Percent calculation does not exceed 100%
    let dl_overflow = DownloadPresentation::from_status(&DownloadStatus::Downloading {
        downloaded_bytes: 1500,
        total_bytes: Some(1000),
    });
    assert_eq!(dl_overflow.progress_percent, Some(100));
    assert_eq!(dl_overflow.label.as_deref(), Some("Downloading 100%"));

    // 2. Unknown total still gives meaningful progress text
    let dl_unknown = DownloadPresentation::from_status(&DownloadStatus::Downloading {
        downloaded_bytes: 2048,
        total_bytes: None,
    });
    assert_eq!(dl_unknown.progress_percent, None);
    assert_eq!(dl_unknown.label.as_deref(), Some("Downloading 2.0 KB"));

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

    // 5. Byte formatter scales cleanly
    assert_eq!(format_bytes(500), "500 B");
    assert_eq!(format_bytes(1024), "1.0 KB");
    assert_eq!(format_bytes(10485760), "10.0 MB");
}

// =========================================================================
// G. Action mapping
// =========================================================================

#[test]
fn test_g_action_mapping_deterministic_and_unique() {
    let (tx, rx) = crossbeam_channel::unbounded();

    let pairs = vec![
        (SurfaceAction::ToggleListening, AppAction::ToggleListening),
        (
            SurfaceAction::DownloadModel("model-a".to_string()),
            AppAction::DownloadModel("model-a".to_string()),
        ),
        (
            SurfaceAction::SelectModel("model-a".to_string()),
            AppAction::SelectModel("model-a".to_string()),
        ),
        (
            SurfaceAction::SelectLanguage {
                model_id: "model-a".to_string(),
                locale: Some("ja-JP".to_string()),
            },
            AppAction::SelectLanguage {
                model_id: "model-a".to_string(),
                locale: Some("ja-JP".to_string()),
            },
        ),
        (
            SurfaceAction::SetPreloadModelOnStartup(true),
            AppAction::SetPreloadModelOnStartup(true),
        ),
        (
            SurfaceAction::SetModelIdleUnloadMinutes(Some(30)),
            AppAction::SetModelIdleUnloadMinutes(Some(30)),
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

    for (surface_action, expected_app_action) in pairs {
        // Direct conversion
        let mapped: AppAction = surface_action.clone().into();
        assert_eq!(mapped, expected_app_action);

        // Dispatched through shared dispatcher
        dispatch_surface_action(&tx, surface_action);
        let received = rx.try_recv().expect("action must be delivered");
        assert_eq!(received, expected_app_action);
    }
}
