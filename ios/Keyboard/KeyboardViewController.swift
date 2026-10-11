import UIKit
import SwiftUI

/// SwiftUI Link button for idle Start Listening navigation on iOS/iPadOS.
/// Visual styling strictly matches the dark UIKit Start Listening button:
/// font: bold 17pt, white text, 10pt corner radius, dark background (white 0.15), 46pt height.
private struct StartListeningLinkView: View {
    let url: URL

    var body: some View {
        Link(destination: url) {
            Text("Start Listening")
                .font(.system(size: 17, weight: .bold))
                .foregroundColor(.white)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(height: 46)
        .background(Color(white: 0.15))
        .cornerRadius(10)
    }
}

/// Echolet Voice Keyboard (J5 single-button).
///
/// One prominent primary button for the entire flow:
/// * Dark "Start Listening" while idle — tapping begins an exact-session
///   v2 START when a genuinely warm recorder is already live, or (bounded,
///   once per fresh appearance automatically / on every tap otherwise)
///   attempts to foreground the Echolet containing app through the only
///   public extension-safe open seam and shows neutral "Starting…" while
///   the prepare handshake progresses.
/// * Red "Stop Listening" exclusively AFTER a real `.listening` ACK for the
///   current session; sends exact STOP (fresh requestId, sequence+1).
/// * Neutral "Finalizing…" while waiting for the correlated final transcript
///   which is then inserted into the editable host EXACTLY ONCE via
///   `textDocumentProxy` (no manual Consume gesture in primary UX).
/// * A required Apple globe/next-keyboard key.
///
/// All decision logic lives in the pure Foundation
/// `EcholetKeyboardSessionController` (CLI-testable); this controller is the
/// iOS wiring: App Group defaults, textDocumentProxy, extension open seam,
/// bounded response polling. Strictly no microphone access here and no
/// private/hidden APIs — if the OS refuses the automatic open, the UI says
/// so honestly and the keyboard keeps working on the tap/manual path.
class KeyboardViewController: UIInputViewController {

    static let appGroupId = "group.com.mainstayx.echolet.dev"

    // MARK: - Core session controller + App Group store
    private var sharedDefaults: UserDefaults?
    private var controller: EcholetKeyboardSessionController?

    // MARK: - UI Components
    private let statusLabel = UILabel()
    private let primaryButton = UIButton(type: .custom)
    private var startHostingController: UIHostingController<StartListeningLinkView>?
    private var startHostingContainer = UIView()
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

    // MARK: - Public extension open seam (only public API surface)

    private func performHostOpenAttempt(url: URL) -> EcholetKeyboardSessionController.OpenAttemptResult {
        guard let context = extensionContext else {
            return .unsupported(reason: "no_extension_context")
        }
        guard context.responds(to: NSSelectorFromString("openURL:completionHandler:")) else {
            return .unsupported(reason: "no_public_open_api")
        }
        context.open(url) { [weak self] success in
            DispatchQueue.main.async {
                self?.controller?.handleOpenCompletion(success: success, reason: success ? nil : "ext_open_rejected")
            }
        }
        return .initiated
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

        // ONE prominent primary button
        primaryButton.titleLabel?.font = .boldSystemFont(ofSize: 17)
        primaryButton.setTitleColor(.white, for: .normal)
        primaryButton.layer.cornerRadius = 10
        primaryButton.translatesAutoresizingMaskIntoConstraints = false
        primaryButton.addTarget(self, action: #selector(didTapPrimary), for: .touchUpInside)
        mainStack.addArrangedSubview(primaryButton)
        primaryButton.widthAnchor.constraint(equalTo: mainStack.widthAnchor).isActive = true
        primaryButton.heightAnchor.constraint(equalToConstant: 46).isActive = true

        // SwiftUI Link container for idle Start Listening navigation
        startHostingContainer.translatesAutoresizingMaskIntoConstraints = false
        startHostingContainer.isHidden = true
        mainStack.addArrangedSubview(startHostingContainer)
        startHostingContainer.widthAnchor.constraint(equalTo: mainStack.widthAnchor).isActive = true
        startHostingContainer.heightAnchor.constraint(equalToConstant: 46).isActive = true

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
            startHostingContainer.isHidden = true
            primaryButton.isEnabled = false
            primaryButton.setTitle("Start Listening", for: .normal)
            primaryButton.setTitleColor(UIColor(white: 1.0, alpha: 0.4), for: .normal)
            primaryButton.backgroundColor = UIColor(white: 0.2, alpha: 1.0)
            return
        }

        let showSwiftUILink = (controller.phase == .ready && !controller.freshWarmLiveExists() && controller.preparedDemandURL != nil)

        if showSwiftUILink, let url = controller.preparedDemandURL {
            primaryButton.isHidden = true
            startHostingContainer.isHidden = false
            updateSwiftUILinkHosting(url: url)
        } else {
            primaryButton.isHidden = false
            startHostingContainer.isHidden = true
            primaryButton.setEnabled(interactive: controller.isPrimaryButtonInteractive)
            primaryButton.setTitle(controller.primaryButtonLabel, for: .normal)
            if controller.shouldShowRedStopButton {
                primaryButton.backgroundColor = .systemRed
            } else if controller.isPrimaryButtonInteractive {
                primaryButton.backgroundColor = UIColor(white: 0.15, alpha: 1.0) // dark Start
            } else {
                primaryButton.backgroundColor = .systemGray3
            }
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

    private func updateSwiftUILinkHosting(url: URL) {
        if let existing = startHostingController {
            existing.rootView = StartListeningLinkView(url: url)
        } else {
            let hosting = UIHostingController(rootView: StartListeningLinkView(url: url))
            hosting.view.translatesAutoresizingMaskIntoConstraints = false
            hosting.view.backgroundColor = .clear
            addChild(hosting)
            startHostingContainer.addSubview(hosting.view)
            NSLayoutConstraint.activate([
                hosting.view.topAnchor.constraint(equalTo: startHostingContainer.topAnchor),
                hosting.view.leadingAnchor.constraint(equalTo: startHostingContainer.leadingAnchor),
                hosting.view.trailingAnchor.constraint(equalTo: startHostingContainer.trailingAnchor),
                hosting.view.bottomAnchor.constraint(equalTo: startHostingContainer.bottomAnchor)
            ])
            hosting.didMove(toParent: self)
            self.startHostingController = hosting
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
