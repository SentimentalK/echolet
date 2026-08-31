use crossbeam_channel::unbounded;
use echolet::actions::AppAction;
use echolet::app::App;
use echolet::models::manifest::ModelManifest;
use echolet::models::manager::ModelManager;
use echolet::models::registry::ModelRegistry;
use echolet::platform::{PlatformHandle, PlatformRuntime, TextInjector};
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

struct FakeInjector;
impl TextInjector for FakeInjector {
    fn apply_diff(&self, _backspaces: usize, _new_suffix: &str) {}
}

struct FakePlatformHandle {
    listening_history: Arc<Mutex<Vec<bool>>>,
    models_history: Arc<Mutex<Vec<String>>>,
    shutdown_called: Arc<AtomicBool>,
}

impl PlatformHandle for FakePlatformHandle {
    fn set_listening(&self, listening: bool) {
        self.listening_history.lock().unwrap().push(listening);
    }

    fn shutdown(&self) {
        self.shutdown_called.store(true, Ordering::SeqCst);
    }

    fn update_models(
        &self,
        active_id: &str,
        _installed_ids: &[String],
        _downloading_ids: &[String],
    ) {
        self.models_history.lock().unwrap().push(active_id.to_string());
    }
}

#[test]
fn test_registry_parsing_and_invariants() {
    let registry_content = include_str!("../models/registry.json");
    let registry = ModelRegistry::from_str(registry_content).expect("Failed to parse registry.json");

    assert_eq!(registry.schema_version, 1);
    assert_eq!(
        registry.default_model_id,
        "sherpa-onnx-streaming-zipformer-small-bilingual-zh-en-2023-02-16"
    );
    assert_eq!(registry.models.len(), 3, "Registry must contain exactly 3 verified models");

    // 1. Bilingual Model
    let zh_en = registry
        .get_model("sherpa-onnx-streaming-zipformer-small-bilingual-zh-en-2023-02-16")
        .expect("Missing zh-en model");
    assert_eq!(
        zh_en.display_title(),
        "Chinese + English (Zipformer-Small) — 2023-02-16"
    );
    assert_eq!(
        zh_en.source.sha256,
        "2b7c63322b32e5e0f2526043a1103366119ca58dd615cd7105a37c01db9553d7"
    );
    assert_eq!(zh_en.files.encoder, "encoder-epoch-99-avg-1.int8.onnx");
    assert_eq!(zh_en.files.decoder, "decoder-epoch-99-avg-1.onnx");
    assert_eq!(zh_en.files.joiner, "joiner-epoch-99-avg-1.int8.onnx");
    assert_eq!(zh_en.files.tokens, "tokens.txt");

    // 2. English Model
    let en = registry
        .get_model("sherpa-onnx-streaming-zipformer-en-20M-2023-02-17")
        .expect("Missing en model");
    assert_eq!(
        en.display_title(),
        "English (Zipformer-20M) — 2023-02-17"
    );
    assert_eq!(
        en.source.sha256,
        "9c559283e8498d3fe95913c79ca1cb454bb26281ac2b102b41306c7d752765d9"
    );
    assert_eq!(en.files.encoder, "encoder-epoch-99-avg-1.int8.onnx");
    assert_eq!(en.files.tokens, "tokens.txt");

    // 3. Chinese Model
    let zh = registry
        .get_model("sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30")
        .expect("Missing zh model");
    assert_eq!(
        zh.display_title(),
        "Chinese (Zipformer-int8) — 2025-06-30"
    );
    assert_eq!(
        zh.source.sha256,
        "5a2832047ea1f97dd0dc595b816c230c4bafad65cfc0341fa57517cadc50afd0"
    );
    assert_eq!(zh.files.encoder, "encoder.int8.onnx");
    assert_eq!(zh.files.tokens, "tokens.txt");
}

#[test]
fn test_manifest_validation_catches_missing_files() {
    let tmp_dir = std::env::temp_dir().join(format!("echolet-test-manifest-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&tmp_dir).unwrap();

    let manifest = ModelManifest {
        id: "test-model".into(),
        display_name: "Test Model".into(),
        version: "2026-01-01".into(),
        language: "en".into(),
        family: "online-transducer".into(),
        encoder: "encoder.onnx".into(),
        decoder: "decoder.onnx".into(),
        joiner: "joiner.onnx".into(),
        tokens: "tokens.txt".into(),
        ..Default::default()
    };

    // Missing all files -> Error
    let err = manifest.validate_files(&tmp_dir).unwrap_err();
    assert!(err.contains("missing Encoder ONNX model"), "Error: {}", err);

    // Create partial files
    fs::write(tmp_dir.join("encoder.onnx"), b"dummy").unwrap();
    fs::write(tmp_dir.join("decoder.onnx"), b"dummy").unwrap();
    let err2 = manifest.validate_files(&tmp_dir).unwrap_err();
    assert!(err2.contains("missing Joiner ONNX model"), "Error: {}", err2);

    // Create remaining files
    fs::write(tmp_dir.join("joiner.onnx"), b"dummy").unwrap();
    fs::write(tmp_dir.join("tokens.txt"), b"dummy").unwrap();
    assert!(manifest.validate_files(&tmp_dir).is_ok());

    let _ = fs::remove_dir_all(&tmp_dir);
}

#[test]
fn test_config_persistence_and_fallback() {
    let tmp_dir = std::env::temp_dir().join(format!("echolet-test-config-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&tmp_dir).unwrap();

    let mut manager = ModelManager::new().expect("Failed to initialize ModelManager");
    manager.config_path = tmp_dir.join("config.json");

    // Save config
    manager.active_model_id = "sherpa-onnx-streaming-zipformer-small-bilingual-zh-en-2023-02-16".into();
    manager.save_config().expect("Failed to save config");
    assert!(manager.config_path.exists());

    // Load config
    let loaded = manager.load_config().expect("Failed to load config");
    assert_eq!(loaded.selected_model, "sherpa-onnx-streaming-zipformer-small-bilingual-zh-en-2023-02-16");

    let _ = fs::remove_dir_all(&tmp_dir);
}

#[test]
fn test_transactional_model_switch_and_listening_guard() {
    let (action_tx, action_rx) = unbounded::<AppAction>();
    let listening_history = Arc::new(Mutex::new(Vec::new()));
    let models_history = Arc::new(Mutex::new(Vec::new()));
    let shutdown_called = Arc::new(AtomicBool::new(false));

    let fake_handle = Box::new(FakePlatformHandle {
        listening_history: listening_history.clone(),
        models_history: models_history.clone(),
        shutdown_called: shutdown_called.clone(),
    });

    let platform = PlatformRuntime {
        injector: Box::new(FakeInjector),
        handle: fake_handle,
        _resources: Box::new(()),
    };

    let (_, audio_rx) = unbounded();
    let mut app = App::new_with_audio(platform, action_rx, audio_rx, None)
        .expect("Failed to create App");

    let initial_model = app.model_manager.active_model_id.clone();
    assert_eq!(
        initial_model,
        "sherpa-onnx-streaming-zipformer-small-bilingual-zh-en-2023-02-16"
    );

    // 1. Guard check: Switching while Listening must be rejected
    app.start_listening();
    assert!(app.state.listening);

    action_tx.send(AppAction::SelectModel("non-existent-model".into())).unwrap();
    app.tick();
    assert_eq!(app.model_manager.active_model_id, initial_model, "Model must not change while listening");

    app.stop_listening();
    assert!(!app.state.listening);

    // 2. Transactional safety check: Switching to invalid/uninstalled model must not crash or drop active recognizer
    action_tx.send(AppAction::SelectModel("corrupted-model-id".into())).unwrap();
    app.tick();
    assert_eq!(app.model_manager.active_model_id, initial_model, "Active model must remain untouched on failure");
}
