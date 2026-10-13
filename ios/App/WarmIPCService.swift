import Foundation
import CoreFoundation
import AVFoundation
import UIKit

/// Delegate protocol for `WarmIPCService` events.
public protocol WarmIPCServiceDelegate: AnyObject {
    func warmIPCService(_ service: WarmIPCService, didAdmitStartSession sessionId: String, intentSequence: UInt64)
    func warmIPCService(_ service: WarmIPCService, didAdmitStopSession sessionId: String, intentSequence: UInt64)
    func warmIPCService(_ service: WarmIPCService, didRejectRequest description: String)
    func warmIPCService(_ service: WarmIPCService, didEncounterBlockedState reason: String)
    func warmIPCService(_ service: WarmIPCService, didChangeWarmStandbyActive active: Bool)
    func warmIPCService(_ service: WarmIPCService, didUpdateReturnGuidance text: String?)
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
    private var pollTimer: DispatchSourceTimer?
    private var darwinObserverInstalled = false
    /// J5 activation context; mutated ONLY on `ipcQueue`.
    private var pendingActivation: PendingActivation?

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
        setupStopTranscriptProvider()
        setupLivePartialForwarding()
        setupInterruptionObserver()
    }

    deinit {
        teardownDarwinObserver()
        stopPolling()
    }

    // MARK: - Stop Final Transcript (J4-E2E)
    //
    // The producer of the stop-transcript provider lives on the SAME single
    // writer queue (`ipcQueue`): Stop ASR finalize happens EXACTLY once per
    // admitted keyboard STOP before the hardware stop completion is allowed
    // to publish the completed response snapshot.
    private let pendingFinalTranscriptsLock = NSLock()
    private var pendingFinalTranscripts: [String: String] = [:]
    private var lastReceivedSourceApp: String?

    public func setLastReceivedSourceApp(_ bundleId: String) {
        ipcQueue.async { [weak self] in
            self?.lastReceivedSourceApp = bundleId
            print("[Echolet-APP] SOURCE_APP_RECORDED(\(bundleId))")
        }
    }

    private func setupStopTranscriptProvider() {
        self.flowCoordinator.stopFinalTranscriptProvider = { [weak self] sessionId in
            guard let self = self else { return nil }
            self.pendingFinalTranscriptsLock.lock()
            defer { self.pendingFinalTranscriptsLock.unlock() }
            return self.pendingFinalTranscripts.removeValue(forKey: sessionId)
        }
    }

    // MARK: - J5 Live Partial Streaming (keyboard-visible real-time text)
    //
    // While an ADOPTED keyboard session is genuinely listening, bounded
    // cumulative ASR partials are forwarded into the App Group response slot
    // (state .processing, isFinal false, ACK of the START identity, strictly
    // monotonic revision) so the keyboard can stream the text into the host
    // editor live instead of waiting for the final transcript.
    // Rate-limited to ~2.5 Hz with latest-partial flush; snapshot writes run
    // exclusively on `ipcQueue`.
    private var pendingPartialBySession: [String: String] = [:]
    private var partialFlushScheduled: Set<String> = []

    private func setupLivePartialForwarding() {
        EcholetAsrService.shared.partialTextObserver = { [weak self] sessionId, partial in
            guard let self = self else { return }
            self.ipcQueue.async {
                guard self.flowCoordinator.activeSessionToken?.sessionId == sessionId else { return }
                self.pendingPartialBySession[sessionId] = partial // latest wins
                guard !self.partialFlushScheduled.contains(sessionId) else { return }
                self.partialFlushScheduled.insert(sessionId)
                self.ipcQueue.asyncAfter(deadline: .now() + 0.4) { [weak self] in
                    guard let self = self else { return }
                    self.partialFlushScheduled.remove(sessionId)
                    guard let token = self.flowCoordinator.activeSessionToken,
                          token.sessionId == sessionId,
                          let text = self.pendingPartialBySession.removeValue(forKey: sessionId),
                          let defaults = self.sharedDefaults else { return }
                    let _ = self.writeResponseSnapshotOnQueue(
                        sessionId: sessionId,
                        acknowledgedRequestId: token.requestId,
                        acknowledgedSequence: token.sequence,
                        state: .processing,
                        recognizedText: text,
                        isFinal: false,
                        errorCode: nil
                    )
                }
            }
        }
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

    // MARK: - Audio Interruption / Hardware Loss (warm persist fence)
    //
    // iOS refuses starting an audio session from the background. An audio
    // session interruption or capture failure kills the recorder; the warm
    // persist path must then collapse truthfully and publish warm-live
    // recorderActive=false so the keyboard falls back to opening the app.
    private var interruptionObserverInstalled = false

    private func setupInterruptionObserver() {
        guard !interruptionObserverInstalled else { return }
        let center = NotificationCenter.default
        center.addObserver(
            self,
            selector: #selector(handleRecorderInterruption(_:)),
            name: AVAudioSession.interruptionNotification,
            object: nil
        )
        center.addObserver(
            self,
            selector: #selector(handleRoutePulled(_:)),
            name: AVAudioSession.routeChangeNotification,
            object: nil
        )
        interruptionObserverInstalled = true
    }

    @objc private func handleRecorderInterruption(_ notification: Notification) {
        guard let userInfo = notification.userInfo,
              let typeValue = userInfo[AVAudioSessionInterruptionTypeKey] as? UInt,
              AVAudioSession.InterruptionType(rawValue: typeValue) == .began else {
            return
        }
        ipcQueue.async { [weak self] in
            self?.collapseForcedHardwareLoss(reason: "audio_session_interruption")
        }
    }

    @objc private func handleRoutePulled(_ notification: Notification) {
        guard let userInfo = notification.userInfo,
              let reasonValue = userInfo[AVAudioSessionRouteChangeReasonKey] as? UInt,
              AVAudioSession.RouteChangeReason(rawValue: reasonValue) == .oldDeviceUnavailable else {
            return
        }
        ipcQueue.async { [weak self] in
            self?.collapseForcedHardwareLoss(reason: "audio_route_pulled")
        }
    }

    /// Runs on ipcQueue: collapses live warm capture when the recorder was
    /// forcibly lost, so warmLive recorderActive=false is published.
    private func collapseForcedHardwareLoss(reason: String) {
        let effects = self.flowCoordinator.handleMicrophoneHardwareInterrupted(reason: reason)
        guard !effects.isEmpty else { return }
        print("[Echolet-APP] RECORDER_INTERRUPTED(\(reason)) warm persist collapsed")
        self.executeFlowEffects(effects, triggeringRequestId: nil, triggeringSessionId: nil, triggeringSequence: nil)
    }

    // MARK: - Best-effort Return To Host App (openApplicationWithBundleID)
    //
    // After MIC_NATIVE_READY for a keyboard demand, bring forward the app the
    // user just left. The keyboard-resolved bundle id is preferred. One
    // bundle-id open is attempted; a URL-scheme open happens only if that
    // API returns false. `true` only means Return Requested — SpringBoard
    // often drops that switch when Echolet has just become foreground, which
    // is why the jump back sometimes happens and sometimes does not.
    // If we are still foreground after the confirm window, one
    // `suspendReturningToLastApp:` runs. That call does not need a bundle id
    // and is the system "back to the previous app" path. It is not repeated.
    public static var debugOverrideHostBundleId: String? = nil
    private static let hostResolveRetryLimit = 4
    private static let returnConfirmWindowSeconds: Double = 0.8
    /// Minimum time after becoming active before the first host switch.
    /// Firing during the incoming animation is the intermittent failure.
    private static let returnSettleAfterActiveSeconds: TimeInterval = 0.55
    private var sceneBecameActiveAt: Date?
    private var didAttemptSuspendReturn = false

    /// SceneDelegate: each time Echolet actually becomes the foreground app.
    public func noteSceneBecameActive() {
        sceneBecameActiveAt = Date()
    }

    private func attemptReturnToHostApp(nonce: String?, reason: String) {
        guard let defaults = sharedDefaults else {
            print("[Echolet-APP] RETURN_ATTEMPT(unavailable_defaults reason=\(reason))")
            return
        }
        let now = WarmIPCService.currentTimeMs()
        var storedHost: String? = nil
        let sourceApp = self.lastReceivedSourceApp

        if let overrideId = Self.debugOverrideHostBundleId, EcholetLaunch.isReturnableHostBundleId(overrideId) {
            storedHost = EcholetLaunch.normalizeHostBundleId(overrideId)
            print("[Echolet-APP] RETURN_ATTEMPT(debug_override host=\(storedHost ?? "nil") reason=\(reason))")
        } else if let nonce = nonce, let ctx = EcholetLaunch.freshHostReturnContext(forNonce: nonce, nowMs: now, defaults: defaults),
                  EcholetLaunch.isReturnableHostBundleId(ctx.hostBundleId) {
            storedHost = ctx.hostBundleId
            print("[Echolet-APP] RETURN_ATTEMPT(nonce_matched=\(String(nonce.prefix(8)))… host=\(ctx.hostBundleId) reason=\(reason))")
        } else {
            print("[Echolet-APP] RETURN_ATTEMPT(no_stored_host source=\(sourceApp ?? "nil") reason=\(reason) -> will read system breadcrumb)")
        }

        // The mic is already live. Wait out the foreground animation before
        // asking SpringBoard to switch away, or the request is silently dropped.
        // Settle math and the suspend flag stay on the main thread.
        let capturedStored = storedHost
        let capturedSource = sourceApp
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            self.didAttemptSuspendReturn = false
            let delay = self.hostReturnSettleDelay()
            print("[Echolet-APP] RETURN_SETTLE(delay=\(String(format: "%.2f", delay))s reason=\(reason))")
            DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
                guard let self = self else { return }
                self.ipcQueue.async {
                    if self.flowCoordinator.isWarmStandbyHardwareActive, let defaults = self.sharedDefaults {
                        _ = EcholetLaunch.writeWarmLive(
                            appEpoch: AppDelegate.sharedEpoch,
                            recorderActive: true,
                            nowMs: WarmIPCService.currentTimeMs(),
                            defaults: defaults
                        )
                    }
                }
                self.beginHostReturn(storedHost: capturedStored, sourceApp: capturedSource, resolveAttempt: 0)
            }
        }
    }

    /// How long to wait on the main thread before the first host switch.
    private func hostReturnSettleDelay() -> TimeInterval {
        let minimum = Self.returnSettleAfterActiveSeconds
        guard let activeAt = sceneBecameActiveAt else { return minimum }
        let elapsed = Date().timeIntervalSince(activeAt)
        return max(0.2, minimum - elapsed)
    }

    /// Resolves one host, then performs at most one bundle open and, only if
    /// that call returns false, one URL open. Reading the back-badge a few
    /// times is host resolution, not a return retry.
    private func beginHostReturn(storedHost: String?, sourceApp: String?, resolveAttempt: Int) {
        let breadcrumb = Self.systemBreadcrumbBundleId()
        let hosts = EcholetLaunch.orderedReturnHosts(
            breadcrumb: breadcrumb,
            storedHost: storedHost,
            sourceApp: sourceApp
        )
        print("[Echolet-APP] RETURN_CANDIDATES(resolve=\(resolveAttempt) breadcrumb=\(breadcrumb ?? "nil") hosts=\(hosts.joined(separator: ",")))")
        if let host = hosts.first {
            performSingleReturn(to: host)
            return
        }
        if storedHost == nil, breadcrumb == nil, resolveAttempt < Self.hostResolveRetryLimit {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.12) { [weak self] in
                self?.beginHostReturn(storedHost: storedHost, sourceApp: sourceApp, resolveAttempt: resolveAttempt + 1)
            }
            return
        }
        stayInEcholet(reason: "no_returnable_host")
    }

    private func performSingleReturn(to host: String) {
        publishReturnGuidance(nil)
        switch EcholetLaunch.initialReturnStep(hostBundleId: host) {
        case .stay(let reason):
            stayInEcholet(reason: reason)
        case .openURL:
            openHostURL(host)
        case .openBundle(let bundleId):
            let opened = Self.performWorkspaceOpen(bundleId: bundleId)
            print("[Echolet-APP] RETURN_EXECUTION(method=workspace_open,host=\(bundleId),result=\(opened))")
            if opened {
                beginConfirmationWatch(host: bundleId, method: "workspace_open")
                return
            }
            switch EcholetLaunch.afterBundleOpenRejected(hostBundleId: bundleId) {
            case .openURL(let url, let id):
                print("[Echolet-APP] RETURN_EXECUTION(method=open_url,host=\(id),url=\(url.absoluteString),because=bundle_rejected)")
                openHostURL(id)
            case .stay(let reason):
                stayInEcholet(reason: reason)
            case .openBundle:
                stayInEcholet(reason: "bundle_open_rejected")
            }
        }
    }

    private func openHostURL(_ host: String) {
        guard let url = EcholetLaunch.returnURL(forHostBundleId: host) else {
            stayInEcholet(reason: "no_url_scheme")
            return
        }
        UIApplication.shared.open(url, options: [:]) { [weak self] accepted in
            print("[Echolet-APP] RETURN_EXECUTION(method=open_url,host=\(host),url=\(url.absoluteString),accepted=\(accepted))")
            if accepted {
                self?.beginConfirmationWatch(host: host, method: "open_url")
            } else {
                self?.stayInEcholet(reason: "open_url_rejected")
            }
        }
    }

    private func stayInEcholet(reason: String) {
        if shouldTrySuspendFallback(reason: reason) {
            fallbackSuspendReturn()
            return
        }
        print("[Echolet-APP] RETURN_STAY(reason=\(reason))")
        publishReturnGuidance(EcholetLaunch.manualReturnHint)
    }

    /// Bundle-id and URL opens both miss often. One suspend-return is the
    /// fallback that jumps back to whichever app was in front before Echolet.
    private func shouldTrySuspendFallback(reason: String) -> Bool {
        guard !didAttemptSuspendReturn else { return false }
        switch reason {
        case "no_returnable_host", "no_url_scheme", "bundle_open_rejected", "open_url_rejected", "return_unconfirmed":
            return true
        default:
            return false
        }
    }

    private func fallbackSuspendReturn() {
        didAttemptSuspendReturn = true
        let called = Self.performSuspendReturningToLastApp(arg: true)
        if called {
            beginConfirmationWatch(host: "previous_app", method: "suspend_returning")
        } else {
            stayInEcholet(reason: "suspend_unavailable")
        }
    }

    /// Manual experiments. Each call is one open, then the same foreground check.
    public func debugReturnByBundleId(_ bundleId: String) {
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            print("[Echolet-APP] DEBUG_RETURN(bundle=\(bundleId))")
            let opened = Self.performWorkspaceOpen(bundleId: bundleId)
            if opened {
                self.beginConfirmationWatch(host: bundleId, method: "debug_workspace_open")
            } else {
                self.stayInEcholet(reason: "debug_bundle_rejected")
            }
        }
    }

    public func debugReturnByURL(bundleId: String) {
        DispatchQueue.main.async { [weak self] in
            self?.openHostURL(bundleId)
        }
    }

    private struct ReturnWatch {
        let token: UUID
        let host: String
        let method: String
        var leftForeground = false
    }

    private var returnWatch: ReturnWatch?
    private var returnForegroundObserver: NSObjectProtocol?

    private func beginConfirmationWatch(host: String, method: String) {
        let start = { [weak self] in
            guard let self = self else { return }
            self.installReturnForegroundObserver()
            let token = UUID()
            self.returnWatch = ReturnWatch(token: token, host: host, method: method)
            print("[Echolet-APP] RETURN_REQUESTED(host=\(host) method=\(method))")
            DispatchQueue.main.asyncAfter(deadline: .now() + Self.returnConfirmWindowSeconds) { [weak self] in
                guard let self = self, let watch = self.returnWatch, watch.token == token else { return }
                self.returnWatch = nil
                if watch.leftForeground {
                    print("[Echolet-APP] RETURN_LEFT_FOREGROUND(host=\(watch.host) method=\(watch.method))")
                    return
                }
                print("[Echolet-APP] RETURN_UNCONFIRMED(host=\(watch.host) method=\(watch.method) still_foreground)")
                if watch.method == "suspend_returning" {
                    self.stayInEcholet(reason: "suspend_unconfirmed")
                } else {
                    self.stayInEcholet(reason: "return_unconfirmed")
                }
            }
        }
        if Thread.isMainThread {
            start()
        } else {
            DispatchQueue.main.async(execute: start)
        }
    }

    private func installReturnForegroundObserver() {
        guard returnForegroundObserver == nil else { return }
        returnForegroundObserver = NotificationCenter.default.addObserver(
            forName: UIApplication.willResignActiveNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            guard let self = self, var watch = self.returnWatch, !watch.leftForeground else { return }
            watch.leftForeground = true
            self.returnWatch = watch
            print("[Echolet-APP] RETURN_WILL_RESIGN(host=\(watch.host) method=\(watch.method))")
        }
    }

    private func publishReturnGuidance(_ text: String?) {
        let deliver = { [weak self] in
            guard let self = self else { return }
            print("[Echolet-APP] RETURN_GUIDANCE(\(text ?? "clear"))")
            self.delegate?.warmIPCService(self, didUpdateReturnGuidance: text)
        }
        if Thread.isMainThread {
            deliver()
        } else {
            DispatchQueue.main.async(execute: deliver)
        }
    }

    /// Bundle id of the system "Back to X" badge, if UIKit has one.
    private static func systemBreadcrumbBundleId() -> String? {
        let app = UIApplication.shared
        guard let action = performObjectReturning(app, selectorName: "_systemNavigationAction")
                ?? performObjectReturning(app, selectorName: "systemNavigationAction") else {
            print("[Echolet-APP] BREADCRUMB(action=nil)")
            return nil
        }
        guard let actionNS = action as? NSObject else {
            print("[Echolet-APP] BREADCRUMB(action_not_nsobject class=\(type(of: action)))")
            return nil
        }
        print("[Echolet-APP] BREADCRUMB(actionClass=\(String(describing: type(of: actionNS))))")

        var destinations: [NSObject] = []
        if let raw = performObjectReturning(actionNS, selectorName: "destinations") as? NSArray {
            destinations = raw.compactMap { $0 as? NSObject }
        }
        if destinations.isEmpty, let raw = objectIvar(actionNS, name: "_destinations") as? NSArray
            ?? objectIvar(actionNS, name: "destinations") as? NSArray {
            destinations = raw.compactMap { $0 as? NSObject }
        }
        let selfId = Bundle.main.bundleIdentifier
        for dest in destinations {
            if let id = bundleIdString(on: dest), id != selfId {
                print("[Echolet-APP] BREADCRUMB(host=\(id) class=\(String(describing: type(of: dest))))")
                return id
            }
            print("[Echolet-APP] BREADCRUMB(unreadable class=\(String(describing: type(of: dest))) ivars=\(debugIvarNames(dest)))")
        }
        if destinations.isEmpty {
            print("[Echolet-APP] BREADCRUMB(no_destinations ivars=\(debugIvarNames(actionNS)))")
        }
        return nil
    }

    private static func performObjectReturning(_ object: NSObject, selectorName: String) -> AnyObject? {
        let sel = NSSelectorFromString(selectorName)
        guard object.responds(to: sel),
              let method = class_getInstanceMethod(type(of: object), sel) else {
            return nil
        }
        let ret = method_copyReturnType(method)
        defer { free(ret) }
        guard ret[0] == UInt8(ascii: "@") else { return nil }
        return object.perform(sel)?.takeUnretainedValue()
    }

    private static func objectIvar(_ object: NSObject, name: String) -> AnyObject? {
        var cls: AnyClass? = type(of: object)
        while let current = cls {
            if let ivar = class_getInstanceVariable(current, name),
               let enc = ivar_getTypeEncoding(ivar), enc[0] == UInt8(ascii: "@") {
                return object_getIvar(object, ivar) as AnyObject?
            }
            cls = class_getSuperclass(current)
        }
        return nil
    }

    private static func bundleIdString(on object: NSObject) -> String? {
        let selectors = ["bundleId", "bundleIdentifier", "applicationBundleIdentifier", "_bundleId"]
        for name in selectors {
            if let raw = performObjectReturning(object, selectorName: name) as? String,
               let norm = EcholetLaunch.normalizeHostBundleId(raw),
               EcholetLaunch.isReturnableHostBundleId(norm) {
                return norm
            }
        }
        var cls: AnyClass? = type(of: object)
        while let current = cls {
            var count: UInt32 = 0
            if let ivars = class_copyIvarList(current, &count) {
                defer { free(ivars) }
                for i in 0..<Int(count) {
                    let ivar = ivars[i]
                    guard let enc = ivar_getTypeEncoding(ivar), enc[0] == UInt8(ascii: "@") else { continue }
                    guard let raw = object_getIvar(object, ivar) as? String,
                          let norm = EcholetLaunch.normalizeHostBundleId(raw),
                          EcholetLaunch.isReturnableHostBundleId(norm) else { continue }
                    return norm
                }
            }
            cls = class_getSuperclass(current)
        }
        return nil
    }

    private static func debugIvarNames(_ object: NSObject) -> String {
        var names: [String] = []
        var cls: AnyClass? = type(of: object)
        while let current = cls, names.count < 24 {
            var count: UInt32 = 0
            if let ivars = class_copyIvarList(current, &count) {
                for i in 0..<Int(count) where names.count < 24 {
                    if let n = ivar_getName(ivars[i]) {
                        names.append(String(cString: n))
                    }
                }
                free(ivars)
            }
            cls = class_getSuperclass(current)
        }
        return names.joined(separator: ",")
    }

    @discardableResult
    public static func performWorkspaceOpen(bundleId: String) -> Bool {
        print("[Echolet-APP] WORKSPACE_OPEN_ATTEMPT(host=\(bundleId),isMainThread=\(Thread.isMainThread))")
        guard let workspaceClass = NSClassFromString("LSApplicationWorkspace") as? NSObject.Type else {
            print("[Echolet-APP] RETURN_RESULT(false LSApplicationWorkspace class unavailable) user stays in Echolet")
            return false
        }
        let defaultWorkspaceSel = NSSelectorFromString("defaultWorkspace")
        guard workspaceClass.responds(to: defaultWorkspaceSel),
              let workspace = workspaceClass.perform(defaultWorkspaceSel)?.takeUnretainedValue() as? NSObject else {
            print("[Echolet-APP] RETURN_RESULT(false defaultWorkspace nil) user stays in Echolet")
            return false
        }

        let openSel = NSSelectorFromString("openApplicationWithBundleID:")
        guard workspace.responds(to: openSel) else {
            print("[Echolet-APP] RETURN_RESULT(false openApplicationWithBundleID: missing) user stays in Echolet")
            return false
        }

        typealias OpenAppFn = @convention(c) (NSObject, Selector, NSString) -> Bool
        let fn = unsafeBitCast(workspace.method(for: openSel), to: OpenAppFn.self)
        let opened = fn(workspace, openSel, bundleId as NSString)
        print("[Echolet-APP] RETURN_RESULT(openApplicationWithBundleID=\(opened) host=\(bundleId))")
        return opened
    }

    @discardableResult
    public static func performSuspendReturningToLastApp(arg: Bool = true) -> Bool {
        print("[Echolet-APP] SUSPEND_RETURNING_TO_LAST_APP_ATTEMPT(arg=\(arg),isMainThread=\(Thread.isMainThread))")
        let app = UIApplication.shared
        let sel = NSSelectorFromString("suspendReturningToLastApp:")
        guard app.responds(to: sel) else {
            print("[Echolet-APP] SUSPEND_RETURNING_RESULT(false suspendReturningToLastApp: missing)")
            return false
        }
        typealias SuspendFn = @convention(c) (NSObject, Selector, Bool) -> Void
        let fn = unsafeBitCast(app.method(for: sel), to: SuspendFn.self)
        fn(app, sel, arg)
        print("[Echolet-APP] SUSPEND_RETURNING_RESULT(called arg=\(arg))")
        return true
    }

    // MARK: - User Arming State
    public func setUserArmMicrophone(_ armed: Bool) {
        ipcQueue.async { [weak self] in
            guard let self = self else { return }
            let effects = self.flowCoordinator.setUserArmMicrophone(armed)
            self.executeFlowEffects(effects, triggeringRequestId: nil, triggeringSessionId: nil, triggeringSequence: nil)
        }
    }

    /// J4-E2E: explicit user-initiated warm dictation prepare. Requires the
    /// app foreground; loads the offline model, then claims warm standby
    /// ownership (mic permission + one running recorder + bounded discard
    /// handoff for the keyboard START adoption).
    public func prepareWarmDictation() {
        EcholetAsrService.shared.loadModelIfNeeded { [weak self] _, errorCode in
            guard let self = self else { return }
            self.ipcQueue.async {
                let effects = self.flowCoordinator.prepareWarmStandbyCapture()
                // If the model is still missing, honest startup failure is
                // surfaced by the ASR adoption path (fail closed); here the
                // permission/gate machinery drives the warm standby flow.
                self.executeFlowEffects(effects, triggeringRequestId: nil, triggeringSessionId: nil, triggeringSequence: nil)
                if let errorCode = errorCode, self.flowCoordinator.captureOwner != .warmStandby {
                    self.notifyBlocked("ASR model unavailable: \(errorCode)")
                }
            }
        }
    }

    // MARK: - J5 Deep-Link Keyboard Activation (echolet://dictation/prepare)

    /// One awaited activation context (single in-flight launch handshake).
    /// ALL mutations happen exclusively on `ipcQueue`.
    private struct PendingActivation {
        let nonce: String
        let startedAtMs: UInt64
    }

    /// Activation timeout: honest failed phase when the prepare never
    /// reaches a confirmed native recorder within the bound.
    private static let activationPrepareTimeoutSeconds: Double = 20.0
    /// Launch demand record age bound applied by the app for one-shot use.
    private static let demandOneShotTtlMs: UInt64 = EcholetLaunch.defaultDemandTtlMs

    public static func currentTimeMs() -> UInt64 {
        return UInt64(Date().timeIntervalSince1970 * 1000)
    }

    /// SceneDelegate entry for `echolet://dictation/prepare?nonce=...&ts=...`.
    ///
    /// The keyboard may only REQUEST activation (it has no microphone access
    /// and no private launch APIs); this app-side entry point is the only
    /// writer of the launch handshake phase records. Ordering guarantee:
    /// arming AND the model-dependent warm prepare both execute on the
    /// shared `ipcQueue`, and the warm-ready phase is published exclusively
    /// from the confirmed native hardware start completion path.
    /// Nonce-matched activation or ordered warm standby bootstrap.
    ///
    /// Cold connect, openURLContexts, and foreground transitions can arrive
    /// in arbitrary order. This method guarantees:
    /// 1. Exactly one execution wins on `ipcQueue`.
    /// 2. If a matching demand is found (or was passed via URI), it is
    ///    claimed once and processed with guaranteed arm-before-prepare ordering.
    /// 3. If no demand exists, an ordered warm bootstrap (arm -> model -> standby)
    ///    is executed only if explicitly allowed, ensuring mic readiness without racing.
    public func coordinatedForegroundBootstrap(url: URL? = nil, source: String, allowStandbyIfNoDemand: Bool = true) {
        let incomingNonce: String?
        if let url = url {
            guard let parsed = EcholetLaunch.parseDictationPrepareURL(url) else {
                print("[Echolet-APP] LAUNCH_IGNORED(unrelated_url source=\(source))")
                return
            }
            incomingNonce = parsed.nonce
            print("[Echolet-APP] APP_URI_RECEIVED(source=\(source),nonce=\(String(parsed.nonce.prefix(8)))…)")
        } else {
            incomingNonce = nil
            print("[Echolet-APP] APP_COORDINATION_CHECK(source=\(source))")
        }

        ipcQueue.async { [weak self] in
            guard let self = self, let defaults = self.sharedDefaults else { return }
            let now = WarmIPCService.currentTimeMs()

            // If an activation is already in flight:
            if let pending = self.pendingActivation {
                if let nonce = incomingNonce, pending.nonce == nonce {
                    // Idempotent duplicate delivery for the winning nonce
                    EcholetLaunch.writePhase(
                        nonce: nonce,
                        appEpoch: AppDelegate.sharedEpoch,
                        phase: .uriReceived,
                        nowMs: now,
                        defaults: defaults
                    )
                    print("[Echolet-APP] BOOT_PREPARE_DEDUPED(already_pending_matching_nonce=\(String(nonce.prefix(8)))…)")
                    return
                } else if incomingNonce == nil {
                    // Foreground tick while activation is already in flight is deduped
                    print("[Echolet-APP] BOOT_PREPARE_DEDUPED(activation_already_in_flight)")
                    return
                }
            }

            // Check if there is an active demand matching incoming nonce or fresh in App Group
            let demand = EcholetLaunch.readDemand(defaults: defaults)
            if let incomingNonce = incomingNonce {
                guard let demand = demand,
                      demand.nonce == incomingNonce,
                      EcholetLaunch.isDemandFresh(demand, nowMs: now) else {
                    EcholetLaunch.writePhase(
                        nonce: incomingNonce,
                        appEpoch: AppDelegate.sharedEpoch,
                        phase: .failed,
                        nowMs: now,
                        errorCode: "demand_missing_or_expired",
                        defaults: defaults
                    )
                    print("[Echolet-APP] BLOCKED(demand_missing_or_expired)")
                    return
                }
                print("[Echolet-APP] DEMAND_CLAIMED_ONCE(source=\(source),nonce=\(String(incomingNonce.prefix(8)))…)")
                self.beginActivationForDemand(nonce: incomingNonce, defaults: defaults, nowMs: now)
            } else if let demand = demand, EcholetLaunch.isDemandFresh(demand, nowMs: now) {
                // Pending fresh keyboard demand found on foreground check
                print("[Echolet-APP] DEMAND_CLAIMED_ONCE(source=\(source),nonce=\(String(demand.nonce.prefix(8)))…)")
                self.beginActivationForDemand(nonce: demand.nonce, defaults: defaults, nowMs: now)
            } else if allowStandbyIfNoDemand {
                // No keyboard demand; perform ordered standby bootstrap if mic is armed or requested
                print("[Echolet-APP] BOOT_PREPARE_WINNER(source=\(source),ordered_standby)")
                self.performOrderedWarmStandbyBootstrap()
            }
        }
    }

    /// SceneDelegate entry for `echolet://dictation/prepare?nonce=...&ts=...`.
    public func handleDictationActivationURL(_ url: URL, source: String) {
        coordinatedForegroundBootstrap(url: url, source: source, allowStandbyIfNoDemand: false)
    }

    /// MANUAL foreground entry: honors fresh keyboard demand or safe warm prepare.
    public func activatePendingDemandIfFresh(source: String = "foreground") {
        coordinatedForegroundBootstrap(url: nil, source: source, allowStandbyIfNoDemand: false)
    }

    /// Ordered warm bootstrap for app-initiated preparation: arm first, load model, start mic.
    public func performOrderedWarmStandbyBootstrap() {
        if self.flowCoordinator.isWarmStandbyHardwareActive {
            print("[Echolet-APP] BOOT_PREPARE_DEDUPED(mic_already_hardware_active)")
            return
        }
        let armEffects = self.flowCoordinator.setUserArmMicrophone(true)
        self.executeFlowEffects(armEffects, triggeringRequestId: nil, triggeringSessionId: nil, triggeringSequence: nil)
        print("[Echolet-APP] MIC_ARMED_ORDERED(before_prepare_standby)")

        EcholetAsrService.shared.loadModelIfNeeded { [weak self] ok, errorCode in
            guard let self = self else { return }
            self.ipcQueue.async {
                guard ok else {
                    print("[Echolet-APP] BLOCKED(model_load_\(errorCode ?? "unknown"))")
                    if self.flowCoordinator.captureOwner != .warmStandby {
                        self.notifyBlocked("ASR model unavailable: \(errorCode ?? "unknown")")
                    }
                    return
                }
                print("[Echolet-APP] MODEL_LOAD(ok)")
                let effects = self.flowCoordinator.prepareWarmStandbyCapture()
                self.executeFlowEffects(effects, triggeringRequestId: nil, triggeringSessionId: nil, triggeringSequence: nil)
            }
        }
    }

    /// Shared activation core; MUST run on `ipcQueue` with a validated fresh
    /// demand decision already made (or the demand unconditionally fresh in
    /// the foreground hand path).
    private func beginActivationForDemand(nonce: String, defaults: UserDefaults, nowMs: UInt64) {
        let now = nowMs

        if let pending = self.pendingActivation, pending.nonce == nonce {
            print("[Echolet-APP] APP_URI_RECEIVED(duplicate_ignored)")
            return
        }

        // Warm-already-truth shortcut: if the native recorder is STILL
        // hardware-active (warm standby or an adopted keyboard session),
        // never re-arm/re-prepare above it — simply republish the acceptance
        // phases for this demand so the keyboard observes warmReady.
        if self.flowCoordinator.isWarmStandbyHardwareActive {
            EcholetLaunch.writePhase(
                nonce: nonce,
                appEpoch: AppDelegate.sharedEpoch,
                phase: .micNativeReady,
                nowMs: now,
                defaults: defaults
            )
            EcholetLaunch.writePhase(
                nonce: nonce,
                appEpoch: AppDelegate.sharedEpoch,
                phase: .warmReady,
                nowMs: now,
                defaults: defaults
            )
            print("[Echolet-APP] MIC_NATIVE_READY(adopted_existing_recorder,nonce=\(String(nonce.prefix(8)))…)")
            self.attemptReturnToHostApp(nonce: nonce, reason: "mic_native_ready_adopted_existing_recorder")
            return
        }

        // One-shot launch context: consume the demand atomically.
        _ = EcholetLaunch.consumeDemand(defaults: defaults)
        self.pendingActivation = PendingActivation(nonce: nonce, startedAtMs: now)
        EcholetLaunch.writePhase(
            nonce: nonce,
            appEpoch: AppDelegate.sharedEpoch,
            phase: .uriReceived,
            nowMs: now,
            defaults: defaults
        )

        // GUARANTEED ORDERING on the shared single-writer queue:
        // 1) setUserArmMicrophone(true) first (arming state mutation and
        //    its effects), 2) the model-dependent warm prepare after the
        //    arm effects. Both reuse the proven flow machinery; no fork.
        let armEffects = self.flowCoordinator.setUserArmMicrophone(true)
        self.executeFlowEffects(armEffects, triggeringRequestId: nil, triggeringSessionId: nil, triggeringSequence: nil)
        print("[Echolet-APP] MIC_ARMED_ORDERED(before_prepare)")

        EcholetAsrService.shared.loadModelIfNeeded { [weak self] ok, errorCode in
            guard let self = self else { return }
            self.ipcQueue.async { [self] in
                guard let defaults = self.sharedDefaults else { return }
                guard let pending = self.pendingActivation, pending.nonce == nonce else { return }
                let nowLoad = WarmIPCService.currentTimeMs()
                guard ok else {
                    EcholetLaunch.writePhase(
                        nonce: nonce,
                        appEpoch: AppDelegate.sharedEpoch,
                        phase: .failed,
                        nowMs: nowLoad,
                        errorCode: "model_load_\(errorCode ?? "unknown")",
                        defaults: defaults
                    )
                    self.pendingActivation = nil
                    print("[Echolet-APP] BLOCKED(model_load_\(errorCode ?? "unknown"))")
                    return
                }
                EcholetLaunch.writePhase(
                    nonce: nonce,
                    appEpoch: AppDelegate.sharedEpoch,
                    phase: .modelLoaded,
                    nowMs: nowLoad,
                    defaults: defaults
                )
                print("[Echolet-APP] MODEL_LOAD(ok)")
                let effects = self.flowCoordinator.prepareWarmStandbyCapture()
                self.executeFlowEffects(effects, triggeringRequestId: nil, triggeringSessionId: nil, triggeringSequence: nil)
                self.scheduleActivationTimeout(nonce: nonce)
            }
        }
    }

    /// Bounded, honest prepare timeout: if the native recorder never reaches
    /// a confirmed hardware-active state for this activation, publish a
    /// failed phase (the keyboard surfaces it) and clear the context.
    private func scheduleActivationTimeout(nonce: String) {
        ipcQueue.asyncAfter(deadline: .now() + Self.activationPrepareTimeoutSeconds) { [weak self] in
            guard let self = self, let defaults = self.sharedDefaults else { return }
            guard let pending = self.pendingActivation, pending.nonce == nonce else { return }
            self.pendingActivation = nil
            EcholetLaunch.writePhase(
                nonce: nonce,
                appEpoch: AppDelegate.sharedEpoch,
                phase: .failed,
                nowMs: WarmIPCService.currentTimeMs(),
                errorCode: "warm_prepare_timeout",
                defaults: defaults
            )
            print("[Echolet-APP] BLOCKED(warm_prepare_timeout)")
            self.notifyBlocked("Warm prepare timed out for keyboard activation (\(String(nonce.prefix(8)))…)")
        }
    }

    public var isUserArmed: Bool {
        return ipcQueue.sync { self.flowCoordinator.isMicrophoneArmedByUser }
    }

    /// Main-thread safe truth readouts for the Home panel hardware/session
    /// projection (Increment B). Single reads are serialized on `ipcQueue`;
    /// the Home view additionally subscribes to the delegate callbacks.
    public var warmStandbyHardwareActive: Bool {
        return ipcQueue.sync { self.flowCoordinator.isWarmStandbyHardwareActive }
    }

    public var hasActiveKeyboardSession: Bool {
        return ipcQueue.sync { self.flowCoordinator.activeSessionToken != nil }
    }

    /// Queue-serialized truth for the model-select gate: a keyboard START
    /// activation is pending (bootstrap/prepare in flight). Mutated only on
    /// `ipcQueue`; readers never mutate queue state.
    public var hasPendingActivation: Bool {
        return ipcQueue.sync { self.pendingActivation != nil }
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
    public func startPolling(interval: TimeInterval = 0.25) {
        ipcQueue.async { [weak self] in
            guard let self = self else { return }
            self.stopPollingInternal()
            let timer = DispatchSource.makeTimerSource(queue: self.ipcQueue)
            let intervalMs = Int(interval * 1000)
            timer.schedule(
                deadline: .now(),
                repeating: .milliseconds(intervalMs),
                leeway: .milliseconds(25)
            )
            timer.setEventHandler { [weak self] in
                self?.pollIncomingRequestsInternal()
            }
            self.pollTimer = timer
            timer.resume()
        }
    }

    public func stopPolling() {
        ipcQueue.async { [weak self] in
            self?.stopPollingInternal()
        }
    }

    private func stopPollingInternal() {
        if let timer = pollTimer {
            timer.cancel()
            pollTimer = nil
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

        // J5 warm-live heartbeat: while the native recorder is genuinely
        // running, refresh the warm-live record on the single-writer queue at
        // the bounded poll cadence (also runs while background recording is
        // scheduled, because AppDelegate retains polling in that state). A
        // user returning to the keyboard a minute later still sees a
        // positively fresh active recorder instead of a falsely "stale" one
        // (this was the dead-loop trigger in the first device run: aged
        // records made the keyboard re-attempt opening the app while the mic
        // was live the whole time).
        if self.flowCoordinator.isWarmStandbyHardwareActive {
            _ = EcholetLaunch.writeWarmLive(
                appEpoch: AppDelegate.sharedEpoch,
                recorderActive: true,
                nowMs: WarmIPCService.currentTimeMs(),
                defaults: defaults
            )
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
                let _ = self.writeResponseSnapshotOnQueue(
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
                // J4-E2E: FINAL real ASR transcript must be collected BEFORE
                // the completed response snapshot. Admitted keyboard STOP
                // (reason "keyboard_stop") finalizes the ASR session
                // (drain ring → trailing-silence flush → final decode →
                // destroy stream) FIRST, stores the real result, and only
                // then stops the hardware and ACKs completion. Cancel /
                // disarm / replacement stops dispose the ASR session
                // truthfully without fabricating transcript text. A stale
                // or unrelated stop can never collide with a newer session.
                if directive.reason == "keyboard_stop" {
                    self.collectFinalTranscriptThenStopHardware(directive: directive)
                } else {
                    EcholetAsrService.shared.disposeSession(
                        sessionId: directive.sessionId,
                        reason: "stop_effect_reason_\(directive.reason)"
                    )
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
                }

            case .finalizeAsrReturnToWarmStandby(let directive):
                // Warm persist: dispose ONLY the ASR session (the recorder
                // hardware stays live; it is the app-owned warm standby the
                // keyboard re-adopts later). A keyboard STOP finalizes the
                // real transcript first; cancel/disposal ends without
                // fabricating text. The exact completed ACK for the stop
                // identity is emitted by the coordinator afterward.
                if directive.reason == "keyboard_stop" {
                    EcholetAsrService.shared.finalizeSession(sessionId: directive.sessionId) { [weak self] finalText in
                        guard let self = self else { return }
                        self.ipcQueue.async {
                            self.pendingFinalTranscriptsLock.lock()
                            self.pendingFinalTranscripts[directive.sessionId] = finalText
                            self.pendingFinalTranscriptsLock.unlock()
                            let stopEffects = self.flowCoordinator.handleKeyboardSessionEndedReturnToWarmStandby(directive: directive)
                            self.executeFlowEffects(
                                stopEffects,
                                triggeringRequestId: directive.requestId,
                                triggeringSessionId: directive.sessionId,
                                triggeringSequence: directive.sequence
                            )
                        }
                    }
                } else {
                    EcholetAsrService.shared.disposeSession(
                        sessionId: directive.sessionId,
                        reason: "warm_persist_\(directive.reason)"
                    )
                    let stopEffects = self.flowCoordinator.handleKeyboardSessionEndedReturnToWarmStandby(directive: directive)
                    self.executeFlowEffects(
                        stopEffects,
                        triggeringRequestId: directive.requestId,
                        triggeringSessionId: directive.sessionId,
                        triggeringSequence: directive.sequence
                    )
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

            // J4-E2E warm standby effects
            case .requestWarmPermission(let token):
                AudioCaptureController.shared.requestMicrophonePermission { [weak self] granted in
                    guard let self = self else { return }
                    self.ipcQueue.async {
                        let permEffects = self.flowCoordinator.handleWarmStandbyPermissionCallback(token: token, granted: granted)
                        self.executeFlowEffects(
                            permEffects,
                            triggeringRequestId: nil,
                            triggeringSessionId: nil,
                            triggeringSequence: nil
                        )
                    }
                }

            case .startHardwareWarmStandby(let token):
                AudioCaptureController.shared.startCapture(includePCM: true) { [weak self] result in
                    guard let self = self else { return }
                    self.ipcQueue.async {
                        let (warmEffects, warmCleanup) = self.flowCoordinator.handleWarmStandbyHardwareStartCompletion(
                            token: token,
                            result: result.mapError { $0 as Error }
                        )
                        if let cleanup = warmCleanup {
                            AudioCaptureController.shared.stopCaptureIfGeneration(
                                expectedGeneration: cleanup.nativeGeneration
                            )
                        }
                        self.executeFlowEffects(
                            warmEffects,
                            triggeringRequestId: nil,
                            triggeringSessionId: nil,
                            triggeringSequence: nil
                        )
                    }
                }

            case .adoptWarmStandbyAsr(let token):
                // Bind the keyboard session to an EXACT ASR session token on
                // the single ASR worker; stale/mismatched capture generations
                // fail closed inside the service. Adoption discards the
                // pre-START ring backlog at the ring boundary.
                EcholetAsrService.shared.beginSession(
                    sessionId: token.sessionId,
                    kind: .warmKeyboard,
                    expectedGeneration: AudioCaptureController.shared.activeGeneration
                ) { [weak self] succeed, errorCode in
                    guard let self = self else { return }
                    self.ipcQueue.async {
                        let adoptEffects = self.flowCoordinator.handleWarmAsrAdoptionCompleted(
                            token: token,
                            success: succeed,
                            errorCode: errorCode
                        )
                        self.executeFlowEffects(
                            adoptEffects,
                            triggeringRequestId: token.requestId,
                            triggeringSessionId: token.sessionId,
                            triggeringSequence: token.sequence
                        )
                    }
                }

            case .notifyWarmStandbyActive(let active):
                // J5: native hardware truth change is written to the App
                // Group warm-live record EXACTLY HERE, on ipcQueue (single
                // writer), keyed to the current process epoch. Keyboard uses
                // this as its "positively fresh active warm mic" authority.
                if let defaults = self.sharedDefaults {
                    _ = EcholetLaunch.writeWarmLive(
                        appEpoch: AppDelegate.sharedEpoch,
                        recorderActive: active,
                        nowMs: WarmIPCService.currentTimeMs(),
                        defaults: defaults
                    )
                }
                if active, let pending = self.pendingActivation {
                    // Native recorder observed running: publish proof
                    // phases AFTER the truth, i.e. only now may the keyboard
                    // consider the recorder READY.
                    if let defaults = self.sharedDefaults {
                        let nowWarm = WarmIPCService.currentTimeMs()
                        EcholetLaunch.writePhase(
                            nonce: pending.nonce,
                            appEpoch: AppDelegate.sharedEpoch,
                            phase: .micNativeReady,
                            nowMs: nowWarm,
                            defaults: defaults
                        )
                        EcholetLaunch.writePhase(
                            nonce: pending.nonce,
                            appEpoch: AppDelegate.sharedEpoch,
                            phase: .warmReady,
                            nowMs: nowWarm,
                            defaults: defaults
                        )
                    }
                    self.pendingActivation = nil
                    print("[Echolet-APP] MIC_NATIVE_READY (nonce=\(String(pending.nonce.prefix(8)))…)")
                    // Best-effort return to the app the keyboard was summoned
                    // into (LSApplicationWorkspace via responder/perform; if
                    // it fails, the user simply stays in the Echolet app).
                    self.attemptReturnToHostApp(nonce: pending.nonce, reason: "mic_native_ready")
                }
                DispatchQueue.main.async { [weak self] in
                    guard let self = self else { return }
                    self.delegate?.warmIPCService(self, didChangeWarmStandbyActive: active)
                }
            }
        }
    }

    // MARK: - Final Transcript Collection Then Hardware Stop (ipcQueue owned)
    //
    // Collects the REAL final recognizer text for an admitted keyboard STOP,
    // stopping the native capture exactly once in the process: the ASR final
    // text drain happens first (worker queue), then the Stop is issued and a
    // hardware completion ACKs the exact directive identity through the
    // coordinator.
    private func collectFinalTranscriptThenStopHardware(directive: WarmCaptureFlowCoordinator.HardwareStopDirective) {
        EcholetAsrService.shared.finalizeSession(sessionId: directive.sessionId) { [weak self] finalText in
            guard let self = self else { return }
            self.ipcQueue.async { [weak self] in
                guard let self = self else { return }
                // De-duped store: only ONE response slot per stop directive.
                self.pendingFinalTranscriptsLock.lock()
                self.pendingFinalTranscripts[directive.sessionId] = finalText
                self.pendingFinalTranscriptsLock.unlock()
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
