use crossbeam_channel::unbounded;
use echolet::actions::AppAction;
use echolet::app::App;
use echolet::audio::{AudioChunk, AudioSource, AudioStarter};
use echolet::platform::{PlatformHandle, PlatformRuntime, TextInjector};
use echolet::state::AppState;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

struct FakeInjector {
    diffs: Arc<Mutex<Vec<(usize, String)>>>,
}

impl TextInjector for FakeInjector {
    fn apply_diff(&self, backspaces: usize, new_suffix: &str) {
        self.diffs
            .lock()
            .unwrap()
            .push((backspaces, new_suffix.to_string()));
    }
}

struct FakePlatformHandle {
    listening_history: Arc<Mutex<Vec<bool>>>,
    history_state_history: Arc<Mutex<Vec<bool>>>,
    opened_folders: Arc<Mutex<Vec<PathBuf>>>,
    shutdown_called: Arc<AtomicBool>,
}

impl PlatformHandle for FakePlatformHandle {
    fn set_listening(&self, listening: bool) {
        self.listening_history.lock().unwrap().push(listening);
    }

    fn shutdown(&self) {
        self.shutdown_called.store(true, Ordering::SeqCst);
    }

    fn update_history_state(&self, enabled: bool) {
        self.history_state_history.lock().unwrap().push(enabled);
    }

    fn open_history_folder(&self, history_dir: &Path) {
        self.opened_folders
            .lock()
            .unwrap()
            .push(history_dir.to_path_buf());
    }
}

fn create_test_app() -> (
    App,
    crossbeam_channel::Sender<AppAction>,
    Arc<Mutex<Vec<bool>>>,
    Arc<AtomicBool>,
    Arc<Mutex<Vec<(usize, String)>>>,
    Arc<Mutex<Vec<bool>>>,
    Arc<Mutex<Vec<PathBuf>>>,
) {
    let (action_tx, action_rx) = unbounded::<AppAction>();
    let listening_history = Arc::new(Mutex::new(Vec::new()));
    let history_state_history = Arc::new(Mutex::new(Vec::new()));
    let opened_folders = Arc::new(Mutex::new(Vec::new()));
    let shutdown_called = Arc::new(AtomicBool::new(false));
    let diffs = Arc::new(Mutex::new(Vec::new()));

    let fake_handle = Box::new(FakePlatformHandle {
        listening_history: listening_history.clone(),
        history_state_history: history_state_history.clone(),
        opened_folders: opened_folders.clone(),
        shutdown_called: shutdown_called.clone(),
    });

    let fake_injector = Box::new(FakeInjector {
        diffs: diffs.clone(),
    });

    let platform = PlatformRuntime {
        injector: fake_injector,
        handle: fake_handle,
        _resources: Box::new(()),
    };

    let (_, audio_rx) = unbounded();
    let app = App::new_with_audio(platform, action_rx, audio_rx, None)
        .expect("Failed to create App with fake platform");

    (
        app,
        action_tx,
        listening_history,
        shutdown_called,
        diffs,
        history_state_history,
        opened_folders,
    )
}

fn create_test_app_with_starter(
    starter: AudioStarter,
) -> (
    App,
    crossbeam_channel::Sender<AppAction>,
    Arc<Mutex<Vec<bool>>>,
) {
    let (action_tx, action_rx) = unbounded::<AppAction>();
    let listening_history = Arc::new(Mutex::new(Vec::new()));
    let history_state_history = Arc::new(Mutex::new(Vec::new()));
    let opened_folders = Arc::new(Mutex::new(Vec::new()));
    let shutdown_called = Arc::new(AtomicBool::new(false));
    let diffs = Arc::new(Mutex::new(Vec::new()));

    let fake_handle = Box::new(FakePlatformHandle {
        listening_history: listening_history.clone(),
        history_state_history: history_state_history.clone(),
        opened_folders: opened_folders.clone(),
        shutdown_called: shutdown_called.clone(),
    });

    let fake_injector = Box::new(FakeInjector {
        diffs: diffs.clone(),
    });

    let platform = PlatformRuntime {
        injector: fake_injector,
        handle: fake_handle,
        _resources: Box::new(()),
    };

    let (audio_tx, audio_rx) = unbounded::<AudioChunk>();
    let app = App::new_with_starter(
        platform,
        Some(action_tx.clone()),
        action_rx,
        audio_rx,
        audio_tx,
        starter,
        None,
    )
    .expect("Failed to create App with custom starter");

    (app, action_tx, listening_history)
}

#[test]
fn test_app_state_defaults() {
    let state = AppState::new();
    assert!(!state.listening, "Initial listening state must be false");
    assert!(state.running, "Initial running state must be true");

    let default_state = AppState::default();
    assert!(!default_state.listening);
    assert!(default_state.running);
}

#[test]
fn test_state_start_stop_transitions() {
    let (mut app, action_tx, history, _, _, _, _) = create_test_app();

    // Startup should explicitly initialize UI state to false
    assert_eq!(*history.lock().unwrap(), vec![false]);
    assert!(!app.state.listening);

    // Send StartListening
    action_tx.send(AppAction::StartListening).unwrap();
    app.tick();
    assert!(app.state.listening);
    assert_eq!(*history.lock().unwrap(), vec![false, true]);

    // Send StopListening
    action_tx.send(AppAction::StopListening).unwrap();
    app.tick();
    assert!(!app.state.listening);
    assert_eq!(*history.lock().unwrap(), vec![false, true, false]);
}

#[test]
fn test_state_toggle_transition() {
    let (mut app, action_tx, history, _, _, _, _) = create_test_app();

    // Toggle 1: Start
    action_tx.send(AppAction::ToggleListening).unwrap();
    app.tick();
    assert!(app.state.listening);
    assert_eq!(*history.lock().unwrap(), vec![false, true]);

    // Toggle 2: Stop
    action_tx.send(AppAction::ToggleListening).unwrap();
    app.tick();
    assert!(!app.state.listening);
    assert_eq!(*history.lock().unwrap(), vec![false, true, false]);
}

#[test]
fn test_quit_action_terminates_running() {
    let (mut app, action_tx, _, shutdown, _, _, _) = create_test_app();

    assert!(app.state.running);
    assert!(!shutdown.load(Ordering::SeqCst));

    action_tx.send(AppAction::Quit).unwrap();
    app.tick();

    assert!(!app.state.running, "Quit action must set running = false");
    assert!(
        shutdown.load(Ordering::SeqCst),
        "Quit action must notify platform shutdown"
    );
}

#[test]
fn test_endpoint_segment_finalization_invariant() {
    let (mut app, _, _, _, _, _, _) = create_test_app();

    // Manually transition to listening
    app.start_listening();
    assert!(app.state.listening);

    // When an endpoint occurs, finalize_current_segment() is invoked
    app.finalize_current_segment();

    // Invariant check: listening MUST remain true across sentence boundaries
    assert!(
        app.state.listening,
        "Listening state must remain TRUE after endpoint finalization"
    );
}

#[test]
fn test_mic_dynamic_lifecycle_transitions() {
    let (mut app, action_tx, _history, _, _, _, _) = create_test_app();

    // 1. Initial state: Standby, microphone is NOT open
    assert!(!app.state.listening);
    assert!(!app.is_audio_active());

    // 2. Start listening: microphone opens on demand
    action_tx.send(AppAction::StartListening).unwrap();
    app.tick();
    assert!(app.state.listening);
    assert!(app.is_audio_active());

    // 3. Stop listening: microphone is dropped and released
    action_tx.send(AppAction::StopListening).unwrap();
    app.tick();
    assert!(!app.state.listening);
    assert!(!app.is_audio_active());

    // 4. Toggle back to listening: microphone re-opens cleanly
    action_tx.send(AppAction::ToggleListening).unwrap();
    app.tick();
    assert!(app.state.listening);
    assert!(app.is_audio_active());

    // 5. Toggle to stop: microphone dropped again
    action_tx.send(AppAction::ToggleListening).unwrap();
    app.tick();
    assert!(!app.state.listening);
    assert!(!app.is_audio_active());
}

#[test]
fn test_mic_start_failure_graceful_fallback() {
    // Inject an AudioStarter that simulates hardware error (e.g. mic unplugged / device busy)
    let failing_starter: AudioStarter =
        Box::new(|_tx| Err("Device busy or disconnected".to_string()));

    let (mut app, action_tx, history) = create_test_app_with_starter(failing_starter);

    assert!(!app.state.listening);
    assert!(!app.is_audio_active());

    // Attempt to start listening
    action_tx.send(AppAction::StartListening).unwrap();
    app.tick();

    // Invariant: Failure to open mic must keep app safely in Standby without crashing
    assert!(
        !app.state.listening,
        "App must remain in Standby when mic open fails"
    );
    assert!(
        !app.is_audio_active(),
        "Audio capture must not be active when mic open fails"
    );
    // UI history should only have the initial false, no state change occurred
    assert_eq!(*history.lock().unwrap(), vec![false]);
}

#[test]
fn test_app_history_toggle_and_open_folder_actions() {
    let (mut app, action_tx, _, _, _, hist_history, opened_folders) = create_test_app();

    let initial_state = app.history_manager.enabled;
    assert_eq!(*hist_history.lock().unwrap(), vec![initial_state]);

    // Send ToggleHistory -> Invert state
    action_tx.send(AppAction::ToggleHistory).unwrap();
    app.tick();
    assert_eq!(app.history_manager.enabled, !initial_state);
    assert_eq!(
        *hist_history.lock().unwrap(),
        vec![initial_state, !initial_state]
    );

    // Send OpenHistoryFolder
    action_tx.send(AppAction::OpenHistoryFolder).unwrap();
    app.tick();
    assert_eq!(
        *opened_folders.lock().unwrap(),
        vec![app.history_manager.history_dir.clone()]
    );

    // Send ToggleHistory again -> Revert to initial state
    action_tx.send(AppAction::ToggleHistory).unwrap();
    app.tick();
    assert_eq!(app.history_manager.enabled, initial_state);
    assert_eq!(
        *hist_history.lock().unwrap(),
        vec![initial_state, !initial_state, initial_state]
    );
}

#[test]
fn test_optional_recognizer_lifecycle_unload_and_reload() {
    let (mut app, _, _, _, _, _, _) = create_test_app();

    // 1. Initial eager-loaded state
    assert!(
        app.is_model_loaded(),
        "App must eagerly load active model on startup"
    );

    // 2. Unload model
    let unloaded = app.unload_model();
    assert!(unloaded, "unload_model must succeed when in Standby");
    assert!(!app.is_model_loaded(), "Model must be unloaded");

    // 3. Idempotency of unload_model
    let unloaded_again = app.unload_model();
    assert!(
        unloaded_again,
        "Repeated unload_model must be idempotent and succeed"
    );
    assert!(!app.is_model_loaded());

    // 4. Safe operations while unloaded
    app.finalize_current_segment();
    app.tick();
    assert!(!app.is_model_loaded());

    // 5. Reload via ensure_model_loaded
    let reload_res = app.ensure_model_loaded();
    assert!(
        reload_res.is_ok(),
        "ensure_model_loaded must succeed to reload active model"
    );
    assert!(
        app.is_model_loaded(),
        "Model must be loaded after ensure_model_loaded"
    );

    // 6. Idempotency of ensure_model_loaded (already valid)
    let reload_again = app.ensure_model_loaded();
    assert!(
        reload_again.is_ok(),
        "ensure_model_loaded must be idempotent when already loaded"
    );
    assert!(app.is_model_loaded());

    // 7. Repeated unload -> reload cycles
    for cycle in 1..=3 {
        assert!(app.unload_model(), "Cycle {}: unload must succeed", cycle);
        assert!(
            !app.is_model_loaded(),
            "Cycle {}: model must be unloaded",
            cycle
        );
        assert!(
            app.ensure_model_loaded().is_ok(),
            "Cycle {}: reload must succeed",
            cycle
        );
        assert!(
            app.is_model_loaded(),
            "Cycle {}: model must be loaded",
            cycle
        );
    }
}

#[test]
fn test_unload_rejected_while_listening() {
    let (mut app, _, _, _, _, _, _) = create_test_app();

    app.start_listening();
    assert!(app.state.listening);
    assert!(app.is_model_loaded());

    // Invariant: unload_model must be rejected while Listening
    let unloaded = app.unload_model();
    assert!(!unloaded, "unload_model must return false while listening");
    assert!(app.is_model_loaded(), "Model must remain loaded");
    assert!(app.state.listening, "Listening state must remain active");

    app.stop_listening();
    assert!(!app.state.listening);

    // After stopping, unload succeeds
    assert!(app.unload_model());
    assert!(!app.is_model_loaded());
}

#[test]
fn test_start_listening_auto_reloads_if_unloaded() {
    let (mut app, _, _, _, _, _, _) = create_test_app();

    // Start from unloaded state
    assert!(app.unload_model());
    assert!(!app.is_model_loaded());

    // start_listening must ensure model is loaded before entering Listening
    app.start_listening();
    assert!(
        app.is_model_loaded(),
        "start_listening must load runtime before listening"
    );
    assert!(app.state.listening);
    assert!(app.is_audio_active());

    app.stop_listening();
    assert!(!app.state.listening);
    assert!(!app.is_audio_active());
}

#[test]
fn test_select_model_while_unloaded_and_transactional_safety() {
    let (mut app, _, _, _, _, _, _) = create_test_app();

    let active_id = app.model_manager.active_model_id.clone();
    assert!(app.unload_model());
    assert!(!app.is_model_loaded());

    // Selecting currently active model while unloaded should reload it
    let switched = app.select_model(&active_id);
    assert!(
        switched,
        "select_model for active model while unloaded must succeed"
    );
    assert!(
        app.is_model_loaded(),
        "Model must be loaded after selecting active model"
    );

    // Unload again
    assert!(app.unload_model());
    assert!(!app.is_model_loaded());

    // Selecting nonexistent model while unloaded should fail safely and retain unloaded state
    let failed_switch = app.select_model("nonexistent-model-xyz");
    assert!(
        !failed_switch,
        "select_model for nonexistent model must fail"
    );
    assert!(
        !app.is_model_loaded(),
        "Model must remain unloaded on candidate failure"
    );
    assert_eq!(
        app.model_manager.active_model_id, active_id,
        "Active model ID must remain unchanged"
    );
}

#[test]
fn test_tick_safe_when_unloaded_even_if_corrupted_listening_flag() {
    let (action_tx, action_rx) = unbounded::<AppAction>();
    let (audio_tx, audio_rx) = unbounded::<AudioChunk>();
    let starter: AudioStarter = Box::new(|_tx| Ok(Box::new(()) as Box<dyn AudioSource>));
    let platform = PlatformRuntime {
        injector: Box::new(FakeInjector {
            diffs: Arc::new(Mutex::new(Vec::new())),
        }),
        handle: Box::new(FakePlatformHandle {
            listening_history: Arc::new(Mutex::new(Vec::new())),
            history_state_history: Arc::new(Mutex::new(Vec::new())),
            opened_folders: Arc::new(Mutex::new(Vec::new())),
            shutdown_called: Arc::new(AtomicBool::new(false)),
        }),
        _resources: Box::new(()),
    };

    let mut app = App::new_with_starter(
        platform,
        Some(action_tx),
        action_rx,
        audio_rx,
        audio_tx.clone(),
        starter,
        None,
    )
    .expect("Failed to create App");

    assert!(app.unload_model());
    assert!(!app.is_model_loaded());

    // Force an invariant violation where listening is true but runtime is unloaded
    app.state.listening = true;

    // Send audio chunk
    let chunk = AudioChunk {
        samples: vec![0.0; 160],
        sample_rate: 16000,
    };
    audio_tx.send(chunk).unwrap();

    // tick() must not panic or dereference absent stream
    app.tick();
    assert!(!app.is_model_loaded());
}
