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

    /// Immutable hardware stop directive with complete operation identity.
    ///
    /// A stop completion is only a valid ACK for the exact stop operation that was
    /// issued: same app epoch, same request/session/intent/sequence, same
    /// coordinator-assigned operation id. This prevents a stale (or superseded,
    /// including same-sessionId-but-higher-intent) STOP completion from ACKing or
    /// overwriting the latest App Group response slot of a newer intent.
    public struct HardwareStopDirective: Equatable {
        public let appEpoch: String
        public let sessionId: String
        public let intentSequence: UInt64
        public let requestId: String
        public let sequence: UInt64
        public let operationId: UInt64
        public let reason: String

        public init(
            appEpoch: String,
            sessionId: String,
            intentSequence: UInt64,
            requestId: String,
            sequence: UInt64,
            operationId: UInt64,
            reason: String
        ) {
            self.appEpoch = appEpoch
            self.sessionId = sessionId
            self.intentSequence = intentSequence
            self.requestId = requestId
            self.sequence = sequence
            self.operationId = operationId
            self.reason = reason
        }
    }

    /// Immutable cleanup directive for a stale successful hardware start.
    ///
    /// Carries the exact native capture generation of the stale start plus its
    /// session token, so the executor can issue a generation-bound conditional
    /// stop that can never halt a different, newer capture generation.
    public struct HardwareStartCleanupDirective: Equatable {
        public let nativeGeneration: UInt64
        public let token: SessionToken

        public init(nativeGeneration: UInt64, token: SessionToken) {
            self.nativeGeneration = nativeGeneration
            self.token = token
        }
    }

    public enum FlowEffect: Equatable {
        case none
        case requestPermission(token: SessionToken)
        case startHardware(token: SessionToken)
        case stopHardwareKeyboard(directive: HardwareStopDirective)
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

    // Hardware stop identity tracking (single-writer, exact operation matching)
    public private(set) var pendingStopAcknowledgmentDirective: HardwareStopDirective?
    public private(set) var pendingReplacementStopDirective: HardwareStopDirective?
    private var nextStopOperationId: UInt64 = 0

    // Arming & Cancellation generation
    public private(set) var isMicrophoneArmedByUser: Bool = false
    public private(set) var monotonicCancelGeneration: UInt64 = 0

    /// Terminal fail-closed flag latched when the cancellation generation counter
    /// would overflow. No further microphone starts are permitted and old in-flight
    /// tokens are permanently fenced. Never resets for the lifetime of the process.
    public private(set) var isCancellationGenerationFailClosed: Bool = false

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

    // MARK: - DEBUG Mock Response Eligibility Probe (read-only, non-mutating)
    /// Delegates to the admission gate's read-only eligibility probe so the
    /// DEBUG mock submit path can test the CURRENT request identity under the
    /// same serialization domain as the live intake without applying state.
    public func evaluateMockResponseEligibility(_ request: EcholetIPC.KeyboardRequest) -> EcholetAdmission.Gate.MockResponseEligibility {
        guard let gate = admissionGate else {
            return .rejected(reason: "admissionGateUnavailable")
        }
        return gate.evaluateMockResponseEligibility(request)
    }

    // MARK: - Checked Cancellation Generation Advance
    /// Checked (non-wrapping) increment of the cancellation generation.
    /// On overflow this latches the terminal fail-closed state: no further mic
    /// starts, and all previously issued tokens become permanently invalid.
    private func advanceCancelGeneration() {
        let (next, overflow) = monotonicCancelGeneration.addingReportingOverflow(1)
        if overflow {
            isCancellationGenerationFailClosed = true
            return
        }
        monotonicCancelGeneration = next
    }

    // MARK: - Stop Directive Minting
    /// Mints a fully identified, immutable hardware stop directive.
    /// Operation ids are also checked-incremented; overflow latches the terminal
    /// fail-closed state (no new stops can be responsibly issued past UInt64).
    private func mintQualifiedStopDirective(
        sessionId: String,
        intentSequence: UInt64,
        requestId: String,
        sequence: UInt64,
        reason: String
    ) -> HardwareStopDirective? {
        guard nextStopOperationId < UInt64.max else {
            isCancellationGenerationFailClosed = true
            return nil
        }
        nextStopOperationId += 1
        return HardwareStopDirective(
            appEpoch: appEpoch,
            sessionId: sessionId,
            intentSequence: intentSequence,
            requestId: requestId,
            sequence: sequence,
            operationId: nextStopOperationId,
            reason: reason
        )
    }

    private func mintStopDirectiveFromToken(reason: String, token: SessionToken) -> HardwareStopDirective? {
        mintQualifiedStopDirective(
            sessionId: token.sessionId,
            intentSequence: token.intentSequence,
            requestId: token.requestId,
            sequence: token.sequence,
            reason: reason
        )
    }

    // MARK: - User Arming Mutation
    public func setUserArmMicrophone(_ armed: Bool) -> [FlowEffect] {
        self.isMicrophoneArmedByUser = armed
        advanceCancelGeneration()

        var effects: [FlowEffect] = []
        if !armed {
            // Invalidate pending starts immediately
            pendingStartToken = nil
            pendingReplacementStartToken = nil
            isWaitingForHardwareStopToStartReplacement = false
            pendingReplacementStopDirective = nil

            // If keyboard session is active, cancel it and request stop
            if let active = activeSessionToken {
                activeSessionToken = nil
                admissionGate?.endActiveSession()
                if let directive = mintStopDirectiveFromToken(reason: "user_disarmed", token: active) {
                    effects.append(.stopHardwareKeyboard(directive: directive))
                }
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
                if let directive = mintQualifiedStopDirective(
                    sessionId: "",
                    intentSequence: 0,
                    requestId: "",
                    sequence: 0,
                    reason: "user_disarmed"
                ) {
                    effects.append(.stopHardwareKeyboard(directive: directive))
                }
            }
        }
        return effects
    }

    // MARK: - Manual In-App Audio Test Management
    /// Simplified manual capture safety:
    /// - If manual capture is already active: duplicate tap, no-op (caller keeps state).
    /// - If ANY keyboard hardware is active, or a keyboard permission/native start is
    ///   pending, or a replacement stop for the previous keyboard engine is still
    ///   in flight: manual start is rejected as BUSY without changing ownership,
    ///   touching the mic, or issuing any stop/start. The user can retry after the
    ///   keyboard stop has truly completed.
    /// - Otherwise manual capture claims ownership and manual UI keeps full control.
    public func handleManualStartCaptureInitiated() -> (shouldProceed: Bool, busyReason: String?, effects: [FlowEffect]) {
        // If manual capture is already active, no-op (already app-owned)
        if case .manualTest = captureOwner {
            return (false, nil, [])
        }

        // Reject as BUSY while any keyboard hardware lifecycle is in flight or active.
        // Pending keyboard hardware callbacks (permission, engine start, replacement
        // barrier) must never be treated as manual ownership.
        if isWaitingForHardwareStopToStartReplacement {
            return (false, "keyboard replacement stop in progress", [.notifyBlocked(reason: "Microphone BUSY: keyboard session replacement stop in progress. Try again after it completes.")])
        }
        if pendingReplacementStartToken != nil || pendingStartToken != nil {
            return (false, "keyboard start pending", [.notifyBlocked(reason: "Microphone BUSY: keyboard capture start is pending. Try again after the keyboard session stops.")])
        }
        if activeSessionToken != nil {
            return (false, "keyboard capture active", [.notifyBlocked(reason: "Microphone BUSY: a keyboard session is actively recording. Stop it, then try the App audio test again.")])
        }
        if case .keyboard = captureOwner {
            return (false, "keyboard capture owner unclean", [.notifyBlocked(reason: "Microphone BUSY: keyboard capture is still finalizing. Try again in a moment.")])
        }

        captureOwner = .manualTest
        return (true, nil, [])
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

            // Terminal fail-closed: cancellation generation overflowed; no further
            // mic starts and no old token replay will ever be accepted.
            guard !isCancellationGenerationFailClosed else {
                pendingStartToken = nil
                pendingReplacementStartToken = nil
                isWaitingForHardwareStopToStartReplacement = false
                pendingReplacementStopDirective = nil
                admissionGate?.endActiveSession()
                effects.append(.writeResponse(
                    sessionId: request.sessionId,
                    requestId: request.requestId,
                    sequence: request.sequence,
                    state: .blocked,
                    recognizedText: nil,
                    isFinal: true,
                    errorCode: "capture_generation_overflow_fail_closed"
                ))
                effects.append(.notifyBlocked(reason: "Capture generation counter overflowed; microphone permanently disabled (fail-closed)."))
                return effects
            }

            // Invalidate any prior in-flight start callbacks (checked increment;
            // overflow latches terminal fail-closed which fences all future starts)
            advanceCancelGeneration()
            guard !isCancellationGenerationFailClosed else {
                pendingStartToken = nil
                pendingReplacementStartToken = nil
                isWaitingForHardwareStopToStartReplacement = false
                pendingReplacementStopDirective = nil
                admissionGate?.endActiveSession()
                effects.append(.writeResponse(
                    sessionId: request.sessionId,
                    requestId: request.requestId,
                    sequence: request.sequence,
                    state: .blocked,
                    recognizedText: nil,
                    isFinal: true,
                    errorCode: "capture_generation_overflow_fail_closed"
                ))
                effects.append(.notifyBlocked(reason: "Capture generation counter overflowed; microphone permanently disabled (fail-closed)."))
                return effects
            }

            let startToken = SessionToken(
                appEpoch: request.appEpoch,
                sessionId: request.sessionId,
                intentSequence: request.intentSequence,
                requestId: request.requestId,
                sequence: request.sequence,
                cancelGeneration: monotonicCancelGeneration
            )

            // Check if this replaces a prior session
            if case .replacedPriorSession = admission {
                let priorActive = activeSessionToken
                activeSessionToken = nil
                pendingStartToken = nil
                pendingReplacementStartToken = startToken
                isWaitingForHardwareStopToStartReplacement = true

                // Register the exact replacement-barrier stop directive for the
                // retired session. Selected by exact identity at completion; never
                // ACKs a response. Issued unconditionally so the native stop of the
                // retired session is enqueued BEFORE session B's native start even
                // when the retired session only had a pending (not yet active)
                // engine start — preserving strict A->B hardware ordering.
                pendingReplacementStopDirective = mintStopDirectiveFromToken(
                    reason: "session_replacement",
                    token: priorActive ?? SessionToken(
                        appEpoch: request.appEpoch,
                        sessionId: "",
                        intentSequence: request.intentSequence,
                        requestId: "",
                        sequence: request.sequence,
                        cancelGeneration: monotonicCancelGeneration
                    )
                )

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
                if let retiredToken = priorActive {
                    effects.append(.notifyStopAdmitted(sessionId: retiredToken.sessionId, intentSequence: retiredToken.intentSequence))
                } else {
                    effects.append(.notifyStopAdmitted(sessionId: "", intentSequence: request.intentSequence))
                }

                // Stop hardware for previous session before initiating new start
                if let stopDirective = pendingReplacementStopDirective {
                    effects.append(.stopHardwareKeyboard(directive: stopDirective))
                }
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
            advanceCancelGeneration()
            activeSessionToken = nil
            pendingStartToken = nil
            pendingReplacementStartToken = nil
            isWaitingForHardwareStopToStartReplacement = false
            pendingReplacementStopDirective = nil
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
            // Register the exact stop directive awaiting its hardware-completed ACK.
            // Identity: exact app epoch, stop request id/session/intent/sequence —
            // a stale STOP can never ACK a newer intent (even same sessionId).
            if let directive = mintQualifiedStopDirective(
                sessionId: request.sessionId,
                intentSequence: request.intentSequence,
                requestId: request.requestId,
                sequence: request.sequence,
                reason: "keyboard_\(request.command.rawValue)"
            ) {
                // Single-writer: any previous in-flight stop ACK is superseded.
                pendingStopAcknowledgmentDirective = directive
                effects.append(.stopHardwareKeyboard(directive: directive))
            }
            // Note: completed snapshot will be written after stop completion confirms
            return effects
        }
    }

    // MARK: - Asynchronous Permission & Hardware Completion Intake
    public func handlePermissionCallback(token: SessionToken, granted: Bool) -> [FlowEffect] {
        // Revalidate fencing conditions:
        // 1. Cancel generation must match (not cancelled by disarm or newer start);
        //    terminal overflow fail-closed permanently fences every token.
        guard !isCancellationGenerationFailClosed else { return [] }
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
    ) -> (effects: [FlowEffect], cleanup: HardwareStartCleanupDirective?) {
        // Check fence:
        let isStale = (token.cancelGeneration != monotonicCancelGeneration) ||
                      (isCancellationGenerationFailClosed) ||
                      (!isMicrophoneArmedByUser) ||
                      (admissionGate?.activeSessionId != token.sessionId) ||
                      (admissionGate?.lastAppliedRequestId != token.requestId) ||
                      (pendingStartToken != token)

        if isStale {
            // Stale success: request a generation-bound conditional cleanup of ONLY
            // the native capture generation this stale start produced. If a newer
            // keyboard or manual generation owns the hardware, the conditional stop
            // no-ops. If A already stopped (generation advanced), it also no-ops.
            if case .success(let nativeGeneration) = result {
                return ([], HardwareStartCleanupDirective(nativeGeneration: nativeGeneration, token: token))
            }
            return ([], nil)
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
            ], nil)

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
            ], nil)
        }
    }

    /// Hardware stop completion intake, keyed by the exact stop directive that was
    /// issued. A completion whose directive does not exactly match a pending
    /// registered directive (epoch, session, intent, request, sequence, operation id)
    /// is stale and produces ZERO effects — it can never ACK the current session,
    /// even when a newer intent reuses the same sessionId.
    public func handleHardwareStopCompletion(directive: HardwareStopDirective) -> [FlowEffect] {
        var effects: [FlowEffect] = []

        // Exact replacement-barrier matching: only the directive issued when B
        // replaced A may release the barrier and start B.
        if let pendingReplacement = pendingReplacementStopDirective, pendingReplacement == directive {
            pendingReplacementStopDirective = nil
            isWaitingForHardwareStopToStartReplacement = false

            // If we were waiting for stop completion to start a replacement session:
            if let nextToken = pendingReplacementStartToken {
                isWaitingForHardwareStopToStartReplacement = false
                pendingReplacementStartToken = nil

                // Check if nextToken is still valid and not cancelled while waiting
                if nextToken.cancelGeneration == monotonicCancelGeneration &&
                   !isCancellationGenerationFailClosed &&
                   isMicrophoneArmedByUser &&
                   admissionGate?.activeSessionId == nextToken.sessionId &&
                   admissionGate?.lastAppliedRequestId == nextToken.requestId {

                    pendingStartToken = nextToken
                    captureOwner = .keyboard(sessionId: nextToken.sessionId, intentSequence: nextToken.intentSequence)
                    effects.append(.requestPermission(token: nextToken))
                }
                return effects
            }
            return effects
        }

        // Exact ACK matching for admitted keyboard stops:
        guard let pendingAck = pendingStopAcknowledgmentDirective, pendingAck == directive else {
            // Unknown, superseded, or already-consumed stop completion: no effects,
            // no response overwrite of the current latest slot.
            return effects
        }

        // Fencing: a stale STOP completes must never ACK a newer intent, including
        // same-sessionId reuse with a higher intent. Compare exact intent identity.
        // The pending ACK directive is only consumed when every fence passes.
        if let currentActive = activeSessionToken, currentActive.intentSequence > directive.intentSequence {
            return effects
        }
        if let pending = pendingStartToken, pending.intentSequence > directive.intentSequence {
            return effects
        }
        if let pendingRep = pendingReplacementStartToken, pendingRep.intentSequence > directive.intentSequence {
            return effects
        }
        pendingStopAcknowledgmentDirective = nil

        effects.append(.writeResponse(
            sessionId: directive.sessionId,
            requestId: directive.requestId,
            sequence: directive.sequence,
            state: .completed,
            recognizedText: nil,
            isFinal: true,
            errorCode: nil
        ))
        effects.append(.notifyStopAdmitted(sessionId: directive.sessionId, intentSequence: directive.intentSequence))
        return effects
    }

    // MARK: - Injectable CLI Test Seam (Overflow)
    /// Small, injectable seam used ONLY by the pure Swift CLI test executable to
    /// drive the cancellation generation directly into the overflow transition and
    /// assert terminal fail-closed behavior without waiting for UInt64.MAX events.
    /// Exercises the exact same checked-increment path used in production.
    public func debugForceCancellationGenerationOverflowForCLITests() {
        monotonicCancelGeneration = UInt64.max
        // Next advance attempts to exceed UInt64.max and must latch fail-closed.
        advanceCancelGeneration()
        pendingStartToken = nil
        pendingReplacementStartToken = nil
        isWaitingForHardwareStopToStartReplacement = false
        pendingReplacementStopDirective = nil
    }
}
