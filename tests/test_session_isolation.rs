//! Deterministic behavioral regression tests for voice-session lifecycle
//! isolation (engine-owned stop/cancel, session generations, fresh audio
//! queue and stream boundaries).
//!
//! Invariants under test:
//! 1. Speech produces a visible partial; Stop keeps it EXACTLY (including
//!    the partial) and never injects or deletes anything after stop.
//! 2. All undelivered audio of a stopped session is released: late chunks
//!    from an old capture fail to send and never transcribe into a newer
//!    session.
//! 3. Stop is idempotent; the next Start is a clean generation — fresh
//!    engine session (token/queue/revision/diff window), fresh recognizer
//!    stream, and a fresh FIRST APPEND (no destructive backspaces against
//!    the already-visible text).
//! 4. Model unload / failed starts cannot resurrect a stopped session.
//! 5. Pre-existing platform text injection behavior (diff semantics via the
//!    TextInjector adapter), per-model language options and resident model
//!    behavior are unchanged.
//!
//! These tests run against the REAL bundled model (same convention as
//! `test_stream.rs`: the staged fixture under `.local-runtime/models`, which
//! CI guarantees) and a scripted `AudioStarter` that hands the test the
//! per-session sender, so "late producer chunks" are simulated
//! deterministically. A missing fixture FAILS with the actionable path —
//! tests are never silently skipped.
//!
//! No process-global state is mutated: user data is isolated through an
//! explicit temporary `ModelManager` and an explicit `EcholetConfig`.

use crossbeam_channel::unbounded;
use echolet::actions::AppAction;
use echolet::app::App;
use echolet::audio::{AudioChunk, AudioSource, AudioStarter};
use echolet::config::EcholetConfig;
use echolet::models::ModelManager;
use echolet::paths;
use echolet::platform::{PlatformHandle, PlatformRuntime, PlatformView, TextInjector};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// RAII unique temporary directory (parallel-test safe).
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "echolet-session-test-{}-{}-{}",
            label,
            std::process::id(),
            id
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("creating unique temp dir must succeed");
        TempDir(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The staged real model under the resolved bundled models dir. CI stages
/// `.local-runtime` via `scripts/download-official-assets.sh`; a missing
/// fixture is a hard, actionable failure — never a silent skip.
fn staged_model_dir() -> PathBuf {
    let dir = paths::bundled_models_dir().join("bilingual-zh-en");
    let manifest_path = dir.join("model.json");
    assert!(
        manifest_path.exists(),
        "required staged ASR model fixture missing at {:?} — `.local-runtime` must be staged \
         (scripts/download-official-assets.sh); refusing to fake a skip",
        manifest_path
    );
    dir
}

fn staged_model_id() -> String {
    let dir = staged_model_dir();
    let manifest_path = dir.join("model.json");
    let manifest = echolet::models::manifest::ModelManifest::from_file(&manifest_path)
        .unwrap_or_else(|err| {
            panic!(
                "staged model manifest at {:?} must parse: {}",
                manifest_path, err
            )
        });
    for file in [
        &manifest.encoder,
        &manifest.decoder,
        &manifest.joiner,
        &manifest.tokens,
    ] {
        let path = dir.join(file);
        assert!(
            path.exists(),
            "staged model fixture at {:?} is incomplete: {:?} missing",
            dir,
            path
        );
    }
    manifest.id
}

/// Splits the staged fixture wav into 0.2s mono chunks at 16 kHz. A missing,
/// malformed or empty fixture is a hard failure with the exact path.
fn load_fixture_chunks() -> (Vec<Vec<f32>>, u32) {
    let wav_path = staged_model_dir().join("test_wavs/0.wav");
    let bytes = std::fs::read(&wav_path)
        .unwrap_or_else(|err| panic!("required speech fixture {:?} missing: {}", wav_path, err));
    assert!(
        bytes.len() > 44,
        "speech fixture {:?} is too small to contain PCM samples",
        wav_path
    );
    let pcm = &bytes[44..];
    let samples: Vec<f32> = pcm
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
        .collect();
    let chunk_size = 3200; // 0.2s at 16 kHz
    let chunks: Vec<Vec<f32>> = samples.chunks(chunk_size).map(<[f32]>::to_vec).collect();
    assert!(
        !chunks.is_empty(),
        "speech fixture {:?} must contain at least one chunk of audio",
        wav_path
    );
    (chunks, 16000)
}

fn chunk_at(chunks: &[Vec<f32>], i: usize) -> Vec<f32> {
    chunks[i % chunks.len()].clone()
}

/// Applies a diff log to reconstruct the text currently visible in the
/// "editor" — exactly what a TextInjector consumer would have produced.
fn replay_diffs(log: &[(usize, String)]) -> String {
    let mut visible: Vec<char> = Vec::new();
    for (backspaces, suffix) in log {
        for _ in 0..*backspaces {
            visible.pop();
        }
        visible.extend(suffix.chars());
    }
    visible.into_iter().collect()
}

/// Test platform capturing every diff and listening transition.
struct CapturedDiffs(Arc<Mutex<Vec<(usize, String)>>>);

impl TextInjector for CapturedDiffs {
    fn apply_diff(&self, backspaces: usize, new_suffix: &str) {
        self.0
            .lock()
            .unwrap()
            .push((backspaces, new_suffix.to_string()));
    }
}

struct FakeHandle {
    listening: Arc<Mutex<Vec<bool>>>,
}

impl PlatformHandle for FakeHandle {
    fn set_listening(&self, listening: bool) {
        self.listening.lock().unwrap().push(listening);
    }
    fn update_models(&self, _view: &PlatformView) {}
    fn update_history_state(&self, _enabled: bool) {}
    fn shutdown(&self) {}
}

/// Harness that records the sender handed to each session start, so a test
/// can keep "producing" after the session has been stopped.
struct Harness {
    app: App,
    action_tx: crossbeam_channel::Sender<AppAction>,
    txs: Arc<Mutex<Vec<crossbeam_channel::Sender<AudioChunk>>>>,
    diffs: Arc<Mutex<Vec<(usize, String)>>>,
    listening: Arc<Mutex<Vec<bool>>>,
    #[allow(dead_code)]
    _user_models_dir: TempDir,
    #[allow(dead_code)]
    _config_dir: TempDir,
}

/// Scripted AudioStarter: no hardware needed; it records each session's
/// queue sender so the test can produce audio (and keep producing late).
fn scripted_starter() -> (
    AudioStarter,
    Arc<Mutex<Vec<crossbeam_channel::Sender<AudioChunk>>>>,
) {
    let txs = Arc::new(Mutex::new(Vec::new()));
    let keep = txs.clone();
    (
        Box::new(move |tx| {
            keep.lock().unwrap().push(tx);
            Ok(Box::new(()) as Box<dyn AudioSource>)
        }),
        txs,
    )
}

/// AudioStarter that simulates capture hardware failure (e.g. mic busy) —
/// it still records the handed-out sender so late-send failures can be
/// checked even for the failed session.
fn failing_starter() -> (
    AudioStarter,
    Arc<Mutex<Vec<crossbeam_channel::Sender<AudioChunk>>>>,
) {
    let txs = Arc::new(Mutex::new(Vec::new()));
    let keep = txs.clone();
    (
        Box::new(move |tx| {
            keep.lock().unwrap().push(tx);
            Err("simulated microphone unavailable".to_string())
        }),
        txs,
    )
}

/// Builds a Desktop composition root over the REAL staged model with an
/// isolated temporary ModelManager and explicit config (preload off,
/// history off). Unique temp dirs keep parallel test threads apart with
/// RAII cleanup. No process-global environment is ever mutated.
fn make_harness_with(
    config: EcholetConfig,
    starter: AudioStarter,
    txs: Arc<Mutex<Vec<crossbeam_channel::Sender<AudioChunk>>>>,
) -> Harness {
    let user_models_dir = TempDir::new("user-models");
    let config_dir = TempDir::new("config");
    let config_path = config_dir.path().join("config.json");

    let model_manager = ModelManager::new_with_paths(
        paths::bundled_models_dir(),
        user_models_dir.path().to_path_buf(),
        config_path,
    )
    .expect("ModelManager over the staged bundled fixture directory");
    let staged = staged_model_id();
    assert!(
        model_manager.installed.contains_key(&staged),
        "staged model '{}' must be discovered from {:?}; installed = {:?}",
        staged,
        paths::bundled_models_dir(),
        model_manager.installed.keys().collect::<Vec<_>>(),
    );

    let (action_tx, action_rx) = unbounded::<AppAction>();
    let diffs = Arc::new(Mutex::new(Vec::new()));
    let listening = Arc::new(Mutex::new(Vec::new()));
    let platform = PlatformRuntime {
        injector: Box::new(CapturedDiffs(diffs.clone())),
        handle: Box::new(FakeHandle {
            listening: listening.clone(),
        }),
        _resources: Box::new(()),
    };
    let app = App::new_with_manager_and_config(
        platform,
        Some(action_tx.clone()),
        action_rx,
        None,
        starter,
        None,
        Some(config),
        model_manager,
    )
    .expect("harness app must construct");
    Harness {
        app,
        action_tx,
        txs,
        diffs,
        listening,
        _user_models_dir: user_models_dir,
        _config_dir: config_dir,
    }
}

fn make_harness(config: EcholetConfig) -> Harness {
    let (starter, txs) = scripted_starter();
    make_harness_with(config, starter, txs)
}

/// Hermetic harness config: no preload, no history writes anywhere.
fn harness_config() -> EcholetConfig {
    let mut config = EcholetConfig::default();
    config.preload_model_on_startup = false;
    config.history_enabled = false;
    config
}

impl Harness {
    fn active_sender(&self) -> crossbeam_channel::Sender<AudioChunk> {
        self.txs
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a session must have started to own a sender")
    }

    fn starts(&self) -> usize {
        self.txs.lock().unwrap().len()
    }

    fn recorded_senders(&self) -> Vec<crossbeam_channel::Sender<AudioChunk>> {
        self.txs.lock().unwrap().clone()
    }

    fn send(&self, tx: &crossbeam_channel::Sender<AudioChunk>, samples: Vec<f32>, rate: u32) {
        tx.send(AudioChunk {
            samples,
            sample_rate: rate,
        })
        .expect("channel to an active session queue must be open");
    }

    /// Ticks until the diff log stops changing (decode is bounded per tick).
    fn settle(&mut self, max_ticks: usize) {
        for _ in 0..max_ticks {
            let before = self.diffs.lock().unwrap().len();
            self.app.tick();
            if self.diffs.lock().unwrap().len() == before {
                break;
            }
        }
    }
}

/// Invariant 1 + 2 + the Stop-half of 3:
/// a visible partial from session 1 survives stop unchanged; nothing is
/// injected after stop; late audio from the stopped session is rejected
/// (its queue is disconnected); the next Start arms a fresh queue and its
/// first diff is an append, never a destructive backspace.
#[test]
fn stop_retains_visible_partial_and_rejects_late_audio() {
    let (chunks, rate) = load_fixture_chunks();
    let mut h = make_harness(harness_config());

    // Session 1: recognize real speech into a visible partial.
    h.action_tx.send(AppAction::StartListening).unwrap();
    h.settle(3);
    assert!(h.app.state.listening);
    let tx1 = h.active_sender();
    for chunk in chunks.iter().take(40) {
        h.send(&tx1, chunk.clone(), rate);
        h.app.tick();
    }
    h.settle(8);
    let log = h.diffs.lock().unwrap().clone();
    assert!(
        !log.is_empty(),
        "session 1 must have injected at least one diff from real speech"
    );
    let visible_at_stop = replay_diffs(&log);
    assert!(!visible_at_stop.is_empty());

    // Stop: session cancelled first, then mic released, then history I/O.
    h.app.stop_listening();
    assert!(!h.app.state.listening);
    let log_len_at_stop = h.diffs.lock().unwrap().len();
    h.settle(6);
    assert_eq!(
        h.diffs.lock().unwrap().len(),
        log_len_at_stop,
        "stop must discard undelivered inference callbacks: no post-stop text"
    );
    assert_eq!(
        replay_diffs(&h.diffs.lock().unwrap().clone()),
        visible_at_stop
    );

    // Late producer chunks raced the release: the old queue is disconnected,
    // so the sends must FAIL and nothing observable changes.
    let late1 = AudioChunk {
        samples: chunk_at(&chunks, 40),
        sample_rate: rate,
    };
    assert!(
        tx1.send(late1).is_err(),
        "old session queue must be disconnected after stop"
    );
    h.settle(4);
    assert_eq!(h.diffs.lock().unwrap().len(), log_len_at_stop);

    // The next Start is a clean generation with a brand-new queue.
    h.action_tx.send(AppAction::StartListening).unwrap();
    h.settle(3);
    assert!(h.app.state.listening);
    assert_eq!(h.starts(), 2, "each start must arm a fresh audio queue");
    let tx2 = h.active_sender();
    assert!(
        tx2.send(AudioChunk {
            samples: vec![0.0; 4],
            sample_rate: rate
        })
        .is_ok(),
        "new session queue must be live"
    );
    // Old-session audio still cannot cross into session 2.
    let log_len_pre_session2 = h.diffs.lock().unwrap().len();
    for chunk in chunks.iter().take(50) {
        h.send(&tx2, chunk.clone(), rate);
        h.app.tick();
    }
    h.settle(8);
    let log = h.diffs.lock().unwrap().clone();
    assert!(
        log.len() > log_len_pre_session2,
        "session 2 must recognize fresh speech"
    );
    let session2_log = &log[log_len_pre_session2..];
    assert_eq!(
        session2_log[0].0, 0,
        "first diff of a new session must not retro-edit the committed partial"
    );
    assert!(
        !session2_log[0].1.is_empty(),
        "first diff of a new session must append real text"
    );
    let visible_final = replay_diffs(&log);
    assert!(
        visible_final.len() >= visible_at_stop.len() && visible_final.starts_with(&visible_at_stop),
        "session 2 must never shrink or alter already visible text"
    );
    h.app.stop_listening();
}

/// Invariant 3 (idempotency): repeated stop calls are complete no-ops.
#[test]
fn stop_is_idempotent() {
    let (chunks, rate) = load_fixture_chunks();
    let mut h = make_harness(harness_config());

    h.action_tx.send(AppAction::StartListening).unwrap();
    h.settle(2);
    let tx = h.active_sender();
    h.send(&tx, chunk_at(&chunks, 0), rate);
    h.app.tick();

    h.app.stop_listening();
    let listening_after = h.listening.lock().unwrap().clone();
    let diffs_after = h.diffs.lock().unwrap().len();
    assert!(!h.app.state.listening);
    assert!(!h.app.is_audio_active());
    assert!(!h.app.is_session_generatively_current());

    for _ in 0..5 {
        h.app.stop_listening();
    }
    assert_eq!(
        h.listening.lock().unwrap().clone(),
        listening_after,
        "idempotent stop must not re-project the listening transition"
    );
    assert_eq!(h.diffs.lock().unwrap().len(), diffs_after);
    assert!(!h.app.state.listening);
    assert!(!h.app.is_audio_active());
    assert!(!h.app.is_session_generatively_current());
}

/// Invariant 3 (rapid cycles): ten rapid stop/start cycles — fresh queue per
/// start, no old text reappearing, no post-stop injections.
#[test]
fn ten_rapid_stop_start_cycles_stay_isolated() {
    let (chunks, rate) = load_fixture_chunks();
    let mut h = make_harness(harness_config());

    for cycle in 0..10 {
        h.action_tx.send(AppAction::StartListening).unwrap();
        h.settle(2);
        assert!(h.app.state.listening, "cycle {}", cycle);
        assert_eq!(
            h.starts(),
            cycle + 1,
            "cycle {}: fresh queue per start",
            cycle
        );
        let tx = h.active_sender();

        for chunk in chunks.iter().take(8) {
            h.send(&tx, chunk.clone(), rate);
            h.app.tick();
        }
        h.settle(4);
        let visible_before_stop = replay_diffs(&h.diffs.lock().unwrap().clone());
        let log_len_before_stop = h.diffs.lock().unwrap().len();

        h.app.stop_listening();
        h.settle(5);
        assert_eq!(
            h.diffs.lock().unwrap().len(),
            log_len_before_stop,
            "cycle {}: no post-stop injection",
            cycle
        );
        assert_eq!(
            replay_diffs(&h.diffs.lock().unwrap().clone()),
            visible_before_stop,
            "cycle {}: visible transcript must be retained verbatim",
            cycle
        );

        // The stopped session's queue must be fully disconnected so even a
        // hypothetical late callback fails to deliver anything.
        assert!(
            tx.send(AudioChunk {
                samples: vec![0.0; 4],
                sample_rate: rate
            })
            .is_err(),
            "cycle {}: late send into the stopped session's queue must fail",
            cycle
        );
    }

    assert!(!h.app.state.listening);
    assert!(!h.app.is_audio_active());
}

/// Invariant 4: model unload cannot resurrect a stopped session — late audio
/// stays rejected and no transcript can appear from the old queue.
#[test]
fn unload_does_not_resurrect_stopped_session() {
    let (chunks, rate) = load_fixture_chunks();
    let mut h = make_harness(harness_config());

    h.action_tx.send(AppAction::StartListening).unwrap();
    h.settle(2);
    let tx1 = h.active_sender();
    for chunk in chunks.iter().take(12) {
        h.send(&tx1, chunk.clone(), rate);
        h.app.tick();
    }
    h.settle(6);
    h.app.stop_listening();
    let frozen = h.diffs.lock().unwrap().len();

    // Unload (as the idle policy would) then try stale audio.
    assert!(h.app.unload_model(), "unload in standby must succeed");
    assert!(!h.app.is_model_loaded());
    assert!(!h.app.is_session_generatively_current());
    h.send_failed_to(
        &tx1,
        chunk_at(&chunks, 13),
        rate,
        "stopped queue must stay disconnected across unload",
    );
    h.settle(5);
    assert_eq!(
        h.diffs.lock().unwrap().len(),
        frozen,
        "unloaded model must not resurrect a stopped session"
    );

    // A later reload serves only future sessions, never the dead one.
    assert!(h.app.ensure_model_loaded().is_ok());
    h.settle(3);
    assert_eq!(h.diffs.lock().unwrap().len(), frozen);
}

impl Harness {
    /// Sends audio but EXPECTS the send itself to fail (disconnected queue).
    fn send_failed_to(
        &self,
        tx: &crossbeam_channel::Sender<AudioChunk>,
        samples: Vec<f32>,
        rate: u32,
        why: &str,
    ) {
        let ok = tx.send(AudioChunk {
            samples,
            sample_rate: rate,
        });
        assert!(ok.is_err(), "{}", why);
    }
}

/// A failed capture open arms no observable session: no mic, no text, and a
/// later start can still recover cleanly.
#[test]
fn failed_start_leaves_clean_engine_state() {
    let (chunks, rate) = load_fixture_chunks();
    let (starter, _txs) = failing_starter();
    let mut h = make_harness_with(harness_config(), starter, _txs);

    assert!(
        h.app.start_listening().is_none(),
        "mic failure must fail start"
    );
    assert!(!h.app.state.listening);
    assert!(!h.app.is_audio_active());
    assert!(!h.app.is_session_generatively_current());
    h.settle(5);
    assert!(
        h.diffs.lock().unwrap().is_empty(),
        "a failed start must never inject text"
    );

    // Even the handed-out sender of the failed session is dead: the engine
    // cancelled it the moment capture failed, releasing the queue.
    let failed_tx = h
        .recorded_senders()
        .pop()
        .expect("failed starter must record the session sender");
    h.send_failed_to(
        &failed_tx,
        chunk_at(&chunks, 0),
        rate,
        "queue of a failed-start session must be disconnected",
    );
    h.settle(3);
    assert!(h.diffs.lock().unwrap().is_empty());

    // Repeated starts remain cleanly failing and engine-quiet.
    for _ in 0..3 {
        assert!(h.app.start_listening().is_none());
        assert!(!h.app.is_session_generatively_current());
    }
    h.settle(4);
    assert!(h.diffs.lock().unwrap().is_empty());
}

/// The staged model resolves as active resident, per-model language options
/// behave exactly as before, and a failed (uninstalled) switch keeps the
/// previously valid selected model.
#[test]
fn language_options_and_resident_model_behavior_unchanged() {
    let mut h = make_harness(harness_config());
    let id = staged_model_id();
    assert_eq!(
        h.app.model_manager.active_model_id.as_deref(),
        Some(id.as_str()),
        "active model must resolve to the staged fixture model"
    );

    assert!(
        h.app.ensure_model_loaded().is_ok(),
        "resident model must load"
    );
    assert!(h.app.is_model_loaded());
    // Model keeps working alive: another ensure is a no-op (no churn).
    assert!(h.app.ensure_model_loaded().is_ok());
    assert!(h.app.is_model_loaded());

    // The staged X-ASR model has no forced-language options: selection is
    // gracefully refused, exactly as before the refactor.
    assert!(
        !h.app.set_language(&id, Some("en-US")),
        "models without language options must refuse selection"
    );
    assert!(
        h.app.is_model_loaded(),
        "refused language must not disturb the resident model"
    );

    // Switching to a nonexistent model fails and retains the active model.
    assert!(
        !h.app.select_model("echolet-does-not-exist"),
        "unknown model selection must fail"
    );
    assert_eq!(
        h.app.model_manager.active_model_id.as_deref(),
        Some(id.as_str()),
        "failed switch must keep the previously valid selected model"
    );
    assert!(h.app.is_model_loaded());
}

/// Invariant 5: pre-existing diff/injection semantics are unchanged
/// (suffix append, in-session tail correction, finalize boundary).
#[test]
fn platform_diff_injection_semantics_unchanged() {
    // Suffix append within one session.
    let mut session = echolet::diff::PartialSession::new();
    let mut log: Vec<(usize, String)> = Vec::new();
    for step in ["hello ", "hello world"] {
        if let Some(diff) = session.update(step) {
            log.push((diff.backspaces, diff.new_suffix.clone()));
        }
    }
    assert_eq!(
        log,
        vec![(0, "hello ".to_string()), (0, "world".to_string())]
    );

    // Tail correction within one session via the same diff->injector pairing.
    let mut session = echolet::diff::PartialSession::new();
    let mut log2: Vec<(usize, String)> = Vec::new();
    for step in ["abc def", "abc dgh"] {
        if let Some(diff) = session.update(step) {
            log2.push((diff.backspaces, diff.new_suffix.clone()));
        }
    }
    let last = log2.last().expect("correction diff");
    assert_eq!(last.0, 2, "tail correction deletes only the revised tail");
    assert_eq!(last.1, "gh");

    // Finalize boundary: committed text is never backspaced by later text.
    let mut session = echolet::diff::PartialSession::new();
    session.update("committed");
    session.finalize();
    let d = session.update("fresh").unwrap();
    assert_eq!(d.backspaces, 0);
    assert_eq!(d.new_suffix, "fresh");
}
