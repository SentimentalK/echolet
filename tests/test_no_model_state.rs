use crossbeam_channel::unbounded;
use echolet::actions::AppAction;
use echolet::app::App;
use echolet::audio::{AudioChunk, AudioSource, AudioStarter};
use echolet::config::EcholetConfig;
use echolet::models::manager::ModelManager;
use echolet::models::manifest::ModelManifest;
use echolet::platform::{PlatformHandle, PlatformRuntime, PlatformView, TextInjector};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct FakeInjector;
impl TextInjector for FakeInjector {
    fn apply_diff(&self, _backspaces: usize, _new_suffix: &str) {}
}

struct TestPlatformHandle {
    listening_history: Arc<Mutex<Vec<bool>>>,
    last_active_model: Arc<Mutex<Option<String>>>,
    last_installed_models: Arc<Mutex<Vec<String>>>,
}

impl PlatformHandle for TestPlatformHandle {
    fn set_listening(&self, listening: bool) {
        self.listening_history.lock().unwrap().push(listening);
    }
    fn shutdown(&self) {}
    fn update_models(&self, view: &PlatformView) {
        *self.last_active_model.lock().unwrap() = view.selected_model().map(|m| m.id.clone());
        *self.last_installed_models.lock().unwrap() = view
            .all_models()
            .filter(|m| m.installed)
            .map(|m| m.id.clone())
            .collect();
    }
}

fn create_temp_test_dir(prefix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("echolet-test-{}-{}", prefix, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn test_model_manager_initializes_with_empty_directories() {
    let tmp = create_temp_test_dir("mm-empty");
    let bundled = tmp.join("bundled");
    let user = tmp.join("user");
    let cfg = tmp.join("config.json");
    fs::create_dir_all(&bundled).unwrap();
    fs::create_dir_all(&user).unwrap();

    let manager = ModelManager::new_with_paths(bundled, user, cfg)
        .expect("ModelManager::new_with_paths must succeed with empty directories");

    assert!(
        manager.installed.is_empty(),
        "Installed models must be empty"
    );
    assert_eq!(
        manager.active_model_id, None,
        "Active model must be None, not empty string or fake ID"
    );
    assert_eq!(manager.active_model_id(), None);
    assert!(
        manager.get_active_model().is_err(),
        "get_active_model must return error when zero models installed"
    );
    let err = manager.get_active_model().unwrap_err();
    assert!(err.contains("No active model installed"), "Error: {}", err);

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_stale_config_selected_model_does_not_become_active_when_not_installed() {
    let tmp = create_temp_test_dir("stale-config");
    let bundled = tmp.join("bundled");
    let user = tmp.join("user");
    let cfg_path = tmp.join("config.json");
    fs::create_dir_all(&bundled).unwrap();
    fs::create_dir_all(&user).unwrap();

    // Write a config with a non-installed model
    let mut config = EcholetConfig::default();
    config.selected_model = "stale-deleted-model-id".into();
    config.save_to(&cfg_path).unwrap();

    let manager = ModelManager::new_with_paths(bundled, user, cfg_path.clone())
        .expect("ModelManager must succeed even with stale config");

    assert_eq!(
        manager.active_model_id, None,
        "Stale selected model must NOT become active"
    );
    assert!(manager.get_active_model().is_err());

    // Saving config must preserve preferred selected model and not overwrite it with empty string
    manager.save_config().unwrap();
    let reloaded = EcholetConfig::load_from(&cfg_path);
    assert_eq!(reloaded.selected_model, "stale-deleted-model-id");

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_app_startup_preload_false_with_zero_installed_succeeds_unloaded() {
    let tmp = create_temp_test_dir("preload-false");
    let bundled = tmp.join("bundled");
    let user = tmp.join("user");
    let cfg_path = tmp.join("config.json");
    fs::create_dir_all(&bundled).unwrap();
    fs::create_dir_all(&user).unwrap();

    let mm = ModelManager::new_with_paths(bundled, user, cfg_path).unwrap();

    let (action_tx, action_rx) = unbounded::<AppAction>();
    let (audio_tx, audio_rx) = unbounded::<AudioChunk>();
    let starter: AudioStarter = Box::new(|_tx| Ok(Box::new(()) as Box<dyn AudioSource>));

    let listening_history = Arc::new(Mutex::new(Vec::new()));
    let last_active = Arc::new(Mutex::new(Some("sentinel".into())));
    let last_installed = Arc::new(Mutex::new(Vec::new()));

    let platform = PlatformRuntime {
        injector: Box::new(FakeInjector),
        handle: Box::new(TestPlatformHandle {
            listening_history: listening_history.clone(),
            last_active_model: last_active.clone(),
            last_installed_models: last_installed.clone(),
        }),
        _resources: Box::new(()),
    };

    let mut cfg = EcholetConfig::default();
    cfg.preload_model_on_startup = false;

    let app = App::new_with_manager_and_config(
        platform,
        Some(action_tx),
        action_rx,
        audio_rx,
        audio_tx,
        starter,
        None,
        Some(cfg),
        mm,
    )
    .expect("App creation must succeed with zero installed models and preload=false");

    assert!(
        !app.has_active_model(),
        "App must report has_active_model=false (NO_MODEL)"
    );
    assert!(!app.is_model_loaded(), "App must not have loaded model");
    assert_eq!(
        app.idle_unload_deadline(),
        None,
        "No idle deadline should exist"
    );
    assert_eq!(
        *last_active.lock().unwrap(),
        None,
        "Platform projection must receive None for active model"
    );
    assert!(
        last_installed.lock().unwrap().is_empty(),
        "Platform projection must receive empty installed list"
    );

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_app_startup_preload_true_with_zero_installed_stays_alive_and_unloaded() {
    let tmp = create_temp_test_dir("preload-true");
    let bundled = tmp.join("bundled");
    let user = tmp.join("user");
    let cfg_path = tmp.join("config.json");
    fs::create_dir_all(&bundled).unwrap();
    fs::create_dir_all(&user).unwrap();

    let mm = ModelManager::new_with_paths(bundled, user, cfg_path).unwrap();

    let (action_tx, action_rx) = unbounded::<AppAction>();
    let (audio_tx, audio_rx) = unbounded::<AudioChunk>();
    let starter: AudioStarter = Box::new(|_tx| Ok(Box::new(()) as Box<dyn AudioSource>));

    let platform = PlatformRuntime {
        injector: Box::new(FakeInjector),
        handle: Box::new(TestPlatformHandle {
            listening_history: Arc::new(Mutex::new(Vec::new())),
            last_active_model: Arc::new(Mutex::new(None)),
            last_installed_models: Arc::new(Mutex::new(Vec::new())),
        }),
        _resources: Box::new(()),
    };

    let mut cfg = EcholetConfig::default();
    cfg.preload_model_on_startup = true;
    cfg.model_idle_unload_minutes = Some(10);

    let app = App::new_with_manager_and_config(
        platform,
        Some(action_tx),
        action_rx,
        audio_rx,
        audio_tx,
        starter,
        None,
        Some(cfg),
        mm,
    )
    .expect("App startup with preload=true and zero models must stay alive and not crash");

    assert!(!app.has_active_model());
    assert!(!app.is_model_loaded());
    assert_eq!(app.idle_unload_deadline(), None);

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_start_listening_with_zero_installed_leaves_standby_and_mic_unopened() {
    let tmp = create_temp_test_dir("listen-zero");
    let mm = ModelManager::new_with_paths(
        tmp.join("bundled"),
        tmp.join("user"),
        tmp.join("config.json"),
    )
    .unwrap();

    let (action_tx, action_rx) = unbounded::<AppAction>();
    let (audio_tx, audio_rx) = unbounded::<AudioChunk>();

    let mic_opened = Arc::new(AtomicBool::new(false));
    let mic_opened_clone = mic_opened.clone();
    let starter: AudioStarter = Box::new(move |_tx| {
        mic_opened_clone.store(true, Ordering::SeqCst);
        Ok(Box::new(()) as Box<dyn AudioSource>)
    });

    let listening_history = Arc::new(Mutex::new(Vec::new()));
    let platform = PlatformRuntime {
        injector: Box::new(FakeInjector),
        handle: Box::new(TestPlatformHandle {
            listening_history: listening_history.clone(),
            last_active_model: Arc::new(Mutex::new(None)),
            last_installed_models: Arc::new(Mutex::new(Vec::new())),
        }),
        _resources: Box::new(()),
    };

    let mut app = App::new_with_manager_and_config(
        platform,
        Some(action_tx.clone()),
        action_rx,
        audio_rx,
        audio_tx,
        starter,
        None,
        None,
        mm,
    )
    .unwrap();

    // Direct start_listening call
    let metrics = app.start_listening();
    assert!(
        metrics.is_none(),
        "start_listening must return None when no model installed"
    );
    assert!(
        !app.state.listening,
        "State must remain Standby (listening = false)"
    );
    assert!(
        !mic_opened.load(Ordering::SeqCst),
        "Microphone capture must NEVER open when no model installed"
    );
    assert_eq!(
        app.idle_unload_deadline(),
        None,
        "No idle deadline should be created"
    );

    // Action-based StartListening
    action_tx.send(AppAction::StartListening).unwrap();
    app.tick();
    assert!(!app.state.listening);
    assert!(!mic_opened.load(Ordering::SeqCst));

    // Action-based ToggleListening (simulating F10 hotkey)
    action_tx.send(AppAction::ToggleListening).unwrap();
    app.tick();
    assert!(!app.state.listening);
    assert!(!mic_opened.load(Ordering::SeqCst));

    // Platform handle should never have received set_listening(true)
    assert!(
        !listening_history.lock().unwrap().contains(&true),
        "Platform handle must never enter listening state"
    );

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_residency_helpers_tolerate_no_model_and_unloaded_runtime() {
    let tmp = create_temp_test_dir("residency-no-model");
    let mm = ModelManager::new_with_paths(
        tmp.join("bundled"),
        tmp.join("user"),
        tmp.join("config.json"),
    )
    .unwrap();

    let (_, action_rx) = unbounded::<AppAction>();
    let (audio_tx, audio_rx) = unbounded::<AudioChunk>();
    let starter: AudioStarter = Box::new(|_tx| Ok(Box::new(()) as Box<dyn AudioSource>));

    let platform = PlatformRuntime {
        injector: Box::new(FakeInjector),
        handle: Box::new(TestPlatformHandle {
            listening_history: Arc::new(Mutex::new(Vec::new())),
            last_active_model: Arc::new(Mutex::new(None)),
            last_installed_models: Arc::new(Mutex::new(Vec::new())),
        }),
        _resources: Box::new(()),
    };

    let mut app = App::new_with_manager_and_config(
        platform, None, action_rx, audio_rx, audio_tx, starter, None, None, mm,
    )
    .unwrap();

    // schedule_idle_unload must not panic and must ensure deadline is None
    app.schedule_idle_unload();
    assert_eq!(app.idle_unload_deadline(), None);
    assert_eq!(app.idle_unload_model_id(), None);

    // check_idle_unload must tolerate no model safely
    app.check_idle_unload();
    assert_eq!(app.idle_unload_deadline(), None);

    // expire_idle_unload_deadline must tolerate no active model safely
    app.expire_idle_unload_deadline();
    assert!(app.idle_unload_deadline().is_some());
    assert_eq!(app.idle_unload_model_id(), None);

    // cancel_idle_unload resets cleanly
    app.cancel_idle_unload();
    assert_eq!(app.idle_unload_deadline(), None);
    assert_eq!(app.idle_unload_model_id(), None);

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_distinction_between_no_model_and_unloaded_semantics() {
    let _guard = ENV_LOCK.lock().unwrap();
    // 1. Zero installed state: NO_MODEL
    let tmp = create_temp_test_dir("distinct-states");
    let mm_empty = ModelManager::new_with_paths(
        tmp.join("bundled"),
        tmp.join("user"),
        tmp.join("config.json"),
    )
    .unwrap();

    let (_, action_rx) = unbounded::<AppAction>();
    let (audio_tx, audio_rx) = unbounded::<AudioChunk>();
    let starter: AudioStarter = Box::new(|_tx| Ok(Box::new(()) as Box<dyn AudioSource>));

    let platform = PlatformRuntime {
        injector: Box::new(FakeInjector),
        handle: Box::new(TestPlatformHandle {
            listening_history: Arc::new(Mutex::new(Vec::new())),
            last_active_model: Arc::new(Mutex::new(None)),
            last_installed_models: Arc::new(Mutex::new(Vec::new())),
        }),
        _resources: Box::new(()),
    };

    let app_no_model = App::new_with_manager_and_config(
        platform, None, action_rx, audio_rx, audio_tx, starter, None, None, mm_empty,
    )
    .unwrap();

    assert!(!app_no_model.has_active_model(), "State must be NO_MODEL");
    assert!(!app_no_model.is_model_loaded(), "Runtime is not loaded");

    // 2. Installed baseline state with preload=false: UNLOADED (not NO_MODEL)
    let (_, action_rx2) = unbounded::<AppAction>();
    let (audio_tx2, audio_rx2) = unbounded::<AudioChunk>();
    let starter2: AudioStarter = Box::new(|_tx| Ok(Box::new(()) as Box<dyn AudioSource>));
    let platform2 = PlatformRuntime {
        injector: Box::new(FakeInjector),
        handle: Box::new(TestPlatformHandle {
            listening_history: Arc::new(Mutex::new(Vec::new())),
            last_active_model: Arc::new(Mutex::new(None)),
            last_installed_models: Arc::new(Mutex::new(Vec::new())),
        }),
        _resources: Box::new(()),
    };
    let mut cfg2 = EcholetConfig::default();
    cfg2.preload_model_on_startup = false;
    let app_unloaded = App::new_with_starter_and_config(
        platform2,
        None,
        action_rx2,
        audio_rx2,
        audio_tx2,
        starter2,
        None,
        Some(cfg2),
    )
    .unwrap();

    assert!(
        app_unloaded.has_active_model(),
        "Active model exists (UNLOADED state, not NO_MODEL)"
    );
    assert!(
        !app_unloaded.is_model_loaded(),
        "Runtime is not resident in memory"
    );

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_discovering_or_installing_model_after_starting_in_no_model_state() {
    let tmp = create_temp_test_dir("discover-after-start");
    let bundled = tmp.join("bundled");
    let user = tmp.join("user");
    let cfg = tmp.join("config.json");
    fs::create_dir_all(&bundled).unwrap();
    fs::create_dir_all(&user).unwrap();

    let mut mm = ModelManager::new_with_paths(bundled.clone(), user.clone(), cfg).unwrap();
    assert_eq!(mm.active_model_id, None);

    // Register a mock model into manager
    let manifest = ModelManifest {
        id: "mock-new-model".into(),
        display_name: "Mock New Model".into(),
        version: "2026-01-01".into(),
        languages: vec!["en".into()],
        family: "test".into(),
        encoder: "enc.onnx".into(),
        decoder: "dec.onnx".into(),
        joiner: "join.onnx".into(),
        tokens: "tokens.txt".into(),
        ..Default::default()
    };
    let model_dir = user.join("mock-new-model");
    fs::create_dir_all(&model_dir).unwrap();

    mm.register_installed(manifest, model_dir);
    assert!(mm.is_installed("mock-new-model"));

    // Can now be explicitly selected
    let res = mm.set_active_model("mock-new-model");
    assert!(res.is_ok());
    assert_eq!(mm.active_model_id, Some("mock-new-model".into()));
    assert_eq!(mm.active_model_id(), Some("mock-new-model"));

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_platform_projection_marks_no_entry_selected() {
    let last_active = Arc::new(Mutex::new(Some("sentinel-model".into())));
    let last_installed = Arc::new(Mutex::new(Vec::new()));

    let handle = TestPlatformHandle {
        listening_history: Arc::new(Mutex::new(Vec::new())),
        last_active_model: last_active.clone(),
        last_installed_models: last_installed.clone(),
    };

    // Platform projection called with a neutral view (no model selected).
    handle.update_models(&PlatformView::default());

    assert_eq!(
        *last_active.lock().unwrap(),
        None,
        "Active model in platform projection must be None"
    );
    assert!(
        last_installed.lock().unwrap().is_empty(),
        "Installed models must be empty"
    );
}

#[test]
fn test_benchmark_reports_clear_error_with_no_installed_model() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = create_temp_test_dir("bench-no-model");
    let res_dir = tmp.join("res");
    let user_dir = tmp.join("user");
    fs::create_dir_all(res_dir.join("models")).unwrap();
    fs::create_dir_all(user_dir.join("models")).unwrap();

    let old_res = std::env::var("ECHOLET_RESOURCE_ROOT").ok();
    let old_user = std::env::var("ECHOLET_USER_HOME").ok();

    std::env::set_var("ECHOLET_RESOURCE_ROOT", &res_dir);
    std::env::set_var("ECHOLET_USER_HOME", &user_dir);

    let res = echolet::diagnostics::benchmark::run_in_process_benchmark(1);

    if let Some(r) = old_res {
        std::env::set_var("ECHOLET_RESOURCE_ROOT", r);
    } else {
        std::env::remove_var("ECHOLET_RESOURCE_ROOT");
    }
    if let Some(u) = old_user {
        std::env::set_var("ECHOLET_USER_HOME", u);
    } else {
        std::env::remove_var("ECHOLET_USER_HOME");
    }

    assert!(
        res.is_err(),
        "Benchmark must fail gracefully when no model is installed"
    );
    let err_str = res.unwrap_err().to_string();
    assert!(
        err_str.contains("No model installed to benchmark"),
        "Error message must be actionable: {}",
        err_str
    );

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_env_var_isolated_path_hooks_model_manager_and_app() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = create_temp_test_dir("env-var-hooks");
    let res_dir = tmp.join("res");
    let user_dir = tmp.join("user");
    fs::create_dir_all(res_dir.join("models")).unwrap();
    fs::create_dir_all(user_dir.join("models")).unwrap();

    let old_res = std::env::var("ECHOLET_RESOURCE_ROOT").ok();
    let old_user = std::env::var("ECHOLET_USER_HOME").ok();

    std::env::set_var("ECHOLET_RESOURCE_ROOT", &res_dir);
    std::env::set_var("ECHOLET_USER_HOME", &user_dir);

    // 1. ModelManager::new() with env vars
    let mm =
        ModelManager::new().expect("ModelManager::new() must succeed with isolated empty dirs");
    assert!(mm.installed.is_empty());
    assert_eq!(mm.active_model_id, None);

    // 2. App::new_internal() with env vars
    let (action_tx, action_rx) = unbounded::<AppAction>();
    let (audio_tx, audio_rx) = unbounded::<AudioChunk>();
    let starter: AudioStarter = Box::new(|_tx| Ok(Box::new(()) as Box<dyn AudioSource>));

    let platform = PlatformRuntime {
        injector: Box::new(FakeInjector),
        handle: Box::new(TestPlatformHandle {
            listening_history: Arc::new(Mutex::new(Vec::new())),
            last_active_model: Arc::new(Mutex::new(None)),
            last_installed_models: Arc::new(Mutex::new(Vec::new())),
        }),
        _resources: Box::new(()),
    };

    let app = App::new_internal(
        platform,
        Some(action_tx),
        action_rx,
        audio_rx,
        audio_tx,
        starter,
        None,
        None,
    )
    .expect("App::new_internal must succeed with isolated empty dirs");

    assert!(!app.has_active_model());
    assert!(!app.is_model_loaded());

    if let Some(r) = old_res {
        std::env::set_var("ECHOLET_RESOURCE_ROOT", r);
    } else {
        std::env::remove_var("ECHOLET_RESOURCE_ROOT");
    }
    if let Some(u) = old_user {
        std::env::set_var("ECHOLET_USER_HOME", u);
    } else {
        std::env::remove_var("ECHOLET_USER_HOME");
    }

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_staged_bundle_layout_first_run_smoke() {
    let _guard = ENV_LOCK.lock().unwrap();

    let tmp = create_temp_test_dir("bundle-smoke");
    let bundle_root = tmp.join("bundle");
    let bundle_models = bundle_root.join("models");
    let user_home = tmp.join("user_home");
    let user_models = user_home.join("models");

    fs::create_dir_all(&bundle_models).unwrap();
    fs::create_dir_all(&user_models).unwrap();

    // Copy repository registry.json into the bundle layout (matching production packaging)
    let repo_registry = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models/registry.json");
    fs::copy(&repo_registry, bundle_models.join("registry.json")).expect("copy registry.json");

    // Assert bundle has NO onnx files, NO root model.json, NO bilingual-zh-en subdir
    assert!(!bundle_root.join("model.json").exists());
    assert!(!bundle_models.join("bilingual-zh-en").exists());

    let old_res = std::env::var("ECHOLET_RESOURCE_ROOT").ok();
    let old_user = std::env::var("ECHOLET_USER_HOME").ok();

    std::env::set_var("ECHOLET_RESOURCE_ROOT", &bundle_root);
    std::env::set_var("ECHOLET_USER_HOME", &user_home);

    let mm = ModelManager::new().expect("ModelManager::new() must succeed with bundle layout");

    // 1. Must load registry from the bundle file, not fallback
    //    (X-ASR default + 3 English models)
    assert_eq!(mm.registry.models.len(), 4);
    assert_eq!(
        mm.registry.default_model_id,
        "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1"
    );
    let default_reg = mm
        .registry
        .get_model(&mm.registry.default_model_id)
        .unwrap();
    assert!(
        !default_reg.source.bundled,
        "Production bundle registry must mark bundled: false"
    );

    // 2. Must be in clean NO_MODEL first-run state
    assert!(
        mm.installed.is_empty(),
        "Bundle with zero model files must yield zero installed models"
    );
    assert_eq!(
        mm.active_model_id, None,
        "Active model must be None on first run"
    );

    // 3. User directory safety: user_models must not have been deleted or modified
    assert!(
        user_models.exists(),
        "User models directory must remain intact"
    );
    assert_eq!(
        fs::read_dir(&user_models).unwrap().count(),
        0,
        "User models directory must remain untouched"
    );

    if let Some(r) = old_res {
        std::env::set_var("ECHOLET_RESOURCE_ROOT", r);
    } else {
        std::env::remove_var("ECHOLET_RESOURCE_ROOT");
    }
    if let Some(u) = old_user {
        std::env::set_var("ECHOLET_USER_HOME", u);
    } else {
        std::env::remove_var("ECHOLET_USER_HOME");
    }

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_user_models_safety_and_non_interference() {
    let _guard = ENV_LOCK.lock().unwrap();

    let tmp = create_temp_test_dir("user-safety");
    let bundle_root = tmp.join("bundle");
    let bundle_models = bundle_root.join("models");
    let user_home = tmp.join("user_home");
    let user_models = user_home.join("models");

    fs::create_dir_all(&bundle_models).unwrap();
    fs::create_dir_all(&user_models).unwrap();

    let repo_registry = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models/registry.json");
    fs::copy(&repo_registry, bundle_models.join("registry.json")).expect("copy registry.json");

    // Create a mock user-downloaded model under user_home/models/my-custom-model
    let custom_model_dir = user_models.join("my-custom-model");
    fs::create_dir_all(&custom_model_dir).unwrap();
    fs::write(custom_model_dir.join("marker.txt"), b"user-data-content").unwrap();

    let old_res = std::env::var("ECHOLET_RESOURCE_ROOT").ok();
    let old_user = std::env::var("ECHOLET_USER_HOME").ok();

    std::env::set_var("ECHOLET_RESOURCE_ROOT", &bundle_root);
    std::env::set_var("ECHOLET_USER_HOME", &user_home);

    let _mm = ModelManager::new().expect("ModelManager::new() must succeed");

    // Assert custom user model and marker file are completely untouched
    assert!(
        custom_model_dir.exists(),
        "User custom model dir must exist"
    );
    assert!(
        custom_model_dir.join("marker.txt").exists(),
        "User marker file must be preserved"
    );
    let content = fs::read(custom_model_dir.join("marker.txt")).unwrap();
    assert_eq!(content, b"user-data-content");

    if let Some(r) = old_res {
        std::env::set_var("ECHOLET_RESOURCE_ROOT", r);
    } else {
        std::env::remove_var("ECHOLET_RESOURCE_ROOT");
    }
    if let Some(u) = old_user {
        std::env::set_var("ECHOLET_USER_HOME", u);
    } else {
        std::env::remove_var("ECHOLET_USER_HOME");
    }

    let _ = fs::remove_dir_all(&tmp);
}
