import Foundation

/// Pure Foundation flow coordinator and state machine for Warm IPC microphone capture.
///
/// Responsibilities:
/// 1. Bridges wire admission outcomes from `EcholetAdmission.Gate` with physical audio capture lifecycle.
/// 2. Explicit Capture Ownership:
///    - Differentiates keyboard-owned capture sessions from manual in-app testing capture.
///    - If manual capture is active, incoming keyboard START commands are rejected with honest `blocked` (busy) response.
///    - Keyboard STOP/CANCEL commands never stop or interrupt manual in-app capture.
/// 3. Async Permission & Engine Fencing:
///    - Disarming microphone (or disarm via UI toggle) increments `cancelGeneration`, invalidating any in-flight permission prompts or engine starts.
///    - Stale permission callbacks cannot start the microphone if user disarmed or session changed in flight.
///    - Engine start callbacks check exact session token, cancellation generation, and gate active session before publishing `.listening`.
/// 4. Replacement Fencing:
///    - When session B replaces active session A, hardware stop for A is enqueued first.
///    - Session B remains in preparing state; engine start for B is deferred until hardware stop for A finishes cleanly.
///    - Late stop completion for A cannot overwrite the App Group response snapshot of session B.
/// 5. Strictly Monotonic Response Snapshot Policy:
///    - All responses pass through `writeResponseSnapshot` with strictly monotonic revisions and exact ACK correlation.
///    - Protects against response revision overflow (fail-closed).
public final class WarmCaptureFlowCoordinator {

    public enum CaptureOwner: Equatable {
        case none
        case keyboard(sessionId: String, intentSequence: UInt64)
        case manualTest
    }

    public struct SessionToken: Equatable {
        public let appEpoch: String
        public let sessionId: String
        public let intentSequence: UInt64
        public let requestId: String
        public let sequence: UInt64
        public let cancelGeneration: UInt64

        public init(
            appEpoch: String,
            sessionId: String,
            intentSequence: UInt64,
            requestId: String,
            sequence: UInt64,
            cancelGeneration: UInt64
        ) {
            self.appEpoch = appEpoch
            self.sessionId = sessionId
            self.intentSequence = intentSequence
            self.requestId = requestId
            self.sequence = sequence
            self.cancelGeneration = cancelGeneration
        }
    }

    public enum FlowEffect: Equatable {
        case none
        case requestPermission(token: SessionToken)
        case startHardware(token: SessionToken)
        case stopHardwareKeyboard(reason: String)
        case stopHardwareManual
        case writeResponse(
            sessionId: String,
            requestId: String,
            sequence: UInt64,
            state: EcholetIPC.AppState,
            recognizedText: String?,
            isFinal: Bool,
            errorCode: String?
        )
        case notifyStartAdmitted(sessionId: String, intentSequence: UInt64)
        case notifyStopAdmitted(sessionId: String, intentSequence: UInt64)
        case notifyBlocked(reason: String)
        case notifyRejected(reason: String)
    }

    // Process & Gate state
    public let appEpoch: String
    public private(set) var admissionGate: EcholetAdmission.Gate?

    // Ownership & Session state
    public private(set) var captureOwner: CaptureOwner = .none
    public private(set) var activeSessionToken: SessionToken?
    public private(set) var pendingStartToken: SessionToken?
    public private(set) var pendingReplacementStartToken: SessionToken?
    public private(set) var isWaitingForHardwareStopToStartReplacement: Bool = false

    // Arming & Cancellation generation
    public private(set) var isMicrophoneArmedByUser: Bool = false
    public private(set) var monotonicCancelGeneration: UInt64 = 0

    // Monotonic response revision counter
    public private(set) var responseRevision: UInt64 = 0

    public init(appEpoch: String, admissionGate: EcholetAdmission.Gate? = nil) {
        self.appEpoch = appEpoch
        self.admissionGate = admissionGate
    }

    public func setAdmissionGate(_ gate: EcholetAdmission.Gate) {
        self.admissionGate = gate
    }

    // MARK: - Monotonic Revision Helper
    public func nextResponseRevision() -> UInt64? {
        guard responseRevision < UInt64.max else {
            return nil // Fail closed on overflow
        }
        responseRevision += 1
        return responseRevision
    }

    // MARK: - User Arming Mutation
    public func setUserArmMicrophone(_ armed: Bool) -> [FlowEffect] {
        self.isMicrophoneArmedByUser = armed
        monotonicCancelGeneration &+= 1

        var effects: [FlowEffect] = []
        if !armed {
            // Invalidate pending starts immediately
            pendingStartToken = nil
            pendingReplacementStartToken = nil
            isWaitingForHardwareStopToStartReplacement = false

            // If keyboard session is active, cancel it and request stop
            if let active = activeSessionToken {
                activeSessionToken = nil
                admissionGate?.endActiveSession()
                effects.append(.stopHardwareKeyboard(reason: "user_disarmed"))
                effects.append(.writeResponse(
                    sessionId: active.sessionId,
                    requestId: active.requestId,
                    sequence: active.sequence,
                    state: .blocked,
                    recognizedText: nil,
                    isFinal: true,
                    errorCode: "app_microphone_disarmed_by_user"
                ))
            } else if case .keyboard = captureOwner {
                captureOwner = .none
                effects.append(.stopHardwareKeyboard(reason: "user_disarmed"))
            }
        }
        return effects
    }

    // MARK: - Manual In-App Audio Test Management
    public func handleManualStartCaptureInitiated() -> (shouldProceed: Bool, effects: [FlowEffect]) {
        // If manual capture is already active, no-op
        if case .manualTest = captureOwner {
            return (false, [])
        }

        // If keyboard capture is active or pending, stop it cleanly first
        var effects: [FlowEffect] = []
        if let active = activeSessionToken {
            activeSessionToken = nil
            pendingStartToken = nil
            pendingReplacementStartToken = nil
            isWaitingForHardwareStopToStartReplacement = false
            admissionGate?.endActiveSession()
            effects.append(.writeResponse(
                sessionId: active.sessionId,
                requestId: active.requestId,
                sequence: active.sequence,
                state: .blocked,
                recognizedText: nil,
                isFinal: true,
                errorCode: "preempted_by_app_manual_test"
            ))
            effects.append(.stopHardwareKeyboard(reason: "manual_test_preempted"))
        }

        captureOwner = .manualTest
        return (true, effects)
    }

    public func handleManualCaptureStopped() -> [FlowEffect] {
        if case .manualTest = captureOwner {
            captureOwner = .none
            return [.stopHardwareManual]
        }
        return []
    }

    // MARK: - Incoming Request Intake
    public func handleIncomingRequest(_ request: EcholetIPC.KeyboardRequest) -> [FlowEffect] {
        guard let gate = admissionGate else {
            return [.notifyBlocked(reason: "Admission gate uninitialized")]
        }

        // Skip if this request was already admitted and applied
        if let lastApplied = gate.lastAppliedRequestId, lastApplied == request.requestId {
            return []
        }

        let outcome = gate.admitRequest(request)
        switch outcome {
        case .success(let admission):
            return handleAdmittedRequest(request, admission: admission)
        case .failure(let rejection):
            return [.notifyRejected(reason: "Request \(request.requestId) rejected: \(rejection)")]
        }
    }

    private func handleAdmittedRequest(
        _ request: EcholetIPC.KeyboardRequest,
        admission: EcholetAdmission.AdmissionOutcome
    ) -> [FlowEffect] {
        var effects: [FlowEffect] = []

        switch request.command {
        case .start:
            // Check manual capture ownership: if manual test is actively running, reject as busy
            if case .manualTest = captureOwner {
                admissionGate?.endActiveSession()
                effects.append(.writeResponse(
                    sessionId: request.sessionId,
                    requestId: request.requestId,
                    sequence: request.sequence,
                    state: .blocked,
                    recognizedText: nil,
                    isFinal: true,
                    errorCode: "app_audio_busy_manual_test"
                ))
                effects.append(.notifyBlocked(reason: "Microphone busy: App manual test active."))
                return effects
            }

            // Distinguish START receipt from affirmative user arming
            guard isMicrophoneArmedByUser else {
                admissionGate?.endActiveSession()
                effects.append(.writeResponse(
                    sessionId: request.sessionId,
                    requestId: request.requestId,
                    sequence: request.sequence,
                    state: .blocked,
                    recognizedText: nil,
                    isFinal: true,
                    errorCode: "app_microphone_not_armed"
                ))
                effects.append(.notifyBlocked(reason: "Microphone not armed in containing app. Open Echolet and enable mic test."))
                return effects
            }

            // Invalidate any prior in-flight start callbacks
            monotonicCancelGeneration &+= 1
            let startToken = SessionToken(
                appEpoch: request.appEpoch,
                sessionId: request.sessionId,
                intentSequence: request.intentSequence,
                requestId: request.requestId,
                sequence: request.sequence,
                cancelGeneration: monotonicCancelGeneration
            )

            // Check if this replaces a prior session
            if case .replacedPriorSession(let retired) = admission {
                activeSessionToken = nil
                pendingStartToken = nil
                pendingReplacementStartToken = startToken
                isWaitingForHardwareStopToStartReplacement = true

                // Emit preparing state for the replacement session immediately
                effects.append(.writeResponse(
                    sessionId: request.sessionId,
                    requestId: request.requestId,
                    sequence: request.sequence,
                    state: .preparing,
                    recognizedText: nil,
                    isFinal: false,
                    errorCode: nil
                ))
                effects.append(.notifyStopAdmitted(sessionId: retired, intentSequence: request.intentSequence))

                // Stop hardware for previous session before initiating new start
                effects.append(.stopHardwareKeyboard(reason: "session_replacement"))
                return effects
            }

            // Fresh start (no prior active session):
            pendingStartToken = startToken
            captureOwner = .keyboard(sessionId: request.sessionId, intentSequence: request.intentSequence)

            // Emit preparing state while asynchronous start is underway
            effects.append(.writeResponse(
                sessionId: request.sessionId,
                requestId: request.requestId,
                sequence: request.sequence,
                state: .preparing,
                recognizedText: nil,
                isFinal: false,
                errorCode: nil
            ))

            // Trigger permission request
            effects.append(.requestPermission(token: startToken))
            return effects

        case .stop, .cancel:
            if case .tombstoneIgnored = admission {
                // Tombstone when no active session: safe acknowledgment, DO NOT touch manual audio
                effects.append(.writeResponse(
                    sessionId: request.sessionId,
                    requestId: request.requestId,
                    sequence: request.sequence,
                    state: .completed,
                    recognizedText: nil,
                    isFinal: true,
                    errorCode: nil
                ))
                return effects
            }

            // Admitted STOP on active keyboard session:
            // Invalidate pending and active tokens synchronously
            monotonicCancelGeneration &+= 1
            activeSessionToken = nil
            pendingStartToken = nil
            pendingReplacementStartToken = nil
            isWaitingForHardwareStopToStartReplacement = false
            admissionGate?.endActiveSession()

            // If manual test owns audio, DO NOT stop manual capture!
            if case .manualTest = captureOwner {
                effects.append(.writeResponse(
                    sessionId: request.sessionId,
                    requestId: request.requestId,
                    sequence: request.sequence,
                    state: .completed,
                    recognizedText: nil,
                    isFinal: true,
                    errorCode: nil
                ))
                effects.append(.notifyStopAdmitted(sessionId: request.sessionId, intentSequence: request.intentSequence))
                return effects
            }

            // Keyboard owned or none:
            captureOwner = .none
            effects.append(.stopHardwareKeyboard(reason: "keyboard_\(request.command.rawValue)"))
            // Note: completed snapshot will be written after stop completion confirms
            return effects
        }
    }

    // MARK: - Asynchronous Permission & Hardware Completion Intake
    public func handlePermissionCallback(token: SessionToken, granted: Bool) -> [FlowEffect] {
        // Revalidate fencing conditions:
        // 1. Cancel generation must match (not cancelled by disarm or newer start)
        guard token.cancelGeneration == monotonicCancelGeneration else {
            return []
        }
        // 2. Microphone must still be armed by user
        guard isMicrophoneArmedByUser else {
            return []
        }
        // 3. Gate active session and pending token match
        guard let currentGate = admissionGate,
              currentGate.activeSessionId == token.sessionId,
              currentGate.lastAppliedRequestId == token.requestId,
              pendingStartToken == token else {
            return []
        }
        // 4. Must not be preempted by manual audio test
        guard case .keyboard = captureOwner else {
            return []
        }

        if granted {
            // Trigger native hardware start
            return [.startHardware(token: token)]
        } else {
            // Permission denied: fail closed
            pendingStartToken = nil
            captureOwner = .none
            admissionGate?.endActiveSession()
            return [
                .writeResponse(
                    sessionId: token.sessionId,
                    requestId: token.requestId,
                    sequence: token.sequence,
                    state: .blocked,
                    recognizedText: nil,
                    isFinal: true,
                    errorCode: "microphone_permission_denied"
                ),
                .notifyBlocked(reason: "Microphone permission denied")
            ]
        }
    }

    public func handleHardwareStartCompletion(
        token: SessionToken,
        result: Result<UInt64, Error>
    ) -> (effects: [FlowEffect], stopCleanupNeeded: Bool) {
        // Check fence:
        let isStale = (token.cancelGeneration != monotonicCancelGeneration) ||
                      (!isMicrophoneArmedByUser) ||
                      (admissionGate?.activeSessionId != token.sessionId) ||
                      (admissionGate?.lastAppliedRequestId != token.requestId) ||
                      (pendingStartToken != token)

        if isStale {
            // Stale success: if this capture started hardware, clean it up ONLY if keyboard still owns it
            // and NOT if manual capture took over!
            if case .success = result {
                let cleanup = (captureOwner != .manualTest)
                return ([], cleanup)
            }
            return ([], false)
        }

        pendingStartToken = nil

        switch result {
        case .success:
            activeSessionToken = token
            captureOwner = .keyboard(sessionId: token.sessionId, intentSequence: token.intentSequence)
            return ([
                .writeResponse(
                    sessionId: token.sessionId,
                    requestId: token.requestId,
                    sequence: token.sequence,
                    state: .listening,
                    recognizedText: nil,
                    isFinal: false,
                    errorCode: nil
                ),
                .notifyStartAdmitted(sessionId: token.sessionId, intentSequence: token.intentSequence)
            ], false)

        case .failure(let err):
            activeSessionToken = nil
            captureOwner = .none
            admissionGate?.endActiveSession()
            return ([
                .writeResponse(
                    sessionId: token.sessionId,
                    requestId: token.requestId,
                    sequence: token.sequence,
                    state: .blocked,
                    recognizedText: nil,
                    isFinal: true,
                    errorCode: "audio_engine_start_failed: \(err.localizedDescription)"
                ),
                .notifyBlocked(reason: "Audio engine start failed: \(err.localizedDescription)")
            ], false)
        }
    }

    public func handleHardwareStopCompletion(
        stoppedRequestId: String?,
        stoppedSessionId: String?,
        stoppedSequence: UInt64?
    ) -> [FlowEffect] {
        var effects: [FlowEffect] = []

        // If we were waiting for stop completion to start a replacement session:
        if isWaitingForHardwareStopToStartReplacement, let nextToken = pendingReplacementStartToken {
            isWaitingForHardwareStopToStartReplacement = false
            pendingReplacementStartToken = nil

            // Check if nextToken is still valid and not cancelled while waiting
            if nextToken.cancelGeneration == monotonicCancelGeneration &&
               isMicrophoneArmedByUser &&
               admissionGate?.activeSessionId == nextToken.sessionId &&
               admissionGate?.lastAppliedRequestId == nextToken.requestId {

                pendingStartToken = nextToken
                captureOwner = .keyboard(sessionId: nextToken.sessionId, intentSequence: nextToken.intentSequence)
                effects.append(.requestPermission(token: nextToken))
            }
            return effects
        }

        // Normal stop completion for a stopped session:
        guard let reqId = stoppedRequestId, let sessId = stoppedSessionId, let seq = stoppedSequence else {
            return effects
        }

        // Fencing check: ensure a newer session B is not active or pending
        if let currentActive = activeSessionToken, currentActive.sessionId != sessId {
            // Do NOT overwrite current active session response with old stop completion!
            return effects
        }
        if let pending = pendingStartToken, pending.sessionId != sessId {
            return effects
        }
        if let pendingRep = pendingReplacementStartToken, pendingRep.sessionId != sessId {
            return effects
        }

        effects.append(.writeResponse(
            sessionId: sessId,
            requestId: reqId,
            sequence: seq,
            state: .completed,
            recognizedText: nil,
            isFinal: true,
            errorCode: nil
        ))
        effects.append(.notifyStopAdmitted(sessionId: sessId, intentSequence: seq))
        return effects
    }
}
