//! Platform-neutral voice session engine: the single owner of one voice
//! session's lifecycle — session identity, the live audio receiver, the
//! partial-diff window, per-session transcript revisions, the
//! delivered-revision watermark, the visible-history text snapshot and the
//! utterance-start timestamp.
//!
//! # Concurrency model
//!
//! At most one voice session is active at any time. Each session is stamped
//! with a monotonically increasing [`SessionGeneration`]; every lifecycle
//! boundary (capture start, stop/cancel, model switch, unload) invalidates
//! every pre-existing token. `begin` NEVER silently replaces a live session
//! and on identifier exhaustion it fails closed instead of wrapping onto an
//! old live id.
//!
//! Pipeline of one session:
//!
//! ```text
//! capture producer ──(fresh unbounded queue)──> session-owned receiver
//!                                                     │
//!                              SessionGeneration-guarded drain + diff
//!                                                     v
//!              TranscriptDelta --accept_delivery watermark--> transcript
//!              sink (TextInjector today, a focused editor on mobile
//!              keyboard adapters)
//! ```
//!
//! - The producer side (e.g. a cpal callback, or a mobile audio callback
//!   adapter) receives the `Sender` half bound to the session that STARTED it.
//!   Each session begins with a brand-new queue; nothing is ever reused across
//!   session boundaries.
//! - [`SessionEngine::cancel`] takes the session and drops its `Receiver`:
//!   buffered-but-undelivered audio is released with it, and a late chunk
//!   produced by a callback racing the capture release cannot enter any
//!   future session — sending into a disconnected queue fails.
//! - ASR decode and transcript application run exclusively on the composition
//!   root thread, gated on generation equality (see
//!   [`SessionGeneration::matches`]). A future platform callback MUST capture
//!   and revalidate the token/revision (via [`SessionEngine::accept_delivery`])
//!   and bound the editor identity before applying any event.
//! - The online recognizer stream is replaced (not merely reset) at session
//!   boundaries by the composition root, so buffered waveform or endpoint
//!   state never leaks.
//!
//! This module depends only on `crate::capture`, `crate::diff`,
//! `crossbeam-channel`, `chrono` and std — no cpal, no Sherpa FFI, no Slint,
//! no OS types — so the same engine can drive a desktop app or a mobile
//! binding unchanged.
//!
//! # Editor binding (future Android/iOS keyboard adapters)
//!
//! An adapter that "binds" a text field for the current session MUST record
//! the [`SessionGeneration`] it was armed with. Before applying any diff it
//! must revalidate, exactly as the desktop composition root does:
//!
//! 1. the token still matches the live session generation (a late callback
//!    from an old stream may otherwise type into a field of a NEWER session),
//! 2. the event passes the contiguous-revision watermark
//!    ([`SessionEngine::accept_delivery`]) — duplicates, gaps and
//!    out-of-order events are rejected because every diff depends on the
//!    previously applied state,
//! 3. the diff only writes new text or replaces the current partial suffix
//!    window — it never retro-edits committed text (`PartialSession::finalize`
//!    clears the suffix window on every segment boundary).

use crate::capture::AudioChunk;
use crate::diff::{DiffAction, PartialSession};
use chrono::{DateTime, Local};
use crossbeam_channel::{unbounded, Receiver, Sender};
use std::fmt;

/// Monotonic identity of one voice-capture session.
///
/// Issued by advancing an [`SessionIssuer`]; comparing a keeper token
/// against the live generation is the sole admission check for transcript
/// delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SessionGeneration(u64);

impl SessionGeneration {
    /// The invalid "no session has started yet" generation.
    pub const fn root() -> Self {
        Self(0)
    }

    pub fn raw(self) -> u64 {
        self.0
    }

    pub fn matches(self, other: Self) -> bool {
        self.0 == other.0
    }
}

/// Fail-closed reasons a new voice session could not begin.
///
/// Both variants arm NOTHING: no new session, no queue, no receiver, and no
/// previously issued identifier is ever reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionStartError {
    /// A voice session is still active; `begin` never silently replaces it.
    AlreadyActive,
    /// The session identifier counter exhausted `u64`; refusing to wrap onto
    /// a previously live id.
    IdentifierOverflow,
}

impl fmt::Display for SessionStartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyActive => write!(f, "a voice session is already active"),
            Self::IdentifierOverflow => {
                write!(
                    f,
                    "session identifier space exhausted; refusing to reuse ids"
                )
            }
        }
    }
}

impl std::error::Error for SessionStartError {}

/// One live transcript event produced by the session's diff window.
///
/// `revision` is per-session, monotonically increasing and WITHOUT gaps:
/// `accept_delivery` advances the delivered watermark only consecutively, so
/// every applied diff is guaranteed to build on the previously applied state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptDelta {
    pub session: SessionGeneration,
    pub revision: u64,
    pub diff: DiffAction,
    pub recognized_text: String,
}

/// Snapshot of one completed (stopped or endpointed) utterance's already
/// visible text. For HISTORY persistence only — never fed back to a text
/// injector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedUtterance {
    pub text: String,
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
}

/// Issues session generations; doubles as the "live session or not" oracle.
///
/// Every call to [`SessionIssuer::begin`] or [`SessionIssuer::invalidate`]
/// makes all previously issued stale, including any token a producer captured
/// earlier. `begin` fails closed on identifier exhaustion.
#[derive(Debug, Default)]
pub struct SessionIssuer {
    counter: u64,
    live: Option<SessionGeneration>,
}

impl SessionIssuer {
    pub fn new() -> Self {
        Self {
            counter: 0,
            live: None,
        }
    }

    /// Starts a new session: returns its fresh token.
    ///
    /// On `u64` overflow the counter is NOT advanced and the previous (never
    /// wrapped) state is kept: fail closed, never reuse an old live id.
    pub fn begin(&mut self) -> Result<SessionGeneration, SessionStartError> {
        let next = self
            .counter
            .checked_add(1)
            .ok_or(SessionStartError::IdentifierOverflow)?;
        self.counter = next;
        let token = SessionGeneration(next);
        self.live = Some(token);
        Ok(token)
    }

    /// Marks any previously issued token (live or stale) as no longer live
    /// without starting a session. Used by stop/cancel before releasing the
    /// capture, and by model switch/unload.
    pub fn invalidate(&mut self) {
        self.live = None;
    }

    /// The live session token, if any.
    pub fn live(&self) -> Option<SessionGeneration> {
        self.live
    }
}

/// Creates the fresh, unshared audio queue for one session.
///
/// A queue is never reused across sessions: the sender is handed to the
/// producer at capture start, the receiver is owned by the
/// [`SessionEngine`] and dropped on stop so late producer sends fail instead
/// of leaking into the next session.
pub fn new_session_audio_queue() -> (Sender<AudioChunk>, Receiver<AudioChunk>) {
    unbounded()
}

/// First revision number each new session's diff window is numbered from.
const FIRST_REVISION: u64 = 1;

/// One live voice session: identity, its audio receiver, its diff window,
/// per-session revision counters, visible text snapshot and utterance start.
struct ActiveSession {
    token: SessionGeneration,
    /// Receiver half of THIS session's queue. Dropped — not merely drained —
    /// on cancel so late producer sends fail.
    audio_rx: Receiver<AudioChunk>,
    partial: PartialSession,
    next_revision: u64,
    /// Watermark of the last contiguously admitted [`TranscriptDelta`].
    delivered_revision: u64,
    /// Latest recognized partial text that has been ADMITTED through
    /// [`SessionEngine::accept_delivery`] — the history snapshot. Text of a
    /// proposed-but-unadmitted delta never lands here.
    delivered_text: String,
    /// When the currently delivered nonempty text first became visible;
    /// `None` while the delivered snapshot is empty.
    delivered_utterance_start: Option<DateTime<Local>>,
}

/// The platform-neutral session engine.
///
/// Single authority for: the active session generation, the live audio
/// receiver, the partial diff window, per-session revisions, transcript
/// event admission and the history snapshot. Composition roots (desktop
/// `App`, future mobile shells) delegate instead of duplicating this state.
#[derive(Default)]
pub struct SessionEngine {
    issuer: SessionIssuer,
    active: Option<ActiveSession>,
}

impl SessionEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a voice session is currently live.
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// The token of the live session, if any.
    pub fn current_generation(&self) -> Option<SessionGeneration> {
        self.active.as_ref().map(|s| s.token)
    }

    /// Whether `token` is the live session's identity.
    pub fn is_current(&self, token: SessionGeneration) -> bool {
        self.current_generation()
            .map_or(false, |live| live.matches(token))
    }

    /// Begins a new voice session.
    ///
    /// Generates a brand-new token, a brand-new audio queue and an empty
    /// diff window. Requires an INACTIVE engine: a still-active session is
    /// never silently replaced. Revisions, delivered watermarks, partial
    /// text and receivers never leak across sessions.
    pub fn begin(&mut self) -> Result<(SessionGeneration, Sender<AudioChunk>), SessionStartError> {
        if self.active.is_some() {
            return Err(SessionStartError::AlreadyActive);
        }
        let token = self.issuer.begin()?;
        let (audio_tx, audio_rx) = new_session_audio_queue();
        self.active = Some(ActiveSession {
            token,
            audio_rx,
            partial: PartialSession::new(),
            next_revision: FIRST_REVISION,
            delivered_revision: 0,
            delivered_text: String::new(),
            delivered_utterance_start: None,
        });
        Ok((token, audio_tx))
    }

    /// Drains the queued audio of the CURRENT session into `accept`.
    ///
    /// Returns whether at least one NONEMPTY chunk was accepted. Only
    /// nonempty chunks reach `accept` (empty chunks are consumed as
    /// no-ops). A stale token has no side effects and returns false.
    pub fn drain_audio(
        &mut self,
        token: SessionGeneration,
        mut accept: impl FnMut(&AudioChunk),
    ) -> bool {
        let Some(active) = self.active.as_mut() else {
            return false;
        };
        if !active.token.matches(token) {
            return false;
        }
        let mut got_audio = false;
        while let Ok(chunk) = active.audio_rx.try_recv() {
            if !chunk.samples.is_empty() {
                accept(&chunk);
                got_audio = true;
            }
        }
        got_audio
    }

    /// Feeds the newly recognized partial `text` through the diff window.
    ///
    /// This is PROPOSAL ONLY: it may update the `PartialSession` diff window,
    /// advance `next_revision` and construct a `TranscriptDelta`. It NEVER
    /// touches the delivered history snapshot (`delivered_text` /
    /// `delivered_utterance_start`) — that changes only on
    /// [`SessionEngine::accept_delivery`].
    ///
    /// Emits a `TranscriptDelta` ONLY for a real diff (unchanged text yields
    /// `None`) and increments the per-session revision exactly once per
    /// event. A stale token is rejected with no side effects. On revision
    /// counter overflow at this boundary the engine fails closed (no delta,
    /// no state change) instead of reusing a revision.
    pub fn update_partial(
        &mut self,
        token: SessionGeneration,
        text: &str,
    ) -> Option<TranscriptDelta> {
        let active = self.active.as_mut()?;
        if !active.token.matches(token) {
            return None;
        }
        let next = active.next_revision.checked_add(1)?;
        let diff = active.partial.update(text)?;
        let delta = TranscriptDelta {
            session: token,
            revision: active.next_revision,
            diff,
            recognized_text: text.to_string(),
        };
        active.next_revision = next;
        Some(delta)
    }

    /// Atomic transcript admission gate.
    ///
    /// Validates, atomically, that the event's generation is STILL current
    /// AND that its revision is the contiguous successor of the delivered
    /// watermark. Duplicate, gapped, out-of-order or stale-generation events
    /// are rejected with no state change (and no editor writes — the caller
    /// must never apply a rejected diff, because it depends on previous
    /// state).
    ///
    /// On acceptance the delivered history snapshot is set EXACTLY to the
    /// delta's proposed full text — including the empty string, which
    /// represents an erasure of the visible partial. A nonempty accepted
    /// text stamps the utterance start the first time; an empty one resets
    /// the timestamp.
    ///
    /// Boundary note: this admission happens immediately before the
    /// composition root writes to the editor sink. The desktop
    /// `TextInjector::apply_diff` is synchronous and returns void, so
    /// admission is NOT proof of the OS-level editor state; it only gates
    /// the write attempt and advances the delivered watermark.
    pub fn accept_delivery(&mut self, delta: &TranscriptDelta) -> bool {
        let Some(active) = self.active.as_mut() else {
            return false;
        };
        if !active.token.matches(delta.session) {
            return false;
        }
        if active.delivered_revision.checked_add(1) != Some(delta.revision) {
            return false;
        }
        active.delivered_revision = delta.revision;
        active.delivered_text = delta.recognized_text.clone();
        if delta.recognized_text.is_empty() {
            active.delivered_utterance_start = None;
        } else if active.delivered_utterance_start.is_none() {
            active.delivered_utterance_start = Some(Local::now());
        }
        true
    }

    /// Endpoint segmentation of the CURRENT session while it stays live
    /// (listening continues).
    ///
    /// Resets only the active diff window and the delivered-text snapshot
    /// (including its utterance-start timestamp); it does NOT invalidate the
    /// session, disconnect the
    /// queue or persist history. Returns the already-visible utterance for
    /// the caller's existing history logic when that window was nonempty;
    /// `None` otherwise. Stale tokens yield `None`.
    ///
    /// Only ADMITTED, already-visible text is consumed: a `TranscriptDelta`
    /// that was generated but never accepted never reaches history — no
    /// implicit acceptance at the endpoint.
    pub fn finish_segment(&mut self, token: SessionGeneration) -> Option<CompletedUtterance> {
        let Some(active) = self.active.as_mut() else {
            return None;
        };
        if !active.token.matches(token) {
            return None;
        }
        let last_text = std::mem::take(&mut active.delivered_text);
        let start = active.delivered_utterance_start.take();
        active.partial.finalize();
        if last_text.is_empty() {
            return None;
        }
        let end = Local::now();
        Some(CompletedUtterance {
            text: last_text,
            start: start.unwrap_or(end),
            end,
        })
    }

    /// Cancels the live session.
    ///
    /// FIRST detaches and drops the session's receiver — releasing all
    /// buffered audio with it, so late producer sends fail and no future
    /// session can observe them — and THEN invalidates the session identity
    /// so the receiver drop and stale-acceptance fencing cannot race. The
    /// partial window is reset WITHOUT calling any text injector (already
    /// visible text stays untouched by design), and the last ADMITTED,
    /// already-visible utterance is returned for HISTORY ONLY (callers
    /// choose whether to persist it). Proposed-but-unadmitted partial
    /// inference is dropped, never flushed into History. No final
    /// decode/append happens here.
    ///
    /// Idempotent: a second `cancel` on an inactive engine returns `None`
    /// and has no side effects.
    pub fn cancel(&mut self) -> Option<CompletedUtterance> {
        let mut active = self.active.take()?;
        self.issuer.invalidate();
        let end = Local::now();
        let last_text = std::mem::take(&mut active.delivered_text);
        if last_text.is_empty() {
            return None;
        }
        Some(CompletedUtterance {
            text: last_text,
            start: active.delivered_utterance_start.unwrap_or(end),
            end,
        })
    }
}

impl fmt::Debug for SessionEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut dbg = f.debug_struct("SessionEngine");
        dbg.field("is_active", &self.is_active());
        if let Some(token) = self.current_generation() {
            dbg.field("current_generation", &token.raw());
        } else {
            dbg.field("current_generation", &Option::<u64>::None);
        }
        dbg.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Applies a delta to a simulated editor sink; reconstructs the visible
    /// text the way a TextInjector consumer would.
    fn apply_to_sink(visible: &mut Vec<char>, delta: &TranscriptDelta) {
        for _ in 0..delta.diff.backspaces {
            visible.pop();
        }
        visible.extend(delta.diff.new_suffix.chars());
    }

    fn sink_text(visible: &[char]) -> String {
        visible.iter().collect()
    }

    fn chunk_of(n: usize) -> AudioChunk {
        AudioChunk {
            samples: vec![0.125; n],
            sample_rate: 16000,
        }
    }

    /// Ten-plus full begin/cancel cycles: fresh identity and queue each
    /// time, stale producer rejected after cancel, released audio never
    /// visible to the next session.
    #[test]
    fn begin_and_cancel_ten_cycles_issue_fresh_identity_and_queues() {
        let mut engine = SessionEngine::new();
        let mut last_raw = 0u64;
        for cycle in 0..12u64 {
            assert!(
                !engine.is_active(),
                "cycle {}: engine starts inactive",
                cycle
            );
            let (token, tx) = engine.begin().expect("begin on idle engine");
            assert!(engine.is_active());
            assert_eq!(engine.current_generation(), Some(token));
            assert!(engine.is_current(token));
            // Monotonic, non-reused session ids.
            assert!(
                token.raw() > last_raw,
                "cycle {}: session id must strictly increase ({} > {})",
                cycle,
                token.raw(),
                last_raw
            );
            last_raw = token.raw();

            // The fresh queue is live for the current session.
            tx.send(chunk_of(4)).expect("fresh queue is open");
            let mut accepted = 0;
            assert!(
                engine.drain_audio(token, |_| accepted += 1),
                "cycle {}: queued audio is drained",
                cycle
            );
            assert_eq!(accepted, 1);

            let snapshot = engine.cancel();
            assert_eq!(
                snapshot, None,
                "cycle {}: empty session has no history",
                cycle
            );
            assert!(!engine.is_active(), "cycle {}: cancel deactivates", cycle);
            assert_eq!(engine.current_generation(), None);
            // The old producer's queue is disconnected and dead.
            assert!(
                tx.send(chunk_of(4)).is_err(),
                "cycle {}: stale producer send must fail after cancel",
                cycle
            );

            // Next session observes none of the dead producer's audio.
            let (token2, _tx2) = engine.begin().expect("begin after cancel");
            let mut seen_by_new = 0;
            assert!(
                !engine.drain_audio(token2, |_| seen_by_new += 1),
                "cycle {}: cancelled session's queued audio must be released, not replayed",
                cycle
            );
            assert_eq!(seen_by_new, 0);
            engine.cancel();
        }
    }

    #[test]
    fn begin_twice_fails_closed_and_keeps_the_live_session() {
        let mut engine = SessionEngine::new();
        let (token, tx) = engine.begin().unwrap();
        assert!(matches!(
            engine.begin(),
            Err(SessionStartError::AlreadyActive)
        ));
        // The pre-existing session is untouched by the failed begin.
        assert!(engine.is_active());
        assert_eq!(engine.current_generation(), Some(token));
        tx.send(chunk_of(4)).unwrap();
        assert!(engine.drain_audio(token, |_| {}));
    }

    #[test]
    fn identifier_overflow_fails_closed_without_wrapping_onto_a_live_id() {
        let mut engine = SessionEngine::new();
        engine.issuer.counter = u64::MAX - 1;
        engine.issuer.live = Some(SessionGeneration(u64::MAX - 1));
        let (token, _tx) = engine.begin().expect("the final id is still allocatable");
        assert_eq!(token, SessionGeneration(u64::MAX));
        engine.cancel();

        assert_eq!(
            engine.begin().err(),
            Some(SessionStartError::IdentifierOverflow),
            "exhausted id space must fail closed"
        );
        assert!(!engine.is_active(), "no session may be armed on overflow");
        assert_ne!(
            engine.current_generation(),
            Some(SessionGeneration(u64::MAX)),
            "the overflowed id is never resurrected"
        );
    }

    #[test]
    fn drain_audio_only_accepts_nonempty_chunks_of_the_current_session() {
        let mut engine = SessionEngine::new();
        assert!(!engine.drain_audio(SessionGeneration(999), |_| panic!(
            "a stale token must never reach the accept callback"
        )));
        let (token, tx) = engine.begin().unwrap();
        tx.send(chunk_of(3)).unwrap();
        tx.send(AudioChunk {
            samples: Vec::new(),
            sample_rate: 16000,
        })
        .unwrap();
        let mut sizes = Vec::new();
        assert!(engine.drain_audio(token, |c| sizes.push(c.samples.len())));
        assert_eq!(
            sizes,
            vec![3],
            "empty chunks are consumed but never accepted"
        );
        // Queue fully drained: nothing else to accept, no side effects.
        let mut again = 0;
        assert!(!engine.drain_audio(token, |_| again += 1));
        assert_eq!(again, 0);
    }

    #[test]
    fn update_partial_emits_only_real_diffs_with_monotonic_revisions() {
        let mut engine = SessionEngine::new();
        let (token, _tx) = engine.begin().unwrap();

        // Unchanged text: no event, no revision burn.
        assert_eq!(engine.update_partial(token, ""), None);

        let first = engine
            .update_partial(token, "hello")
            .expect("first real diff emits an event");
        assert_eq!(first.session, token);
        assert_eq!(first.revision, 1);
        assert_eq!(
            first.diff,
            DiffAction {
                backspaces: 0,
                new_suffix: "hello".to_string()
            }
        );
        assert_eq!(first.recognized_text, "hello");

        assert_eq!(
            engine.update_partial(token, "hello"),
            None,
            "unchanged partial must not emit"
        );

        let second = engine.update_partial(token, "hello world").unwrap();
        assert_eq!(second.revision, 2, "revisions increment once per event");
        assert_eq!(
            second.diff,
            DiffAction {
                backspaces: 0,
                new_suffix: " world".to_string()
            }
        );

        // In-session tail correction keeps PartialSession semantics exactly
        // (baseline diff.rs behavior is unaffected).
        let third = engine.update_partial(token, "hello deleted").unwrap();
        assert_eq!(third.revision, 3);
        assert_eq!(third.diff.backspaces, 5, "only 'world' is revised");
        assert_eq!(third.diff.new_suffix, "deleted");

        // Stale token: no events, no revision movement, no side effects.
        assert_eq!(engine.update_partial(SessionGeneration(77), "evil"), None);
        assert!(engine.is_current(token));
    }

    #[test]
    fn accept_delivery_admits_only_contiguous_revisions() {
        let mut engine = SessionEngine::new();
        let (token, _tx) = engine.begin().unwrap();
        let d1 = engine.update_partial(token, "a").unwrap();
        let d2 = engine.update_partial(token, "ab").unwrap();
        let d3 = engine.update_partial(token, "abc").unwrap();

        // The NEXT revision out of order must be rejected...
        assert!(
            !engine.accept_delivery(&d2),
            "out-of-order delivery must be rejected"
        );
        // ...and admission stays fenced at the next contiguous revision.
        assert!(engine.accept_delivery(&d1));
        // Duplicate after acceptance is rejected.
        assert!(!engine.accept_delivery(&d1));
        // A revision gap must never be admitted later.
        assert!(!engine.accept_delivery(&d3));
        // But the contiguous successor still passes.
        assert!(engine.accept_delivery(&d2));
        assert!(
            !engine.accept_delivery(&d2),
            "no late duplicates after watermark"
        );
    }

    #[test]
    fn stale_generation_events_are_rejected_after_cancel_and_new_session() {
        let mut engine = SessionEngine::new();
        let (token1, _tx1) = engine.begin().unwrap();
        let d1 = engine.update_partial(token1, "old").unwrap();
        assert!(engine.accept_delivery(&d1));
        engine.cancel();

        // Events of the dead session are rejected in every gate.
        assert_eq!(engine.update_partial(token1, "late"), None);
        assert!(!engine.accept_delivery(&d1), "stale generation rejected");

        let (token2, _tx2) = engine.begin().unwrap();
        assert_ne!(token1, token2);
        let fresh = engine.update_partial(token2, "new").unwrap();
        assert_eq!(fresh.revision, 1, "revisions restart per session");
        assert!(
            !engine.accept_delivery(&d1),
            "an old-generation event must never slip into the new session"
        );
        assert!(engine.accept_delivery(&fresh));
    }

    #[test]
    fn finish_segment_opens_a_new_partial_window_same_session() {
        let mut engine = SessionEngine::new();
        let (token, _tx) = engine.begin().unwrap();
        let delta = engine.update_partial(token, "hello").unwrap();
        assert!(engine.accept_delivery(&delta));
        let completed = engine.finish_segment(token).expect("nonempty window");
        assert_eq!(completed.text, "hello");
        assert!(completed.start <= completed.end);

        // The SAME session continues appended, never retro-editing.
        let next = engine.update_partial(token, "world").unwrap();
        assert_eq!(
            next.diff,
            DiffAction {
                backspaces: 0,
                new_suffix: "world".to_string()
            }
        );
        assert!(engine.is_active(), "finish_segment keeps the session live");

        // Resetting clears the snapshot: a completed-out-of-order window
        // yields None, then the just-completed window is returned once.
        assert!(engine.accept_delivery(&next));
        let completed2 = engine.finish_segment(token).expect("window nonempty");
        assert_eq!(completed2.text, "world");
        assert!(
            engine.finish_segment(token).is_none(),
            "an empty window finishes with None"
        );
    }

    #[test]
    fn finish_segment_rejects_stale_tokens() {
        let mut engine = SessionEngine::new();
        let (token, _tx) = engine.begin().unwrap();
        let d1 = engine.update_partial(token, "text").unwrap();
        assert!(engine.accept_delivery(&d1));
        assert!(engine.finish_segment(SessionGeneration(7)).is_none());
        assert!(engine.finish_segment(SessionGeneration::root()).is_none());
        // Session state is untouched by the stale requests.
        let d = engine.update_partial(token, "text more").unwrap();
        assert!(engine.accept_delivery(&d));

        engine.cancel();
        let (token2, _tx2) = engine.begin().unwrap();
        assert!(engine.finish_segment(token).is_none());
        assert!(engine.finish_segment(token2).is_none(), "empty window");
    }

    #[test]
    fn cancel_returns_seen_utterance_then_is_a_noop() {
        let mut engine = SessionEngine::new();
        assert_eq!(engine.cancel(), None, "cancel on idle engine is None");

        let (token, _tx) = engine.begin().unwrap();
        let delta = engine.update_partial(token, "committed voice").unwrap();
        assert!(engine.accept_delivery(&delta));
        let snapshot = engine.cancel().expect("visible utterance snapshot");
        assert_eq!(snapshot.text, "committed voice");
        assert!(snapshot.start <= snapshot.end);
        assert!(!engine.is_active());

        // Idempotent, no side effects: no text, no resurrection.
        assert_eq!(engine.cancel(), None);
        assert!(!engine.is_active());
        assert_eq!(
            engine.update_partial(token, "late"),
            None,
            "no update after invalidation"
        );
    }

    /// A delta that was generated but never accepted must never reach
    /// history: cancel over a session with only unadmitted inference
    /// records nothing.
    #[test]
    fn cancel_with_only_generated_deltas_records_nothing() {
        let mut engine = SessionEngine::new();
        let (token, _tx) = engine.begin().unwrap();
        let _delta = engine.update_partial(token, "never delivered").unwrap();
        assert_eq!(
            engine.cancel(),
            None,
            "unadmitted inference must not enter history"
        );
        // The generated delta is now stale on every gate.
        assert!(!engine.is_active());
        assert!(engine.begin().is_ok());
    }

    /// Accepted 'hello' plus a still-unaccepted 'hello world' proposal:
    /// stop must record exactly the delivered text.
    #[test]
    fn cancel_records_only_last_accepted_text_not_pending_proposals() {
        let mut engine = SessionEngine::new();
        let (token, _tx) = engine.begin().unwrap();
        let d1 = engine.update_partial(token, "hello").unwrap();
        assert!(engine.accept_delivery(&d1));
        let _d2 = engine.update_partial(token, "hello world").unwrap();
        let snapshot = engine.cancel().expect("accepted text is in history");
        assert_eq!(snapshot.text, "hello");
    }

    /// Revision 2 proposed and delivered BEFORE revision 1: revision 2 is
    /// rejected (gap), revision 1 is then admitted, and the snapshot shows
    /// only revision 1's text with revision 2 still fenced out.
    #[test]
    fn out_of_order_revision_is_rejected_and_delivered_snapshot_stays_contiguous() {
        let mut engine = SessionEngine::new();
        let (token, _tx) = engine.begin().unwrap();
        let d1 = engine.update_partial(token, "first").unwrap();
        let d2 = engine.update_partial(token, "first second").unwrap();
        assert!(
            !engine.accept_delivery(&d2),
            "revision 2 before revision 1 must be rejected"
        );
        assert!(engine.accept_delivery(&d1));
        let snapshot = engine.cancel().expect("revision 1 was admitted");
        assert_eq!(
            snapshot.text, "first",
            "rejecting revision 2 must not leave any text or timestamp of it behind"
        );
    }

    /// Duplicates and stale-generation replays never mutate the delivered
    /// snapshot, even when the duplicate carries different text.
    #[test]
    fn duplicate_and_stale_deltas_never_mutate_the_snapshot() {
        let mut engine = SessionEngine::new();
        let (token, _tx) = engine.begin().unwrap();
        let d1 = engine.update_partial(token, "hello").unwrap();
        assert!(engine.accept_delivery(&d1));
        // A forged "duplicate" whose text differs from what was delivered:
        // it shares the rejected revision, so it mutates nothing.
        let mut forged = d1.clone();
        forged.recognized_text = "tampered".to_string();
        assert!(!engine.accept_delivery(&forged));
        engine.cancel();
        // A delivered delta of a dead session replayed against a fresh one:
        assert_eq!(
            engine.cancel(),
            None,
            "stale generation must not produce a new snapshot"
        );
        let (token2, _tx2) = engine.begin().unwrap();
        let d = engine.update_partial(token2, "new text").unwrap();
        assert!(!engine.accept_delivery(&d1), "old generation rejected");
        assert!(engine.accept_delivery(&d));
        let snapshot = engine.cancel().expect("fresh admitted text");
        assert_eq!(snapshot.text, "new text", "snapshot matches fresh session");
    }

    /// An accepted EMPTY delta (the recognizer proposing an erasure of the
    /// visible partial) must clear both the delivered text AND its
    /// utterance-start timestamp, so no history entry is produced at Stop.
    #[test]
    fn accepted_empty_delta_clears_snapshot_and_utterance_start() {
        let mut engine = SessionEngine::new();
        let (token, _tx) = engine.begin().unwrap();
        let d1 = engine.update_partial(token, "partial").unwrap();
        assert!(engine.accept_delivery(&d1));
        // The erasure proposal: PartialSession emits a pure-backspace diff.
        let erase = engine.update_partial(token, "").unwrap();
        assert!(
            erase.diff.backspaces > 0 && erase.diff.new_suffix.is_empty(),
            "the erasure diff must clear the visible partial"
        );
        assert!(engine.accept_delivery(&erase));
        assert!(
            engine.active.as_ref().unwrap().delivered_text.is_empty(),
            "accepted empty text must overwrite the delivered snapshot exactly"
        );
        assert!(
            engine
                .active
                .as_ref()
                .unwrap()
                .delivered_utterance_start
                .is_none(),
            "an empty delivered snapshot has no utterance start"
        );
        // Both endpoints see an empty snapshot: no history entry at all.
        assert!(engine.finish_segment(token).is_none(), "endpoint no entry");
        let snapshot = engine.cancel();
        assert_eq!(snapshot, None, "stop records no entry for erased text");
        assert!(
            !engine.is_active(),
            "cancel still invalidates the session even with an empty snapshot"
        );
    }

    /// Stop must never implicitly accept a pending unaccepted draft: the
    /// editor sink keeps the previously admitted text and the late event is
    /// dropped on every gate.
    #[test]
    fn stop_preserves_visible_sink_and_drops_late_draft_events() {
        let mut engine = SessionEngine::new();
        let mut visible: Vec<char> = Vec::new();
        let (token, _tx) = engine.begin().unwrap();
        let d1 = engine.update_partial(token, "kept").unwrap();
        assert!(engine.accept_delivery(&d1));
        apply_to_sink(&mut visible, &d1);
        let d2 = engine.update_partial(token, "kept draft tail").unwrap();
        // Stop while the draft is still unaccepted:
        let snapshot = engine.cancel().expect("visible text for history");
        assert_eq!(snapshot.text, "kept");
        assert_eq!(
            sink_text(&visible),
            "kept",
            "Stop must not backspace or extend already visible editor text"
        );
        // Late drafts and their deliveries cannot resurrect anything.
        assert_eq!(engine.update_partial(token, "late"), None);
        assert!(!engine.accept_delivery(&d2), "dead session draft rejected");
        assert_eq!(sink_text(&visible), "kept");
        // A fresh session starts the sink from current editor state without
        // retro-editing it.
        let (token2, _tx2) = engine.begin().unwrap();
        let fresh = engine.update_partial(token2, "next").unwrap();
        assert!(engine.accept_delivery(&fresh));
        apply_to_sink(&mut visible, &fresh);
        assert_eq!(sink_text(&visible), "keptnext");
    }

    /// Accepted mid-utterance corrections apply to the sink like any other
    /// contiguous delta, and finish_segment resets the diff window so the
    /// next segment's first diff is a pure append.
    #[test]
    fn accepted_correction_and_following_segment_reset_work_together() {
        let mut engine = SessionEngine::new();
        let mut visible: Vec<char> = Vec::new();
        let (token, _tx) = engine.begin().unwrap();
        let d1 = engine.update_partial(token, "teh quick").unwrap();
        assert!(engine.accept_delivery(&d1));
        apply_to_sink(&mut visible, &d1);
        assert_eq!(sink_text(&visible), "teh quick");
        let d2 = engine.update_partial(token, "the quick").unwrap();
        assert!(engine.accept_delivery(&d2));
        apply_to_sink(&mut visible, &d2);
        assert_eq!(sink_text(&visible), "the quick");
        assert_eq!(
            engine.finish_segment(token).expect("admitted segment").text,
            "the quick"
        );
        // New segment: pure append, never a retro-edit.
        let d3 = engine.update_partial(token, "brown fox").unwrap();
        assert!(engine.accept_delivery(&d3));
        apply_to_sink(&mut visible, &d3);
        assert_eq!(sink_text(&visible), "the quickbrown fox");
        assert_eq!(
            d3.diff.backspaces, 0,
            "segment boundary finalized the window"
        );
    }

    #[test]
    fn stop_retains_delivered_partial_and_emits_no_replacement() {
        let mut engine = SessionEngine::new();
        // Simulated TextInjector sink: what the editor currently shows.
        let mut visible: Vec<char> = Vec::new();

        let (token, _tx) = engine.begin().unwrap();
        let delta = engine.update_partial(token, "hello worl").unwrap();
        assert!(engine.accept_delivery(&delta));
        apply_to_sink(&mut visible, &delta);
        let delta = engine.update_partial(token, "hello world").unwrap();
        assert!(engine.accept_delivery(&delta));
        apply_to_sink(&mut visible, &delta);
        assert_eq!(sink_text(&visible), "hello world");

        // Stop: token invalidated, receiver dropped, snapshot for history.
        let snapshot = engine.cancel().expect("already visible text for history");
        assert_eq!(snapshot.text, "hello world");

        // The sink log is untouched, and the engine emits NO replacement:
        assert_eq!(engine.update_partial(token, ""), None);
        assert_eq!(engine.update_partial(token, "anything"), None);
        assert!(!engine.accept_delivery(&delta));
        assert_eq!(sink_text(&visible), "hello world");

        // The engine is clean for a fresh start.
        let (token2, _tx2) = engine.begin().unwrap();
        let fresh = engine.update_partial(token2, "next").unwrap();
        assert_eq!(
            fresh.diff,
            DiffAction {
                backspaces: 0,
                new_suffix: "next".to_string()
            },
            "a fresh session must not backspace text that is already visible"
        );
        apply_to_sink(&mut visible, &fresh);
        assert_eq!(sink_text(&visible), "hello worldnext");
    }
}
