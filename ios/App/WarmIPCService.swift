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
/// 4. Integrates pure Foundation `EcholetAdmission.Gate` and `WarmCaptureFlowCoordinator` with the containing app process epoch.
/// 5. Async fences & Ownership:
///    - On START: checks user arming and permission. Emits `preparing` while asynchronous engine start is in flight.
///    - Emits `listening` ONLY AFTER `AudioCaptureController.startCapture` confirms engine start succeeded for this exact session token.
///    - If user has NOT armed mic or permission is denied, emits `blocked`.
///    - Differentiates manual audio test capture from keyboard-owned capture. If manual audio test is active, keyboard START is rejected as busy.
///    - On STOP/CANCEL: cancels pending token synchronously; halts native hardware ONLY if keyboard owned; emits `completed` ACK only after hardware stops.
///    - On session replacement (B replaces A): hardware Stop(A) is awaited before initiating native start for B.
///    - Late old Stop(A) completion cannot overwrite active session B response.
///    - Fences monotonic response revision and matching sequence/request_id with fail-closed revision checks.
/// 6. Honest background semantics:
///    - While active background recording is ongoing, iOS schedules the app via `UIBackgroundModes audio`. Command intake continues as long as scheduled.
///    - If suspended/idle, does NOT claim cold wake or stealth background recording.
public final class WarmIPCService {

    public static let shared = WarmIPCService()

    public weak var delegate: WarmIPCServiceDelegate?

    public private(set) var flowCoordinator: WarmCaptureFlowCoordinator
    private var sharedDefaults: UserDefaults?
    private var pollTimer: Timer?
    private var darwinObserverInstalled = false

    /// Serial queue to sequence all IPC polling, admission, and response mutations
    public let ipcQueue = DispatchQueue(label: "com.echolet.warmipc.service", qos: .userInitiated)

    public var admissionGate: EcholetAdmission.Gate? {
        return ipcQueue.sync { flowCoordinator.admissionGate }
    }

    private init() {
        self.flowCoordinator = WarmCaptureFlowCoordinator(appEpoch: AppDelegate.sharedEpoch)
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
                self.flowCoordinator.setAdmissionGate(gate)
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
            let effects = self.flowCoordinator.setUserArmMicrophone(armed)
            self.executeFlowEffects(effects, triggeringRequestId: nil, triggeringSessionId: nil, triggeringSequence: nil)
        }
    }

    public var isUserArmed: Bool {
        return ipcQueue.sync { self.flowCoordinator.isMicrophoneArmedByUser }
    }

    // MARK: - Manual Audio Capture Ownership Bridge
    public func registerManualStartCapture(completion: @escaping (Bool) -> Void) {
        ipcQueue.async { [weak self] in
            guard let self = self else {
                completion(false)
                return
            }
            let (shouldProceed, effects) = self.flowCoordinator.handleManualStartCaptureInitiated()
            self.executeFlowEffects(effects, triggeringRequestId: nil, triggeringSessionId: nil, triggeringSequence: nil)
            completion(shouldProceed)
        }
    }

    public func registerManualCaptureStopped() {
        ipcQueue.async { [weak self] in
            guard let self = self else { return }
            let effects = self.flowCoordinator.handleManualCaptureStopped()
            self.executeFlowEffects(effects, triggeringRequestId: nil, triggeringSessionId: nil, triggeringSequence: nil)
        }
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

    // MARK: - Poll & Intake Logic
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

        guard let requestData = defaults.data(forKey: EcholetIPC.keyboardRequestKey) else {
            return
        }

        do {
            let decoder = EcholetIPC.makeDecoder()
            let request = try decoder.decode(EcholetIPC.KeyboardRequest.self, from: requestData)

            let effects = self.flowCoordinator.handleIncomingRequest(request)
            self.executeFlowEffects(
                effects,
                triggeringRequestId: request.requestId,
                triggeringSessionId: request.sessionId,
                triggeringSequence: request.sequence
            )
        } catch {
            notifyRejected("Malformed request payload: \(error.localizedDescription)")
        }
    }

    // MARK: - Effect Execution Engine (ipcQueue Only)
    private func executeFlowEffects(
        _ effects: [WarmCaptureFlowCoordinator.FlowEffect],
        triggeringRequestId: String?,
        triggeringSessionId: String?,
        triggeringSequence: UInt64?
    ) {
        for effect in effects {
            switch effect {
            case .none:
                break

            case .writeResponse(let sessionId, let requestId, let sequence, let state, let text, let isFinal, let errorCode):
                self.writeResponseSnapshot(
                    sessionId: sessionId,
                    acknowledgedRequestId: requestId,
                    acknowledgedSequence: sequence,
                    state: state,
                    recognizedText: text,
                    isFinal: isFinal,
                    errorCode: errorCode
                )

            case .requestPermission(let token):
                AudioCaptureController.shared.requestMicrophonePermission { [weak self] granted in
                    guard let self = self else { return }
                    self.ipcQueue.async {
                        let permEffects = self.flowCoordinator.handlePermissionCallback(token: token, granted: granted)
                        self.executeFlowEffects(
                            permEffects,
                            triggeringRequestId: token.requestId,
                            triggeringSessionId: token.sessionId,
                            triggeringSequence: token.sequence
                        )
                    }
                }

            case .startHardware(let token):
                AudioCaptureController.shared.startCapture { [weak self] result in
                    guard let self = self else { return }
                    self.ipcQueue.async {
                        let (startEffects, cleanupNeeded) = self.flowCoordinator.handleHardwareStartCompletion(
                            token: token,
                            result: result.mapError { $0 as Error }
                        )
                        if cleanupNeeded {
                            AudioCaptureController.shared.stopCapture()
                        }
                        self.executeFlowEffects(
                            startEffects,
                            triggeringRequestId: token.requestId,
                            triggeringSessionId: token.sessionId,
                            triggeringSequence: token.sequence
                        )
                    }
                }

            case .stopHardwareKeyboard:
                // Hardware stop completion callback reenters ipcQueue
                AudioCaptureController.shared.stopCapture { [weak self] in
                    guard let self = self else { return }
                    self.ipcQueue.async {
                        let stopEffects = self.flowCoordinator.handleHardwareStopCompletion(
                            stoppedRequestId: triggeringRequestId,
                            stoppedSessionId: triggeringSessionId,
                            stoppedSequence: triggeringSequence
                        )
                        self.executeFlowEffects(
                            stopEffects,
                            triggeringRequestId: triggeringRequestId,
                            triggeringSessionId: triggeringSessionId,
                            triggeringSequence: triggeringSequence
                        )
                    }
                }

            case .stopHardwareManual:
                AudioCaptureController.shared.stopCapture()

            case .notifyStartAdmitted(let sessionId, let intentSequence):
                self.notifyStartAdmitted(sessionId: sessionId, intentSequence: intentSequence)

            case .notifyStopAdmitted(let sessionId, let intentSequence):
                self.notifyStopAdmitted(sessionId: sessionId, intentSequence: intentSequence)

            case .notifyBlocked(let reason):
                self.notifyBlocked(reason)

            case .notifyRejected(let reason):
                self.notifyRejected(reason)
            }
        }
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

    // MARK: - Write Response Snapshots (ipcQueue Only)
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

        guard let rev = flowCoordinator.nextResponseRevision() else {
            print("[WarmIPCService] Response revision overflowed UInt64.max; failing closed.")
            return
        }

        do {
            let response = try EcholetIPC.AppResponse(
                appEpoch: AppDelegate.sharedEpoch,
                sessionId: sessionId,
                acknowledgedRequestId: acknowledgedRequestId,
                acknowledgedSequence: acknowledgedSequence,
                revision: rev,
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
