import Foundation

/// Delegate protocol for `WarmIPCService` events.
public protocol WarmIPCServiceDelegate: AnyObject {
    func warmIPCService(_ service: WarmIPCService, didAdmitStartSession sessionId: String, intentSequence: UInt64)
    func warmIPCService(_ service: WarmIPCService, didAdmitStopSession sessionId: String, intentSequence: UInt64)
    func warmIPCService(_ service: WarmIPCService, didRejectRequest description: String)
    func warmIPCService(_ service: WarmIPCService, didEncounterBlockedState reason: String)
}

/// App-process-owned warm IPC coordinator service.
///
/// Responsibilities:
/// 1. Polls and validates incoming requests from App Group `UserDefaults` (`echolet.keyboard.request.v2`).
/// 2. Integrates `EcholetAdmission.Gate` with the containing app process epoch.
/// 3. Writes responsive `EcholetIPC.AppResponse` snapshots with strictly monotonic revisions, matching ACK,
///    and honest app state (requested, preparing, listening, blocked, completed).
/// 4. Distinguishes receipt of START from affirmative user arming:
///    - If user has armed microphone and permission is granted, START may begin real audio capture.
///    - If user has NOT armed microphone or permission is denied, writes `state: .blocked` snapshot.
/// 5. Posts optional Darwin notifications on response write.
/// 6. Safely handles stale requests, tombstones, and session replacement without hot mic leaks.
public final class WarmIPCService {

    public static let shared = WarmIPCService()

    public weak var delegate: WarmIPCServiceDelegate?

    public private(set) var admissionGate: EcholetAdmission.Gate?
    private var sharedDefaults: UserDefaults?
    private var responseRevision: UInt64 = 0
    private var isMicrophoneArmedByUser: Bool = false
    private var pollTimer: Timer?

    private init() {
        self.sharedDefaults = UserDefaults(suiteName: AppDelegate.appGroupId)
        initializeGate()
    }

    public func initializeGate() {
        do {
            let epoch = AppDelegate.sharedEpoch
            let gate = try EcholetAdmission.Gate(appEpoch: epoch, coldBootArmed: true)
            // Foreground / active app authorizes boot gate
            gate.authorizeBoot()
            self.admissionGate = gate
        } catch {
            print("[WarmIPCService] Failed to initialize admission gate: \(error)")
        }
    }

    // MARK: - User Arming State
    public func setUserArmMicrophone(_ armed: Bool) {
        self.isMicrophoneArmedByUser = armed
    }

    public var isUserArmed: Bool {
        return isMicrophoneArmedByUser
    }

    // MARK: - Foreground Polling Lifecycle
    public func startPolling(interval: TimeInterval = 0.3) {
        stopPolling()
        pollTimer = Timer.scheduledTimer(withTimeInterval: interval, repeats: true) { [weak self] _ in
            self?.pollIncomingRequests()
        }
    }

    public func stopPolling() {
        pollTimer?.invalidate()
        pollTimer = nil
    }

    // MARK: - Poll & Admission Logic
    public func pollIncomingRequests() {
        guard let defaults = sharedDefaults else {
            delegate?.warmIPCService(self, didEncounterBlockedState: "App Group unavailable")
            return
        }

        guard let gate = admissionGate else {
            delegate?.warmIPCService(self, didEncounterBlockedState: "Admission gate uninitialized")
            return
        }

        guard let requestData = defaults.data(forKey: EcholetIPC.keyboardRequestKey) else {
            return
        }

        do {
            let decoder = EcholetIPC.makeDecoder()
            let request = try decoder.decode(EcholetIPC.KeyboardRequest.self, from: requestData)

            // Skip if this request was already admitted and applied
            if let lastApplied = gate.lastAppliedRequestId, lastApplied == request.requestId {
                return
            }

            let outcome = gate.admitRequest(request)
            switch outcome {
            case .success(let admission):
                handleAdmittedRequest(request, admission: admission)
            case .failure(let rejection):
                handleRejectedRequest(request, rejection: rejection)
            }
        } catch {
            delegate?.warmIPCService(self, didRejectRequest: "Malformed request payload: \(error.localizedDescription)")
        }
    }

    private func handleAdmittedRequest(_ request: EcholetIPC.KeyboardRequest, admission: EcholetAdmission.AdmissionOutcome) {
        switch request.command {
        case .start:
            if case .replacedPriorSession(let retired) = admission {
                // If previous capture was running, stop it first before arming new session
                AudioCaptureController.shared.stopCapture()
                delegate?.warmIPCService(self, didAdmitStopSession: retired, intentSequence: request.intentSequence)
            }

            // Distinguish START receipt from affirmative user arming and permission
            if isMicrophoneArmedByUser {
                AudioCaptureController.shared.requestMicrophonePermission { [weak self] granted in
                    guard let self = self else { return }
                    if granted {
                        // Begin real capture and write listening snapshot
                        AudioCaptureController.shared.startCapture()
                        self.writeResponseSnapshot(
                            sessionId: request.sessionId,
                            acknowledgedRequestId: request.requestId,
                            acknowledgedSequence: request.sequence,
                            state: .listening,
                            recognizedText: nil,
                            isFinal: false,
                            errorCode: nil
                        )
                        self.delegate?.warmIPCService(self, didAdmitStartSession: request.sessionId, intentSequence: request.intentSequence)
                    } else {
                        // Permission denied -> write blocked snapshot
                        self.writeResponseSnapshot(
                            sessionId: request.sessionId,
                            acknowledgedRequestId: request.requestId,
                            acknowledgedSequence: request.sequence,
                            state: .blocked,
                            recognizedText: nil,
                            isFinal: true,
                            errorCode: "microphone_permission_denied"
                        )
                        self.delegate?.warmIPCService(self, didEncounterBlockedState: "Microphone permission denied")
                    }
                }
            } else {
                // App not armed by user -> write blocked snapshot requiring explicit foreground arm
                writeResponseSnapshot(
                    sessionId: request.sessionId,
                    acknowledgedRequestId: request.requestId,
                    acknowledgedSequence: request.sequence,
                    state: .blocked,
                    recognizedText: nil,
                    isFinal: false,
                    errorCode: "app_microphone_not_armed"
                )
                delegate?.warmIPCService(self, didEncounterBlockedState: "Microphone not armed in containing app. Open Echolet and enable mic test.")
            }

        case .stop, .cancel:
            AudioCaptureController.shared.stopCapture()
            admissionGate?.endActiveSession()

            writeResponseSnapshot(
                sessionId: request.sessionId,
                acknowledgedRequestId: request.requestId,
                acknowledgedSequence: request.sequence,
                state: .completed,
                recognizedText: nil, // J3A audio probe only - no fabricated transcript
                isFinal: true,
                errorCode: nil
            )
            delegate?.warmIPCService(self, didAdmitStopSession: request.sessionId, intentSequence: request.intentSequence)
        }
    }

    private func handleRejectedRequest(_ request: EcholetIPC.KeyboardRequest, rejection: EcholetAdmission.AdmissionRejection) {
        delegate?.warmIPCService(self, didRejectRequest: "Request \(request.requestId) rejected: \(rejection)")
    }

    // MARK: - Write Response Snapshots
    public func writeResponseSnapshot(
        sessionId: String,
        acknowledgedRequestId: String,
        acknowledgedSequence: UInt64,
        state: EcholetIPC.AppState,
        recognizedText: String?,
        isFinal: Bool,
        errorCode: String?
    ) {
        guard let defaults = sharedDefaults else { return }

        responseRevision += 1
        do {
            let response = try EcholetIPC.AppResponse(
                appEpoch: AppDelegate.sharedEpoch,
                sessionId: sessionId,
                acknowledgedRequestId: acknowledgedRequestId,
                acknowledgedSequence: acknowledgedSequence,
                revision: responseRevision,
                state: state,
                recognizedText: recognizedText,
                isFinal: isFinal,
                errorCode: errorCode,
                serverTimestampMs: UInt64(Date().timeIntervalSince1970 * 1000)
            )

            let encoder = EcholetIPC.makeEncoder()
            let data = try encoder.encode(response)
            defaults.set(data, forKey: EcholetIPC.appResponseKey)
            defaults.synchronize()

            // Post Darwin notification hint
            let notificationName = CFNotificationName(EcholetIPC.darwinNotificationResponse as CFString)
            CFNotificationCenterPostNotification(
                CFNotificationCenterGetDarwinNotifyCenter(),
                notificationName,
                nil,
                nil,
                true
            )
        } catch {
            print("[WarmIPCService] Failed to write response snapshot: \(error)")
        }
    }
}
