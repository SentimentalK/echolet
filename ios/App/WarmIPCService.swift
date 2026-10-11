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
    public func registerManualStartCapture(completion: @escaping (Bool, String?) -> Void) {
        ipcQueue.async { [weak self] in
            guard let self = self else {
                completion(false, "warm ipc unavailable")
                return
            }
            let (shouldProceed, busyReason, effects) = self.flowCoordinator.handleManualStartCaptureInitiated()
            self.executeFlowEffects(effects, triggeringRequestId: nil, triggeringSessionId: nil, triggeringSequence: nil)
            completion(shouldProceed, shouldProceed ? nil : busyReason)
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
                self.writeResponseSnapshotOnQueue(
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
                        let (startEffects, startCleanup) = self.flowCoordinator.handleHardwareStartCompletion(
                            token: token,
                            result: result.mapError { $0 as Error }
                        )
                        if let cleanup = startCleanup {
                            // Generation-bound conditional cleanup: stops ONLY the
                            // stale start's native capture generation. A newer
                            // keyboard/manual generation (or an already-stopped
                            // generation) is never touched.
                            AudioCaptureController.shared.stopCaptureIfGeneration(
                                expectedGeneration: cleanup.nativeGeneration
                            )
                        }
                        self.executeFlowEffects(
                            startEffects,
                            triggeringRequestId: token.requestId,
                            triggeringSessionId: token.sessionId,
                            triggeringSequence: token.sequence
                        )
                    }
                }

            case .stopHardwareKeyboard(let directive):
                // Hardware stop completion callback reenters ipcQueue carrying the
                // exact stop directive identity for ACK correlation.
                AudioCaptureController.shared.stopCapture { [weak self] in
                    guard let self = self else { return }
                    self.ipcQueue.async {
                        let stopEffects = self.flowCoordinator.handleHardwareStopCompletion(directive: directive)
                        self.executeFlowEffects(
                            stopEffects,
                            triggeringRequestId: directive.requestId,
                            triggeringSessionId: directive.sessionId,
                            triggeringSequence: directive.sequence
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
    //
    // Single-writer invariant: ALL response revision increments and App Group
    // response key mutations are owned exclusively by `ipcQueue`. The revision
    // mailbox (`WarmCaptureFlowCoordinator.responseRevision`) and the latest
    // response snapshot are mutated only inside this helper, which is private
    // and must only run from code already executing on `ipcQueue` (the live
    // flow effect engine and the DEBUG mock submit path both enter via
    // `ipcQueue`; DEBUG exposes a dispatchPrecondition to prove it).
    private func writeResponseSnapshotOnQueue(
        sessionId: String,
        acknowledgedRequestId: String,
        acknowledgedSequence: UInt64,
        state: EcholetIPC.AppState,
        recognizedText: String?,
        isFinal: Bool,
        errorCode: String?
    ) -> Bool {
        #if DEBUG
        dispatchPrecondition(condition: .onQueue(ipcQueue))
        #endif

        guard let defaults = sharedDefaults else {
            return false
        }

        guard let rev = flowCoordinator.nextResponseRevision() else {
            print("[WarmIPCService] Response revision overflowed UInt64.max; failing closed.")
            return false
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
            return true
        } catch {
            print("[WarmIPCService] Failed to write response snapshot: \(error)")
            return false
        }
    }

    #if DEBUG
    // MARK: - DEBUG Mock Response Submission (Single-Writer Queue-Owned)
    //
    /// Asynchronous DEBUG mock submit path for the containing app's regression
    /// harness (`AppStatusViewController`). Re-reads and validates the latest
    /// App Group request ON `ipcQueue`, evaluates the read-only eligibility
    /// probe so a stale mock (older intent superseded by a newer session or
    /// STOP, a superseded same-intent request, or a conflicting active session)
    /// can never overwrite the newest response snapshot, and only then writes
    /// the final recognized text through the same queue-owned single-writer
    /// helper as the real IPC path. The completion dispatches to main after the
    /// actual write outcome and never claims success for a rejected or failed
    /// write.
    public func submitDebugMockResponse(
        recognizedText: String,
        completion: @escaping (_ succeeded: Bool, _ rejectionReason: String?) -> Void
    ) {
        ipcQueue.async { [weak self] in
            guard let self = self else {
                DispatchQueue.main.async { completion(false, "WarmIPCService unavailable") }
                return
            }

            guard let defaults = self.sharedDefaults,
                  let requestData = defaults.data(forKey: EcholetIPC.keyboardRequestKey) else {
                DispatchQueue.main.async { completion(false, "No keyboard request to respond to") }
                return
            }

            do {
                let request = try EcholetIPC.makeDecoder().decode(EcholetIPC.KeyboardRequest.self, from: requestData)
                try request.validate()

                guard request.appEpoch == self.flowCoordinator.appEpoch else {
                    DispatchQueue.main.async { completion(false, "Request belongs to different app epoch") }
                    return
                }

                // Serialized together with live intake on ipcQueue: eligibility
                // evaluation and the write below form one atomic queue step.
                let eligibility = self.flowCoordinator.evaluateMockResponseEligibility(request)
                guard eligibility == .eligible else {
                    DispatchQueue.main.async {
                        completion(false, "Mock response rejected: \(eligibility.rejectedReason ?? "unknown")")
                    }
                    return
                }

                let wrote = self.writeResponseSnapshotOnQueue(
                    sessionId: request.sessionId,
                    acknowledgedRequestId: request.requestId,
                    acknowledgedSequence: request.sequence,
                    state: .completed,
                    recognizedText: recognizedText,
                    isFinal: true,
                    errorCode: nil
                )

                DispatchQueue.main.async {
                    if wrote {
                        completion(true, nil)
                    } else {
                        completion(false, "Failed to write App Group response snapshot")
                    }
                }
            } catch {
                DispatchQueue.main.async {
                    completion(false, "Failed to decode latest request: \(error.localizedDescription)")
                }
            }
        }
    }
    #endif
}
