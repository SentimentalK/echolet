use crossbeam_channel::unbounded;
use echolet::actions::AppAction;
use echolet::app::App;
use echolet::models::manager::ModelManager;
use echolet::models::manifest::ModelManifest;
use echolet::models::registry::{
    ModelLicense, ModelRegistry, VerificationStatus, CURRENT_SCHEMA_VERSION,
};
use echolet::platform::{PlatformHandle, PlatformRuntime, PlatformView, TextInjector};
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

    fn update_models(&self, view: &PlatformView) {
        self.models_history.lock().unwrap().push(
            view.selected_model()
                .map(|m| m.id.clone())
                .unwrap_or_default(),
        );
    }
}

#[test]
fn test_registry_parsing_and_invariants() {
    let registry_content = include_str!("../models/registry.json");
    let registry =
        ModelRegistry::from_str(registry_content).expect("Failed to parse registry.json");

    assert_eq!(registry.schema_version, 2);
    assert_eq!(
        registry.default_model_id,
        "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1"
    );
    assert_eq!(
        registry.models.len(),
        3,
        "Registry must contain exactly 3 models: X-ASR, Zipformer Streaming, and Nemotron Speech Streaming 0.6B"
    );

    let model_ids: Vec<&str> = registry.models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        model_ids,
        vec![
            "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1",
            "echolet-zipformer-streaming-en-2023-06-26-r1",
            "echolet-nemotron-speech-streaming-en-0.6b-560ms-int8-2026-04-25-r1",
        ]
    );

    // Old multilingual model ID must NOT be present
    assert!(registry
        .get_model("echolet-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11-r1")
        .is_none());

    // 1. X-ASR Bilingual Model (2026 Default Bundled)
    let xasr = registry
        .get_model("echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1")
        .expect("Missing X-ASR model");
    assert_eq!(xasr.display_name, "X-ASR");
    assert_eq!(xasr.version, "2026");
    assert_eq!(xasr.display_title(), "X-ASR — 2026");
    assert_eq!(
        xasr.verification_status,
        VerificationStatus::EcholetVerified
    );
    assert_eq!(xasr.languages, vec!["zh", "en"]);
    assert_eq!(xasr.language_key(), "zh-en");
    assert_eq!(xasr.language_label(), "Chinese + English");
    assert!(xasr.supports_language("zh"));
    assert!(xasr.supports_language("zh-en"));
    assert!(!xasr.supports_language("ja"));
    assert_eq!(xasr.primary_language(), Some("zh"));
    assert!(!xasr.source.bundled);
    assert_eq!(
        xasr.source.repository.as_deref(),
        Some("https://huggingface.co/GilgameshWind/X-ASR-zh-en")
    );
    assert_eq!(
        xasr.source.revision.as_deref(),
        Some("689ff18c584d29910da37b6fe904db0c1489c9d1")
    );
    assert_eq!(xasr.files.encoder, "encoder-480ms.onnx");
    assert_eq!(xasr.files.decoder, "decoder-480ms.onnx");
    assert_eq!(xasr.files.joiner, "joiner-480ms.onnx");
    assert_eq!(xasr.files.tokens, "tokens.txt");
    assert_eq!(xasr.runtime.model_type, Some("zipformer2".into()));

    // 2. Zipformer Streaming English Model
    let zipformer = registry
        .get_model("echolet-zipformer-streaming-en-2023-06-26-r1")
        .expect("Missing Zipformer model");
    assert_eq!(zipformer.display_name, "Zipformer Streaming");
    assert_eq!(zipformer.version, "2023");
    assert_eq!(zipformer.display_title(), "Zipformer Streaming — 2023");
    assert!(!zipformer.display_title().contains("Small English"));
    assert_eq!(
        zipformer.verification_status,
        VerificationStatus::Experimental
    );
    assert_eq!(zipformer.languages, vec!["en"]);
    assert_eq!(zipformer.language_key(), "en");
    assert_eq!(zipformer.language_label(), "English");
    assert!(zipformer.language_options.is_none());
    assert!(zipformer.supported_language_options().is_empty());
    assert_eq!(
        zipformer.files.encoder,
        "encoder-epoch-99-avg-1-chunk-16-left-128.int8.onnx"
    );
    assert_eq!(
        zipformer.files.decoder,
        "decoder-epoch-99-avg-1-chunk-16-left-128.int8.onnx"
    );
    assert_eq!(
        zipformer.files.joiner,
        "joiner-epoch-99-avg-1-chunk-16-left-128.int8.onnx"
    );
    assert_eq!(zipformer.files.tokens, "tokens.txt");
    assert_eq!(zipformer.runtime.model_type, None);
    assert_eq!(
        zipformer.source.url.as_deref(),
        Some("https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-en-2023-06-26.tar.bz2")
    );
    assert_eq!(
        zipformer.source.sha256.as_deref(),
        Some("639e25b578e9e997131402199419c13a941f8e4e198e2da1ce57dbf5cf401282")
    );
    assert_eq!(
        zipformer.source.repository.as_deref(),
        Some(
            "https://huggingface.co/Zengwei/icefall-asr-librispeech-streaming-zipformer-2023-05-17"
        )
    );
    assert_eq!(
        zipformer.source.revision.as_deref(),
        Some("37cb5606808f3d5e55a3fc73554bdf757d82465a")
    );
    assert_eq!(zipformer.download_size_bytes, Some(310414022));
    assert_eq!(zipformer.installed_size_bytes, Some(70913968));
    assert_eq!(
        zipformer.license.as_ref().and_then(|l| l.spdx.as_deref()),
        Some("Apache-2.0")
    );

    // 3. Nemotron Speech Streaming 0.6B English Model
    let nemotron = registry
        .get_model("echolet-nemotron-speech-streaming-en-0.6b-560ms-int8-2026-04-25-r1")
        .expect("Missing Nemotron model");
    assert_eq!(nemotron.display_name, "Nemotron Speech Streaming 0.6B");
    assert_eq!(nemotron.version, "2026");
    assert_eq!(
        nemotron.display_title(),
        "Nemotron Speech Streaming 0.6B — 2026"
    );
    assert!(!nemotron.display_title().contains("Large English"));
    assert_eq!(
        nemotron.verification_status,
        VerificationStatus::Experimental
    );
    assert_eq!(nemotron.languages, vec!["en"]);
    assert_eq!(nemotron.language_key(), "en");
    assert_eq!(nemotron.language_label(), "English");
    assert!(nemotron.language_options.is_none());
    assert!(nemotron.supported_language_options().is_empty());
    assert_eq!(nemotron.files.encoder, "encoder.int8.onnx");
    assert_eq!(nemotron.files.decoder, "decoder.int8.onnx");
    assert_eq!(nemotron.files.joiner, "joiner.int8.onnx");
    assert_eq!(nemotron.files.tokens, "tokens.txt");
    assert_eq!(nemotron.runtime.model_type, None);
    assert_eq!(
        nemotron.source.url.as_deref(),
        Some("https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemotron-speech-streaming-en-0.6b-560ms-int8-2026-04-25.tar.bz2")
    );
    assert_eq!(
        nemotron.source.sha256.as_deref(),
        Some("78e2b79fcf7271553a74402a76b771b09ea40117a39566a79f52235b23db6358")
    );
    assert_eq!(
        nemotron.source.repository.as_deref(),
        Some("https://huggingface.co/nvidia/nemotron-speech-streaming-en-0.6b")
    );
    assert_eq!(
        nemotron.source.revision.as_deref(),
        Some("ebe59e5a817142986528bbbee5dba8db7b38ed50")
    );
    assert_eq!(nemotron.download_size_bytes, Some(463945051));
    assert_eq!(nemotron.installed_size_bytes, Some(661919416));
    assert_eq!(
        nemotron
            .license
            .as_ref()
            .map(|l| l.name.as_deref().unwrap_or("")),
        Some("NVIDIA Open Model License Agreement")
    );
    assert_eq!(
        nemotron.license.as_ref().and_then(|l| l.spdx.as_deref()),
        None
    );
}

#[test]
fn test_manifest_validation_catches_missing_files() {
    let tmp_dir =
        std::env::temp_dir().join(format!("echolet-test-manifest-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&tmp_dir).unwrap();

    let manifest = ModelManifest {
        id: "test-model".into(),
        display_name: "Test Model".into(),
        version: "2026-01-01".into(),
        languages: vec!["en".into()],
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
    assert!(
        err2.contains("missing Joiner ONNX model"),
        "Error: {}",
        err2
    );

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
    manager.active_model_id =
        Some("echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1".into());
    manager.save_config().expect("Failed to save config");
    assert!(manager.config_path.exists());

    // Load config
    let loaded = manager.load_config().expect("Failed to load config");
    assert_eq!(
        loaded.selected_model,
        "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1"
    );

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
    let mut app =
        App::new_with_audio(platform, action_rx, audio_rx, None).expect("Failed to create App");

    let initial_model = app.model_manager.active_model_id.clone();
    assert!(
        initial_model.is_some(),
        "Initial active model must not be absent"
    );

    // 1. Guard check: Switching while Listening must be rejected
    app.start_listening();
    assert!(app.state.listening);

    action_tx
        .send(AppAction::SelectModel("non-existent-model".into()))
        .unwrap();
    app.tick();
    assert_eq!(
        app.model_manager.active_model_id, initial_model,
        "Model must not change while listening"
    );

    app.stop_listening();
    assert!(!app.state.listening);

    // 2. Transactional safety check: Switching to invalid/uninstalled model must not crash or drop active recognizer
    action_tx
        .send(AppAction::SelectModel("corrupted-model-id".into()))
        .unwrap();
    app.tick();
    assert_eq!(
        app.model_manager.active_model_id, initial_model,
        "Active model must remain untouched on failure"
    );
}

// ---------------------------------------------------------------------------
// J7: registry schema v2 / backward compatibility
// ---------------------------------------------------------------------------

/// Exact legacy v1 registry shape: singular `language` string, no optional
/// v2 metadata, no `verification_status`.
const LEGACY_V1_REGISTRY: &str = r#"
{
  "schema_version": 1,
  "default_model_id": "legacy-model",
  "models": [
    {
      "id": "legacy-model",
      "display_name": "Legacy Bilingual",
      "version": "1",
      "language": "zh-en",
      "family": "online-transducer",
      "source": {
        "bundled": false,
        "url": "https://example.invalid/legacy.tar.zst",
        "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "repository": "https://example.invalid/repo",
        "revision": "rev-legacy"
      },
      "files": {
        "encoder": "encoder.onnx",
        "decoder": "decoder.onnx",
        "joiner": "joiner.onnx",
        "tokens": "tokens.txt"
      },
      "runtime": {
        "model_type": "zipformer2",
        "sample_rate": 16000,
        "feature_dim": 80,
        "num_threads": 1,
        "provider": "cpu",
        "decoding_method": "greedy_search",
        "max_active_paths": 4
      }
    }
  ]
}
"#;

#[test]
fn test_legacy_v1_registry_parses_and_normalizes() {
    let registry = ModelRegistry::from_str(LEGACY_V1_REGISTRY).expect("legacy v1 must parse");
    assert_eq!(registry.schema_version, 1);
    let entry = registry.get_model("legacy-model").expect("legacy model");
    // Legacy singular `language: "zh-en"` normalizes centrally to plural codes.
    assert_eq!(entry.languages, vec!["zh", "en"]);
    assert_eq!(entry.language_key(), "zh-en");
    assert!(entry.supports_language("zh-en"));
    // Missing legacy verification_status defaults conservatively to non-Verified.
    assert_eq!(entry.verification_status, VerificationStatus::Experimental);
    assert!(!entry.verification_status.is_verified());
    // Optional v2 metadata is absent, not required.
    assert_eq!(entry.download_size_bytes, None);
    assert_eq!(entry.installed_size_bytes, None);
    assert_eq!(entry.upstream_release_date, None);
    assert_eq!(entry.license, None);

    // v1 -> normalized in-memory -> canonical v2 serialization.
    let canonical = registry
        .to_canonical_string()
        .expect("canonical serialization must succeed");
    assert!(canonical.contains("\"schema_version\": 2"));
    assert!(canonical.contains("\"languages\""));
    assert!(
        !canonical.contains("\"language\""),
        "canonical v2 must not emit the legacy singular field: {}",
        canonical
    );
    let reparsed = ModelRegistry::from_str(&canonical).expect("canonical v2 must reparse");
    assert_eq!(reparsed.schema_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(
        reparsed.models, registry.models,
        "normalization must be lossless for model metadata"
    );
}

fn canonical_v2_registry() -> String {
    r#"
{
  "schema_version": 2,
  "default_model_id": "v2-model",
  "models": [
    {
      "id": "v2-model",
      "display_name": "V2 Bilingual",
      "version": "2",
      "languages": ["zh", "en"],
      "family": "online-transducer",
      "source": {
        "bundled": false,
        "url": "https://example.invalid/v2.tar.zst",
        "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "repository": "https://example.invalid/repo",
        "revision": "rev-v2"
      },
      "files": {
        "encoder": "encoder.onnx",
        "decoder": "decoder.onnx",
        "joiner": "joiner.onnx",
        "tokens": "tokens.txt"
      },
      "runtime": {
        "model_type": "zipformer2",
        "sample_rate": 16000,
        "feature_dim": 80,
        "num_threads": 2,
        "provider": "cpu",
        "decoding_method": "greedy_search",
        "max_active_paths": 4
      },
      "download_size_bytes": 123456,
      "installed_size_bytes": 234567,
      "upstream_release_date": "2026-04-25",
      "license": {
        "spdx": "Apache-2.0",
        "name": "Apache License 2.0",
        "url": "https://www.apache.org/licenses/LICENSE-2.0"
      },
      "verification_status": "Community"
    }
  ]
}
"#
    .to_string()
}

#[test]
fn test_canonical_v2_round_trip_preserves_all_metadata() {
    let registry = ModelRegistry::from_str(&canonical_v2_registry()).expect("v2 must parse");
    assert_eq!(registry.schema_version, CURRENT_SCHEMA_VERSION);

    let entry = registry.get_model("v2-model").expect("v2 model");
    assert_eq!(entry.download_size_bytes, Some(123456));
    assert_eq!(entry.installed_size_bytes, Some(234567));
    assert_eq!(entry.upstream_release_date.as_deref(), Some("2026-04-25"));
    assert_eq!(
        entry.license,
        Some(ModelLicense {
            spdx: Some("Apache-2.0".into()),
            name: Some("Apache License 2.0".into()),
            url: Some("https://www.apache.org/licenses/LICENSE-2.0".into()),
        })
    );
    assert_eq!(registry.models[0].verification_status.label(), "Community");

    let serialized = registry.to_canonical_string().unwrap();
    let reparsed = ModelRegistry::from_str(&serialized).expect("round-trip must parse");
    assert_eq!(reparsed, registry, "round-trip must not lose metadata");
}

#[test]
fn test_missing_optional_v2_metadata_parses() {
    let json = r#"
    {
      "schema_version": 2,
      "default_model_id": "m",
      "models": [
        {
          "id": "m",
          "display_name": "M",
          "version": "1",
          "languages": ["en"],
          "family": "online-transducer",
          "source": {},
          "files": {
            "encoder": "e.onnx", "decoder": "d.onnx",
            "joiner": "j.onnx", "tokens": "tokens.txt"
          },
          "runtime": {}
        }
      ]
    }
    "#;
    let registry = ModelRegistry::from_str(json).expect("minimal v2 must parse");
    let entry = registry.get_model("m").unwrap();
    assert_eq!(entry.download_size_bytes, None);
    assert_eq!(entry.installed_size_bytes, None);
    assert_eq!(entry.upstream_release_date, None);
    // license URL (and license entirely) absent is fine.
    assert_eq!(entry.license, None);
    // Runtime config falls back to backward-compatible defaults.
    assert_eq!(entry.runtime.sample_rate, 16000);
    assert_eq!(entry.runtime.provider, "cpu");
    // Unknown/omitted verification defaults to non-Verified.
    assert!(!entry.verification_status.is_verified());
}

#[test]
fn test_unknown_future_schema_version_fails_clearly() {
    let json = r#"{"schema_version": 99, "default_model_id": "m", "models": []}"#;
    let err = ModelRegistry::from_str(json).unwrap_err();
    assert!(
        err.contains("Unsupported registry schema_version 99"),
        "unexpected error: {}",
        err
    );
    assert!(
        err.contains("up to 2"),
        "error must state the supported ceiling: {}",
        err
    );
}

// ---------------------------------------------------------------------------
// J8: canonical X-ASR promotion + identity drift guards
// ---------------------------------------------------------------------------

/// The shipped registry's canonical X-ASR entry is `Echolet Verified`, and that
/// promotion is tied to the *exact* immutable identity from the repo
/// authorities. If a future model change updates `base-model.json` /
/// `base-model.lock.json` without updating (and re-verifying) the shipped
/// registry entry, this test fails rather than letting the stale entry keep
/// inheriting `Verified`.
#[test]
fn test_shipped_registry_xasr_is_verified_and_matches_authorities() {
    let registry = ModelRegistry::from_str(include_str!("../models/registry.json"))
        .expect("shipped registry must parse");
    assert_eq!(registry.schema_version, CURRENT_SCHEMA_VERSION);

    let base: serde_json::Value =
        serde_json::from_str(include_str!("../models/base-model.json")).expect("base-model.json");
    let lock: serde_json::Value =
        serde_json::from_str(include_str!("../models/base-model.lock.json"))
            .expect("base-model.lock.json");

    let xasr = registry
        .get_model("echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1")
        .expect("current X-ASR entry");

    // J8 promotion: the exact canonical identity is now Echolet Verified.
    assert!(
        xasr.verification_status.is_verified(),
        "canonical X-ASR must be Echolet Verified after J8"
    );
    assert_eq!(
        xasr.verification_status,
        VerificationStatus::EcholetVerified
    );
    assert_eq!(xasr.verification_status.label(), "Echolet Verified");

    // URL + archive SHA256 must match the immutable Echolet Release lock.
    assert_eq!(xasr.source.url.as_deref(), lock["url"].as_str());
    assert_eq!(xasr.source.sha256.as_deref(), lock["sha256"].as_str());

    // Provenance, languages and license must match the base-model authority.
    assert_eq!(
        xasr.source.revision.as_deref(),
        base["upstream_revision"].as_str()
    );
    assert_eq!(
        xasr.languages,
        base["language"]
            .as_array()
            .expect("base-model language array")
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    );
    let license = xasr.license.as_ref().expect("license metadata present");
    assert_eq!(license.spdx.as_deref(), base["license"].as_str());
    assert_eq!(license.spdx.as_deref(), Some("Apache-2.0"));

    // The model id itself encodes the upstream revision, so a revision bump
    // cannot silently keep the old Verified id.
    assert!(
        xasr.id
            .contains(base["upstream_revision"].as_str().unwrap()),
        "model id must encode the upstream revision: {}",
        xasr.id
    );

    // J8 populated authoritative size metadata for the verified pack: the
    // immutable archive length, and the deterministic sum of the four
    // canonical installed model-pack files (see the field docs in registry.rs).
    assert_eq!(xasr.download_size_bytes, Some(551_847_917));
    assert_eq!(xasr.installed_size_bytes, Some(614_596_718));

    // Every Verified entry must be one of the two frozen identities and must
    // point at its own immutable Echolet Release lock. After J8/J10 that is the
    // X-ASR baseline and the additive Nemotron multilingual pack.
    let nemotron_lock: serde_json::Value =
        serde_json::from_str(include_str!("../models/nemotron-model.lock.json"))
            .expect("nemotron-model.lock.json");
    for entry in registry
        .models
        .iter()
        .filter(|m| m.verification_status.is_verified())
    {
        let is_baseline =
            entry.id == xasr.id && entry.source.sha256.as_deref() == lock["sha256"].as_str();
        let is_nemotron = entry.id.as_str() == nemotron_lock["id"].as_str().unwrap_or_default()
            && entry.source.url.as_deref() == nemotron_lock["url"].as_str()
            && entry.source.sha256.as_deref() == nemotron_lock["sha256"].as_str();
        assert!(
            is_baseline || is_nemotron,
            "Verified entry '{}' does not match any frozen Echolet release identity",
            entry.id
        );
    }
    assert!(
        xasr.verification_status.is_verified(),
        "X-ASR must remain Echolet Verified"
    );

    // Round-trip of the shipped registry is stable.
    let round_trip = ModelRegistry::from_str(&registry.to_canonical_string().unwrap()).unwrap();
    assert_eq!(round_trip, registry);
}

#[test]
fn test_manager_discovers_registry_model_without_manifest() {
    let tmp = std::env::temp_dir().join(format!("echolet-j7-discovery-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let bundled = tmp.join("bundled");
    let user = tmp.join("user");
    let cfg = tmp.join("config.json");
    // Legacy-style install directory name derived from the language pair; no
    // model.json is written, so discovery must fall back to the registry entry.
    let model_dir = bundled.join("bilingual-zh-en");
    fs::create_dir_all(&model_dir).unwrap();
    for file in [
        "encoder-480ms.onnx",
        "decoder-480ms.onnx",
        "joiner-480ms.onnx",
        "tokens.txt",
    ] {
        fs::write(model_dir.join(file), b"dummy").unwrap();
    }
    fs::create_dir_all(&user).unwrap();

    let manager = ModelManager::new_with_paths(bundled, user, cfg).expect("manager must init");
    let id = "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1";
    assert!(
        manager.is_installed(id),
        "installed models: {:?}",
        manager.installed.keys().collect::<Vec<_>>()
    );
    let installed = manager.get_model(id).unwrap();
    assert_eq!(installed.manifest.languages, vec!["zh", "en"]);
    assert_eq!(installed.manifest.language_label(), "Chinese + English");

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_platform_projection_derives_language_label_from_normalized_schema() {
    let registry = ModelRegistry::from_str(include_str!("../models/registry.json")).unwrap();
    let entry = registry.default_entry().expect("default entry");
    // The platform tray projection derives a display title; the language label
    // is derived from the normalized plural schema.
    assert_eq!(entry.display_title(), "X-ASR — 2026");
    assert_eq!(entry.language_label(), "Chinese + English");
    assert_eq!(entry.language_key(), "zh-en");
    assert!(entry.matches_install_dir("bilingual-zh-en"));
    assert!(entry
        .matches_install_dir("echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1"));
    assert!(!entry.matches_install_dir("bilingual-ja-en"));
}

#[test]
fn test_project_041_control_surface_grouping_and_actions() {
    use echolet::config::EcholetConfig;
    use echolet::ui::control_surface::{
        build_control_surface_state, ModelPrimaryAction, RuntimeState, SurfaceAction,
    };
    use std::collections::{HashMap, HashSet};

    let registry = ModelRegistry::from_str(include_str!("../models/registry.json")).unwrap();

    // Default model remains X-ASR
    assert_eq!(
        registry.default_model_id,
        "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1"
    );

    let zipformer_id = "echolet-zipformer-streaming-en-2023-06-26-r1";
    let nemotron_id = "echolet-nemotron-speech-streaming-en-0.6b-560ms-int8-2026-04-25-r1";
    let xasr_id = "echolet-xasr-zh-en-480ms-689ff18c584d29910da37b6fe904db0c1489c9d1";

    // 1. Uninstalled state
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

    // Grouping assertions
    assert_eq!(state.model_groups.len(), 2);
    assert_eq!(state.model_groups[0].id, "zh-en");
    assert_eq!(state.model_groups[0].label, "Chinese + English");
    assert_eq!(state.model_groups[0].models.len(), 1);
    assert_eq!(state.model_groups[0].models[0].id, xasr_id);

    assert_eq!(state.model_groups[1].id, "en");
    assert_eq!(state.model_groups[1].label, "English");
    assert_eq!(state.model_groups[1].models.len(), 2);
    assert_eq!(state.model_groups[1].models[0].id, zipformer_id);
    assert_eq!(state.model_groups[1].models[1].id, nemotron_id);

    // No Multilingual group from current catalog
    assert!(state.model_groups.iter().all(|g| g.id != "multilingual"));

    // Product names & clean labels
    let m_xasr = state.find_model(xasr_id).unwrap();
    assert_eq!(m_xasr.label, "X-ASR — 2026");
    assert!(m_xasr.is_verified);

    let m_zip = state.find_model(zipformer_id).unwrap();
    assert_eq!(m_zip.label, "Zipformer Streaming — 2023");
    assert!(!m_zip.label.contains("Small English"));
    assert!(!m_zip.is_verified);
    assert!(m_zip.language.options.is_empty());

    let m_nemo = state.find_model(nemotron_id).unwrap();
    assert_eq!(m_nemo.label, "Nemotron Speech Streaming 0.6B — 2026");
    assert!(!m_nemo.label.contains("Large English"));
    assert!(!m_nemo.is_verified);
    assert!(m_nemo.language.options.is_empty());

    // Action when uninstalled: Download
    assert_eq!(m_zip.primary_action, ModelPrimaryAction::Download);
    assert!(m_zip.enabled);
    assert_eq!(
        m_zip.surface_action(),
        Some(SurfaceAction::DownloadModel(zipformer_id.to_string()))
    );

    assert_eq!(m_nemo.primary_action, ModelPrimaryAction::Download);
    assert!(m_nemo.enabled);
    assert_eq!(
        m_nemo.surface_action(),
        Some(SurfaceAction::DownloadModel(nemotron_id.to_string()))
    );

    // 2. Installed, non-selected state
    let mut installed = HashSet::new();
    installed.insert(zipformer_id.to_string());
    installed.insert(nemotron_id.to_string());
    installed.insert(xasr_id.to_string());

    let state_installed = build_control_surface_state(
        &registry,
        Some(xasr_id),
        &installed,
        &HashSet::new(),
        &HashMap::new(),
        &EcholetConfig::default(),
        RuntimeState::Ready,
        false,
    );

    let m_zip_inst = state_installed.find_model(zipformer_id).unwrap();
    assert_eq!(m_zip_inst.primary_action, ModelPrimaryAction::Select);
    assert!(m_zip_inst.enabled);
    assert_eq!(
        m_zip_inst.surface_action(),
        Some(SurfaceAction::SelectModel(zipformer_id.to_string()))
    );

    let m_xasr_sel = state_installed.find_model(xasr_id).unwrap();
    assert_eq!(m_xasr_sel.primary_action, ModelPrimaryAction::None);
    assert!(!m_xasr_sel.enabled);
    assert_eq!(m_xasr_sel.surface_action(), None);
    assert!(state_installed.active_language().is_none());

    // 3. Selected English model exposes no language options / active language
    let state_selected_zip = build_control_surface_state(
        &registry,
        Some(zipformer_id),
        &installed,
        &HashSet::new(),
        &HashMap::new(),
        &EcholetConfig::default(),
        RuntimeState::Ready,
        false,
    );
    assert!(state_selected_zip.active_language().is_none());

    let state_selected_nemo = build_control_surface_state(
        &registry,
        Some(nemotron_id),
        &installed,
        &HashSet::new(),
        &HashMap::new(),
        &EcholetConfig::default(),
        RuntimeState::Ready,
        false,
    );
    assert!(state_selected_nemo.active_language().is_none());

    // 4. Listening safety prevents actions
    let state_listening = build_control_surface_state(
        &registry,
        Some(xasr_id),
        &installed,
        &HashSet::new(),
        &HashMap::new(),
        &EcholetConfig::default(),
        RuntimeState::Listening,
        false,
    );
    let m_zip_listening = state_listening.find_model(zipformer_id).unwrap();
    assert_eq!(m_zip_listening.primary_action, ModelPrimaryAction::Select);
    assert!(!m_zip_listening.enabled);
    assert_eq!(m_zip_listening.surface_action(), None);
}
