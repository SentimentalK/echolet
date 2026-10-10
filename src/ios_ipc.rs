//! Cross-process IPC protocol and stale-session safety logic for iOS App and
//! Keyboard Extension communication (Protocol v2).
//!
//! # Protocol Overview
//!
//! In iOS, the containing application and keyboard extension run in separate
//! sandbox processes. Communication is mediated via App Groups:
//! 1. App Group `UserDefaults` holds dedicated single-writer keys:
//!    - [`APP_EPOCH_KEY`]: written ONLY by Containing App on process launch (`echolet.app.epoch.v2`).
//!    - [`KEYBOARD_REQUEST_KEY`]: written ONLY by Keyboard Extension (`echolet.keyboard.request.v2`).
//!    - [`APP_RESPONSE_KEY`]: written ONLY by Containing App (`echolet.app.response.v2`).
//! 2. Darwin Notifications (`notify_post` / `notify_register_dispatch`) provide
//!    edge-trigger wake hints when a key changes:
//!    - [`DARWIN_NOTIFICATION_REQUEST`]: posted by Keyboard when writing a request.
//!    - [`DARWIN_NOTIFICATION_RESPONSE`]: posted by App when writing a response.
//!
//! # Strict Process-Epoch & Monotonic Intent Protocol (v2)
//!
//! - **Wire Version 2**: Upgraded from v1 because v1 did not enforce cross-process
//!   launch fencing or global intent sequencing, leaving durable App Group keys
//!   vulnerable to cached replay. v1 payloads are rejected fail-closed.
//! - **App Process Epoch**: The containing app host mints an unpredictable UUID on
//!   each process launch and publishes it to `echolet.app.epoch.v2` before admitting
//!   keyboard commands. Both [`KeyboardRequest`] and [`AppResponse`] envelopes carry
//!   a mandatory non-blank `app_epoch`.
//!   `AppAdmissionGate` checks equality against its host-provided `app_epoch` on EVERY
//!   command. A cached request from a previous app run is permanently rejected even
//!   if the app is later foregrounded or authorized.
//!   *Security note*: `app_epoch` is a process-liveness and replay fence, not an
//!   authorization token against hostile apps.
//! - **Global Keyboard Intent Sequence**: In addition to per-session `sequence`,
//!   every request carries `intent_sequence` (`u64 >= 1`), which monotonically
//!   increases ACROSS editor session boundaries. The Keyboard Extension reserves
//!   this sequence in persistent storage. `AppAdmissionGate` maintains a watermark;
//!   any delayed or replayed request with `intent_sequence <= last_admitted_intent`
//!   is rejected, preventing old START, STOP, or CANCEL from preempting or disrupting
//!   newer sessions.
//! - **Single-Slot Loss-Tolerant Tombstone**: If the latest slot contains STOP or CANCEL
//!   with no known active session, it is treated as an idle tombstone that advances
//!   the intent watermark without arming the microphone.
//! - **Clean Handoff / Session Replacement**: A valid fresh START (matching epoch,
//!   `intent_sequence > watermark`, `sequence == 1`) replaces a prior active session
//!   cleanly ([`AppAdmissionOutcome::ReplacedPriorSession`]). The host adapter must
//!   cancel the old recognizer and stop audio before starting the new session.
//! - **Keyboard Response Admission**: [`KeyboardAdmissionGate`] validates matching
//!   session, matching `app_epoch`, valid correlation with an issued command, and
//!   monotonically increasing response `revision`. An epoch change immediately
//!   fences pending responses. Command history is bounded/compacted to prevent
//!   unbounded memory growth while preserving correlation for multi-revision updates.
//!
//! This module contains NO UIKit/Foundation/cpal dependencies and compiles on all
//! platforms (macOS, Linux, iOS, Android).

use serde::{Deserialize, Serialize};

/// Wire protocol version 2.
pub const PROTOCOL_VERSION: u32 = 2;

/// App Group UserDefaults key written only by Containing App publishing its launch process epoch UUID.
pub const APP_EPOCH_KEY: &str = "echolet.app.epoch.v2";

/// App Group UserDefaults key written only by the Keyboard Extension.
pub const KEYBOARD_REQUEST_KEY: &str = "echolet.keyboard.request.v2";

/// App Group UserDefaults key written only by the Containing App.
pub const APP_RESPONSE_KEY: &str = "echolet.app.response.v2";

/// Darwin notification posted by Keyboard Extension when a new request is written.
pub const DARWIN_NOTIFICATION_REQUEST: &str = "com.echolet.ipc.request.v2";

/// Darwin notification posted by Containing App when a new response snapshot is written.
pub const DARWIN_NOTIFICATION_RESPONSE: &str = "com.echolet.ipc.response.v2";

/// Maximum number of issued commands retained in KeyboardAdmissionGate to bound memory.
pub const MAX_ISSUED_COMMANDS_HISTORY: usize = 32;

/// Commands issued from the Keyboard Extension to the App.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcCommand {
    Start,
    Stop,
    Cancel,
}

/// Request envelope written by the Keyboard Extension (v2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyboardRequest {
    pub protocol_version: u32,
    pub app_epoch: String,
    pub intent_sequence: u64,
    pub session_id: String,
    pub sequence: u64,
    pub request_id: String,
    pub command: IpcCommand,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_timestamp_ms: Option<u64>,
}

impl KeyboardRequest {
    /// Creates a new request validating non-empty identifiers, valid epoch, and monotonic sequences.
    pub fn new(
        app_epoch: impl Into<String>,
        intent_sequence: u64,
        session_id: impl Into<String>,
        sequence: u64,
        request_id: impl Into<String>,
        command: IpcCommand,
        client_timestamp_ms: Option<u64>,
    ) -> Result<Self, IpcValidationError> {
        let app_epoch = app_epoch.into();
        let session_id = session_id.into();
        let request_id = request_id.into();

        if app_epoch.trim().is_empty() {
            return Err(IpcValidationError::EmptyAppEpoch);
        }
        if intent_sequence == 0 {
            return Err(IpcValidationError::InvalidIntentSequence(intent_sequence));
        }
        if session_id.trim().is_empty() {
            return Err(IpcValidationError::EmptySessionId);
        }
        if request_id.trim().is_empty() {
            return Err(IpcValidationError::EmptyRequestId);
        }
        if sequence == 0 {
            return Err(IpcValidationError::InvalidSequence(sequence));
        }

        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            app_epoch,
            intent_sequence,
            session_id,
            sequence,
            request_id,
            command,
            client_timestamp_ms,
        })
    }

    /// Validates self against wire invariants.
    pub fn validate(&self) -> Result<(), IpcValidationError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(IpcValidationError::UnsupportedProtocolVersion(
                self.protocol_version,
            ));
        }
        if self.app_epoch.trim().is_empty() {
            return Err(IpcValidationError::EmptyAppEpoch);
        }
        if self.intent_sequence == 0 {
            return Err(IpcValidationError::InvalidIntentSequence(
                self.intent_sequence,
            ));
        }
        if self.session_id.trim().is_empty() {
            return Err(IpcValidationError::EmptySessionId);
        }
        if self.request_id.trim().is_empty() {
            return Err(IpcValidationError::EmptyRequestId);
        }
        if self.sequence == 0 {
            return Err(IpcValidationError::InvalidSequence(self.sequence));
        }
        Ok(())
    }

    /// Serializes request to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Deserializes and validates request from JSON string.
    pub fn from_json_str(json: &str) -> Result<Self, IpcValidationError> {
        let req: Self = serde_json::from_str(json)
            .map_err(|e| IpcValidationError::MalformedJson(e.to_string()))?;
        req.validate()?;
        Ok(req)
    }
}

/// Lifecycle state reported by the App to the Keyboard Extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcAppState {
    Idle,
    Requested,
    Preparing,
    Listening,
    Processing,
    Completed,
    Blocked,
}

/// Response envelope written by the Containing App (v2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppResponse {
    pub protocol_version: u32,
    pub app_epoch: String,
    pub session_id: String,
    pub acknowledged_request_id: String,
    pub acknowledged_sequence: u64,
    pub revision: u64,
    pub state: IpcAppState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recognized_text: Option<String>,
    pub is_final: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_timestamp_ms: Option<u64>,
}

impl AppResponse {
    /// Creates a new AppResponse validating non-empty identifiers, valid epoch, and version.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        app_epoch: impl Into<String>,
        session_id: impl Into<String>,
        acknowledged_request_id: impl Into<String>,
        acknowledged_sequence: u64,
        revision: u64,
        state: IpcAppState,
        recognized_text: Option<String>,
        is_final: bool,
        error_code: Option<String>,
        server_timestamp_ms: Option<u64>,
    ) -> Result<Self, IpcValidationError> {
        let app_epoch = app_epoch.into();
        let session_id = session_id.into();
        let acknowledged_request_id = acknowledged_request_id.into();

        if app_epoch.trim().is_empty() {
            return Err(IpcValidationError::EmptyAppEpoch);
        }
        if session_id.trim().is_empty() {
            return Err(IpcValidationError::EmptySessionId);
        }
        if acknowledged_request_id.trim().is_empty() {
            return Err(IpcValidationError::EmptyRequestId);
        }
        if acknowledged_sequence == 0 {
            return Err(IpcValidationError::InvalidAcknowledgedSequence(0));
        }
        if revision == 0 {
            return Err(IpcValidationError::InvalidRevision(0));
        }

        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            app_epoch,
            session_id,
            acknowledged_request_id,
            acknowledged_sequence,
            revision,
            state,
            recognized_text,
            is_final,
            error_code,
            server_timestamp_ms,
        })
    }

    /// Validates self against wire invariants.
    pub fn validate(&self) -> Result<(), IpcValidationError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(IpcValidationError::UnsupportedProtocolVersion(
                self.protocol_version,
            ));
        }
        if self.app_epoch.trim().is_empty() {
            return Err(IpcValidationError::EmptyAppEpoch);
        }
        if self.session_id.trim().is_empty() {
            return Err(IpcValidationError::EmptySessionId);
        }
        if self.acknowledged_request_id.trim().is_empty() {
            return Err(IpcValidationError::EmptyRequestId);
        }
        if self.acknowledged_sequence == 0 {
            return Err(IpcValidationError::InvalidAcknowledgedSequence(
                self.acknowledged_sequence,
            ));
        }
        if self.revision == 0 {
            return Err(IpcValidationError::InvalidRevision(self.revision));
        }
        Ok(())
    }

    /// Serializes response to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Deserializes and validates response from JSON string.
    pub fn from_json_str(json: &str) -> Result<Self, IpcValidationError> {
        let resp: Self = serde_json::from_str(json)
            .map_err(|e| IpcValidationError::MalformedJson(e.to_string()))?;
        resp.validate()?;
        Ok(resp)
    }
}

/// Errors raised during validation of IPC envelopes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcValidationError {
    UnsupportedProtocolVersion(u32),
    EmptyAppEpoch,
    EmptySessionId,
    EmptyRequestId,
    InvalidIntentSequence(u64),
    InvalidSequence(u64),
    InvalidAcknowledgedSequence(u64),
    InvalidRevision(u64),
    MalformedJson(String),
}

impl std::fmt::Display for IpcValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedProtocolVersion(v) => write!(f, "unsupported protocol version: {v}"),
            Self::EmptyAppEpoch => write!(f, "app_epoch cannot be empty"),
            Self::EmptySessionId => write!(f, "session_id cannot be empty"),
            Self::EmptyRequestId => write!(f, "request_id cannot be empty"),
            Self::InvalidIntentSequence(s) => {
                write!(f, "invalid intent_sequence (must be >= 1): {s}")
            }
            Self::InvalidSequence(s) => write!(f, "invalid sequence (must be >= 1): {s}"),
            Self::InvalidAcknowledgedSequence(s) => {
                write!(f, "invalid acknowledged_sequence (must be >= 1): {s}")
            }
            Self::InvalidRevision(r) => write!(f, "invalid revision (must be >= 1): {r}"),
            Self::MalformedJson(msg) => write!(f, "malformed JSON: {msg}"),
        }
    }
}

impl std::error::Error for IpcValidationError {}

/// Rejection reasons for App-side request admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppAdmissionRejection {
    InvalidPayload(IpcValidationError),
    StaleAppEpoch {
        incoming_epoch: String,
        active_epoch: String,
    },
    NonMonotonicIntentSequence {
        incoming_intent: u64,
        last_admitted_intent: u64,
    },
    StaleSessionCommand {
        incoming_session_id: String,
        active_session_id: Option<String>,
        command: IpcCommand,
    },
    ActiveSessionConflict {
        incoming_session_id: String,
        active_session_id: String,
    },
    NonMonotonicSequence {
        session_id: String,
        incoming_seq: u64,
        last_seq: u64,
    },
    CommandOrderViolation {
        session_id: String,
        command: IpcCommand,
        reason: &'static str,
    },
    ColdBootFenceReject {
        reason: &'static str,
    },
}

impl std::fmt::Display for AppAdmissionRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPayload(e) => write!(f, "invalid payload: {e}"),
            Self::StaleAppEpoch {
                incoming_epoch,
                active_epoch,
            } => write!(
                f,
                "stale app epoch '{incoming_epoch}' does not match active process epoch '{active_epoch}'"
            ),
            Self::NonMonotonicIntentSequence {
                incoming_intent,
                last_admitted_intent,
            } => write!(
                f,
                "non-monotonic intent_sequence: incoming {incoming_intent} <= last admitted {last_admitted_intent}"
            ),
            Self::StaleSessionCommand {
                incoming_session_id,
                active_session_id,
                command,
            } => write!(
                f,
                "stale {command:?} for session '{incoming_session_id}' (active session: {active_session_id:?}) rejected"
            ),
            Self::ActiveSessionConflict {
                incoming_session_id,
                active_session_id,
            } => write!(
                f,
                "START for session '{incoming_session_id}' conflicts with existing active session '{active_session_id}'"
            ),
            Self::NonMonotonicSequence {
                session_id,
                incoming_seq,
                last_seq,
            } => write!(
                f,
                "non-monotonic sequence for session '{session_id}': incoming {incoming_seq} <= last {last_seq}"
            ),
            Self::CommandOrderViolation {
                session_id,
                command,
                reason,
            } => write!(
                f,
                "invalid command order {command:?} for session '{session_id}': {reason}"
            ),
            Self::ColdBootFenceReject { reason } => {
                write!(f, "request rejected by cold-boot fence: {reason}")
            }
        }
    }
}

impl std::error::Error for AppAdmissionRejection {}

/// State of an active session within the App request admission gate.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ActiveSessionState {
    Listening,
    Ended,
}

/// Outcome of admitting an incoming request on the App side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppAdmissionOutcome {
    /// Request admitted to the existing or current session.
    Admitted,
    /// Request was a fresh START for a new editor session that replaced
    /// a prior unstopped/stranded session. The prior session is retired.
    ReplacedPriorSession { retired_session_id: String },
    /// Request was a STOP or CANCEL with no active session to stop;
    /// safe tombstone that advanced the intent watermark without arming the microphone.
    TombstoneIgnored { command: IpcCommand },
}

/// Producer/App-side admission gate (v2).
///
/// Ensures that incoming requests obey process epoch and intent boundaries:
/// 1. Must be constructed with an explicit, non-blank host `app_epoch`.
///    Rejects requests whose `app_epoch` differs from this instance.
/// 2. Enforces strictly monotonic `intent_sequence` (`intent > last_admitted_intent`).
/// 3. A `START` command initiates a new session if no live session is active,
///    or transitions cleanly if the previous session has ended.
/// 4. If a prior session was active, a fresh authenticated `START` with higher
///    intent_sequence and sequence == 1 cleanly retires the prior session
///    (`ReplacedPriorSession`).
/// 5. A delayed or replayed request with lower or equal intent_sequence cannot
///    preempt an active session or terminate a newer session.
/// 6. A `STOP` or `CANCEL` arriving when no session is active is admitted as a
///    `TombstoneIgnored` that advances the intent watermark safely without starting audio.
/// 7. Cold boot fencing fails closed until explicitly authorized, and even after
///    authorization, cached requests from a prior process run remain rejected by epoch.
#[derive(Debug)]
pub struct AppAdmissionGate {
    app_epoch: String,
    active_session_id: Option<String>,
    session_state: Option<ActiveSessionState>,
    last_sequence: u64,
    last_intent_sequence: u64,
    cold_boot_armed: bool,
    last_applied_request_id: Option<String>,
}

impl AppAdmissionGate {
    /// Initializes an AppAdmissionGate with the current process host epoch.
    ///
    /// Requires a non-empty `app_epoch`. Cold boot mode is armed by default.
    pub fn cold_boot(app_epoch: impl Into<String>) -> Result<Self, IpcValidationError> {
        let app_epoch = app_epoch.into();
        if app_epoch.trim().is_empty() {
            return Err(IpcValidationError::EmptyAppEpoch);
        }
        Ok(Self {
            app_epoch,
            active_session_id: None,
            session_state: None,
            last_sequence: 0,
            last_intent_sequence: 0,
            cold_boot_armed: true,
            last_applied_request_id: None,
        })
    }

    /// Initializes an AppAdmissionGate with current process host epoch, un-armed (already authorized).
    pub fn with_epoch(app_epoch: impl Into<String>) -> Result<Self, IpcValidationError> {
        let mut gate = Self::cold_boot(app_epoch)?;
        gate.cold_boot_armed = false;
        Ok(gate)
    }

    /// Authorizes the gate after boot (e.g. when the containing app is foregrounded
    /// or explicitly verified by the host adapter).
    pub fn authorize_boot(&mut self) {
        self.cold_boot_armed = false;
    }

    /// Whether the gate is currently armed in cold-boot mode.
    pub fn is_cold_boot_armed(&self) -> bool {
        self.cold_boot_armed
    }

    /// Current App process epoch.
    pub fn app_epoch(&self) -> &str {
        &self.app_epoch
    }

    /// Watermark of the last admitted intent sequence.
    pub fn last_intent_sequence(&self) -> u64 {
        self.last_intent_sequence
    }

    /// Last applied request ID admitted by the gate.
    pub fn last_applied_request_id(&self) -> Option<&str> {
        self.last_applied_request_id.as_deref()
    }

    /// Active editor session ID if currently armed/listening.
    pub fn active_session_id(&self) -> Option<&str> {
        self.active_session_id.as_deref()
    }

    /// Evaluates and admits or rejects a keyboard request.
    pub fn admit_request(
        &mut self,
        request: &KeyboardRequest,
    ) -> Result<AppAdmissionOutcome, AppAdmissionRejection> {
        if let Err(err) = request.validate() {
            return Err(AppAdmissionRejection::InvalidPayload(err));
        }

        // 1. Process epoch equality check: MUST match current process launch epoch
        if request.app_epoch != self.app_epoch {
            return Err(AppAdmissionRejection::StaleAppEpoch {
                incoming_epoch: request.app_epoch.clone(),
                active_epoch: self.app_epoch.clone(),
            });
        }

        // 2. Cold-boot fence check: do not autonomously execute cached requests upon restart
        if self.cold_boot_armed {
            return Err(AppAdmissionRejection::ColdBootFenceReject {
                reason: "app cold-boot fence active; cannot execute unverified cached request",
            });
        }

        // 3. Global intent sequence monotonic check across sessions
        if request.intent_sequence <= self.last_intent_sequence {
            return Err(AppAdmissionRejection::NonMonotonicIntentSequence {
                incoming_intent: request.intent_sequence,
                last_admitted_intent: self.last_intent_sequence,
            });
        }

        // 4. Duplicate request check
        if let Some(ref last_req) = self.last_applied_request_id {
            if last_req == &request.request_id {
                return Err(AppAdmissionRejection::CommandOrderViolation {
                    session_id: request.session_id.clone(),
                    command: request.command,
                    reason: "duplicate request_id already applied",
                });
            }
        }

        match request.command {
            IpcCommand::Start => {
                if let Some(ref current_id) = self.active_session_id {
                    if current_id == &request.session_id {
                        // Duplicate or re-start on same session
                        return Err(AppAdmissionRejection::CommandOrderViolation {
                            session_id: request.session_id.clone(),
                            command: IpcCommand::Start,
                            reason: "session already started",
                        });
                    }

                    // A new session arrives (different session_id).
                    // If sequence == 1 and intent_sequence > last_intent_sequence:
                    // Atomically replace prior session identity and arm new session.
                    if request.sequence == 1 {
                        let retired = current_id.clone();
                        self.active_session_id = Some(request.session_id.clone());
                        self.session_state = Some(ActiveSessionState::Listening);
                        self.last_sequence = request.sequence;
                        self.last_intent_sequence = request.intent_sequence;
                        self.last_applied_request_id = Some(request.request_id.clone());
                        return Ok(AppAdmissionOutcome::ReplacedPriorSession {
                            retired_session_id: retired,
                        });
                    }

                    // If sequence > 1 for a new session while prior session is still active:
                    if self.session_state != Some(ActiveSessionState::Ended) {
                        return Err(AppAdmissionRejection::ActiveSessionConflict {
                            incoming_session_id: request.session_id.clone(),
                            active_session_id: current_id.clone(),
                        });
                    }
                }

                // New session starts cleanly (no prior session active)
                self.active_session_id = Some(request.session_id.clone());
                self.session_state = Some(ActiveSessionState::Listening);
                self.last_sequence = request.sequence;
                self.last_intent_sequence = request.intent_sequence;
                self.last_applied_request_id = Some(request.request_id.clone());
                Ok(AppAdmissionOutcome::Admitted)
            }
            IpcCommand::Stop | IpcCommand::Cancel => match &self.active_session_id {
                Some(current_id) if current_id == &request.session_id => {
                    if request.sequence <= self.last_sequence {
                        return Err(AppAdmissionRejection::NonMonotonicSequence {
                            session_id: request.session_id.clone(),
                            incoming_seq: request.sequence,
                            last_seq: self.last_sequence,
                        });
                    }
                    if self.session_state == Some(ActiveSessionState::Ended) {
                        return Err(AppAdmissionRejection::CommandOrderViolation {
                            session_id: request.session_id.clone(),
                            command: request.command,
                            reason: "session already ended",
                        });
                    }

                    self.last_sequence = request.sequence;
                    self.last_intent_sequence = request.intent_sequence;
                    self.session_state = Some(ActiveSessionState::Ended);
                    self.last_applied_request_id = Some(request.request_id.clone());
                    Ok(AppAdmissionOutcome::Admitted)
                }
                Some(_) => {
                    // Stale command for an older session while a different session is active:
                    // Intent was already checked, but it targets a non-active session.
                    Err(AppAdmissionRejection::StaleSessionCommand {
                        incoming_session_id: request.session_id.clone(),
                        active_session_id: self.active_session_id.clone(),
                        command: request.command,
                    })
                }
                None => {
                    // Single-slot loss-tolerant tombstone:
                    // STOP or CANCEL arrived with higher intent_sequence, but no session is currently active.
                    // Safely advance the intent watermark and record request_id, without starting audio.
                    self.last_intent_sequence = request.intent_sequence;
                    self.last_applied_request_id = Some(request.request_id.clone());
                    Ok(AppAdmissionOutcome::TombstoneIgnored {
                        command: request.command,
                    })
                }
            },
        }
    }

    /// Mark active session as ended (e.g. when speech recognition completes or stops internally).
    pub fn end_active_session(&mut self) {
        self.session_state = Some(ActiveSessionState::Ended);
    }

    /// Clear all active session state without resetting epoch or intent watermark.
    pub fn reset_session(&mut self) {
        self.active_session_id = None;
        self.session_state = None;
        self.last_sequence = 0;
        self.last_applied_request_id = None;
    }
}

/// Rejection reasons for Keyboard-side response admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyboardAdmissionRejection {
    InvalidPayload(IpcValidationError),
    StaleAppEpoch {
        expected_epoch: String,
        received_epoch: String,
    },
    MismatchedSession {
        expected_session_id: String,
        received_session_id: String,
    },
    SessionInactive,
    NonMonotonicRevision {
        session_id: String,
        received_revision: u64,
        last_accepted_revision: u64,
    },
    ResurrectionAttemptAfterClose {
        session_id: String,
    },
    RequestCorrelationMismatch {
        session_id: String,
        acknowledged_request_id: String,
        acknowledged_sequence: u64,
        reason: &'static str,
    },
}

impl std::fmt::Display for KeyboardAdmissionRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPayload(e) => write!(f, "invalid response payload: {e}"),
            Self::StaleAppEpoch {
                expected_epoch,
                received_epoch,
            } => write!(
                f,
                "response app_epoch mismatch: expected '{expected_epoch}', received '{received_epoch}'"
            ),
            Self::MismatchedSession {
                expected_session_id,
                received_session_id,
            } => write!(
                f,
                "session mismatch: expected '{expected_session_id}', received '{received_session_id}'"
            ),
            Self::SessionInactive => write!(f, "keyboard session is currently inactive"),
            Self::NonMonotonicRevision {
                session_id,
                received_revision,
                last_accepted_revision,
            } => write!(
                f,
                "non-monotonic revision for session '{session_id}': received {received_revision} <= last {last_accepted_revision}"
            ),
            Self::ResurrectionAttemptAfterClose { session_id } => write!(
                f,
                "late response for session '{session_id}' after session was closed/finalized"
            ),
            Self::RequestCorrelationMismatch {
                session_id,
                acknowledged_request_id,
                acknowledged_sequence,
                reason,
            } => write!(
                f,
                "request correlation mismatch for session '{session_id}' (ack_req: '{acknowledged_request_id}', ack_seq: {acknowledged_sequence}): {reason}"
            ),
        }
    }
}

impl std::error::Error for KeyboardAdmissionRejection {}

/// Record of an outgoing command issued by the Keyboard Extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedCommand {
    pub request_id: String,
    pub sequence: u64,
    pub command: IpcCommand,
}

/// Consumer/Keyboard-side admission gate (v2).
///
/// Ensures that incoming responses from App UserDefaults snapshots:
/// 1. Match the currently observed `app_epoch`. Reject responses from an old App process.
/// 2. Match the currently active editor `session_id`.
/// 3. Explicitly correlate with an outgoing issued command (`acknowledged_request_id`
///    and `acknowledged_sequence` match a sent command in the active session).
/// 4. Reject responses acknowledging unissued or future sequence numbers.
/// 5. Strictly increase the response `revision` watermark.
/// 6. Discard duplicate snapshots, out-of-order snapshots, or snapshots
///    received after the local session has ended or closed.
/// 7. Maintain bounded memory for `issued_commands` history, compacting older
///    commands after acknowledgment while preserving active correlation.
#[derive(Debug, Default)]
pub struct KeyboardAdmissionGate {
    observed_app_epoch: Option<String>,
    current_session_id: Option<String>,
    last_accepted_revision: u64,
    is_closed: bool,
    issued_commands: Vec<IssuedCommand>,
}

impl KeyboardAdmissionGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe the App's launch epoch (read from `echolet.app.epoch.v2`).
    ///
    /// If the observed epoch changes while a session is active, the active session
    /// is immediately closed/fenced because the prior app process died.
    pub fn observe_app_epoch(
        &mut self,
        app_epoch: impl Into<String>,
    ) -> Result<(), IpcValidationError> {
        let app_epoch = app_epoch.into();
        if app_epoch.trim().is_empty() {
            return Err(IpcValidationError::EmptyAppEpoch);
        }
        if let Some(ref current) = self.observed_app_epoch {
            if current != &app_epoch {
                // App restarted under a new epoch: fence active session immediately
                self.close_session();
                self.observed_app_epoch = Some(app_epoch);
                return Ok(());
            }
        }
        self.observed_app_epoch = Some(app_epoch);
        Ok(())
    }

    /// Currently observed App epoch.
    pub fn observed_app_epoch(&self) -> Option<&str> {
        self.observed_app_epoch.as_deref()
    }

    /// Open a new editor session bound to the specified session ID and current app epoch.
    pub fn open_session(
        &mut self,
        session_id: impl Into<String>,
        app_epoch: impl Into<String>,
    ) -> Result<(), IpcValidationError> {
        let session_id = session_id.into();
        let app_epoch = app_epoch.into();
        if session_id.trim().is_empty() {
            return Err(IpcValidationError::EmptySessionId);
        }
        if app_epoch.trim().is_empty() {
            return Err(IpcValidationError::EmptyAppEpoch);
        }

        self.observed_app_epoch = Some(app_epoch);
        self.current_session_id = Some(session_id);
        self.last_accepted_revision = 0;
        self.is_closed = false;
        self.issued_commands.clear();
        Ok(())
    }

    /// Register an outgoing command issued by the Keyboard Extension for correlation.
    pub fn register_issued_command(
        &mut self,
        request: &KeyboardRequest,
    ) -> Result<(), KeyboardAdmissionRejection> {
        if let Err(err) = request.validate() {
            return Err(KeyboardAdmissionRejection::InvalidPayload(err));
        }

        let current_id = match &self.current_session_id {
            Some(id) => id,
            None => return Err(KeyboardAdmissionRejection::SessionInactive),
        };

        if current_id != &request.session_id {
            return Err(KeyboardAdmissionRejection::MismatchedSession {
                expected_session_id: current_id.clone(),
                received_session_id: request.session_id.clone(),
            });
        }

        if let Some(ref observed_epoch) = self.observed_app_epoch {
            if observed_epoch != &request.app_epoch {
                return Err(KeyboardAdmissionRejection::StaleAppEpoch {
                    expected_epoch: observed_epoch.clone(),
                    received_epoch: request.app_epoch.clone(),
                });
            }
        }

        if self.is_closed {
            return Err(KeyboardAdmissionRejection::ResurrectionAttemptAfterClose {
                session_id: current_id.clone(),
            });
        }

        self.issued_commands.push(IssuedCommand {
            request_id: request.request_id.clone(),
            sequence: request.sequence,
            command: request.command,
        });

        // Compact history if it exceeds maximum bounded length:
        // Always preserve the earliest command (START, needed for multiple partial transcript ACKs)
        // and keep the most recent commands.
        if self.issued_commands.len() > MAX_ISSUED_COMMANDS_HISTORY {
            let overflow = self.issued_commands.len() - MAX_ISSUED_COMMANDS_HISTORY;
            // Drain elements in range 1..=overflow
            self.issued_commands.drain(1..=overflow);
        }

        Ok(())
    }

    /// Closes the current session locally (e.g. keyboard dismissed, field lost focus, or user tapped Stop/Cancel).
    /// Prevents any subsequent response from being admitted.
    pub fn close_session(&mut self) {
        self.is_closed = true;
    }

    /// Invalidate and reset current session completely.
    pub fn reset(&mut self) {
        self.current_session_id = None;
        self.last_accepted_revision = 0;
        self.is_closed = false;
        self.issued_commands.clear();
    }

    pub fn current_session_id(&self) -> Option<&str> {
        self.current_session_id.as_deref()
    }

    pub fn is_active(&self) -> bool {
        self.current_session_id.is_some() && !self.is_closed
    }

    pub fn last_accepted_revision(&self) -> u64 {
        self.last_accepted_revision
    }

    /// Admits or rejects an AppResponse snapshot.
    pub fn admit_response(
        &mut self,
        response: &AppResponse,
    ) -> Result<(), KeyboardAdmissionRejection> {
        if let Err(err) = response.validate() {
            return Err(KeyboardAdmissionRejection::InvalidPayload(err));
        }

        let current_id = match &self.current_session_id {
            Some(id) => id,
            None => return Err(KeyboardAdmissionRejection::SessionInactive),
        };

        // 1. Verify response app_epoch matches observed app_epoch
        if let Some(ref observed_epoch) = self.observed_app_epoch {
            if observed_epoch != &response.app_epoch {
                return Err(KeyboardAdmissionRejection::StaleAppEpoch {
                    expected_epoch: observed_epoch.clone(),
                    received_epoch: response.app_epoch.clone(),
                });
            }
        }

        // 2. Verify session ID
        if current_id != &response.session_id {
            return Err(KeyboardAdmissionRejection::MismatchedSession {
                expected_session_id: current_id.clone(),
                received_session_id: response.session_id.clone(),
            });
        }

        // 3. Reject resurrection if local session is closed
        if self.is_closed {
            return Err(KeyboardAdmissionRejection::ResurrectionAttemptAfterClose {
                session_id: current_id.clone(),
            });
        }

        // 4. Verify request correlation against issued commands
        let matched = self.issued_commands.iter().find(|cmd| {
            cmd.request_id == response.acknowledged_request_id
                && cmd.sequence == response.acknowledged_sequence
        });

        if matched.is_none() {
            let reason = if self.issued_commands.is_empty() {
                "no issued commands recorded for active session"
            } else if self
                .issued_commands
                .iter()
                .all(|c| response.acknowledged_sequence > c.sequence)
            {
                "acknowledged sequence is ahead of any issued sequence"
            } else {
                "acknowledged request id/sequence does not match any issued command"
            };
            return Err(KeyboardAdmissionRejection::RequestCorrelationMismatch {
                session_id: current_id.clone(),
                acknowledged_request_id: response.acknowledged_request_id.clone(),
                acknowledged_sequence: response.acknowledged_sequence,
                reason,
            });
        }

        // 5. Monotonic revision check
        if response.revision <= self.last_accepted_revision {
            return Err(KeyboardAdmissionRejection::NonMonotonicRevision {
                session_id: current_id.clone(),
                received_revision: response.revision,
                last_accepted_revision: self.last_accepted_revision,
            });
        }

        self.last_accepted_revision = response.revision;
        if response.is_final {
            self.is_closed = true;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_request_serialization_round_trip() {
        let req = KeyboardRequest::new(
            "epoch-100",
            1,
            "sess-123",
            1,
            "req-abc",
            IpcCommand::Start,
            Some(1728570000000),
        )
        .unwrap();

        let json = req.to_json().unwrap();
        let decoded = KeyboardRequest::from_json_str(&json).unwrap();
        assert_eq!(req, decoded);
    }

    #[test]
    fn test_request_validation_failures() {
        // Empty app_epoch
        assert_eq!(
            KeyboardRequest::new("", 1, "sess-1", 1, "req-1", IpcCommand::Start, None).unwrap_err(),
            IpcValidationError::EmptyAppEpoch
        );
        // Zero intent_sequence
        assert_eq!(
            KeyboardRequest::new("ep-1", 0, "sess-1", 1, "req-1", IpcCommand::Start, None)
                .unwrap_err(),
            IpcValidationError::InvalidIntentSequence(0)
        );
        // Empty session
        assert_eq!(
            KeyboardRequest::new("ep-1", 1, "", 1, "req-1", IpcCommand::Start, None).unwrap_err(),
            IpcValidationError::EmptySessionId
        );
        // Empty request
        assert_eq!(
            KeyboardRequest::new("ep-1", 1, "sess-1", 1, "  ", IpcCommand::Start, None)
                .unwrap_err(),
            IpcValidationError::EmptyRequestId
        );
        // Zero sequence
        assert_eq!(
            KeyboardRequest::new("ep-1", 1, "sess-1", 0, "req-1", IpcCommand::Start, None)
                .unwrap_err(),
            IpcValidationError::InvalidSequence(0)
        );

        // Malformed json
        assert!(matches!(
            KeyboardRequest::from_json_str("{ bad json }"),
            Err(IpcValidationError::MalformedJson(_))
        ));

        // Unsupported version (v1 is now rejected!)
        let bad_ver_json = r#"{
            "protocol_version": 1,
            "app_epoch": "ep-1",
            "intent_sequence": 1,
            "session_id": "sess-1",
            "sequence": 1,
            "request_id": "req-1",
            "command": "start"
        }"#;
        assert_eq!(
            KeyboardRequest::from_json_str(bad_ver_json).unwrap_err(),
            IpcValidationError::UnsupportedProtocolVersion(1)
        );
    }

    #[test]
    fn test_valid_response_serialization_round_trip() {
        let resp = AppResponse::new(
            "epoch-100",
            "sess-123",
            "req-abc",
            1,
            5,
            IpcAppState::Listening,
            Some("Hello world".to_string()),
            false,
            None,
            Some(1728570001000),
        )
        .unwrap();

        let json = resp.to_json().unwrap();
        let decoded = AppResponse::from_json_str(&json).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn test_response_validation_failures() {
        let bad_ver = r#"{
            "protocol_version": 1,
            "app_epoch": "ep-1",
            "session_id": "sess-1",
            "acknowledged_request_id": "req-1",
            "acknowledged_sequence": 1,
            "revision": 1,
            "state": "listening",
            "is_final": false
        }"#;
        assert_eq!(
            AppResponse::from_json_str(bad_ver).unwrap_err(),
            IpcValidationError::UnsupportedProtocolVersion(1)
        );

        let empty_epoch = r#"{
            "protocol_version": 2,
            "app_epoch": "  ",
            "session_id": "sess-1",
            "acknowledged_request_id": "req-1",
            "acknowledged_sequence": 1,
            "revision": 1,
            "state": "listening",
            "is_final": false
        }"#;
        assert_eq!(
            AppResponse::from_json_str(empty_epoch).unwrap_err(),
            IpcValidationError::EmptyAppEpoch
        );
    }

    #[test]
    fn test_app_admission_gate_lifecycle_and_stale_rejection() {
        let mut gate = AppAdmissionGate::with_epoch("epoch-test-1").unwrap();

        // 1. Admit start for sess-1 with intent 1
        let req1 = KeyboardRequest::new(
            "epoch-test-1",
            1,
            "sess-1",
            1,
            "req-1",
            IpcCommand::Start,
            None,
        )
        .unwrap();
        assert!(gate.admit_request(&req1).is_ok());
        assert_eq!(gate.active_session_id(), Some("sess-1"));
        assert_eq!(gate.last_intent_sequence(), 1);

        // 2. Reject duplicate start for sess-1
        let req1_dup = KeyboardRequest::new(
            "epoch-test-1",
            2,
            "sess-1",
            2,
            "req-2",
            IpcCommand::Start,
            None,
        )
        .unwrap();
        assert!(matches!(
            gate.admit_request(&req1_dup),
            Err(AppAdmissionRejection::CommandOrderViolation { .. })
        ));

        // 3. Reject start for sess-2 if sequence > 1 while sess-1 is active
        let req2_start_seq2 = KeyboardRequest::new(
            "epoch-test-1",
            3,
            "sess-2",
            2,
            "req-3",
            IpcCommand::Start,
            None,
        )
        .unwrap();
        assert!(matches!(
            gate.admit_request(&req2_start_seq2),
            Err(AppAdmissionRejection::ActiveSessionConflict { .. })
        ));

        // 4. Reject non-monotonic sequence within sess-1 (e.g. sequence 1 again for Stop)
        let req1_bad_seq = KeyboardRequest::new(
            "epoch-test-1",
            4,
            "sess-1",
            1,
            "req-4",
            IpcCommand::Stop,
            None,
        )
        .unwrap();
        assert!(matches!(
            gate.admit_request(&req1_bad_seq),
            Err(AppAdmissionRejection::NonMonotonicSequence { .. })
        ));

        // 5. Admit Stop for sess-1
        let req1_stop = KeyboardRequest::new(
            "epoch-test-1",
            5,
            "sess-1",
            2,
            "req-5",
            IpcCommand::Stop,
            None,
        )
        .unwrap();
        assert!(gate.admit_request(&req1_stop).is_ok());

        // 6. Now sess-2 can start cleanly
        let req2_start = KeyboardRequest::new(
            "epoch-test-1",
            6,
            "sess-2",
            1,
            "req-6",
            IpcCommand::Start,
            None,
        )
        .unwrap();
        assert!(gate.admit_request(&req2_start).is_ok());
        assert_eq!(gate.active_session_id(), Some("sess-2"));

        // 7. CRITICAL SAFETY: Late STOP belonging to sess-1 MUST NOT stop sess-2!
        let req1_late_stop = KeyboardRequest::new(
            "epoch-test-1",
            7,
            "sess-1",
            99,
            "req-99",
            IpcCommand::Stop,
            None,
        )
        .unwrap();
        assert!(matches!(
            gate.admit_request(&req1_late_stop),
            Err(AppAdmissionRejection::StaleSessionCommand { .. })
        ));
        assert_eq!(gate.active_session_id(), Some("sess-2"));

        // 8. Cancel sess-2
        let req2_cancel = KeyboardRequest::new(
            "epoch-test-1",
            8,
            "sess-2",
            2,
            "req-7",
            IpcCommand::Cancel,
            None,
        )
        .unwrap();
        assert!(gate.admit_request(&req2_cancel).is_ok());
    }

    #[test]
    fn test_keyboard_admission_gate_lifecycle_and_stale_rejection() {
        let mut gate = KeyboardAdmissionGate::new();

        // No session open -> reject
        let resp1 = AppResponse::new(
            "epoch-1",
            "sess-1",
            "req-1",
            1,
            1,
            IpcAppState::Listening,
            Some("Hi".to_string()),
            false,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            gate.admit_response(&resp1).unwrap_err(),
            KeyboardAdmissionRejection::SessionInactive
        );

        // Open sess-1 with epoch-1
        gate.open_session("sess-1", "epoch-1").unwrap();
        assert!(gate.is_active());

        // Uncorrelated response rejected
        assert!(matches!(
            gate.admit_response(&resp1),
            Err(KeyboardAdmissionRejection::RequestCorrelationMismatch { .. })
        ));

        // Register outgoing START request
        let req1 =
            KeyboardRequest::new("epoch-1", 1, "sess-1", 1, "req-1", IpcCommand::Start, None)
                .unwrap();
        gate.register_issued_command(&req1).unwrap();

        // Admit first revision
        assert!(gate.admit_response(&resp1).is_ok());
        assert_eq!(gate.last_accepted_revision(), 1);

        // Duplicate revision 1 -> rejected
        assert_eq!(
            gate.admit_response(&resp1).unwrap_err(),
            KeyboardAdmissionRejection::NonMonotonicRevision {
                session_id: "sess-1".to_string(),
                received_revision: 1,
                last_accepted_revision: 1,
            }
        );

        // Advance to revision 2 (partial update for same req-1)
        let mut resp2 = resp1.clone();
        resp2.revision = 2;
        resp2.recognized_text = Some("Hi there".to_string());
        assert!(gate.admit_response(&resp2).is_ok());
        assert_eq!(gate.last_accepted_revision(), 2);

        // Issue STOP request req-2
        let req2 = KeyboardRequest::new("epoch-1", 2, "sess-1", 2, "req-2", IpcCommand::Stop, None)
            .unwrap();
        gate.register_issued_command(&req2).unwrap();

        // Final response at revision 3 acknowledging req-2 closes the session
        let mut resp3 = resp1.clone();
        resp3.acknowledged_request_id = "req-2".to_string();
        resp3.acknowledged_sequence = 2;
        resp3.revision = 3;
        resp3.state = IpcAppState::Completed;
        resp3.is_final = true;
        resp3.recognized_text = Some("Hi there.".to_string());
        assert!(gate.admit_response(&resp3).is_ok());
        assert!(!gate.is_active());

        // Subsequent response rejected as resurrection
        let mut resp4 = resp3.clone();
        resp4.revision = 4;
        assert_eq!(
            gate.admit_response(&resp4).unwrap_err(),
            KeyboardAdmissionRejection::ResurrectionAttemptAfterClose {
                session_id: "sess-1".to_string()
            }
        );

        // Open new session sess-2 with epoch-1
        gate.open_session("sess-2", "epoch-1").unwrap();
        assert!(gate.is_active());

        // Late response from sess-1 received now -> rejected due to session mismatch
        let resp_late_sess1 = resp4;
        assert_eq!(
            gate.admit_response(&resp_late_sess1).unwrap_err(),
            KeyboardAdmissionRejection::MismatchedSession {
                expected_session_id: "sess-2".to_string(),
                received_session_id: "sess-1".to_string(),
            }
        );
    }

    #[test]
    fn test_adversarial_defect_1_mismatched_ack_with_same_session_and_higher_revision() {
        let mut gate = KeyboardAdmissionGate::new();
        gate.open_session("sess-adv-1", "epoch-1").unwrap();

        let req1 = KeyboardRequest::new(
            "epoch-1",
            1,
            "sess-adv-1",
            1,
            "req-real-1",
            IpcCommand::Start,
            None,
        )
        .unwrap();
        gate.register_issued_command(&req1).unwrap();

        // Valid initial response
        let resp1 = AppResponse::new(
            "epoch-1",
            "sess-adv-1",
            "req-real-1",
            1,
            1,
            IpcAppState::Listening,
            Some("Partial 1".to_string()),
            false,
            None,
            None,
        )
        .unwrap();
        assert!(gate.admit_response(&resp1).is_ok());
        assert_eq!(gate.last_accepted_revision(), 1);

        // Adversarial response: wrong acknowledged_request_id
        let resp_wrong_id = AppResponse::new(
            "epoch-1",
            "sess-adv-1",
            "req-phantom-99",
            1,
            2,
            IpcAppState::Listening,
            Some("Malicious injected text".to_string()),
            false,
            None,
            None,
        )
        .unwrap();

        let err = gate.admit_response(&resp_wrong_id).unwrap_err();
        assert!(matches!(
            err,
            KeyboardAdmissionRejection::RequestCorrelationMismatch { .. }
        ));
        assert_eq!(gate.last_accepted_revision(), 1);
    }

    #[test]
    fn test_adversarial_defect_2_zero_sequence_and_zero_revision_rejection() {
        let resp_zero_seq = AppResponse {
            protocol_version: PROTOCOL_VERSION,
            app_epoch: "epoch-1".to_string(),
            session_id: "sess-1".to_string(),
            acknowledged_request_id: "req-1".to_string(),
            acknowledged_sequence: 0,
            revision: 1,
            state: IpcAppState::Listening,
            recognized_text: None,
            is_final: false,
            error_code: None,
            server_timestamp_ms: None,
        };
        assert_eq!(
            resp_zero_seq.validate().unwrap_err(),
            IpcValidationError::InvalidAcknowledgedSequence(0)
        );

        let resp_zero_rev = AppResponse {
            protocol_version: PROTOCOL_VERSION,
            app_epoch: "epoch-1".to_string(),
            session_id: "sess-1".to_string(),
            acknowledged_request_id: "req-1".to_string(),
            acknowledged_sequence: 1,
            revision: 0,
            state: IpcAppState::Listening,
            recognized_text: None,
            is_final: false,
            error_code: None,
            server_timestamp_ms: None,
        };
        assert_eq!(
            resp_zero_rev.validate().unwrap_err(),
            IpcValidationError::InvalidRevision(0)
        );
    }

    #[test]
    fn test_adversarial_defect_3_lost_stop_then_new_start_and_stale_stop() {
        let mut app_gate = AppAdmissionGate::with_epoch("epoch-gate-3").unwrap();

        // Editor 1 starts session A with intent 1
        let req_start_a = KeyboardRequest::new(
            "epoch-gate-3",
            1,
            "sess-A",
            1,
            "req-A-1",
            IpcCommand::Start,
            None,
        )
        .unwrap();
        assert_eq!(
            app_gate.admit_request(&req_start_a).unwrap(),
            AppAdmissionOutcome::Admitted
        );
        assert_eq!(app_gate.active_session_id(), Some("sess-A"));

        // Preceding STOP for sess-A is lost in transit!
        // User moves focus to Editor 2: Keyboard issues fresh START for sess-B with intent 2 (sequence=1)
        let req_start_b = KeyboardRequest::new(
            "epoch-gate-3",
            2,
            "sess-B",
            1,
            "req-B-1",
            IpcCommand::Start,
            None,
        )
        .unwrap();
        let outcome = app_gate.admit_request(&req_start_b).unwrap();
        assert_eq!(
            outcome,
            AppAdmissionOutcome::ReplacedPriorSession {
                retired_session_id: "sess-A".to_string(),
            }
        );
        assert_eq!(app_gate.active_session_id(), Some("sess-B"));

        // Late STOP from old sess-A arrives with intent 3: rejected because session is stale
        let late_stop_a = KeyboardRequest::new(
            "epoch-gate-3",
            3,
            "sess-A",
            2,
            "req-A-2",
            IpcCommand::Stop,
            None,
        )
        .unwrap();
        let rejection = app_gate.admit_request(&late_stop_a).unwrap_err();
        assert!(matches!(
            rejection,
            AppAdmissionRejection::StaleSessionCommand {
                ref incoming_session_id,
                ref active_session_id,
                command: IpcCommand::Stop,
            } if incoming_session_id == "sess-A" && active_session_id.as_deref() == Some("sess-B")
        ));
        assert_eq!(app_gate.active_session_id(), Some("sess-B"));
    }

    // Concrete Adverse Tests required by Spec:
    // (1) cold boot with old cached START, reject; change to new app_epoch,
    // after any explicit foreground authorization SAME cached old request remains rejected.
    #[test]
    fn test_adverse_1_cold_boot_old_cached_start_and_epoch_mismatch_after_authorization() {
        let mut app_gate = AppAdmissionGate::cold_boot("app-epoch-boot-1").unwrap();
        assert!(app_gate.is_cold_boot_armed());

        let cached_req = KeyboardRequest::new(
            "app-epoch-old-run",
            1,
            "sess-old",
            1,
            "req-old",
            IpcCommand::Start,
            None,
        )
        .unwrap();

        // 1. Rejected while cold boot armed
        let err = app_gate.admit_request(&cached_req).unwrap_err();
        // Epoch mismatch is checked first, rejecting it as StaleAppEpoch!
        assert!(matches!(err, AppAdmissionRejection::StaleAppEpoch { .. }));

        // Even with same epoch before authorization, cold boot fence rejects:
        let cached_req_same_epoch = KeyboardRequest::new(
            "app-epoch-boot-1",
            1,
            "sess-old",
            1,
            "req-old",
            IpcCommand::Start,
            None,
        )
        .unwrap();
        let err2 = app_gate.admit_request(&cached_req_same_epoch).unwrap_err();
        assert!(matches!(
            err2,
            AppAdmissionRejection::ColdBootFenceReject { .. }
        ));

        // 2. Host authorizes boot
        app_gate.authorize_boot();
        assert!(!app_gate.is_cold_boot_armed());

        // 3. Process restart or foreground authorization: SAME old cached request with old epoch remains rejected!
        let err3 = app_gate.admit_request(&cached_req).unwrap_err();
        assert!(matches!(err3, AppAdmissionRejection::StaleAppEpoch { .. }));
        assert_eq!(app_gate.active_session_id(), None);
    }

    // (2) new START with fresh app_epoch and higher intent accepted,
    // earlier delayed START (different session, seq=1 but lower intent) rejected without mutating active session.
    #[test]
    fn test_adverse_2_delayed_old_start_lower_intent_rejected_without_mutating_session() {
        let mut app_gate = AppAdmissionGate::with_epoch("app-epoch-active").unwrap();

        // Fresh START with intent 10
        let req_start = KeyboardRequest::new(
            "app-epoch-active",
            10,
            "sess-new",
            1,
            "req-new-1",
            IpcCommand::Start,
            None,
        )
        .unwrap();
        assert_eq!(
            app_gate.admit_request(&req_start).unwrap(),
            AppAdmissionOutcome::Admitted
        );
        assert_eq!(app_gate.active_session_id(), Some("sess-new"));

        // Delayed older START: seq=1, different session, but intent=5 (lower than 10)
        let delayed_old_start = KeyboardRequest::new(
            "app-epoch-active",
            5,
            "sess-old-delayed",
            1,
            "req-delayed-1",
            IpcCommand::Start,
            None,
        )
        .unwrap();
        let err = app_gate.admit_request(&delayed_old_start).unwrap_err();
        assert!(matches!(
            err,
            AppAdmissionRejection::NonMonotonicIntentSequence {
                incoming_intent: 5,
                last_admitted_intent: 10
            }
        ));
        // Active session remains sess-new untouched
        assert_eq!(app_gate.active_session_id(), Some("sess-new"));
    }

    // (3) old STOP after replacement rejected, no side effects
    #[test]
    fn test_adverse_3_old_stop_after_replacement_rejected_no_side_effects() {
        let mut app_gate = AppAdmissionGate::with_epoch("app-epoch-3").unwrap();

        let start1 = KeyboardRequest::new(
            "app-epoch-3",
            1,
            "sess-1",
            1,
            "req-1",
            IpcCommand::Start,
            None,
        )
        .unwrap();
        assert_eq!(
            app_gate.admit_request(&start1).unwrap(),
            AppAdmissionOutcome::Admitted
        );

        // sess-2 replaces sess-1
        let start2 = KeyboardRequest::new(
            "app-epoch-3",
            2,
            "sess-2",
            1,
            "req-2",
            IpcCommand::Start,
            None,
        )
        .unwrap();
        assert_eq!(
            app_gate.admit_request(&start2).unwrap(),
            AppAdmissionOutcome::ReplacedPriorSession {
                retired_session_id: "sess-1".to_string()
            }
        );
        assert_eq!(app_gate.active_session_id(), Some("sess-2"));

        // Late stop from sess-1 with intent 3
        let old_stop = KeyboardRequest::new(
            "app-epoch-3",
            3,
            "sess-1",
            2,
            "req-old-stop",
            IpcCommand::Stop,
            None,
        )
        .unwrap();
        let err = app_gate.admit_request(&old_stop).unwrap_err();
        assert!(matches!(
            err,
            AppAdmissionRejection::StaleSessionCommand { .. }
        ));
        assert_eq!(app_gate.active_session_id(), Some("sess-2"));
    }

    // (4) STOP overwrites unseen START: safe tombstone, no auto-mic
    #[test]
    fn test_adverse_4_stop_tombstone_no_auto_mic() {
        let mut app_gate = AppAdmissionGate::with_epoch("app-epoch-4").unwrap();

        // Keyboard wrote START then quickly wrote STOP, so App only reads STOP
        let stop_req = KeyboardRequest::new(
            "app-epoch-4",
            1,
            "sess-overwritten",
            2,
            "req-stop-alone",
            IpcCommand::Stop,
            None,
        )
        .unwrap();
        let outcome = app_gate.admit_request(&stop_req).unwrap();
        assert_eq!(
            outcome,
            AppAdmissionOutcome::TombstoneIgnored {
                command: IpcCommand::Stop
            }
        );
        assert_eq!(app_gate.active_session_id(), None);
        assert_eq!(app_gate.last_intent_sequence(), 1);

        // Next START with higher intent 2 starts cleanly
        let next_start = KeyboardRequest::new(
            "app-epoch-4",
            2,
            "sess-clean",
            1,
            "req-start-2",
            IpcCommand::Start,
            None,
        )
        .unwrap();
        assert_eq!(
            app_gate.admit_request(&next_start).unwrap(),
            AppAdmissionOutcome::Admitted
        );
        assert_eq!(app_gate.active_session_id(), Some("sess-clean"));
    }

    // (5) duplicate/replayed request same or lower intent, rejected
    #[test]
    fn test_adverse_5_duplicate_replayed_request_same_or_lower_intent_rejected() {
        let mut app_gate = AppAdmissionGate::with_epoch("app-epoch-5").unwrap();

        let req = KeyboardRequest::new(
            "app-epoch-5",
            5,
            "sess-5",
            1,
            "req-5-1",
            IpcCommand::Start,
            None,
        )
        .unwrap();
        assert!(app_gate.admit_request(&req).is_ok());

        // Replay same intent
        let err = app_gate.admit_request(&req).unwrap_err();
        assert!(matches!(
            err,
            AppAdmissionRejection::NonMonotonicIntentSequence {
                incoming_intent: 5,
                last_admitted_intent: 5
            }
        ));

        // Lower intent
        let req_lower = KeyboardRequest::new(
            "app-epoch-5",
            4,
            "sess-5",
            2,
            "req-5-2",
            IpcCommand::Stop,
            None,
        )
        .unwrap();
        let err2 = app_gate.admit_request(&req_lower).unwrap_err();
        assert!(matches!(
            err2,
            AppAdmissionRejection::NonMonotonicIntentSequence {
                incoming_intent: 4,
                last_admitted_intent: 5
            }
        ));
    }

    // (6) ACK matching remains enforced, old app_epoch response rejected even if ID+revision correct
    #[test]
    fn test_adverse_6_ack_matching_and_old_app_epoch_response_rejected() {
        let mut kb_gate = KeyboardAdmissionGate::new();
        kb_gate.open_session("sess-6", "app-epoch-current").unwrap();

        let req = KeyboardRequest::new(
            "app-epoch-current",
            1,
            "sess-6",
            1,
            "req-6-1",
            IpcCommand::Start,
            None,
        )
        .unwrap();
        kb_gate.register_issued_command(&req).unwrap();

        // Response from old app_epoch with valid request correlation and revision
        let resp_old_epoch = AppResponse::new(
            "app-epoch-previous",
            "sess-6",
            "req-6-1",
            1,
            1,
            IpcAppState::Listening,
            Some("Old text".to_string()),
            false,
            None,
            None,
        )
        .unwrap();

        let err = kb_gate.admit_response(&resp_old_epoch).unwrap_err();
        assert_eq!(
            err,
            KeyboardAdmissionRejection::StaleAppEpoch {
                expected_epoch: "app-epoch-current".to_string(),
                received_epoch: "app-epoch-previous".to_string(),
            }
        );
        assert_eq!(kb_gate.last_accepted_revision(), 0);
    }

    // (7) malformed/blank/unsupported version, zero values, overflow
    #[test]
    fn test_adverse_7_malformed_blank_version_zero_values_overflow() {
        // Blank app_epoch
        assert_eq!(
            KeyboardRequest::new("  ", 1, "s", 1, "r", IpcCommand::Start, None).unwrap_err(),
            IpcValidationError::EmptyAppEpoch
        );
        assert_eq!(
            AppResponse::new(
                "  ",
                "s",
                "r",
                1,
                1,
                IpcAppState::Listening,
                None,
                false,
                None,
                None
            )
            .unwrap_err(),
            IpcValidationError::EmptyAppEpoch
        );

        // AppAdmissionGate with empty epoch
        assert!(AppAdmissionGate::cold_boot("  ").is_err());
        assert!(AppAdmissionGate::with_epoch("  ").is_err());

        // Max intent_sequence + 1 -> u64::MAX is valid, overflow handled by caller failing closed
        let req_max = KeyboardRequest::new(
            "epoch-max",
            u64::MAX,
            "sess-max",
            1,
            "req-max",
            IpcCommand::Start,
            None,
        )
        .unwrap();
        assert!(req_max.validate().is_ok());
    }

    // (8) newly minted app_epoch rebind with new editor session resumes
    #[test]
    fn test_adverse_8_newly_minted_app_epoch_rebind_resumes() {
        let mut kb_gate = KeyboardAdmissionGate::new();
        kb_gate.open_session("sess-8a", "epoch-boot-1").unwrap();

        // App crashes and boots with epoch-boot-2
        // Keyboard observes new epoch:
        kb_gate.observe_app_epoch("epoch-boot-2").unwrap();
        // Previous session is closed immediately!
        assert!(!kb_gate.is_active());

        // Keyboard opens new editor session bound to epoch-boot-2:
        kb_gate.open_session("sess-8b", "epoch-boot-2").unwrap();
        assert!(kb_gate.is_active());

        let req_new = KeyboardRequest::new(
            "epoch-boot-2",
            10,
            "sess-8b",
            1,
            "req-8b-1",
            IpcCommand::Start,
            None,
        )
        .unwrap();
        kb_gate.register_issued_command(&req_new).unwrap();

        let resp_new = AppResponse::new(
            "epoch-boot-2",
            "sess-8b",
            "req-8b-1",
            1,
            1,
            IpcAppState::Listening,
            Some("Fresh text".to_string()),
            false,
            None,
            None,
        )
        .unwrap();
        assert!(kb_gate.admit_response(&resp_new).is_ok());
        assert_eq!(kb_gate.last_accepted_revision(), 1);
    }

    // (9) multiple partial response revisions acknowledging same issued START valid, memory bounded
    #[test]
    fn test_adverse_9_multiple_partial_response_revisions_and_bounded_memory() {
        let mut kb_gate = KeyboardAdmissionGate::new();
        kb_gate.open_session("sess-9", "epoch-9").unwrap();

        let req_start = KeyboardRequest::new(
            "epoch-9",
            1,
            "sess-9",
            1,
            "req-start-1",
            IpcCommand::Start,
            None,
        )
        .unwrap();
        kb_gate.register_issued_command(&req_start).unwrap();

        // Issue 50 intermediate dummy commands to trigger compaction
        for i in 2..=50 {
            let req_i = KeyboardRequest::new(
                "epoch-9",
                i,
                "sess-9",
                i,
                format!("req-dummy-{i}"),
                IpcCommand::Stop,
                None,
            )
            .unwrap();
            kb_gate.register_issued_command(&req_i).unwrap();
        }

        // Bounded memory check: length must not exceed MAX_ISSUED_COMMANDS_HISTORY
        assert!(kb_gate.issued_commands.len() <= MAX_ISSUED_COMMANDS_HISTORY);

        // START (seq 1) must still be preserved for multi-revision partials!
        assert_eq!(kb_gate.issued_commands[0].request_id, "req-start-1");

        // Admit multiple partial revisions acknowledging req-start-1
        for rev in 1..=5 {
            let resp = AppResponse::new(
                "epoch-9",
                "sess-9",
                "req-start-1",
                1,
                rev,
                IpcAppState::Listening,
                Some(format!("Partial revision {rev}")),
                false,
                None,
                None,
            )
            .unwrap();
            assert!(kb_gate.admit_response(&resp).is_ok());
            assert_eq!(kb_gate.last_accepted_revision(), rev);
        }
    }

    // (10) valid STOP/Cancel immediately closes local session, late final text discarded
    #[test]
    fn test_adverse_10_valid_stop_cancel_immediately_fences_late_text() {
        let mut kb_gate = KeyboardAdmissionGate::new();
        kb_gate.open_session("sess-10", "epoch-10").unwrap();

        let req_start = KeyboardRequest::new(
            "epoch-10",
            1,
            "sess-10",
            1,
            "req-start",
            IpcCommand::Start,
            None,
        )
        .unwrap();
        kb_gate.register_issued_command(&req_start).unwrap();

        let resp_part = AppResponse::new(
            "epoch-10",
            "sess-10",
            "req-start",
            1,
            1,
            IpcAppState::Listening,
            Some("Heard audio".to_string()),
            false,
            None,
            None,
        )
        .unwrap();
        assert!(kb_gate.admit_response(&resp_part).is_ok());

        // User hits Stop / Cancel / Dismisses keyboard: local session closed immediately
        kb_gate.close_session();
        assert!(!kb_gate.is_active());

        // Late final response arrives
        let resp_late_final = AppResponse::new(
            "epoch-10",
            "sess-10",
            "req-start",
            1,
            2,
            IpcAppState::Completed,
            Some("Heard audio and more words.".to_string()),
            true,
            None,
            None,
        )
        .unwrap();

        let err = kb_gate.admit_response(&resp_late_final).unwrap_err();
        assert_eq!(
            err,
            KeyboardAdmissionRejection::ResurrectionAttemptAfterClose {
                session_id: "sess-10".to_string()
            }
        );
        assert_eq!(kb_gate.last_accepted_revision(), 1);
    }
}

#[test]
fn test_golden_fixtures_decode_and_validate() {
    let fixture_req_start = include_str!("../ios/protocol/fixtures/golden_request_start.json");
    let req_start =
        KeyboardRequest::from_json_str(fixture_req_start).expect("decode start fixture");
    assert_eq!(req_start.protocol_version, 2);
    assert_eq!(req_start.app_epoch, "epoch-test-v2-golden");
    assert_eq!(req_start.intent_sequence, 1);
    assert_eq!(req_start.session_id, "session-golden-42");
    assert_eq!(req_start.sequence, 1);
    assert_eq!(req_start.request_id, "req-golden-001");
    assert_eq!(req_start.command, IpcCommand::Start);
    assert_eq!(req_start.client_timestamp_ms, Some(1728570000123));

    let fixture_req_stop = include_str!("../ios/protocol/fixtures/golden_request_stop.json");
    let req_stop = KeyboardRequest::from_json_str(fixture_req_stop).expect("decode stop fixture");
    assert_eq!(req_stop.protocol_version, 2);
    assert_eq!(req_stop.app_epoch, "epoch-test-v2-golden");
    assert_eq!(req_stop.intent_sequence, 2);
    assert_eq!(req_stop.session_id, "session-golden-42");
    assert_eq!(req_stop.sequence, 2);
    assert_eq!(req_stop.request_id, "req-golden-002");
    assert_eq!(req_stop.command, IpcCommand::Stop);
    assert_eq!(req_stop.client_timestamp_ms, None);

    let fixture_resp_part = include_str!("../ios/protocol/fixtures/golden_response_partial.json");
    let resp_part = AppResponse::from_json_str(fixture_resp_part).expect("decode partial fixture");
    assert_eq!(resp_part.protocol_version, 2);
    assert_eq!(resp_part.app_epoch, "epoch-test-v2-golden");
    assert_eq!(resp_part.session_id, "session-golden-42");
    assert_eq!(resp_part.acknowledged_request_id, "req-golden-001");
    assert_eq!(resp_part.acknowledged_sequence, 1);
    assert_eq!(resp_part.revision, 3);
    assert_eq!(resp_part.state, IpcAppState::Listening);
    assert_eq!(
        resp_part.recognized_text,
        Some("testing speech recognition".to_string())
    );
    assert!(!resp_part.is_final);
    assert_eq!(resp_part.server_timestamp_ms, Some(1728570001500));

    let fixture_resp_final = include_str!("../ios/protocol/fixtures/golden_response_final.json");
    let resp_final = AppResponse::from_json_str(fixture_resp_final).expect("decode final fixture");
    assert_eq!(resp_final.protocol_version, 2);
    assert_eq!(resp_final.app_epoch, "epoch-test-v2-golden");
    assert_eq!(resp_final.session_id, "session-golden-42");
    assert_eq!(resp_final.acknowledged_request_id, "req-golden-002");
    assert_eq!(resp_final.acknowledged_sequence, 2);
    assert_eq!(resp_final.revision, 4);
    assert_eq!(resp_final.state, IpcAppState::Completed);
    assert_eq!(
        resp_final.recognized_text,
        Some("testing speech recognition.".to_string())
    );
    assert!(resp_final.is_final);

    // Also verify admission gates with these golden fixtures in sequence
    let mut app_gate = AppAdmissionGate::with_epoch("epoch-test-v2-golden").unwrap();
    let mut kb_gate = KeyboardAdmissionGate::new();

    kb_gate
        .open_session("session-golden-42", "epoch-test-v2-golden")
        .unwrap();
    kb_gate.register_issued_command(&req_start).unwrap();
    app_gate.admit_request(&req_start).expect("app admit start");
    kb_gate
        .admit_response(&resp_part)
        .expect("kb admit partial");

    kb_gate.register_issued_command(&req_stop).unwrap();
    app_gate.admit_request(&req_stop).expect("app admit stop");
    kb_gate.admit_response(&resp_final).expect("kb admit final");

    // Now kb_gate is closed; late response rejected
    assert!(kb_gate.admit_response(&resp_final).is_err());
}
