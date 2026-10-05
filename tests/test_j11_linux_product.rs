//! PROJECT-041 Stage 4 / J11-Linux — cross-platform product-loop tests.
//!
//! These tests exercise the platform-neutral parts of the Linux dogfood loop
//! (download/select separation, progress/retry projection, runtime state,
//! preload/idle-unload persistence, per-model language selection) without a
//! DBus tray host, a network, or a real recognizer. Linux-only tray menu
//! rendering is covered inside `src/platform/linux/tray.rs`.

use crossbeam_channel::unbounded;
use echolet::actions::AppAction;
use echolet::app::App;
use echolet::audio::{AudioChunk, AudioSource, AudioStarter};
use echolet::config::EcholetConfig;
use echolet::models::download::DownloadStatus;
use echolet::models::manager::ModelManager;
use echolet::models::registry::ModelRegistry;
use echolet::platform::{PlatformHandle, PlatformRuntime, PlatformView, TextInjector};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Recovers from a poisoned lock so one failing assertion does not cascade into
/// unrelated `PoisonError` failures.
fn env_guard() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

struct FakeInjector;
impl TextInjector for FakeInjector {
    fn apply_diff(&self, _backspaces: usize, _new_suffix: &str) {}
}

#[derive(Default)]
struct CapturingHandle {
    views: Arc<Mutex<Vec<PlatformView>>>,
}

impl PlatformHandle for CapturingHandle {
    fn update_models(&self, view: &PlatformView) {
        self.views.lock().unwrap().push(view.clone());
    }
}

fn latest_view(handle: &CapturingHandle) -> PlatformView {
    handle
        .views
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("at least one platform projection must have been emitted")
}

fn unique_tmp(prefix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "echolet-j11-{}-{}-{}",
        prefix,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn registry_json() -> String {
    serde_json::json!({
        "schema_version": 2,
        "default_model_id": "mock-a",
        "models": [
            {
                "id": "mock-a",
                "display_name": "Mock A",
                "version": "1",
                "languages": ["en"],
                "family": "online-transducer",
                "source": { "bundled": false },
                "files": {
                    "encoder": "enc.onnx",
                    "decoder": "dec.onnx",
                    "joiner": "join.onnx",
                    "tokens": "tokens.txt"
                },
                "runtime": {},
                "verification_status": "Echolet Verified"
            },
            {
                "id": "mock-c",
                "display_name": "Mock C",
                "version": "1",
                "languages": ["en"],
                "family": "online-transducer",
                "source": { "bundled": false },
                "files": {
                    "encoder": "enc.onnx",
                    "decoder": "dec.onnx",
                    "joiner": "join.onnx",
                    "tokens": "tokens.txt"
                },
                "runtime": {},
                "verification_status": "Experimental"
            },
            {
                "id": "mock-lang",
                "display_name": "Mock Lang",
                "version": "1",
                "languages": ["en", "ja", "zh"],
                "family": "online-transducer",
                "source": { "bundled": false },
                "files": {
                    "encoder": "enc.onnx",
                    "decoder": "dec.onnx",
                    "joiner": "join.onnx",
                    "tokens": "tokens.txt"
                },
                "runtime": {},
                "language_options": {
                    "supported": [
                        { "locale": "ja-JP", "runtime_code": "ja", "tier": "TranscriptionReady", "display_name": "Japanese (Japan)" },
                        { "locale": "zh-CN", "runtime_code": "zh", "tier": "BroadCoverage", "display_name": "Mandarin Chinese (China)" }
                    ],
                    "adaptation_ready": [
                        { "locale": "el-GR", "runtime_code": "el", "tier": "AdaptationReady", "display_name": "Greek (Greece)" }
                    ]
                },
                "verification_status": "Echolet Verified"
            }
        ]
    })
    .to_string()
}

fn write_model_bundle(
    user_models: &Path,
    id: &str,
    display: &str,
    language_options: Option<serde_json::Value>,
) {
    let dir = user_models.join(id);
    fs::create_dir_all(&dir).unwrap();
    let mut manifest = serde_json::json!({
        "id": id,
        "display_name": display,
        "version": "1",
        "languages": ["en"],
        "family": "online-transducer",
        "encoder": "enc.onnx",
        "decoder": "dec.onnx",
        "joiner": "join.onnx",
        "tokens": "tokens.txt"
    });
    if let Some(opts) = language_options {
        manifest["language_options"] = opts;
    }
    fs::write(
        dir.join("model.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
    for f in ["enc.onnx", "dec.onnx", "join.onnx", "tokens.txt"] {
        fs::write(dir.join(f), b"dummy").unwrap();
    }
}

fn mock_lang_options() -> serde_json::Value {
    serde_json::json!({
        "supported": [
            { "locale": "ja-JP", "runtime_code": "ja", "tier": "TranscriptionReady", "display_name": "Japanese (Japan)" },
            { "locale": "zh-CN", "runtime_code": "zh", "tier": "BroadCoverage", "display_name": "Mandarin Chinese (China)" }
        ],
        "adaptation_ready": [
            { "locale": "el-GR", "runtime_code": "el", "tier": "AdaptationReady", "display_name": "Greek (Greece)" }
        ]
    })
}

/// Isolated environment + manager + app for J11 product tests.
struct Harness {
    app: App,
    action_tx: crossbeam_channel::Sender<AppAction>,
    handle: CapturingHandle,
    _home: PathBuf,
    tmp: PathBuf,
}

fn build_harness(prefix: &str) -> Harness {
    let tmp = unique_tmp(prefix);
    let home = tmp.join("home");
    let bundle_models = tmp.join("bundle/models");
    let user_models = home.join("models");
    fs::create_dir_all(&bundle_models).unwrap();
    fs::create_dir_all(&user_models).unwrap();
    fs::write(bundle_models.join("registry.json"), registry_json()).unwrap();

    // Registries with an invalid URL are used; no real network is ever needed.
    let config_path = home.join("config.json");
    let mm = ModelManager::new_with_paths(bundle_models, user_models.clone(), config_path)
        .expect("manager");
    let mut mm = mm;

    // mock-a is installed and active; mock-lang installed for language tests.
    write_model_bundle(&user_models, "mock-a", "Mock A", None);
    write_model_bundle(
        &user_models,
        "mock-lang",
        "Mock Lang",
        Some(mock_lang_options()),
    );
    mm.discover_installed();
    mm.set_active_model("mock-a").expect("activate mock-a");

    // Point global path resolution at this isolated home before any save().
    std::env::set_var("ECHOLET_USER_HOME", &home);

    let (action_tx, action_rx) = unbounded::<AppAction>();
    let (audio_tx, audio_rx) = unbounded::<AudioChunk>();
    let starter: AudioStarter = Box::new(|_tx| Ok(Box::new(()) as Box<dyn AudioSource>));

    let views = Arc::new(Mutex::new(Vec::new()));
    let platform = PlatformRuntime {
        injector: Box::new(FakeInjector),
        handle: Box::new(CapturingHandle {
            views: views.clone(),
        }),
        _resources: Box::new(()),
    };

    let mut config = EcholetConfig::default();
    config.selected_model = "mock-a".to_string();
    config.preload_model_on_startup = false;

    let app = App::new_with_manager_and_config(
        platform,
        Some(action_tx.clone()),
        action_rx,
        audio_rx,
        audio_tx,
        starter,
        None,
        Some(config),
        mm,
    )
    .expect("app construction");

    Harness {
        app,
        action_tx,
        handle: CapturingHandle { views },
        _home: home,
        tmp,
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        std::env::remove_var("ECHOLET_USER_HOME");
        let _ = fs::remove_dir_all(&self.tmp);
    }
}

#[test]
fn select_uninstalled_model_never_downloads() {
    let _g = env_guard();
    let mut h = build_harness("select-no-download");

    assert!(
        !h.app.select_model("mock-c"),
        "selecting an uninstalled model must not succeed"
    );
    assert!(
        h.app.model_manager.downloading.is_empty(),
        "selecting must never start a download"
    );
    assert_eq!(
        h.app.model_manager.active_model_id.as_deref(),
        Some("mock-a"),
        "active model must be unchanged"
    );

    // The explicit Download action is the only way to install.
    assert!(h.app.start_download("mock-c"));
    assert!(h.app.model_manager.downloading.contains("mock-c"));
}

#[test]
fn download_is_disabled_while_listening() {
    let _g = env_guard();
    let mut h = build_harness("download-listening");
    h.app.state.listening = true;

    assert!(
        !h.app.start_download("mock-c"),
        "download must be rejected while Listening"
    );
    assert!(h.app.model_manager.downloading.is_empty());
    h.app.state.listening = false;
}

#[test]
fn failed_download_is_retryable_and_preserves_active_model() {
    let _g = env_guard();
    let mut h = build_harness("retryable");

    // mock-c has no source URL, so the download fails immediately without
    // touching the network.
    assert!(h.app.start_download("mock-c"));

    // Pump the app until the background thread reports completion.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        h.app.tick();
        if h.app.download_status("mock-c") == Some(&DownloadStatus::Failed) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "download failure was never reported"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    assert_eq!(
        h.app.model_manager.active_model_id.as_deref(),
        Some("mock-a"),
        "failed download must not disturb the active model"
    );
    assert!(!h.app.model_manager.downloading.contains("mock-c"));

    let view = latest_view(&h.handle);
    let failed = view
        .models
        .iter()
        .find(|m| m.id == "mock-c")
        .expect("mock-c in view");
    assert_eq!(failed.download, DownloadStatus::Failed);
    assert!(!failed.is_installed);
    // A failed download stays actionable.
    assert!(h.app.start_download("mock-c"));
}

#[test]
fn successful_download_marks_installed_without_auto_select() {
    let _g = env_guard();
    let mut h = build_harness("no-auto-select");

    // Simulate a successful install of an on-disk bundle.
    write_model_bundle(&h._home.join("models"), "mock-c", "Mock C", None);
    h.action_tx
        .send(AppAction::ModelInstalled {
            model_id: "mock-c".to_string(),
            success: true,
            error: None,
        })
        .unwrap();
    h.app.tick();

    assert!(
        h.app.model_manager.is_installed("mock-c"),
        "successful install must register the model"
    );
    assert_eq!(
        h.app.model_manager.active_model_id.as_deref(),
        Some("mock-a"),
        "a successful download must NOT auto-select"
    );
    let view = latest_view(&h.handle);
    let installed = view.models.iter().find(|m| m.id == "mock-c").unwrap();
    assert!(installed.is_installed);
    assert!(!installed.is_selected);
}

#[test]
fn runtime_state_distinguishes_no_model_unloaded_ready_listening() {
    let _g = env_guard();
    let h = build_harness("runtime-state");

    // Installed but not resident -> UNLOADED.
    assert_eq!(
        h.app.runtime_state(),
        echolet::platform::RuntimeState::Unloaded
    );

    // No active model -> NO_MODEL.
    let mut no_model = {
        let tmp = unique_tmp("runtime-nomodel");
        let bundle = tmp.join("bundle/models");
        let user = tmp.join("home/models");
        fs::create_dir_all(&bundle).unwrap();
        fs::create_dir_all(&user).unwrap();
        fs::write(bundle.join("registry.json"), registry_json()).unwrap();
        let mm = ModelManager::new_with_paths(bundle, user, tmp.join("home/config.json"))
            .expect("manager");
        let (tx, rx) = unbounded::<AppAction>();
        let (audio_tx, audio_rx) = unbounded::<AudioChunk>();
        let starter: AudioStarter = Box::new(|_tx| Ok(Box::new(()) as Box<dyn AudioSource>));
        let platform = PlatformRuntime {
            injector: Box::new(FakeInjector),
            handle: Box::new(CapturingHandle::default()),
            _resources: Box::new(()),
        };
        App::new_with_manager_and_config(
            platform,
            Some(tx),
            rx,
            audio_rx,
            audio_tx,
            starter,
            None,
            None,
            mm,
        )
        .unwrap()
    };
    assert_eq!(
        no_model.runtime_state(),
        echolet::platform::RuntimeState::NoModel
    );

    // Listening overrides everything.
    no_model.state.listening = true;
    assert_eq!(
        no_model.runtime_state(),
        echolet::platform::RuntimeState::Listening
    );
}

#[test]
fn preload_toggle_persists_to_config() {
    let _g = env_guard();
    let mut h = build_harness("preload");

    h.action_tx
        .send(AppAction::SetPreloadModelOnStartup(true))
        .unwrap();
    h.app.tick();
    assert!(h.app.config.preload_model_on_startup);

    let reloaded = EcholetConfig::load_from(&h._home.join("config.json"));
    assert!(
        reloaded.preload_model_on_startup,
        "preload toggle must persist through EcholetConfig"
    );

    h.action_tx
        .send(AppAction::SetPreloadModelOnStartup(false))
        .unwrap();
    h.app.tick();
    assert!(!h.app.config.preload_model_on_startup);
}

#[test]
fn idle_unload_selector_persists_all_accepted_values() {
    let _g = env_guard();
    let mut h = build_harness("idle");
    let config_path = h._home.join("config.json");

    for value in [Some(0), Some(1), Some(5), Some(10), Some(30), None] {
        h.action_tx
            .send(AppAction::SetModelIdleUnloadMinutes(value))
            .unwrap();
        h.app.tick();
        assert_eq!(h.app.config.model_idle_unload_minutes, value);
        let reloaded = EcholetConfig::load_from(&config_path);
        assert_eq!(
            reloaded.model_idle_unload_minutes, value,
            "idle policy {:?} must persist",
            value
        );
    }
}

#[test]
fn per_model_language_selection_validates_persists_and_repairs() {
    let _g = env_guard();
    let mut h = build_harness("language");

    // Supported locale applies and projects.
    assert!(h.app.set_language("mock-lang", Some("ja-JP")));
    assert_eq!(h.app.config.language_preference("mock-lang"), Some("ja-JP"));
    let view = latest_view(&h.handle);
    let active = view.selected_model();
    // mock-a is the active model, so query the lang model directly.
    let lang_item = view.models.iter().find(|m| m.id == "mock-lang").unwrap();
    assert_eq!(lang_item.language.selected_locale.as_deref(), Some("ja-JP"));
    assert!(active.is_some());

    // Adaptation-ready locale is rejected and must not change the preference.
    assert!(!h.app.set_language("mock-lang", Some("el-GR")));
    assert_eq!(
        h.app.config.language_preference("mock-lang"),
        Some("ja-JP"),
        "rejected selection must not overwrite the stored preference"
    );

    // Auto is representable and persists.
    assert!(h.app.set_language("mock-lang", None));
    assert!(h.app.config.language_preference_is_auto("mock-lang"));

    // Selecting while Listening is refused.
    h.app.state.listening = true;
    assert!(!h.app.set_language("mock-lang", Some("ja-JP")));
    h.app.state.listening = false;

    // A model without language options cannot be forced.
    assert!(!h.app.set_language("mock-a", Some("ja")));
}

#[test]
fn runtime_code_mapping_and_stale_preference_fallback() {
    let _g = env_guard();
    let mut h = build_harness("language-codes");
    let manifest = h
        .app
        .model_manager
        .get_model("mock-lang")
        .unwrap()
        .manifest
        .clone();

    h.app
        .config
        .set_language_preference("mock-lang", Some("ja-JP"));
    assert_eq!(
        h.app.resolve_language_code("mock-lang", &manifest),
        Some("ja".to_string())
    );
    h.app
        .config
        .set_language_preference("mock-lang", Some("zh-CN"));
    assert_eq!(
        h.app.resolve_language_code("mock-lang", &manifest),
        Some("zh".to_string())
    );

    // A stale locale (catalog change) falls back to Auto and repairs config.
    h.app
        .config
        .set_language_preference("mock-lang", Some("xx-XX"));
    assert_eq!(h.app.resolve_language_code("mock-lang", &manifest), None);
    assert!(
        h.app.config.language_preference_is_auto("mock-lang"),
        "stale preference must be repaired to Auto"
    );

    // X-ASR-like model with no options yields no forced code.
    let xasr_like = h
        .app
        .model_manager
        .get_model("mock-a")
        .unwrap()
        .manifest
        .clone();
    assert_eq!(h.app.resolve_language_code("mock-a", &xasr_like), None);
}

#[test]
fn view_projects_verified_downloadable_installed_and_selected_states() {
    let _g = env_guard();
    let h = build_harness("view-states");
    let view = h.app.platform_view();

    let a = view.models.iter().find(|m| m.id == "mock-a").unwrap();
    assert!(a.is_selected && a.is_installed && a.is_verified);

    let c = view.models.iter().find(|m| m.id == "mock-c").unwrap();
    assert!(!c.is_installed && !c.is_selected);
    assert_eq!(c.download, DownloadStatus::NotDownloading);
    assert!(!c.is_verified);

    let lang = view.models.iter().find(|m| m.id == "mock-lang").unwrap();
    assert_eq!(lang.language.transcription_ready().count(), 1);
    assert_eq!(lang.language.broad_coverage().count(), 1);

    // The neutral registry projection must not hardcode any label.
    let registry = ModelRegistry::from_str(&registry_json()).unwrap();
    assert!(registry.get_model("mock-a").is_some());
}
