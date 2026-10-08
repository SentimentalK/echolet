//! Deterministic behavioral regression tests for voice-session lifecycle
//! isolation (stop/cancel ownership, session generations, fresh audio queue
//! and stream boundaries).
//!
//! Invariants under test (user-confirmed contract):
//! 1. Stop/Cancel retains the ALREADY VISIBLE transcript (including partial)
//!    and never writes new text on or after stop.
//! 2. All as-yet-undelivered audio of a stopped session is dropped: late
//!    chunks from an old capture must never reach (let alone transcribe
//!    into) a newer session.
//! 3. Stop is idempotent; the next Start is a clean generation — diff window
//!    reset, fresh recognizer stream, fresh audio queue, no stale editor
//!    delivery.
//! 4. Model unload cannot resurrect a stopped session.
//! 5. Pre-existing platform text injection behavior (diff semantics via the
//!    TextInjector adapter) is unchanged.
//!
//! Tests use the real bundled model (same convention as `test_stream.rs`) and
//! a scripted `AudioStarter` that hands test code the per-session sender, so
//! "late producer chunks" can be simulated deterministically.

use crossbeam_channel::unbounded;
use echolet::actions::AppAction;
use echolet::app::App;
use echolet::audio::{AudioChunk, AudioSource, AudioStarter};
use echolet::config::EcholetConfig;
use echolet::platform::{PlatformHandle, PlatformRuntime, PlatformView, TextInjector};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Isolates the user home for the duration of a test so the real desktop
/// config (e.g. model preload enabled) cannot leak in. Returns a guard that
/// restores and cleans up.
fn isolated_home(prefix: &str) -> Option<HomeGuard> {
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "echolet-session-test-{}-{}",
        prefix,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).ok()?;
    std::env::set_var("ECHOLET_USER_HOME", &dir);
    Some(HomeGuard(dir))
}

struct HomeGuard(PathBuf);

impl Drop for HomeGuard {
    fn drop(&mut self) {
        std::env::remove_var("ECHOLET_USER_HOME");
        let _ = std::fs::remove_dir_all(&self.0);
    }
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

/// Splits the bundled fixture wav into 0.2s mono chunks at 16 kHz.
/// `None` when the fixture cannot be read (test reports the skip).
fn load_fixture_chunks() -> Option<(Vec<Vec<f32>>, u32)> {
    let model_dir = echolet::paths::default_model_dir();
    let wav_path = model_dir.join("test_wavs/0.wav");
    let bytes = std::fs::read(&wav_path).ok()?;
    if bytes.len() <= 44 {
        return None;
    }
    let pcm = &bytes[44..];
    let samples: Vec<f32> = pcm
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
        .collect();
    let chunk_size = 3200; // 0.2s at 16 kHz
    let chunks: Vec<Vec<f32>> = samples.chunks(chunk_size).map(<[f32]>::to_vec).collect();
    if chunks.is_empty() {
        return None;
    }
    Some((chunks, 16000))
}

fn chunk_at(chunks: &[Vec<f32>], i: usize) -> Vec<f32> {
    chunks[i % chunks.len()].clone()
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
}

fn make_harness(config: EcholetConfig) -> Harness {
    let (action_tx, action_rx) = unbounded::<AppAction>();
    let txs = Arc::new(Mutex::new(Vec::new()));
    let txs_for_starter = txs.clone();
    let starter: AudioStarter = Box::new(move |tx| {
        txs_for_starter.lock().unwrap().push(tx);
        Ok(Box::new(()) as Box<dyn AudioSource>)
    });
    let diffs = Arc::new(Mutex::new(Vec::new()));
    let listening = Arc::new(Mutex::new(Vec::new()));
    let platform = PlatformRuntime {
        injector: Box::new(CapturedDiffs(diffs.clone())),
        handle: Box::new(FakeHandle {
            listening: listening.clone(),
        }),
        _resources: Box::new(()),
    };
    let app = App::new_with_starter_and_config(
        platform,
        Some(action_tx.clone()),
        action_rx,
        None,
        starter,
        None,
        Some(config),
    )
    .expect("harness app must construct");
    Harness {
        app,
        action_tx,
        txs,
        diffs,
        listening,
    }
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

fn harness_config() -> EcholetConfig {
    let mut config = EcholetConfig::default();
    config.preload_model_on_startup = false;
    config
}

fn skip(reason: &str) {
    eprintln!("[skip] test requires bundled model fixture: {}", reason);
}

/// Invariants 1 + 2 + the Stop-half of 3:
/// a visible partial from session 1 survives stop unchanged; nothing is
/// injected after stop; late audio from the stopped session is rejected
/// (its queue is disconnected); the next Start arms a fresh queue.
#[test]
fn stop_retains_visible_partial_and_rejects_late_audio() {
    let Some(_home) = isolated_home("retain") else {
        skip("temp dir");
        return;
    };
    let Some((chunks, rate)) = load_fixture_chunks() else {
        skip("test_wavs/0.wav");
        return;
    };
    let mut h = make_harness(harness_config());

    // Session 1: produce a visible partial.
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
        "session 1 must have injected at least one diff"
    );
    let visible_at_stop = replay_diffs(&log);
    assert!(!visible_at_stop.is_empty());

    // Stop: generation invalidated before capture release, queue dropped.
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
    let session2_log = &log[log_len_pre_session2..];
    if !session2_log.is_empty() {
        let first = &session2_log[0];
        assert_eq!(
            first.0, 0,
            "first diff of a new session must not retro-edit the committed partial"
        );
    }
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
    let Some(_home) = isolated_home("idempotent") else {
        skip("temp dir");
        return;
    };
    let Some((chunks, rate)) = load_fixture_chunks() else {
        skip("test_wavs/0.wav");
        return;
    };
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
}

/// Invariant 3 (rapid cycles): ten rapid stop/start cycles — fresh queue per
/// start, no old text reappearing, no post-stop injections.
#[test]
fn ten_rapid_stop_start_cycles_stay_isolated() {
    let Some(_home) = isolated_home("cycles") else {
        skip("temp dir");
        return;
    };
    let Some((chunks, rate)) = load_fixture_chunks() else {
        skip("test_wavs/0.wav");
        return;
    };
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

        let before = h.diffs.lock().unwrap().len();
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
        let dead = AudioChunk {
            samples: vec![0.0; 4],
            sample_rate: rate,
        };
        assert!(
            tx.send(dead).is_err(),
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
    let Some(_home) = isolated_home("unload") else {
        skip("temp dir");
        return;
    };
    let Some((chunks, rate)) = load_fixture_chunks() else {
        skip("test_wavs/0.wav");
        return;
    };
    let mut h = make_harness(harness_config());

    h.action_tx.send(AppAction::StartListening).unwrap();
    h.settle(2);
    let tx1 = h.active_sender();
    if chunks.is_empty() {
        skip("empty fixture");
        return;
    }
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
    let late = AudioChunk {
        samples: chunk_at(&chunks, 13),
        sample_rate: rate,
    };
    assert!(
        tx1.send(late).is_err(),
        "stopped queue must stay disconnected across unload"
    );
    h.settle(5);
    assert_eq!(
        h.diffs.lock().unwrap().len(),
        frozen,
        "unloaded model must not resurrect a stopped session"
    );

    //A later reload serves only future sessions, never the dead one.
    assert!(h.app.ensure_model_loaded().is_ok());
    h.settle(3);
    assert_eq!(h.diffs.lock().unwrap().len(), frozen);
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
