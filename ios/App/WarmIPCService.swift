import Foundation
import CoreFoundation

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
/// 1. Lifecycle retained by the app process (AppDelegate / SceneDelegate), independent of UIViewController appearance.
/// 2. Reads latest App Group request on launch/activation and polls at a bounded cadence (e.g. 0.3s) while foreground/active.
/// 3. Darwin notification `com.echolet.ipc.request.v2` observer triggers an immediate re-read hint.
/// 4. Integrates `EcholetAdmission.Gate` with the containing app process epoch.
/// 5. Async fences:
///    - On START: checks permission and user arming. Emits `preparing` while asynchronous engine start is in flight.
///    - Emits `listening` ONLY AFTER `AudioCaptureController.startCapture` confirms engine start succeeded for this exact session token.
///    - If user has NOT armed mic or permission is denied, emits `blocked`.
///    - On STOP/CANCEL: synchronously ends gate session, calls `AudioCaptureController.stopCapture` and writes `completed` ACK ONLY AFTER hardware stops.
///    - Fences monotonic response revision and matching sequence/request_id.
/// 6. Honest background semantics:
///    - While active background recording is ongoing, iOS schedules the app via `UIBackgroundModes audio`. Command intake continues as long as scheduled.
///    - If suspended/idle, does NOT claim cold wake or stealth background recording.
public final class WarmIPCService {

    public static let shared = WarmIPCService()

    public weak var delegate: WarmIPCServiceDelegate?

    public private(set) var admissionGate: EcholetAdmission.Gate?
    private var sharedDefaults: UserDefaults?
    private var responseRevision: UInt64 = 0
    private var isMicrophoneArmedByUser: Bool = false
    private var pollTimer: Timer?
    private var darwinObserverInstalled = false

    /// Serial queue to sequence all IPC polling, admission, and response mutations
    public let ipcQueue = DispatchQueue(label: "com.echolet.warmipc.service", qos: .userInitiated)

    /// Currently active session binding token (sessionId, intentSequence, requestId, sequence)
    private struct PendingSessionToken: Equatable {
        let appEpoch: String
        let sessionId: String
        let intentSequence: UInt64
        let requestId: String
        let sequence: UInt64
        let captureGeneration: UInt64
    }

    private var activeSessionToken: PendingSessionToken?

    private init() {
        self.sharedDefaults = UserDefaults(suiteName: AppDelegate.appGroupId)
        initializeGate()
        setupDarwinObserver()
    }

    deinit {
        teardownDarwinObserver()
        stopPolling()
    }

    public func initializeGate() {
        ipcQueue.sync {
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
    }

    // MARK: - Darwin Notification Observer
    private func setupDarwinObserver() {
        guard !darwinObserverInstalled else { return }
        let center = CFNotificationCenterGetDarwinNotifyCenter()
        let name = CFNotificationName(EcholetIPC.darwinNotificationRequest as CFString)

        CFNotificationCenterAddObserver(
            center,
            Unmanaged.passUnretained(self).toOpaque(),
            { _, observer, _, _, _ in
                guard let observer = observer else { return }
                let service = Unmanaged<WarmIPCService>.fromOpaque(observer).takeUnretainedValue()
                service.handleDarwinNotification()
            },
            name.rawValue,
            nil,
            .deliverImmediately
        )
        darwinObserverInstalled = true
    }

    private func teardownDarwinObserver() {
        guard darwinObserverInstalled else { return }
        let center = CFNotificationCenterGetDarwinNotifyCenter()
        let name = CFNotificationName(EcholetIPC.darwinNotificationRequest as CFString)
        CFNotificationCenterRemoveObserver(center, Unmanaged.passUnretained(self).toOpaque(), name, nil)
        darwinObserverInstalled = false
    }

    private func handleDarwinNotification() {
        ipcQueue.async { [weak self] in
            self?.pollIncomingRequestsInternal()
        }
    }

    // MARK: - User Arming State
    public func setUserArmMicrophone(_ armed: Bool) {
        ipcQueue.async { [weak self] in
            guard let self = self else { return }
            self.isMicrophoneArmedByUser = armed
            if !armed {
                // Disarming mic stops any pending/active recording immediately
                self.activeSessionToken = nil
                self.admissionGate?.endActiveSession()
                AudioCaptureController.shared.stopCapture()
            }
        }
    }

    public var isUserArmed: Bool {
        return ipcQueue.sync { self.isMicrophoneArmedByUser }
    }

    // MARK: - Polling Lifecycle (App-owned)
    public func startPolling(interval: TimeInterval = 0.3) {
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            self.stopPolling()
            self.pollTimer = Timer.scheduledTimer(withTimeInterval: interval, repeats: true) { [weak self] _ in
                self?.ipcQueue.async {
                    self?.pollIncomingRequestsInternal()
                }
            }
            // Trigger immediate read upon start
            self.ipcQueue.async {
                self.pollIncomingRequestsInternal()
            }
        }
    }

    public func stopPolling() {
        DispatchQueue.main.async { [weak self] in
            self?.pollTimer?.invalidate()
            self?.pollTimer = nil
        }
    }

    // MARK: - Poll & Admission Logic
    public func pollIncomingRequests() {
        ipcQueue.async { [weak self] in
            self?.pollIncomingRequestsInternal()
        }
    }

    private func pollIncomingRequestsInternal() {
        guard let defaults = sharedDefaults else {
            notifyBlocked("App Group unavailable")
            return
        }

        guard let gate = admissionGate else {
            notifyBlocked("Admission gate uninitialized")
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
            notifyRejected("Malformed request payload: \(error.localizedDescription)")
        }
    }

    private func handleAdmittedRequest(_ request: EcholetIPC.KeyboardRequest, admission: EcholetAdmission.AdmissionOutcome) {
        switch request.command {
        case .start:
            if case .replacedPriorSession(let retired) = admission {
                // Clean replacement: stop previous hardware capture before starting new session
                self.activeSessionToken = nil
                AudioCaptureController.shared.stopCapture()
                notifyStopAdmitted(sessionId: retired, intentSequence: request.intentSequence)
            }

            // Distinguish START receipt from affirmative user arming and permission
            guard self.isMicrophoneArmedByUser else {
                // App not armed by user -> write blocked snapshot requiring explicit arm
                self.writeResponseSnapshot(
                    sessionId: request.sessionId,
                    acknowledgedRequestId: request.requestId,
                    acknowledgedSequence: request.sequence,
                    state: .blocked,
                    recognizedText: nil,
                    isFinal: false,
                    errorCode: "app_microphone_not_armed"
                )
                self.notifyBlocked("Microphone not armed in containing app. Open Echolet and enable mic test.")
                return
            }

            // Emit preparing state first while asynchronous start is underway
            self.writeResponseSnapshot(
                sessionId: request.sessionId,
                acknowledgedRequestId: request.requestId,
                acknowledgedSequence: request.sequence,
                state: .preparing,
                recognizedText: nil,
                isFinal: false,
                errorCode: nil
            )

            // Asynchronously preflight permission and start native audio engine
            AudioCaptureController.shared.requestMicrophonePermission { [weak self] granted in
                guard let self = self else { return }

                self.ipcQueue.async {
                    // Check if request or session was invalidated / replaced while permission was checking
                    guard let currentGate = self.admissionGate,
                          currentGate.activeSessionId == request.sessionId,
                          currentGate.lastAppliedRequestId == request.requestId else {
                        // Stale permission callback - fenced out
                        return
                    }

                    if granted {
                        // Start real capture and bind generation token
                        let gen = AudioCaptureController.shared.startCapture { [weak self] result in
                            guard let self = self else { return }
                            self.ipcQueue.async {
                                // Fencing: Ensure active session is still this exact token
                                guard let currentGate = self.admissionGate,
                                      currentGate.activeSessionId == request.sessionId,
                                      currentGate.lastAppliedRequestId == request.requestId else {
                                    return
                                }

                                switch result {
                                case .success(let activeGen):
                                    self.activeSessionToken = PendingSessionToken(
                                        appEpoch: request.appEpoch,
                                        sessionId: request.sessionId,
                                        intentSequence: request.intentSequence,
                                        requestId: request.requestId,
                                        sequence: request.sequence,
                                        captureGeneration: activeGen
                                    )
                                    // Hardware start confirmed: emit honest listening state
                                    self.writeResponseSnapshot(
                                        sessionId: request.sessionId,
                                        acknowledgedRequestId: request.requestId,
                                        acknowledgedSequence: request.sequence,
                                        state: .listening,
                                        recognizedText: nil,
                                        isFinal: false,
                                        errorCode: nil
                                    )
                                    self.notifyStartAdmitted(sessionId: request.sessionId, intentSequence: request.intentSequence)

                                case .failure(let err):
                                    self.activeSessionToken = nil
                                    self.admissionGate?.endActiveSession()
                                    self.writeResponseSnapshot(
                                        sessionId: request.sessionId,
                                        acknowledgedRequestId: request.requestId,
                                        acknowledgedSequence: request.sequence,
                                        state: .blocked,
                                        recognizedText: nil,
                                        isFinal: true,
                                        errorCode: "audio_engine_start_failed: \(err.localizedDescription)"
                                    )
                                    self.notifyBlocked("Audio engine start failed: \(err.localizedDescription)")
                                }
                            }
                        }
                        _ = gen
                    } else {
                        // Permission denied
                        self.activeSessionToken = nil
                        self.admissionGate?.endActiveSession()
                        self.writeResponseSnapshot(
                            sessionId: request.sessionId,
                            acknowledgedRequestId: request.requestId,
                            acknowledgedSequence: request.sequence,
                            state: .blocked,
                            recognizedText: nil,
                            isFinal: true,
                            errorCode: "microphone_permission_denied"
                        )
                        self.notifyBlocked("Microphone permission denied")
                    }
                }
            }

        case .stop, .cancel:
            self.activeSessionToken = nil
            self.admissionGate?.endActiveSession()

            // Wait for native capture controller to stop and deallocate tap before sending completed
            AudioCaptureController.shared.stopCapture { [weak self] in
                guard let self = self else { return }
                self.ipcQueue.async {
                    self.writeResponseSnapshot(
                        sessionId: request.sessionId,
                        acknowledgedRequestId: request.requestId,
                        acknowledgedSequence: request.sequence,
                        state: .completed,
                        recognizedText: nil, // Probe mode only - no fabricated transcript
                        isFinal: true,
                        errorCode: nil
                    )
                    self.notifyStopAdmitted(sessionId: request.sessionId, intentSequence: request.intentSequence)
                }
            }
        }
    }

    private func handleRejectedRequest(_ request: EcholetIPC.KeyboardRequest, rejection: EcholetAdmission.AdmissionRejection) {
        notifyRejected("Request \(request.requestId) rejected: \(rejection)")
    }

    // MARK: - Delegates Notification Helpers
    private func notifyStartAdmitted(sessionId: String, intentSequence: UInt64) {
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            self.delegate?.warmIPCService(self, didAdmitStartSession: sessionId, intentSequence: intentSequence)
        }
    }

    private func notifyStopAdmitted(sessionId: String, intentSequence: UInt64) {
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            self.delegate?.warmIPCService(self, didAdmitStopSession: sessionId, intentSequence: intentSequence)
        }
    }

    private func notifyRejected(_ msg: String) {
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            self.delegate?.warmIPCService(self, didRejectRequest: msg)
        }
    }

    private func notifyBlocked(_ msg: String) {
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            self.delegate?.warmIPCService(self, didEncounterBlockedState: msg)
        }
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
