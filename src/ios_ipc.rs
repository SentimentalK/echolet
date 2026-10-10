//! Cross-process IPC protocol and stale-session safety logic for iOS App and
//! Keyboard Extension communication.
//!
//! # Protocol Overview
//!
//! In iOS, the containing application and keyboard extension run in separate
//! sandbox processes. Communication is mediated via App Groups:
//! 1. App Group `UserDefaults` holds two dedicated single-writer keys:
//!    - [`KEYBOARD_REQUEST_KEY`]: written ONLY by Keyboard Extension, read by App.
//!    - [`APP_RESPONSE_KEY`]: written ONLY by App, read by Keyboard Extension.
//! 2. Darwin Notifications (`notify_post` / `notify_register_dispatch`) provide
//!    edge-trigger wake hints when a key changes:
//!    - [`DARWIN_NOTIFICATION_REQUEST`]: posted by Keyboard when writing a request.
//!    - [`DARWIN_NOTIFICATION_RESPONSE`]: posted by App when writing a response.
//!
//! # Strict Stale-Session Safety
//!
//! - **Single Source of Truth for Session**: The Keyboard Extension editor lifecycle
//!   owns `session_id` (a unique opaque UUID string). A new editor session invalidates
//!   the previous editor session locally immediately.
//! - **Monotonic Sequencing**: Requests within the same session must have strictly
//!   increasing sequence numbers (`sequence >= 1`).
//! - **App Admission Guard**: Stale `STOP` or `CANCEL` commands tagged with an older
//!   session cannot terminate a newer active session. Duplicate or out-of-order
//!   requests within a session are rejected.
//! - **Keyboard Response Admission Guard**: The Keyboard Extension only accepts
//!   responses matching its current editor `session_id` and strictly monotonic revisions.
//!   Late responses from an old session, duplicates, or resurrecting events after
//!   local session closure are rejected fail-closed.
//! - **Stop vs Cancel Semantics**:
//!   - `Stop`: Request to stop listening; existing transcribed text snapshot is preserved.
//!   - `Cancel`: Immediately abort; pending audio is discarded; already committed text
//!     is not undone, but session is fenced immediately.
//!
//! This module contains NO UIKit/Foundation/cpal dependencies and compiles on all
//! platforms (macOS, Linux, iOS, Android).

use serde::{Deserialize, Serialize};

/// Wire protocol version 1.
pub const PROTOCOL_VERSION: u32 = 1;

/// App Group UserDefaults key written only by the Keyboard Extension.
pub const KEYBOARD_REQUEST_KEY: &str = "echolet.keyboard.request.v1";

/// App Group UserDefaults key written only by the Containing App.
pub const APP_RESPONSE_KEY: &str = "echolet.app.response.v1";

/// Darwin notification posted by Keyboard Extension when a new request is written.
pub const DARWIN_NOTIFICATION_REQUEST: &str = "com.echolet.ipc.request.v1";

/// Darwin notification posted by Containing App when a new response snapshot is written.
pub const DARWIN_NOTIFICATION_RESPONSE: &str = "com.echolet.ipc.response.v1";

/// Commands issued from the Keyboard Extension to the App.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcCommand {
    Start,
    Stop,
    Cancel,
}

/// Request envelope written by the Keyboard Extension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyboardRequest {
    pub protocol_version: u32,
    pub session_id: String,
    pub sequence: u64,
    pub request_id: String,
    pub command: IpcCommand,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_timestamp_ms: Option<u64>,
}

impl KeyboardRequest {
    /// Creates a new request validating non-empty identifiers and valid sequence.
    pub fn new(
        session_id: impl Into<String>,
        sequence: u64,
        request_id: impl Into<String>,
        command: IpcCommand,
        client_timestamp_ms: Option<u64>,
    ) -> Result<Self, IpcValidationError> {
        let session_id = session_id.into();
        let request_id = request_id.into();

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

/// Response envelope written by the Containing App.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppResponse {
    pub protocol_version: u32,
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
    /// Creates a new AppResponse validating non-empty identifiers and version.
    pub fn new(
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
        let session_id = session_id.into();
        let acknowledged_request_id = acknowledged_request_id.into();

        if session_id.trim().is_empty() {
            return Err(IpcValidationError::EmptySessionId);
        }
        if acknowledged_request_id.trim().is_empty() {
            return Err(IpcValidationError::EmptyRequestId);
        }

        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
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
        if self.session_id.trim().is_empty() {
            return Err(IpcValidationError::EmptySessionId);
        }
        if self.acknowledged_request_id.trim().is_empty() {
            return Err(IpcValidationError::EmptyRequestId);
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
    EmptySessionId,
    EmptyRequestId,
    InvalidSequence(u64),
    MalformedJson(String),
}

impl std::fmt::Display for IpcValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedProtocolVersion(v) => write!(f, "unsupported protocol version: {v}"),
            Self::EmptySessionId => write!(f, "session_id cannot be empty"),
            Self::EmptyRequestId => write!(f, "request_id cannot be empty"),
            Self::InvalidSequence(s) => write!(f, "invalid sequence (must be >= 1): {s}"),
            Self::MalformedJson(msg) => write!(f, "malformed JSON: {msg}"),
        }
    }
}

impl std::error::Error for IpcValidationError {}

/// Rejection reasons for App-side request admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppAdmissionRejection {
    InvalidPayload(IpcValidationError),
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
}

impl std::fmt::Display for AppAdmissionRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPayload(e) => write!(f, "invalid payload: {e}"),
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

/// Producer/App-side admission gate.
///
/// Ensures that incoming requests obey session boundaries:
/// 1. A `START` command initiates a new session if no live session is active,
///    or transitions cleanly if the previous session has ended.
/// 2. If a session is already active, another `START` with a different session ID
///    is rejected (callers must STOP/CANCEL previous first).
/// 3. Stale `STOP` or `CANCEL` commands belonging to an older session are rejected
///    and NEVER terminate or interfere with a newer active session.
/// 4. Sequences within a session must be strictly monotonic (`seq > last_seq`).
/// 5. Duplicate commands or commands after session termination are rejected.
#[derive(Debug, Default)]
pub struct AppAdmissionGate {
    active_session_id: Option<String>,
    session_state: Option<ActiveSessionState>,
    last_sequence: u64,
}

impl AppAdmissionGate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn active_session_id(&self) -> Option<&str> {
        self.active_session_id.as_deref()
    }

    /// Evaluates and admits or rejects a keyboard request.
    pub fn admit_request(
        &mut self,
        request: &KeyboardRequest,
    ) -> Result<(), AppAdmissionRejection> {
        if let Err(err) = request.validate() {
            return Err(AppAdmissionRejection::InvalidPayload(err));
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
                    if self.session_state != Some(ActiveSessionState::Ended) {
                        return Err(AppAdmissionRejection::ActiveSessionConflict {
                            incoming_session_id: request.session_id.clone(),
                            active_session_id: current_id.clone(),
                        });
                    }
                }

                // New session starts
                self.active_session_id = Some(request.session_id.clone());
                self.session_state = Some(ActiveSessionState::Listening);
                self.last_sequence = request.sequence;
                Ok(())
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
                    self.session_state = Some(ActiveSessionState::Ended);
                    Ok(())
                }
                Some(other_active) => Err(AppAdmissionRejection::StaleSessionCommand {
                    incoming_session_id: request.session_id.clone(),
                    active_session_id: Some(other_active.clone()),
                    command: request.command,
                }),
                None => Err(AppAdmissionRejection::StaleSessionCommand {
                    incoming_session_id: request.session_id.clone(),
                    active_session_id: None,
                    command: request.command,
                }),
            },
        }
    }

    /// Mark active session as ended (e.g., when speech recognition completes or stops internally).
    pub fn end_active_session(&mut self) {
        self.session_state = Some(ActiveSessionState::Ended);
    }

    /// Clear all session state (e.g., app reset).
    pub fn reset(&mut self) {
        self.active_session_id = None;
        self.session_state = None;
        self.last_sequence = 0;
    }
}

/// Rejection reasons for Keyboard-side response admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyboardAdmissionRejection {
    InvalidPayload(IpcValidationError),
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
}

impl std::fmt::Display for KeyboardAdmissionRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPayload(e) => write!(f, "invalid response payload: {e}"),
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
        }
    }
}

impl std::error::Error for KeyboardAdmissionRejection {}

/// Consumer/Keyboard-side admission gate.
///
/// Ensures that incoming responses from App UserDefaults snapshots:
/// 1. Match the currently active editor `session_id`.
/// 2. Strictly increase the response `revision` watermark.
/// 3. Discard duplicate snapshots, out-of-order snapshots, or snapshots
///    received after the local session has ended or closed.
/// 4. Cannot resurrect a closed or cancelled session.
#[derive(Debug, Default)]
pub struct KeyboardAdmissionGate {
    current_session_id: Option<String>,
    last_accepted_revision: u64,
    is_closed: bool,
}

impl KeyboardAdmissionGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a new editor session with a freshly generated session ID.
    /// This immediately invalidates any previous session state.
    pub fn open_session(
        &mut self,
        session_id: impl Into<String>,
    ) -> Result<(), IpcValidationError> {
        let session_id = session_id.into();
        if session_id.trim().is_empty() {
            return Err(IpcValidationError::EmptySessionId);
        }
        self.current_session_id = Some(session_id);
        self.last_accepted_revision = 0;
        self.is_closed = false;
        Ok(())
    }

    /// Closes the current session locally (e.g., keyboard dismissed, field lost focus, or user tapped Stop/Cancel).
    /// Prevents any subsequent response from being admitted.
    pub fn close_session(&mut self) {
        self.is_closed = true;
    }

    /// Invalidate and reset current session completely.
    pub fn reset(&mut self) {
        self.current_session_id = None;
        self.last_accepted_revision = 0;
        self.is_closed = false;
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

        if current_id != &response.session_id {
            return Err(KeyboardAdmissionRejection::MismatchedSession {
                expected_session_id: current_id.clone(),
                received_session_id: response.session_id.clone(),
            });
        }

        if self.is_closed {
            return Err(KeyboardAdmissionRejection::ResurrectionAttemptAfterClose {
                session_id: current_id.clone(),
            });
        }

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
        // Empty session
        assert_eq!(
            KeyboardRequest::new("", 1, "req-1", IpcCommand::Start, None).unwrap_err(),
            IpcValidationError::EmptySessionId
        );
        // Empty request
        assert_eq!(
            KeyboardRequest::new("sess-1", 1, "  ", IpcCommand::Start, None).unwrap_err(),
            IpcValidationError::EmptyRequestId
        );
        // Zero sequence
        assert_eq!(
            KeyboardRequest::new("sess-1", 0, "req-1", IpcCommand::Start, None).unwrap_err(),
            IpcValidationError::InvalidSequence(0)
        );

        // Malformed json
        assert!(matches!(
            KeyboardRequest::from_json_str("{ bad json }"),
            Err(IpcValidationError::MalformedJson(_))
        ));

        // Unsupported version
        let bad_ver_json = r#"{
            "protocol_version": 99,
            "session_id": "sess-1",
            "sequence": 1,
            "request_id": "req-1",
            "command": "start"
        }"#;
        assert_eq!(
            KeyboardRequest::from_json_str(bad_ver_json).unwrap_err(),
            IpcValidationError::UnsupportedProtocolVersion(99)
        );
    }

    #[test]
    fn test_valid_response_serialization_round_trip() {
        let resp = AppResponse::new(
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
            "protocol_version": 2,
            "session_id": "sess-1",
            "acknowledged_request_id": "req-1",
            "acknowledged_sequence": 1,
            "revision": 1,
            "state": "listening",
            "is_final": false
        }"#;
        assert_eq!(
            AppResponse::from_json_str(bad_ver).unwrap_err(),
            IpcValidationError::UnsupportedProtocolVersion(2)
        );

        let empty_session = r#"{
            "protocol_version": 1,
            "session_id": "  ",
            "acknowledged_request_id": "req-1",
            "acknowledged_sequence": 1,
            "revision": 1,
            "state": "listening",
            "is_final": false
        }"#;
        assert_eq!(
            AppResponse::from_json_str(empty_session).unwrap_err(),
            IpcValidationError::EmptySessionId
        );
    }

    #[test]
    fn test_app_admission_gate_lifecycle_and_stale_rejection() {
        let mut gate = AppAdmissionGate::new();

        // 1. Admit start for sess-1
        let req1 = KeyboardRequest::new("sess-1", 1, "req-1", IpcCommand::Start, None).unwrap();
        assert!(gate.admit_request(&req1).is_ok());
        assert_eq!(gate.active_session_id(), Some("sess-1"));

        // 2. Reject duplicate start for sess-1
        let req1_dup = KeyboardRequest::new("sess-1", 2, "req-2", IpcCommand::Start, None).unwrap();
        assert!(matches!(
            gate.admit_request(&req1_dup),
            Err(AppAdmissionRejection::CommandOrderViolation { .. })
        ));

        // 3. Reject start for sess-2 while sess-1 is active
        let req2_start =
            KeyboardRequest::new("sess-2", 1, "req-3", IpcCommand::Start, None).unwrap();
        assert!(matches!(
            gate.admit_request(&req2_start),
            Err(AppAdmissionRejection::ActiveSessionConflict { .. })
        ));

        // 4. Reject out-of-order sequence (e.g. sequence 1 again for sess-1)
        let req1_bad_seq =
            KeyboardRequest::new("sess-1", 1, "req-4", IpcCommand::Stop, None).unwrap();
        assert!(matches!(
            gate.admit_request(&req1_bad_seq),
            Err(AppAdmissionRejection::NonMonotonicSequence { .. })
        ));

        // 5. Admit Stop for sess-1
        let req1_stop = KeyboardRequest::new("sess-1", 2, "req-5", IpcCommand::Stop, None).unwrap();
        assert!(gate.admit_request(&req1_stop).is_ok());

        // 6. Now sess-2 can start
        assert!(gate.admit_request(&req2_start).is_ok());
        assert_eq!(gate.active_session_id(), Some("sess-2"));

        // 7. CRITICAL SAFETY: Late STOP belonging to sess-1 MUST NOT stop sess-2!
        let req1_late_stop =
            KeyboardRequest::new("sess-1", 99, "req-99", IpcCommand::Stop, None).unwrap();
        assert!(matches!(
            gate.admit_request(&req1_late_stop),
            Err(AppAdmissionRejection::StaleSessionCommand { .. })
        ));
        // Verify sess-2 is still the active session untouched
        assert_eq!(gate.active_session_id(), Some("sess-2"));

        // 8. Cancel sess-2
        let req2_cancel =
            KeyboardRequest::new("sess-2", 2, "req-6", IpcCommand::Cancel, None).unwrap();
        assert!(gate.admit_request(&req2_cancel).is_ok());
    }

    #[test]
    fn test_keyboard_admission_gate_lifecycle_and_stale_rejection() {
        let mut gate = KeyboardAdmissionGate::new();

        // No session open -> reject
        let resp1 = AppResponse::new(
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

        // Open sess-1
        gate.open_session("sess-1").unwrap();
        assert!(gate.is_active());

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

        // Out-of-order / older revision -> rejected
        let mut resp_old = resp1.clone();
        resp_old.revision = 0;
        assert!(matches!(
            gate.admit_response(&resp_old),
            Err(KeyboardAdmissionRejection::NonMonotonicRevision { .. })
        ));

        // Advance to revision 2
        let mut resp2 = resp1.clone();
        resp2.revision = 2;
        resp2.recognized_text = Some("Hi there".to_string());
        assert!(gate.admit_response(&resp2).is_ok());
        assert_eq!(gate.last_accepted_revision(), 2);

        // Final response at revision 3 closes the session
        let mut resp3 = resp1.clone();
        resp3.revision = 3;
        resp3.state = IpcAppState::Completed;
        resp3.is_final = true;
        resp3.recognized_text = Some("Hi there.".to_string());
        assert!(gate.admit_response(&resp3).is_ok());
        assert!(!gate.is_active());

        // Any subsequent response for sess-1 is rejected as resurrection
        let mut resp4 = resp1.clone();
        resp4.revision = 4;
        assert_eq!(
            gate.admit_response(&resp4).unwrap_err(),
            KeyboardAdmissionRejection::ResurrectionAttemptAfterClose {
                session_id: "sess-1".to_string()
            }
        );

        // Open new session sess-2
        gate.open_session("sess-2").unwrap();
        assert!(gate.is_active());

        // Late response from sess-1 received now -> rejected due to session mismatch!
        let resp_late_sess1 = resp4;
        assert_eq!(
            gate.admit_response(&resp_late_sess1).unwrap_err(),
            KeyboardAdmissionRejection::MismatchedSession {
                expected_session_id: "sess-2".to_string(),
                received_session_id: "sess-1".to_string(),
            }
        );

        // Manual close_session (e.g. keyboard dismissed)
        gate.close_session();
        assert!(!gate.is_active());

        let resp_sess2 = AppResponse::new(
            "sess-2",
            "req-2",
            1,
            1,
            IpcAppState::Listening,
            Some("Test".to_string()),
            false,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            gate.admit_response(&resp_sess2).unwrap_err(),
            KeyboardAdmissionRejection::ResurrectionAttemptAfterClose {
                session_id: "sess-2".to_string()
            }
        );
    }
}

#[test]
fn test_golden_fixtures_decode_and_validate() {
    let fixture_req_start = include_str!("../ios/protocol/fixtures/golden_request_start.json");
    let req_start =
        KeyboardRequest::from_json_str(fixture_req_start).expect("decode start fixture");
    assert_eq!(req_start.protocol_version, 1);
    assert_eq!(req_start.session_id, "session-golden-42");
    assert_eq!(req_start.sequence, 1);
    assert_eq!(req_start.request_id, "req-golden-001");
    assert_eq!(req_start.command, IpcCommand::Start);
    assert_eq!(req_start.client_timestamp_ms, Some(1728570000123));

    let fixture_req_stop = include_str!("../ios/protocol/fixtures/golden_request_stop.json");
    let req_stop = KeyboardRequest::from_json_str(fixture_req_stop).expect("decode stop fixture");
    assert_eq!(req_stop.protocol_version, 1);
    assert_eq!(req_stop.session_id, "session-golden-42");
    assert_eq!(req_stop.sequence, 2);
    assert_eq!(req_stop.request_id, "req-golden-002");
    assert_eq!(req_stop.command, IpcCommand::Stop);
    assert_eq!(req_stop.client_timestamp_ms, None);

    let fixture_resp_part = include_str!("../ios/protocol/fixtures/golden_response_partial.json");
    let resp_part = AppResponse::from_json_str(fixture_resp_part).expect("decode partial fixture");
    assert_eq!(resp_part.protocol_version, 1);
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
    assert_eq!(resp_final.protocol_version, 1);
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
    let mut app_gate = AppAdmissionGate::new();
    let mut kb_gate = KeyboardAdmissionGate::new();

    kb_gate.open_session("session-golden-42").unwrap();
    app_gate.admit_request(&req_start).expect("app admit start");
    kb_gate
        .admit_response(&resp_part)
        .expect("kb admit partial");
    app_gate.admit_request(&req_stop).expect("app admit stop");
    kb_gate.admit_response(&resp_final).expect("kb admit final");

    // Now kb_gate is closed; late response rejected
    assert!(kb_gate.admit_response(&resp_final).is_err());
}
