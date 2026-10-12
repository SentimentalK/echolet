import UIKit
import Darwin

/// Echolet Voice Keyboard (J5 compact voice row).
///
/// ONE horizontal primary row mimics the system keyboard geometry:
/// [Globe keycap][Start/Stop filling the middle][Backspace keycap].
/// * Dark "Start Listening" while idle — tapping begins an exact-session
///   v2 START when a genuinely warm recorder is already live, or (bounded,
///   once per fresh appearance automatically / on every tap otherwise)
///   attempts to open the Echolet containing app and shows neutral
///   "Starting…" while the prepare handshake progresses.
/// * Red "Stop Listening" exclusively AFTER a real `.listening` ACK for the
///   current session; sends exact STOP (fresh requestId, sequence+1).
/// * Neutral "Finalizing…" while waiting for the correlated final transcript
///   which is then inserted into the editable host EXACTLY ONCE via
///   `textDocumentProxy`.
/// * An adjacent native BACKSPACE keycap (`delete.left.fill`): one tap
///   deletes exactly one preceding grapheme, enabled ONLY when the session
///   controller reports a positively idle-ready, session-free state.
/// * An Apple Globe key in the same row (leading edge, system-keyboard
///   convention) for next-keyboard switching.
///
/// All decision logic lives in the pure Foundation
/// `EcholetKeyboardSessionController` (CLI-testable); this controller is the
/// iOS wiring: App Group defaults, textDocumentProxy, responder-chain open,
/// host bundle id probe, bounded response polling. Strictly no microphone
/// access here; if the OS refuses the open, the UI says so honestly and the
/// keyboard keeps working on the tap/manual path.
class KeyboardViewController: UIInputViewController {

    static let appGroupId = "group.com.mainstayx.echolet.dev"

    // MARK: - Core session controller + App Group store
    private var sharedDefaults: UserDefaults?
    private var controller: EcholetKeyboardSessionController?

    // MARK: - UI Components
    private let statusLabel = UILabel()
    private let primaryButton = UIButton(type: .custom)
    private let backspaceButton = UIButton(type: .system)
    private let hintLabel = UILabel()
    private let nextKeyboardButton = UIButton(type: .system)
    private let primaryRow = UIStackView()

    private var pollTimer: Timer?
    private var lastKnownHostBundleId: String?

    // MARK: - Lifecycle

    override func viewDidLoad() {
        super.viewDidLoad()
        self.sharedDefaults = UserDefaults(suiteName: Self.appGroupId)
        buildController()
        setupUI()
    }

    private func buildController() {
        guard let defaults = sharedDefaults else {
            controller = nil
            return
        }
        let env = EcholetKeyboardSessionController.Environment(
            defaults: defaults,
            nowMs: { UInt64(Date().timeIntervalSince1970 * 1000) },
            currentAppEpoch: { [weak defaults] in
                defaults?.string(forKey: EcholetIPC.appEpochKey)
            },
            openHost: { [weak self] url in
                self?.performHostOpenAttempt(url: url) ?? .failed(reason: "controller_unavailable")
            },
            insertText: { [weak self] text in
                guard let self = self, !text.isEmpty else { return false }
                self.textDocumentProxy.insertText(text)
                return true
            },
            deleteBackward: { [weak self] count in
                guard let self = self, count > 0 else { return }
                for _ in 0..<count {
                    self.textDocumentProxy.deleteBackward()
                }
            },
            docFingerprint: { [weak self] in
                (
                    before: self?.textDocumentProxy.documentContextBeforeInput,
                    after: self?.textDocumentProxy.documentContextAfterInput,
                    selected: self?.textDocumentProxy.selectedText
                )
            },
            captureHostReturnContext: { [weak self] nonce in
                self?.captureAndStoreHostReturnContext(forNonce: nonce)
            },
            log: { event in
                // Compact DEBUG event log; never logs audio content.
                print("[Echolet-KBD] \(event)")
            }
        )
        controller = EcholetKeyboardSessionController(environment: env)
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        if let probed = probeCurrentHostBundleId() {
            lastKnownHostBundleId = probed
            print("[Echolet-KBD] APPEAR_HOST_PROBE(\(probed))")
        }
        controller?.handleAppeared()
        startPolling()
        renderDisplay()
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        stopPolling()
        controller?.handleDisappeared()
        renderDisplay()
    }

    override func textWillChange(_ textInput: UITextInput?) {
        super.textWillChange(textInput)
        // Focus switch handling (fail closed on REAL editor changes; keeps
        // valid sessions through our own insertion churn).
        controller?.noteTextWillChange()
        renderDisplay()
    }

    override func textDidChange(_ textInput: UITextInput?) {
        super.textDidChange(textInput)
        renderDisplay()
    }

    // MARK: - Containing-app open via responder chain
    //
    // `NSExtensionContext.open` is rejected for keyboard extensions and
    // legacy `openURL:` is a no-op on iOS 18. The user explicitly permits
    // the non-public path: walk `UIResponder.next` until an object that
    // responds to `open(_:options:completionHandler:)` (the UIApplication
    // instance) is found, then invoke it through method-for-selector /
    // unsafeBitCast to the IMP signature.

    /// ObjC method signature: - (void)openURL:(NSURL*)url
    ///         options:(NSDictionary<UIApplicationOpenURLOptionsKey,id>*)options
    /// completionHandler:(void (^)(BOOL))completion
    private typealias OpenURLMethod =
        @convention(c) (NSObject, Selector, URL, NSDictionary, Any?) -> Void
    private typealias OpenCompletionBlock =
        @convention(block) (Bool) -> Void

    private func performHostOpenAttempt(url: URL) -> EcholetKeyboardSessionController.OpenAttemptResult {
        // Direct private LSApplicationWorkspace path:
        // On iOS/iPadOS extensions with Full Access, LSApplicationWorkspace reliably launches
        // the containing app via custom URL scheme or bundle ID without responder chain restrictions.
        if let workspaceClass = NSClassFromString("LSApplicationWorkspace") as? NSObject.Type,
           let workspace = workspaceClass.perform(NSSelectorFromString("defaultWorkspace"))?.takeUnretainedValue() as? NSObject {
            let sensitiveSel = NSSelectorFromString("openSensitiveURL:withOptions:")
            if workspace.responds(to: sensitiveSel) {
                typealias OpenSensitiveFn = @convention(c) (NSObject, Selector, NSURL, NSDictionary) -> Bool
                let fn = unsafeBitCast(workspace.method(for: sensitiveSel), to: OpenSensitiveFn.self)
                let ok = fn(workspace, sensitiveSel, url as NSURL, [:] as NSDictionary)
                print("[Echolet-KBD] OPEN_LS_WORKSPACE(sensitive=true,result=\(ok))")
                if ok {
                    DispatchQueue.main.async { [weak self] in
                        self?.controller?.handleOpenCompletion(success: true, reason: nil)
                    }
                    return .initiated
                }
            }

            let normalSel = NSSelectorFromString("openURL:")
            if workspace.responds(to: normalSel) {
                typealias OpenFn = @convention(c) (NSObject, Selector, NSURL) -> Bool
                let fn = unsafeBitCast(workspace.method(for: normalSel), to: OpenFn.self)
                let ok = fn(workspace, normalSel, url as NSURL)
                print("[Echolet-KBD] OPEN_LS_WORKSPACE(sensitive=false,result=\(ok))")
                if ok {
                    DispatchQueue.main.async { [weak self] in
                        self?.controller?.handleOpenCompletion(success: true, reason: nil)
                    }
                    return .initiated
                }
            }
        }

        // Secondary fallback: Responder chain search.
        // Important: UIScene implements `openURL:options:completionHandler:` where options is a
        // `UISceneOpenExternalURLOptions` object, NOT an NSDictionary. Passing an NSDictionary to
        // UIScene crashes with `-[__NSDictionary0 universalLinksOnly]: unrecognized selector`.
        // Therefore, we only target UIApplication (which takes NSDictionary), or pass nil options.
        let openSelector = NSSelectorFromString("openURL:options:completionHandler:")
        guard let targetResponder = findOpenURLResponder(matching: openSelector) else {
            print("[Echolet-KBD] OPEN_RESOLVER(no_responder_with_open_options_completion)")
            return .unsupported(reason: "no_open_responder")
        }

        let isScene = targetResponder.isKind(of: NSClassFromString("UIScene") ?? NSObject.self)
        let completion: OpenCompletionBlock = { [weak self] success in
            DispatchQueue.main.async {
                print("[Echolet-KBD] OPEN_IMPL_CALLBACK(success=\(success))")
                self?.controller?.handleOpenCompletion(
                    success: success,
                    reason: success ? nil : "responder_open_rejected"
                )
            }
        }

        if isScene {
            // UIScene requires UISceneOpenExternalURLOptions; passing nil/empty options object
            let sceneOptClass = NSClassFromString("UISceneOpenExternalURLOptions") as? NSObject.Type
            let sceneOptions = sceneOptClass?.init()
            typealias SceneOpenMethod = @convention(c) (NSObject, Selector, URL, AnyObject?, Any?) -> Void
            let method = unsafeBitCast(targetResponder.method(for: openSelector), to: SceneOpenMethod.self)
            method(targetResponder, openSelector, url, sceneOptions, completion)
            print("[Echolet-KBD] OPEN_IMPL_INITIATED(target=UIScene,host=\(url.host ?? ""))")
        } else {
            let method: OpenURLMethod?
            if let target = targetResponder as? NSObject, target.responds(to: openSelector) {
                method = unsafeBitCast(target.method(for: openSelector), to: OpenURLMethod.self)
            } else {
                method = nil
            }
            guard let method = method else {
                return .unsupported(reason: "no_open_responder")
            }
            let options: NSDictionary = [:]
            method(targetResponder, openSelector, url, options, completion)
            print("[Echolet-KBD] OPEN_IMPL_INITIATED(target=UIApplication,host=\(url.host ?? ""))")
        }
        return .initiated
    }

    private func findOpenURLResponder(matching selector: Selector) -> UIResponder? {
        var responder: UIResponder? = self
        while let currentResponder = responder {
            if currentResponder.responds(to: selector) {
                return currentResponder
            }
            responder = currentResponder.next
        }
        return nil
    }

    // MARK: - Host bundle id capture (best-effort, nonce-bound)
    //
    // Probes runtime ivars and selectors for `_hostBundleID`, `_hostApplicationBundleIdentifier`,
    // etc. across parent, self, extensionContext, responder chain, and window.
    // Falls back gracefully if unavailable.

    private func probeCurrentHostBundleId() -> String? {
        func probeTarget(_ target: AnyObject, key: String) -> String? {
            // 1. Selector probe
            let sel = NSSelectorFromString(key)
            if let obj = target as? NSObject, obj.responds(to: sel) {
                if let raw = obj.perform(sel), let str = raw.takeUnretainedValue() as? String {
                    if let norm = EcholetLaunch.normalizeHostBundleId(str), EcholetLaunch.isReturnableHostBundleId(norm) {
                        return norm
                    }
                }
            }
            // 2. Safe ivar probe via ObjC runtime (no NSException possible)
            var cls: AnyClass? = type(of: target)
            while let currentCls = cls {
                if let ivar = class_getInstanceVariable(currentCls, key),
                   let enc = ivar_getTypeEncoding(ivar), enc[0] == UInt8(ascii: "@") {
                    if let raw = object_getIvar(target, ivar) as? String {
                        if let norm = EcholetLaunch.normalizeHostBundleId(raw), EcholetLaunch.isReturnableHostBundleId(norm) {
                            return norm
                        }
                    }
                }
                cls = class_getSuperclass(currentCls)
            }
            return nil
        }

        func probeAllKeys(on target: AnyObject?) -> String? {
            guard let target = target else { return nil }
            let candidateKeys = [
                "_hostBundleID",
                "hostBundleID",
                "_hostApplicationBundleIdentifier",
                "hostApplicationBundleIdentifier",
                "_hostIdentifier",
                "hostIdentifier",
                "_hostProcessBundleIdentifier",
                "__hostBundleIdentifier",
                "appId",
                "_appId",
                "applicationBundleIdentifier",
                "_applicationBundleIdentifier",
                "bundleIdentifier",
                "_bundleIdentifier"
            ]
            for key in candidateKeys {
                if let found = probeTarget(target, key: key) {
                    return found
                }
            }
            return nil
        }

        // Direct checks on self, parent chain, extensionContext, responders, view.window.
        // These succeed for UIKit hosts such as Maps. Chrome often leaves them empty.
        var parent: UIViewController? = self.parent
        var parentDepth = 0
        while let current = parent, parentDepth < 6 {
            if let found = probeAllKeys(on: current) { return found }
            parent = current.parent
            parentDepth += 1
        }
        if let found = probeAllKeys(on: self) { return found }
        if let extCtx = self.extensionContext, let found = probeAllKeys(on: extCtx) { return found }

        var responder: UIResponder? = self.next
        while let r = responder {
            if let found = probeAllKeys(on: r) { return found }
            responder = r.next
        }

        if let win = self.view.window, let found = probeAllKeys(on: win) { return found }

        if let proxy = self.textDocumentProxy as? NSObject {
            if let found = probeAllKeys(on: proxy) { return found }
            if let found = probeNestedHostBundleId(on: proxy, depth: 0) { return found }
        }

        // KeyboardKit's pre-iOS 26.4 resolver: PKService personality XPC
        // connection, then xpc_connection_copy_bundle_id. This is the path
        // that still works on iPadOS 18 when `_hostBundleID` is empty.
        if let xpcHost = hostBundleIdViaXPC() { return xpcHost }

        // Frontmost app while this keyboard is still on screen.
        if let front = frontmostApplicationBundleId() { return front }

        // Associated object check for ___hostApplicationBundleIdentifierKey
        let assocTargets: [AnyObject?] = [self, self.parent, self.extensionContext, self.view.window]
        for t in assocTargets {
            guard let target = t else { continue }
            // Probe any associated objects or properties containing host info
            if let obj = target as? NSObject {
                let keys = ["_hostProcessIdentifier", "_hostPID", "hostPID", "hostProcessIdentifier"]
                for k in keys {
                    let sel = NSSelectorFromString(k)
                    if obj.responds(to: sel) {
                        typealias PidFn = @convention(c) (NSObject, Selector) -> pid_t
                        let fn = unsafeBitCast(obj.method(for: sel), to: PidFn.self)
                        let pid = fn(obj, sel)
                        if pid > 0 {
                            if let bundleId = bundleIdForPid(pid), EcholetLaunch.isReturnableHostBundleId(bundleId) {
                                print("[Echolet-KBD] HOST_PROBE_VIA_PID(pid=\(pid),bundle=\(bundleId))")
                                return bundleId
                            }
                        }
                    }
                }
            }
        }

        return nil
    }

    /// Host bundle id from the extension's PKService XPC personality.
    /// This is the resolver KeyboardKit used through iOS 18. The C string is
    /// nil-checked; a missing id returns nil instead of crashing.
    private func hostBundleIdViaXPC() -> String? {
        let candidates: [AnyObject?] = [self.parent, self, self.extensionContext]
        var pid: Int32 = 0
        for candidate in candidates {
            guard let object = candidate else { continue }
            let found = readNumericIvar(object, name: "_hostPID")
                ?? readNumericIvar(object, name: "hostPID")
                ?? readNumericIvar(object, name: "_hostProcessIdentifier")
            if let found = found, found > 0 {
                pid = found
                break
            }
        }
        guard pid > 0 else {
            print("[Echolet-KBD] HOST_PROBE_XPC(stage=no_host_pid)")
            return nil
        }
        guard let serviceClass = NSClassFromString("PKService") as? NSObject.Type else {
            print("[Echolet-KBD] HOST_PROBE_XPC(stage=no_PKService)")
            return nil
        }
        guard let service = objectReturning(serviceClass as AnyObject, selectorName: "defaultService") as? NSObject else {
            print("[Echolet-KBD] HOST_PROBE_XPC(stage=no_defaultService)")
            return nil
        }
        guard let personalities = objectReturning(service, selectorName: "personalities") as? NSDictionary else {
            print("[Echolet-KBD] HOST_PROBE_XPC(stage=no_personalities)")
            return nil
        }
        let keyboardId = Bundle.main.bundleIdentifier ?? ""
        guard let infos = personalities[keyboardId] as? NSDictionary else {
            let keys = personalities.allKeys.map { String(describing: $0) }.joined(separator: ",")
            print("[Echolet-KBD] HOST_PROBE_XPC(stage=no_personality keyboard=\(keyboardId) keys=\(keys))")
            return nil
        }
        let wanted = NSNumber(value: pid)
        var info = infos[wanted] as? NSObject ?? infos["\(pid)"] as? NSObject
        if info == nil {
            for key in infos.allKeys {
                if let num = key as? NSNumber, num.int32Value == pid {
                    info = infos[key] as? NSObject
                    break
                }
            }
        }
        guard let personality = info else {
            let keys = infos.allKeys.map { String(describing: $0) }.joined(separator: ",")
            print("[Echolet-KBD] HOST_PROBE_XPC(stage=no_pid_entry pid=\(pid) keys=\(keys))")
            return nil
        }
        guard let connection = objectReturning(personality, selectorName: "connection") as? NSObject
                ?? objectReturning(personality, selectorName: "_connection") as? NSObject else {
            print("[Echolet-KBD] HOST_PROBE_XPC(stage=no_connection)")
            return nil
        }
        guard let xpc = objectReturning(connection, selectorName: "_xpcConnection")
                ?? objectReturning(connection, selectorName: "xpcConnection") else {
            print("[Echolet-KBD] HOST_PROBE_XPC(stage=no_xpc_connection)")
            return nil
        }
        guard let bundle = copyXPCBundleId(xpc) else {
            print("[Echolet-KBD] HOST_PROBE_XPC(stage=bundle_id_nil pid=\(pid))")
            return nil
        }
        guard let norm = EcholetLaunch.normalizeHostBundleId(bundle), EcholetLaunch.isReturnableHostBundleId(norm) else {
            print("[Echolet-KBD] HOST_PROBE_XPC(stage=rejected raw=\(bundle))")
            return nil
        }
        print("[Echolet-KBD] HOST_PROBE_XPC(pid=\(pid),host=\(norm))")
        return norm
    }

    private func readNumericIvar(_ object: AnyObject, name: String) -> Int32? {
        var cls: AnyClass? = type(of: object)
        while let current = cls {
            if let ivar = class_getInstanceVariable(current, name),
               let encPtr = ivar_getTypeEncoding(ivar) {
                let enc = String(cString: encPtr)
                let raw = Unmanaged.passUnretained(object).toOpaque().advanced(by: ivar_getOffset(ivar))
                switch enc {
                case "i", "I":
                    return raw.load(as: Int32.self)
                case "q", "Q":
                    let value = raw.load(as: Int64.self)
                    if value < 0 || value > Int64(Int32.max) { return nil }
                    return Int32(value)
                case "l", "L":
                    let value = raw.load(as: Int.self)
                    if value < 0 || value > Int(Int32.max) { return nil }
                    return Int32(value)
                default:
                    if enc.hasPrefix("@"), let num = object_getIvar(object, ivar) as? NSNumber {
                        return num.int32Value
                    }
                }
            }
            cls = class_getSuperclass(current)
        }
        return nil
    }

    private func objectReturning(_ object: AnyObject, selectorName: String) -> AnyObject? {
        let sel = NSSelectorFromString(selectorName)
        guard object.responds(to: sel),
              let cls = object_getClass(object),
              let method = class_getInstanceMethod(cls, sel) else {
            return nil
        }
        let ret = method_copyReturnType(method)
        defer { free(ret) }
        guard ret[0] == UInt8(ascii: "@") else { return nil }
        return object.perform(sel)?.takeUnretainedValue()
    }

    private func copyXPCBundleId(_ connection: AnyObject) -> String? {
        let paths = ["/usr/lib/libSystem.B.dylib", "/usr/lib/libc.dylib"]
        for path in paths {
            guard let handle = dlopen(path, RTLD_NOW) else { continue }
            defer { dlclose(handle) }
            guard let sym = dlsym(handle, "xpc_connection_copy_bundle_id") else { continue }
            typealias CopyFn = @convention(c) (AnyObject) -> UnsafePointer<CChar>?
            let fn = unsafeBitCast(sym, to: CopyFn.self)
            guard let cString = fn(connection) else { return nil }
            return String(cString: cString)
        }
        print("[Echolet-KBD] HOST_PROBE_XPC(stage=symbol_missing)")
        return nil
    }

    /// SpringBoard's frontmost display identifier, called before Echolet is opened.
    private func frontmostApplicationBundleId() -> String? {
        let path = "/System/Library/PrivateFrameworks/SpringBoardServices.framework/SpringBoardServices"
        guard let handle = dlopen(path, RTLD_LAZY) else {
            print("[Echolet-KBD] HOST_PROBE_FRONTMOST(dlopen_failed)")
            return nil
        }
        defer { dlclose(handle) }
        let symbolNames = [
            "SBSCopyFrontmostApplicationDisplayIdentifier",
            "SBFrontmostApplicationDisplayIdentifier"
        ]
        for name in symbolNames {
            guard let sym = dlsym(handle, name) else { continue }
            typealias CopyFn = @convention(c) () -> UnsafeRawPointer?
            let fn = unsafeBitCast(sym, to: CopyFn.self)
            guard let ptr = fn() else {
                print("[Echolet-KBD] HOST_PROBE_FRONTMOST(symbol=\(name),value=nil)")
                continue
            }
            let unmanaged = Unmanaged<CFString>.fromOpaque(ptr)
            let raw = (name.contains("Copy") ? unmanaged.takeRetainedValue() : unmanaged.takeUnretainedValue()) as String
            if let norm = EcholetLaunch.normalizeHostBundleId(raw), EcholetLaunch.isReturnableHostBundleId(norm) {
                print("[Echolet-KBD] HOST_PROBE_FRONTMOST(symbol=\(name),host=\(norm))")
                return norm
            }
            print("[Echolet-KBD] HOST_PROBE_FRONTMOST(symbol=\(name),rejected=\(raw))")
        }
        print("[Echolet-KBD] HOST_PROBE_FRONTMOST(unavailable)")
        return nil
    }

    /// One level into RTI / document-trait objects. Only follows ivars whose
    /// names look like host/session/trait state, and only object ivars.
    private func probeNestedHostBundleId(on target: AnyObject, depth: Int) -> String? {
        guard depth < 2 else { return nil }
        let hints = ["trait", "session", "document", "host", "context", "rti", "state", "proxy"]
        func matches(_ name: String) -> Bool {
            let lower = name.lowercased()
            return hints.contains(where: { lower.contains($0) })
        }
        func stringIfReturnable(_ target: AnyObject, key: String) -> String? {
            let sel = NSSelectorFromString(key)
            if let obj = target as? NSObject, obj.responds(to: sel),
               let raw = obj.perform(sel)?.takeUnretainedValue() as? String,
               let norm = EcholetLaunch.normalizeHostBundleId(raw),
               EcholetLaunch.isReturnableHostBundleId(norm) {
                return norm
            }
            var cls: AnyClass? = type(of: target)
            while let current = cls {
                if let ivar = class_getInstanceVariable(current, key),
                   let enc = ivar_getTypeEncoding(ivar), enc[0] == UInt8(ascii: "@"),
                   let raw = object_getIvar(target, ivar) as? String,
                   let norm = EcholetLaunch.normalizeHostBundleId(raw),
                   EcholetLaunch.isReturnableHostBundleId(norm) {
                    return norm
                }
                cls = class_getSuperclass(current)
            }
            return nil
        }
        let keys = ["appId", "_appId", "bundleIdentifier", "_bundleIdentifier", "_hostBundleID", "hostBundleID"]
        var cls: AnyClass? = type(of: target)
        while let current = cls {
            var count: UInt32 = 0
            guard let ivars = class_copyIvarList(current, &count) else {
                cls = class_getSuperclass(current)
                continue
            }
            defer { free(ivars) }
            for i in 0..<Int(count) {
                let ivar = ivars[i]
                guard let enc = ivar_getTypeEncoding(ivar), enc[0] == UInt8(ascii: "@") else { continue }
                guard let nameC = ivar_getName(ivar) else { continue }
                let name = String(cString: nameC)
                guard matches(name) else { continue }
                guard let child = object_getIvar(target, ivar) as? NSObject else { continue }
                for key in keys {
                    if let found = stringIfReturnable(child, key: key) {
                        print("[Echolet-KBD] HOST_PROBE_NESTED(ivar=\(name),key=\(key),host=\(found))")
                        return found
                    }
                }
                if let found = probeNestedHostBundleId(on: child, depth: depth + 1) { return found }
            }
            cls = class_getSuperclass(current)
        }
        return nil
    }

    /// Resolves bundle identifier for a target process ID via RunningBoardServices RBSProcessHandle
    private func bundleIdForPid(_ pid: pid_t) -> String? {
        guard pid > 0 else { return nil }
        guard let handleClass = NSClassFromString("RBSProcessHandle") as? NSObject.Type,
              let idClass = NSClassFromString("RBSProcessIdentifier") as? NSObject.Type else {
            return nil
        }
        let identSel = NSSelectorFromString("identifierWithPid:")
        guard idClass.responds(to: identSel) else { return nil }
        typealias IdentFn = @convention(c) (NSObject.Type, Selector, Int32) -> AnyObject?
        let identFn = unsafeBitCast(idClass.method(for: identSel), to: IdentFn.self)
        guard let identifier = identFn(idClass, identSel, Int32(pid)) else { return nil }

        let handleSel = NSSelectorFromString("handleForIdentifier:error:")
        guard handleClass.responds(to: handleSel) else { return nil }
        typealias HandleFn = @convention(c) (NSObject.Type, Selector, AnyObject, AutoreleasingUnsafeMutablePointer<NSError?>?) -> AnyObject?
        let handleFn = unsafeBitCast(handleClass.method(for: handleSel), to: HandleFn.self)
        var err: NSError? = nil
        guard let handle = handleFn(handleClass, handleSel, identifier, &err) as? NSObject else { return nil }

        // Read handle.bundle.identifier
        let bundleSel = NSSelectorFromString("bundle")
        if handle.responds(to: bundleSel), let bundle = handle.perform(bundleSel)?.takeUnretainedValue() as? NSObject {
            let idSel = NSSelectorFromString("identifier")
            if bundle.responds(to: idSel), let raw = bundle.perform(idSel)?.takeUnretainedValue() as? String {
                return EcholetLaunch.normalizeHostBundleId(raw)
            }
        }
        return nil
    }

    private func captureAndStoreHostReturnContext(forNonce nonce: String) {
        guard let defaults = sharedDefaults else { return }

        let probed = probeCurrentHostBundleId()
        // Fresh target host only: probed on this appearance, or an explicit debug override.
        // A previous app's bundle id must not leak into this launch.
        let debugHost = defaults.string(forKey: "echolet.debug.target.host.bundleid")
        let targetHost = [probed, debugHost].compactMap { raw -> String? in
            guard let norm = EcholetLaunch.normalizeHostBundleId(raw), EcholetLaunch.isReturnableHostBundleId(norm) else {
                return nil
            }
            return norm
        }.first

        let now = UInt64(Date().timeIntervalSince1970 * 1000)
        if let id = targetHost {
            let ctx = EcholetLaunch.HostReturnContext(launchNonce: nonce, hostBundleId: id, capturedAtMs: now)
            let wrote = EcholetLaunch.writeHostReturnContext(ctx, defaults: defaults)
            _ = EcholetLaunch.writeHostBundleId(id, defaults: defaults)
            print("[Echolet-KBD] HOST_ID(nonce=\(String(nonce.prefix(8)))…,host=\(id),stored=\(wrote))")
        } else {
            // Clear any prior host return context so we NEVER jump to a stale former host!
            EcholetLaunch.clearHostReturnContext(defaults: defaults)
            print("[Echolet-KBD] HOST_ID(no_host_available -> cleared prior host return context)")
        }
    }

    // MARK: - UI Setup

    private func setupUI() {
        view.backgroundColor = .systemGray5

        let mainStack = UIStackView()
        mainStack.translatesAutoresizingMaskIntoConstraints = false
        mainStack.axis = .vertical
        mainStack.alignment = .center
        mainStack.spacing = 4
        view.addSubview(mainStack)

        NSLayoutConstraint.activate([
            mainStack.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 4),
            mainStack.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 6),
            mainStack.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -6),
            mainStack.bottomAnchor.constraint(lessThanOrEqualTo: view.bottomAnchor, constant: -2)
        ])

        // ONE primary row across the full keyboard width, mimicking the
        // system keyboard geometry the user is used to:
        // [Globe keycap][ Start/Stop fills the middle ][Backspace keycap]
        // Both keycaps are fixed ≥44pt targets; no trash icon anywhere.
        primaryRow.translatesAutoresizingMaskIntoConstraints = false
        primaryRow.axis = .horizontal
        primaryRow.alignment = .fill
        primaryRow.distribution = .fill
        primaryRow.spacing = 6
        mainStack.addArrangedSubview(primaryRow)

        // Globe / next-keyboard affordance styled as a native keycap,
        // leading edge like the system keyboard.
        nextKeyboardButton.backgroundColor = Self.keycapColor
        nextKeyboardButton.tintColor = Self.keycapIconColor
        nextKeyboardButton.setImage(
            UIImage(systemName: "globe", withConfiguration: UIImage.SymbolConfiguration(pointSize: 17, weight: .medium)),
            for: .normal
        )
        nextKeyboardButton.accessibilityLabel = "Next keyboard"
        nextKeyboardButton.accessibilityTraits = .button
        nextKeyboardButton.isAccessibilityElement = true
        nextKeyboardButton.layer.cornerRadius = 6
        nextKeyboardButton.translatesAutoresizingMaskIntoConstraints = false
        nextKeyboardButton.setContentHuggingPriority(.required, for: .horizontal)
        nextKeyboardButton.addTarget(self, action: #selector(handleInputModeList(from:with:)), for: .allTouchEvents)
        primaryRow.addArrangedSubview(nextKeyboardButton)

        primaryButton.titleLabel?.font = .boldSystemFont(ofSize: 17)
        primaryButton.setTitleColor(.white, for: .normal)
        primaryButton.layer.cornerRadius = 10
        primaryButton.setContentHuggingPriority(.defaultLow, for: .horizontal)
        primaryButton.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        primaryButton.addTarget(self, action: #selector(didTapPrimary), for: .touchUpInside)
        primaryRow.addArrangedSubview(primaryButton)

        backspaceButton.tintColor = Self.keycapIconColor
        backspaceButton.accessibilityLabel = "Backspace"
        backspaceButton.accessibilityTraits = .button
        backspaceButton.isAccessibilityElement = true
        backspaceButton.layer.cornerRadius = 6
        backspaceButton.backgroundColor = Self.keycapColor
        backspaceButton.translatesAutoresizingMaskIntoConstraints = false
        backspaceButton.addAction(UIAction { [weak self] _ in self?.didTapBackspace() }, for: .touchUpInside)
        primaryRow.addArrangedSubview(backspaceButton)
        NSLayoutConstraint.activate([
            nextKeyboardButton.widthAnchor.constraint(equalToConstant: 46),
            nextKeyboardButton.heightAnchor.constraint(equalToConstant: 46),
            backspaceButton.widthAnchor.constraint(equalToConstant: 46),
            backspaceButton.heightAnchor.constraint(equalToConstant: 46),
            primaryButton.heightAnchor.constraint(equalToConstant: 46)
        ])
        applyBackspace(enabled: false, animated: false)

        // Single-line truthful status.
        statusLabel.font = .systemFont(ofSize: 12, weight: .medium)
        statusLabel.textColor = .label
        statusLabel.textAlignment = .center
        statusLabel.numberOfLines = 1
        statusLabel.adjustsFontSizeToFitWidth = true
        statusLabel.minimumScaleFactor = 0.8
        statusLabel.lineBreakMode = .byTruncatingTail
        mainStack.addArrangedSubview(statusLabel)
        statusLabel.widthAnchor.constraint(equalTo: mainStack.widthAnchor).isActive = true

        // Short failure-only hint. Hidden unless manual return/fallback
        // guidance is actually needed.
        hintLabel.font = .systemFont(ofSize: 10)
        hintLabel.textColor = .secondaryLabel
        hintLabel.textAlignment = .center
        hintLabel.numberOfLines = 2
        hintLabel.lineBreakMode = .byTruncatingTail
        hintLabel.isHidden = true
        mainStack.addArrangedSubview(hintLabel)
        hintLabel.widthAnchor.constraint(equalTo: mainStack.widthAnchor).isActive = true

        renderDisplay()
    }

    // MARK: - Dynamic appearance colors (dark/light contrast)

    /// Dark charcoal Start, native-active in dark mode via lightened shade.
    private static let darkStartColor = UIColor { trait in
        trait.userInterfaceStyle == .dark
            ? UIColor(red: 0.30, green: 0.30, blue: 0.33, alpha: 1.0)
            : UIColor(red: 0.09, green: 0.09, blue: 0.11, alpha: 1.0)
    }
    private static let keycapColor = UIColor { trait in
        trait.userInterfaceStyle == .dark
            ? UIColor(white: 0.32, alpha: 1.0)
            : UIColor(white: 1.0, alpha: 1.0)
    }
    private static let keycapIconColor = UIColor { trait in
        trait.userInterfaceStyle == .dark
            ? UIColor(white: 0.92, alpha: 1.0)
            : UIColor(white: 0.15, alpha: 1.0)
    }

    // MARK: - Rendering (truthful, derived from controller state)

    private func renderDisplay() {
        // Backspace projection: single production gate read; UIKit adds no
        // phase/session policy of its own. Disabled safest before any
        // controller value can be consulted.
        updateBackspace(enabled: controller?.isBackspacePermitted == true, animated: true)

        guard let controller = controller else {
            statusLabel.text = "App Group Unavailable: Entitlement Missing"
            primaryButton.isHidden = false
            primaryButton.isEnabled = false
            primaryButton.setTitle("Start Listening", for: .normal)
            primaryButton.setTitleColor(UIColor(white: 1.0, alpha: 0.4), for: .normal)
            primaryButton.backgroundColor = Self.darkStartColor
            showHint("Open the Echolet app once so this keyboard can reach it.")
            return
        }

        // Warm-standby adoption needs no special UI path; the ONE dark
        // UIKit button covers idle/open/listening states everywhere.
        primaryButton.isHidden = false
        primaryButton.setEnabled(interactive: controller.isPrimaryButtonInteractive)
        primaryButton.setTitle(controller.primaryButtonLabel, for: .normal)
        if controller.shouldShowRedStopButton {
            primaryButton.backgroundColor = .systemRed
        } else if controller.isPrimaryButtonInteractive {
            primaryButton.backgroundColor = Self.darkStartColor // dark Start
        } else {
            primaryButton.backgroundColor = .systemGray3
        }

        statusLabel.text = compactStatusText(for: controller)
        updateHint(for: controller)
    }

    private func compactStatusText(for controller: EcholetKeyboardSessionController) -> String {
        switch controller.phase {
        case .ready: return "Ready"
        case .opening: return "Opening Echolet…"
        case .starting: return "Starting…"
        case .listening: return "Listening…"
        case .finalizing: return "Finalizing…"
        case .blocked(let reason): return "Blocked: \(reason)"
        }
    }

    private func updateHint(for controller: EcholetKeyboardSessionController) {
        var hint: String? = nil
        switch controller.phase {
        case .ready, .opening:
            if controller.autoLaunchUnsupportedReason != nil {
                hint = "Open the Echolet app by hand, then switch back here."
            }
        case .blocked(let reason):
            let lower = reason.lowercased()
            if lower.contains("echolet") || lower.contains("open") {
                hint = "Open the Echolet app by hand, then switch back here."
            }
        case .starting, .listening, .finalizing:
            break
        }
        showHint(hint)
    }

    private func showHint(_ text: String?) {
        hintLabel.text = text
        hintLabel.isHidden = (text == nil)
    }

    // MARK: - Backspace key projection

    private func updateBackspace(enabled: Bool, animated: Bool) {
        backspaceButton.isEnabled = enabled
        backspaceButton.alpha = enabled ? 1.0 : 0.4
    }

    private func applyBackspace(enabled: Bool, animated: Bool) {
        let config = UIImage.SymbolConfiguration(pointSize: 22, weight: .semibold)
        backspaceButton.setImage(UIImage(systemName: "delete.left.fill", withConfiguration: config), for: .normal)
        backspaceButton.setImage(UIImage(systemName: "delete.left", withConfiguration: config), for: .disabled)
        updateBackspace(enabled: enabled, animated: animated)
    }

    // MARK: - Actions

    @objc private func didTapPrimary() {
        if let controller = controller {
            print("[Echolet-KBD-UI] PRIMARY_TAP(phase=\(controller.phase))")
        } else {
            print("[Echolet-KBD-UI] PRIMARY_TAP(no_controller)")
        }
        controller?.handlePrimaryButtonTap()
        renderDisplay()
    }

    /// One tap deletes exactly ONE preceding grapheme via the OS authority
    /// (`textDocumentProxy.deleteBackward`). No transcript buffer mutation,
    /// no repeat, no ASR race: the button is enabled only when the session
    /// controller reports the positively idle-ready state.
    private func didTapBackspace() {
        guard let controller = controller, controller.isBackspacePermitted else {
            print("[Echolet-KBD-UI] BACKSPACE_IGNORED(gate_closed)")
            return
        }
        print("[Echolet-KBD-UI] BACKSPACE_TAP")
        textDocumentProxy.deleteBackward()
        renderDisplay()
    }

    // MARK: - Bounded response polling while visible

    private func startPolling() {
        stopPolling()
        pollTimer = Timer.scheduledTimer(withTimeInterval: 0.3, repeats: true) { [weak self] _ in
            guard let self = self else { return }
            self.controller?.handleTimerTick()
            self.renderDisplay()
        }
    }

    private func stopPolling() {
        pollTimer?.invalidate()
        pollTimer = nil
    }
}

private extension UIButton {
    func setEnabled(interactive: Bool) {
        self.isEnabled = interactive
        self.titleLabel?.alpha = interactive ? 1.0 : 0.55
    }
}
