import UIKit

/// Echolet Voice Keyboard (J5 single-button).
///
/// One prominent primary button for the entire flow:
/// * Dark "Start Listening" while idle — tapping begins an exact-session
///   v2 START when a genuinely warm recorder is already live, or (bounded,
///   once per fresh appearance automatically / on every tap otherwise)
///   attempts to open the Echolet containing app through the responder
///   chain (`open(_:options:completionHandler:)` on the UIApplication
///   instance found by walking `UIResponder.next`; the user explicitly
///   permits this non-public path) and shows neutral "Starting…" while the
///   prepare handshake progresses.
/// * Red "Stop Listening" exclusively AFTER a real `.listening` ACK for the
///   current session; sends exact STOP (fresh requestId, sequence+1).
/// * Neutral "Finalizing…" while waiting for the correlated final transcript
///   which is then inserted into the editable host EXACTLY ONCE via
///   `textDocumentProxy`.
/// * A required Apple globe/next-keyboard key.
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
    private let hintLabel = UILabel()
    private let nextKeyboardButton = UIButton(type: .system)
    private let globeRow = UIStackView()

    private var pollTimer: Timer?

    // MARK: - Lifecycle

    override func viewDidLoad() {
        super.viewDidLoad()
        self.sharedDefaults = UserDefaults(suiteName: Self.appGroupId)
        buildController()
        setupUI()
        captureHostBundleId()
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
            log: { event in
                // Compact DEBUG event log; never logs audio content.
                print("[Echolet-KBD] \(event)")
            }
        )
        controller = EcholetKeyboardSessionController(environment: env)
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
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
    /// completionHandler:(void (^)(BOOL, NSError *))completion
    private typealias OpenURLMethod =
        @convention(c) (NSObject, Selector, URL, NSDictionary, Any?) -> Void
    private typealias OpenCompletionBlock =
        @convention(block) (Bool, Error?) -> Void

    private func performHostOpenAttempt(url: URL) -> EcholetKeyboardSessionController.OpenAttemptResult {
        let openSelector = NSSelectorFromString("openURL:options:completionHandler:")
        guard let targetResponder = findOpenURLResponder(matching: openSelector) else {
            print("[Echolet-KBD] OPEN_RESOLVER(no_responder_with_open_options_completion)")
            return .unsupported(reason: "no_open_responder")
        }

        let completion: OpenCompletionBlock = { [weak self] success, error in
            DispatchQueue.main.async {
                print("[Echolet-KBD] OPEN_IMPL_CALLBACK(success=\(success),error=\(error?.localizedDescription ?? "none"))")
                self?.controller?.handleOpenCompletion(
                    success: success,
                    reason: success ? nil : (error?.localizedDescription ?? "responder_open_rejected")
                )
            }
        }
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
        print("[Echolet-KBD] OPEN_IMPL_INITIATED(host=\(url.host ?? ""))")
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

    // MARK: - Host bundle id capture (best-effort, for return-to-host)
    //
    // Probes KVC `_hostBundleID` on self / its parent, then the responder
    // chain `_hostApplicationBundleIdentifier`. On iOS 18 the keys may
    // legitimately be unavailable or "<null>"; the failure is logged
    // truthfully and the manual system back gesture remains the route.

    private func captureHostBundleId() {
        guard let defaults = sharedDefaults else { return }

        func probe(_ target: NSObject, _ key: String) -> String? {
            // Undefined-key KVC raises NSException (not catchable via
            // Swift try?), so gate on the getter responding first.
            let getter = NSSelectorFromString(key)
            guard target.responds(to: getter) else { return nil }
            guard let raw = target.perform(getter), let str = raw.takeUnretainedValue() as? String else { return nil }
            return EcholetLaunch.normalizeHostBundleId(str)
        }

        var hostBundleId: String? = probe(self, "_hostBundleID")
        if hostBundleId == nil, let parent = parent {
            hostBundleId = probe(parent, "_hostBundleID")
        }
        if hostBundleId == nil {
            hostBundleId = probe(self, "_hostApplicationBundleIdentifier")
        }
        if let id = hostBundleId {
            let wrote = EcholetLaunch.writeHostBundleId(id, defaults: defaults)
            print("[Echolet-KBD] HOST_ID(probe=\(id),stored=\(wrote))")
        } else {
            print("[Echolet-KBD] HOST_ID(no_host_bundle_id_available -> manual back gesture acceptable)")
        }
    }

    // MARK: - UI Setup

    private func setupUI() {
        view.backgroundColor = .systemGray5

        let mainStack = UIStackView()
        mainStack.translatesAutoresizingMaskIntoConstraints = false
        mainStack.axis = .vertical
        mainStack.alignment = .center
        mainStack.spacing = 6
        view.addSubview(mainStack)

        NSLayoutConstraint.activate([
            mainStack.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 6),
            mainStack.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 10),
            mainStack.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -10),
            mainStack.bottomAnchor.constraint(lessThanOrEqualTo: view.bottomAnchor, constant: -6)
        ])

        // Status
        statusLabel.font = .systemFont(ofSize: 12, weight: .medium)
        statusLabel.textColor = .label
        statusLabel.textAlignment = .center
        statusLabel.numberOfLines = 2
        mainStack.addArrangedSubview(statusLabel)

        // ONE prominent primary button (dark Start / red Stop / neutral intermediate)
        primaryButton.titleLabel?.font = .boldSystemFont(ofSize: 17)
        primaryButton.setTitleColor(.white, for: .normal)
        primaryButton.layer.cornerRadius = 10
        primaryButton.translatesAutoresizingMaskIntoConstraints = false
        primaryButton.addTarget(self, action: #selector(didTapPrimary), for: .touchUpInside)
        mainStack.addArrangedSubview(primaryButton)
        primaryButton.widthAnchor.constraint(equalTo: mainStack.widthAnchor).isActive = true
        primaryButton.heightAnchor.constraint(equalToConstant: 46).isActive = true

        // Compact honest hint: manual fallback instruction + privacy note
        hintLabel.font = .systemFont(ofSize: 10)
        hintLabel.textColor = .secondaryLabel
        hintLabel.textAlignment = .center
        hintLabel.numberOfLines = 2
        hintLabel.text = "Audio runs inside the Echolet app only. If auto-return is blocked, open Echolet, then switch back to this app."
        mainStack.addArrangedSubview(hintLabel)

        // Apple-required globe / next keyboard affordance
        globeRow.axis = .horizontal
        globeRow.spacing = 12
        globeRow.alignment = .leading

        nextKeyboardButton.setTitle("🌐", for: .normal)
        nextKeyboardButton.setTitleColor(.secondaryLabel, for: .normal)
        nextKeyboardButton.setContentHuggingPriority(.required, for: .horizontal)
        nextKeyboardButton.addTarget(self, action: #selector(handleInputModeList(from:with:)), for: .allTouchEvents)
        globeRow.addArrangedSubview(nextKeyboardButton)

        mainStack.addArrangedSubview(globeRow)

        renderDisplay()
    }

    // MARK: - Rendering (truthful, derived from controller state)

    private func renderDisplay() {
        guard let controller = controller else {
            statusLabel.text = "App Group Unavailable: Entitlement Missing"
            primaryButton.isHidden = false
            primaryButton.isEnabled = false
            primaryButton.setTitle("Start Listening", for: .normal)
            primaryButton.setTitleColor(UIColor(white: 1.0, alpha: 0.4), for: .normal)
            primaryButton.backgroundColor = UIColor(white: 0.2, alpha: 1.0)
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
            primaryButton.backgroundColor = UIColor(white: 0.15, alpha: 1.0) // dark Start
        } else {
            primaryButton.backgroundColor = .systemGray3
        }

        statusLabel.numberOfLines = 2
        switch controller.phase {
        case .ready:
            if let unsupported = controller.autoLaunchUnsupportedReason {
                statusLabel.text = "Ready. (Auto-launch note: \(shortUnsupported(unsupported)). Tap to open Echolet; return via App Switcher if blocked.)"
            } else {
                statusLabel.text = "Tap the button to start."
            }
        case .opening:
            statusLabel.text = "Starting… opening Echolet app…"
        case .starting:
            statusLabel.text = "Starting… Echolet is arming the offline model + mic."
        case .listening:
            statusLabel.text = "Listening… speak now, then tap Stop."
        case .finalizing:
            statusLabel.text = "Finalizing…"
        case .blocked(let reason):
            statusLabel.text = "Blocked: \(reason)"
        }
    }

    private func shortUnsupported(_ reason: String) -> String {
        reason.count > 48 ? String(reason.prefix(48)) + "…" : reason
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
