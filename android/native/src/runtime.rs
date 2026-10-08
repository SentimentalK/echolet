//! Platform-neutral owner of one Android process's native ASR runtime.
//!
//! This module contains NO JNI and no Android types, so the exact runtime logic
//! the APK relies on is unit-testable on the host against real model fixtures.
//! The thin JNI translation layer in `lib.rs` locks this single
//! [`AndroidRuntime`] and maps results/errors to the Kotlin contract.

use crossbeam_channel::Sender;
use echolet::asr::{OnlineRecognizer, OnlineStream};
use echolet::capture::AudioChunk;
use echolet::session::{SessionEngine, SessionStartError, TranscriptDelta};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// The bridge accepts only the pinned pipeline sample rate: 16 kHz mono.
pub const REQUIRED_SAMPLE_RATE: u32 = 16000;

/// Upper bound on one `nativeFeed` chunk in samples (2 s at 16 kHz), matching
/// the Kotlin side which feeds 3200-frame chunks.
pub const MAX_CHUNK_SAMPLES: usize = 32000;

/// Why an input could not be processed. The JNI layer maps the two categories
/// to `IllegalArgumentException` (malformed input) and `IllegalStateException`
/// (lifecycle or native failures).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeError {
    /// A session is already active; there is exactly one native session owner.
    AlreadyActive,
    /// The process-global handle counter exhausted `u64`; fails closed.
    HandleOverflow,
    /// Stale, unknown or already-closed handle (for example a late feed after
    /// close). No output is produced.
    StaleHandle,
    /// `nativeFeed` was called with a sample rate other than 16 kHz.
    UnsupportedSampleRate { got: u32 },
    /// A sample was not finite (NaN/infinite) or the chunk length exceeds the
    /// documented bound.
    InvalidSamples { reason: String },
    /// The model directory or its contents are unusable by
    /// [`OnlineRecognizer`].
    Model(String),
    /// Native audio queue is gone (internal invariant); the call must fail
    /// closed.
    QueueDisconnected,
    /// Non-lifecycle native wrapper failure carrying its own message text.
    Owned(String),
}

impl BridgeError {
    /// `true` when the failure is a caller-input contract violation.
    pub fn is_input_error(&self) -> bool {
        matches!(
            self,
            BridgeError::UnsupportedSampleRate { .. } | BridgeError::InvalidSamples { .. }
        )
    }

    pub fn message(&self) -> String {
        match self {
            BridgeError::AlreadyActive => {
                "a voice session is already active; close it before opening a new one".to_string()
            }
            BridgeError::HandleOverflow => {
                "session handle space exhausted; refusing to reuse identifiers".to_string()
            }
            BridgeError::StaleHandle => "unknown or closed session handle".to_string(),
            BridgeError::UnsupportedSampleRate { got } => format!(
                "unsupported sample rate {}: the pipeline requires {}",
                got, REQUIRED_SAMPLE_RATE
            ),
            BridgeError::InvalidSamples { reason } => {
                format!("invalid audio samples: {}", reason)
            }
            BridgeError::Model(e) => format!("model open failed: {}", e),
            BridgeError::QueueDisconnected => {
                "audio queue disconnected; the session was invalidated".to_string()
            }
            BridgeError::Owned(m) => m.clone(),
        }
    }
}

/// Process-global, lazily initialized owner of the native ASR runtime.
///
/// This is a LIMITED feasibility owner for Phase 0-A: all JNI entry points
/// lock this mutex. `SessionEngine` persists between opens so the generation
/// counter never resets across open/close cycles.
static RUNTIME: OnceLock<Mutex<AndroidRuntime>> = OnceLock::new();

/// The process-global runtime mutex; every JNI entry point must hold it.
pub fn shared_runtime() -> &'static Mutex<AndroidRuntime> {
    RUNTIME.get_or_init(|| Mutex::new(AndroidRuntime::new()))
}

/// The one native ASR runtime for the process.
pub struct AndroidRuntime {
    engine: SessionEngine,
    active_handle: Option<u64>,
    next_handle: u64,
    resident_model: Option<(PathBuf, Arc<OnlineRecognizer>)>,
    stream: Option<OnlineStream>,
    current_audio_tx: Option<Sender<AudioChunk>>,
}

impl Default for AndroidRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl AndroidRuntime {
    pub fn new() -> Self {
        Self {
            engine: SessionEngine::new(),
            active_handle: None,
            next_handle: 0,
            resident_model: None,
            stream: None,
            current_audio_tx: None,
        }
    }

    /// The live Android handle, if a session is open (read-only for tests).
    pub fn active_handle(&self) -> Option<u64> {
        self.active_handle
    }

    /// Whether the engine has a live generation (read-only for tests).
    pub fn session_active(&self) -> bool {
        self.engine.is_active()
    }

    /// Opens a new ASR session over `model_dir` and returns its handle.
    ///
    /// The resident recognizer is only reused when the canonical model
    /// directory is identical to the one currently held; anything else loads a
    /// fresh recognizer first and swaps it in only after the whole start
    /// succeeded, so a failure leaves the previous usable recognizer intact.
    /// The handle is never published before every step succeeded.
    pub fn open(&mut self, model_dir: &Path) -> Result<u64, BridgeError> {
        if self.active_handle.is_some() || self.engine.is_active() {
            return Err(BridgeError::AlreadyActive);
        }

        let canonical = model_dir
            .canonicalize()
            .map_err(|e| BridgeError::Model(format!("cannot resolve {:?}: {}", model_dir, e)))?;

        // Refuse an unusable model directory before touching any live state.
        if !canonical.join("model.json").exists() {
            return Err(BridgeError::Model(format!(
                "missing model.json in {:?}",
                canonical
            )));
        }

        // Allocate the handle first (fail closed on overflow) so no partial
        // state accumulates below when the counter is exhausted.
        let handle = self
            .next_handle
            .checked_add(1)
            .ok_or(BridgeError::HandleOverflow)?;

        // 1. Recognizer: reuse the resident one only for the identical
        //    canonical directory; otherwise load (fail closed) without
        //    disturbing whatever the process currently holds.
        let recognizer: Arc<OnlineRecognizer> = match &self.resident_model {
            Some((dir, resident)) if *dir == canonical => Arc::clone(resident),
            _ => Arc::new(OnlineRecognizer::new(&canonical).map_err(BridgeError::Model)?),
        };

        // 2. Fresh stream per open; buffered waveform never leaks sessions.
        let stream = recognizer.create_stream().map_err(BridgeError::Model)?;

        // 3. Session engine begin; on identity failure nothing is live yet.
        let (_token, tx) = match self.engine.begin() {
            Ok(ok) => ok,
            Err(SessionStartError::AlreadyActive) => return Err(BridgeError::AlreadyActive),
            Err(SessionStartError::IdentifierOverflow) => return Err(BridgeError::HandleOverflow),
        };

        // Whole start succeeded: publish, all-or-nothing.
        self.next_handle = handle;
        self.resident_model = Some((canonical, recognizer));
        self.stream = Some(stream);
        self.current_audio_tx = Some(tx);
        self.active_handle = Some(handle);
        Ok(handle)
    }

    /// Feeds one chunk of mono PCM and returns the UTF-8 JSON event array of
    /// the JNI wire contract (stable for the next IME slice):
    ///
    /// * partial: `{"kind":"partial","session":u64,"revision":u64,
    ///   "backspaces":usize,"suffix":string,"text":string}` — only deltas
    ///   admitted through `accept_delivery` are returned
    /// * endpoint: `{"kind":"endpoint","session":u64,"text":string}` —
    ///   omitted when this feed produced no completed text
    /// * no progress: `[]`
    ///
    /// The receiving editor reconstructs visible text in CHARACTER units
    /// exactly as `src/diff.rs` (backspaces then suffix), never by UTF-8
    /// byte length. The diagnostic UI is not an InputConnection.
    pub fn feed(
        &mut self,
        handle: u64,
        samples: &[f32],
        sample_rate: u32,
    ) -> Result<String, BridgeError> {
        if Some(&handle) != self.active_handle.as_ref() {
            return Err(BridgeError::StaleHandle);
        }
        if sample_rate != REQUIRED_SAMPLE_RATE {
            return Err(BridgeError::UnsupportedSampleRate { got: sample_rate });
        }
        if samples.len() > MAX_CHUNK_SAMPLES {
            return Err(BridgeError::InvalidSamples {
                reason: format!(
                    "chunk has {} samples; the maximum is {}",
                    samples.len(),
                    MAX_CHUNK_SAMPLES
                ),
            });
        }
        if let Some(bad) = samples.iter().position(|s| !s.is_finite()) {
            return Err(BridgeError::InvalidSamples {
                reason: format!("non-finite sample at index {}", bad),
            });
        }

        let generation = self
            .engine
            .current_generation()
            .ok_or(BridgeError::StaleHandle)?;

        let sender = self
            .current_audio_tx
            .as_ref()
            .ok_or(BridgeError::QueueDisconnected)?;
        sender
            .send(AudioChunk {
                samples: samples.to_vec(),
                sample_rate: REQUIRED_SAMPLE_RATE,
            })
            .map_err(|_| BridgeError::QueueDisconnected)?;

        let stream = self.stream.as_ref().ok_or(BridgeError::QueueDisconnected)?;

        // Disjoint borrows: the engine drains the CURRENT session's queue
        // into the stream; a stale generation has no side effects.
        let mut got_audio = false;
        self.engine.drain_audio(generation, |chunk| {
            if !chunk.samples.is_empty() {
                got_audio = true;
                stream.accept_waveform(chunk.sample_rate as i32, &chunk.samples);
            }
        });

        let mut events: Vec<Value> = Vec::new();
        if got_audio {
            stream.decode_all_ready();
            let current_text = stream.get_result();
            let is_endpoint = stream.is_endpoint();

            if let Some(delta) = self.engine.update_partial(generation, &current_text) {
                // Admission first; a rejected (stale/out-of-order/duplicate)
                // event must never reach the diagnostic sink.
                if self.engine.accept_delivery(&delta) {
                    events.push(partial_event(generation.raw(), &delta));
                }
            }

            if is_endpoint {
                let completed = self.engine.finish_segment(generation);
                if let Some(completed) = completed {
                    events.push(json!({
                        "kind": "endpoint",
                        "session": generation.raw(),
                        "text": completed.text,
                    }));
                }
                // Only the live session's own stream is reset (identical to
                // the desktop composition root).
                if let Some(stream) = self.stream.as_ref() {
                    stream.reset();
                }
            }
        }

        Ok(serde_json::to_string(&events).unwrap_or_else(|_| "[]".to_string()))
    }

    /// Closes a session. A duplicate or already-closed handle is a harmless
    /// no-op; a late feed after close fails with [`BridgeError::StaleHandle`].
    ///
    /// `SessionEngine::cancel` runs FIRST: it drops the queue receiver so
    /// buffered audio is released and every later stale generation is
    /// rejected. History stays off in this slice: the cancel snapshot is
    /// intentionally dropped and no extra final text is appended at close.
    pub fn close(&mut self, handle: u64) {
        if self.active_handle != Some(handle) {
            return;
        }
        // 1. Cancel first: disconnect the queue and invalidate the generation.
        let _ = self.engine.cancel();
        // 2. Clear everything except the resident model, which stays warm for
        //    the next open.
        self.current_audio_tx = None;
        self.stream = None;
        self.active_handle = None;
    }
}

/// Partial wire event from an admitted [`TranscriptDelta`].
fn partial_event(session: u64, delta: &TranscriptDelta) -> Value {
    json!({
        "kind": "partial",
        "session": session,
        "revision": delta.revision,
        "backspaces": delta.diff.backspaces,
        "suffix": delta.diff.new_suffix,
        "text": delta.recognized_text,
    })
}
