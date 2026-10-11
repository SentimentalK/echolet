import Foundation

/// Pure Foundation port of the minimal wire admission policies from `src/ios_ipc.rs`.
///
/// Designed to be executable both on Mac via `swiftc` command line and in iOS containing apps,
/// with ZERO UIKit dependencies.
public enum EcholetAdmission {

    public enum AdmissionOutcome: Equatable {
        case admitted
        case replacedPriorSession(retiredSessionId: String)
        case tombstoneIgnored(command: EcholetIPC.Command)
    }

    public enum AdmissionRejection: Error, Equatable {
        case invalidPayload(EcholetIPC.ValidationError)
        case staleAppEpoch(incomingEpoch: String, activeEpoch: String)
        case nonMonotonicIntentSequence(incomingIntent: UInt64, lastAdmittedIntent: UInt64)
        case staleSessionCommand(incomingSessionId: String, activeSessionId: String?, command: EcholetIPC.Command)
        case activeSessionConflict(incomingSessionId: String, activeSessionId: String)
        case nonMonotonicSequence(sessionId: String, incomingSeq: UInt64, lastSeq: UInt64)
        case commandOrderViolation(sessionId: String, command: EcholetIPC.Command, reason: String)
        case coldBootFenceReject(reason: String)
    }

    private enum SessionState: Equatable {
        case listening
        case ended
    }

    /// App-process-owned admission gate implementing the exact wire rules from `src/ios_ipc.rs`.
    public final class Gate {
        public private(set) var appEpoch: String
        public private(set) var activeSessionId: String?
        public private(set) var lastSequence: UInt64 = 0
        public private(set) var lastIntentSequence: UInt64 = 0
        public private(set) var coldBootArmed: Bool = true
        public private(set) var lastAppliedRequestId: String?
        private var sessionState: SessionState?

        public init(appEpoch: String, coldBootArmed: Bool = true) throws {
            let trimmed = appEpoch.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !trimmed.isEmpty else {
                throw EcholetIPC.ValidationError.emptyAppEpoch
            }
            self.appEpoch = trimmed
            self.coldBootArmed = coldBootArmed
        }

        public func authorizeBoot() {
            self.coldBootArmed = false
        }

        public func endActiveSession() {
            self.sessionState = .ended
        }

        public func resetSession() {
            self.activeSessionId = nil
            self.sessionState = nil
            self.lastSequence = 0
            self.lastAppliedRequestId = nil
        }

        public func updateAppEpoch(_ newEpoch: String) throws {
            let trimmed = newEpoch.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !trimmed.isEmpty else {
                throw EcholetIPC.ValidationError.emptyAppEpoch
            }
            self.appEpoch = trimmed
            resetSession()
            self.lastIntentSequence = 0
            self.coldBootArmed = false
        }

        public func admitRequest(_ request: EcholetIPC.KeyboardRequest) -> Result<AdmissionOutcome, AdmissionRejection> {
            do {
                try request.validate()
            } catch let err as EcholetIPC.ValidationError {
                return .failure(.invalidPayload(err))
            } catch {
                return .failure(.invalidPayload(.emptyRequestId))
            }

            // 1. Process epoch equality check: MUST match current process launch epoch
            if request.appEpoch != self.appEpoch {
                return .failure(.staleAppEpoch(incomingEpoch: request.appEpoch, activeEpoch: self.appEpoch))
            }

            // 2. Cold-boot fence check
            if self.coldBootArmed {
                return .failure(.coldBootFenceReject(reason: "app cold-boot fence active; cannot execute unverified cached request"))
            }

            // 3. Global intent sequence monotonic check across sessions
            if request.intentSequence <= self.lastIntentSequence {
                return .failure(.nonMonotonicIntentSequence(incomingIntent: request.intentSequence, lastAdmittedIntent: self.lastIntentSequence))
            }

            // 4. Duplicate request check
            if let lastReq = self.lastAppliedRequestId, lastReq == request.requestId {
                return .failure(.commandOrderViolation(sessionId: request.sessionId, command: request.command, reason: "duplicate request_id already applied"))
            }

            switch request.command {
            case .start:
                if let currentId = self.activeSessionId {
                    if currentId == request.sessionId {
                        return .failure(.commandOrderViolation(sessionId: request.sessionId, command: .start, reason: "session already started"))
                    }

                    // A new session arrives with sequence == 1 and higher intentSequence:
                    // Clean replacement of prior session
                    if request.sequence == 1 {
                        let retired = currentId
                        self.activeSessionId = request.sessionId
                        self.sessionState = .listening
                        self.lastSequence = request.sequence
                        self.lastIntentSequence = request.intentSequence
                        self.lastAppliedRequestId = request.requestId
                        return .success(.replacedPriorSession(retiredSessionId: retired))
                    }

                    // If sequence > 1 for a new session while prior session is still active:
                    if self.sessionState != .ended {
                        return .failure(.activeSessionConflict(incomingSessionId: request.sessionId, activeSessionId: currentId))
                    }
                }

                // New session starts cleanly (no prior session active)
                self.activeSessionId = request.sessionId
                self.sessionState = .listening
                self.lastSequence = request.sequence
                self.lastIntentSequence = request.intentSequence
                self.lastAppliedRequestId = request.requestId
                return .success(.admitted)

            case .stop, .cancel:
                if let currentId = self.activeSessionId {
                    if currentId == request.sessionId {
                        if request.sequence <= self.lastSequence {
                            return .failure(.nonMonotonicSequence(sessionId: request.sessionId, incomingSeq: request.sequence, lastSeq: self.lastSequence))
                        }
                        if self.sessionState == .ended {
                            return .failure(.commandOrderViolation(sessionId: request.sessionId, command: request.command, reason: "session already ended"))
                        }

                        self.lastSequence = request.sequence
                        self.lastIntentSequence = request.intentSequence
                        self.sessionState = .ended
                        self.lastAppliedRequestId = request.requestId
                        return .success(.admitted)
                    } else {
                        // Stale command targeting non-active session
                        return .failure(.staleSessionCommand(incomingSessionId: request.sessionId, activeSessionId: self.activeSessionId, command: request.command))
                    }
                } else {
                    // Loss-tolerant tombstone: advance intent watermark and record request
                    self.lastIntentSequence = request.intentSequence
                    self.lastAppliedRequestId = request.requestId
                    return .success(.tombstoneIgnored(command: request.command))
                }
            }
        }

    // MARK: - DEBUG Mock Response Eligibility Probe (read-only)
    ///
    /// Non-mutating mirror of the wire admission ordering used by the containing
    /// app's DEBUG mock response path (`WarmIPCService.submitDebugMockResponse`).
    /// A stale mock (older intent sequence superseded by a newer session or
    /// STOP, a superseded same-intent request, or a conflicting active session)
    /// is rejected before it could overwrite the newest App Group response.
    public enum MockResponseEligibility: Equatable {
        case eligible
        case rejected(reason: String)

        public var rejectedReason: String? {
            if case .rejected(let reason) = self { return reason }
            return nil
        }
    }

    public func evaluateMockResponseEligibility(_ request: EcholetIPC.KeyboardRequest) -> MockResponseEligibility {
        do {
            try request.validate()
        } catch let err as EcholetIPC.ValidationError {
            return .rejected(reason: "invalidPayload: \(err)")
        } catch {
            return .rejected(reason: "invalidPayload")
        }

        guard request.appEpoch == self.appEpoch else {
            return .rejected(reason: "staleEpoch request=\(request.appEpoch) active=\(self.appEpoch)")
        }

        guard !self.coldBootArmed else {
            return .rejected(reason: "coldBootFenceActive")
        }

        // Intent fence: a mock may only answer the CURRENT intent. Anything
        // strictly older than the watermark can never overwrite a newer
        // session's or STOP's response slot.
        guard request.intentSequence >= self.lastIntentSequence else {
            return .rejected(reason: "staleIntent mockIntent=\(request.intentSequence) watermark=\(self.lastIntentSequence)")
        }

        guard request.command == .start else {
            return .rejected(reason: "nonStartCommand \(request.command.rawValue)")
        }

        if request.intentSequence == self.lastIntentSequence {
            // Equal watermark: only refining the exact most-recently-applied
            // START request is allowed (e.g. mock .completed after real
            // .listening). A different same-intent request is superseded.
            guard request.requestId == self.lastAppliedRequestId else {
                return .rejected(reason: "supersededSameIntent requestId=\(request.requestId) lastApplied=\(self.lastAppliedRequestId ?? "nil")")
            }
            return .eligible
        }

        // Newer intent not yet applied by the live intake: mirror admission
        // ordering without mutating any state.
        if let activeId = self.activeSessionId {
            if activeId == request.sessionId {
                return .rejected(reason: "sessionAlreadyStarted active=\(activeId)")
            }
            if request.sequence != 1 && self.sessionState != .ended {
                return .rejected(reason: "activeSessionConflict incoming=\(request.sessionId) active=\(activeId)")
            }
        }
        return .eligible
    }
    }
}
