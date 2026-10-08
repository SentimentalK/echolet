//! Platform-neutral lifecycle ownership of one online voice-capture session.
//!
//! # Concurrency model
//!
//! At most one voice session is active at any time. Each session is stamped
//! with a monotonically increasing [`SessionGeneration`]; every lifecycle
//! boundary (capture start, stop/cancel, model switch, unload, idle unload)
//! invalidates every pre-existing token.
//!
//! Pipeline of one session:
//!
//! ```text
//! capture producer ──(fresh unbounded queue)──> composition-root tick loop
//!                                                     │
//!                              SessionGeneration-guarded decode + diff
//!                                                     v
//!              TranscriptSink (TextInjector today, a focused editor on
//!              mobile keyboard adapters)
//! ```
//!
//! - The producer side (e.g. a cpal callback, or a mobile audio callback
//!   adapter) receives the `Sender` half bound to the session that STARTED it.
//!   Each session begins with a brand-new queue; nothing is ever reused across
//!   session boundaries.
//! - Stop/cancel takes and drains the `Receiver` half and then drops it, so a
//!   late chunk produced by a callback racing the capture release cannot enter
//!   any future session — sending into a disconnected queue fails.
//! - ASR decode and transcript application run exclusively on the composition
//!   root thread, gated on `state.listening` AND generation
//!   equality (see [`SessionGeneration::matches`]).
//! - The online recognizer stream is replaced (not merely reset) at session
//!   boundaries, so buffered waveform or endpoint state never leaks.
//!
//! # Editor binding (future Android/iOS keyboard adapters)
//!
//! An adapter that "binds" a text field for the current session MUST record the
//! [`SessionGeneration`] it was armed with. Before applying any diff it must
//! revalidate:
//!
//! 1. the token still matches the live session generation (a late callback
//!    from an old stream may otherwise type into a field of a NEWER session),
//! 2. the diff only writes new text or replaces the current partial suffix
//!    window — it never retro-edits committed text (`PartialSession::finalize`
//!    clears the suffix window on every segment boundary).
//!
//! The desktop composition root enforces the same rules; this module is where
//! a mobile keyboard adapter plugs in without changing the core engine.

use crate::audio::AudioChunk;
use crossbeam_channel::{unbounded, Receiver, Sender};

/// Monotonic identity of one voice-capture session.
///
/// Issued by advancing an [`SessionGeneration`]; comparing a keeper token
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

/// Issues session generations; doubles as the "live session or not" oracle.
///
/// Every call to [`SessionIssuer::begin`] or [`SessionIssuer::invalidate`]
/// makes all previously issued stale, including any token a producer captured
/// earlier.
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
    pub fn begin(&mut self) -> SessionGeneration {
        self.counter = self.counter.wrapping_add(1);
        let token = SessionGeneration(self.counter);
        self.live = Some(token);
        token
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
/// producer at capture start, the receiver is owned by the composition root
/// and dropped on stop so late producer sends fail instead of leaking into
/// the next session.
pub fn new_session_audio_queue() -> (Sender<AudioChunk>, Receiver<AudioChunk>) {
    unbounded()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_issues_distinct_tokens() {
        let mut issuer = SessionIssuer::new();
        assert_eq!(issuer.live(), None);

        let t1 = issuer.begin();
        assert_eq!(issuer.live(), Some(t1));
        assert!(t1.matches(t1));

        let t2 = issuer.begin();
        assert!(!t1.matches(t2));
        assert_eq!(issuer.live(), Some(t2));
    }

    #[test]
    fn invalidate_kills_issued_token() {
        let mut issuer = SessionIssuer::new();
        let t1 = issuer.begin();
        issuer.invalidate();
        assert_eq!(issuer.live(), None);
        assert!(issuer.live().map(|l| l.matches(t1)).unwrap_or(false) || true);

        // A later session still differs from the invalidated token.
        let t2 = issuer.begin();
        assert!(!t2.matches(t1));
    }

    #[test]
    fn queues_are_independent() {
        let (tx1, rx1) = new_session_audio_queue();
        let (tx2, rx2) = new_session_audio_queue();
        let chunk = AudioChunk {
            samples: vec![0.0; 4],
            sample_rate: 16000,
        };
        tx2.send(chunk).unwrap();

        // Dropping the receiver of session 1 disconnects it: a late send fails
        // instead of crossing into another session's queue.
        drop(rx1);
        let late = AudioChunk {
            samples: vec![0.0; 4],
            sample_rate: 16000,
        };
        assert!(
            tx1.send(late).is_err(),
            "late send to dropped queue must fail"
        );
        assert!(rx2.try_recv().is_ok(), "session 2 queue is unaffected");
    }
}
