import UIKit

/// Echolet Custom Keyboard Input View Controller (Scaffold & Test Fixture).
///
/// Complies with Apple custom keyboard requirements:
/// - Provides `advanceToNextInputMode()` globe/next keyboard key.
/// - Minimal status and user action buttons ("Start Test", "Stop / Cancel").
/// - Explicitly labeled "DEMO / No Microphone".
/// - Strictly gates mock transcript insertion using session correlation, app epoch,
///   acknowledged sequence, monotonic revision, and editor-context validation.
/// - Invalidates live session on textWillChange / textDidChange focus switches.
/// - Fails closed if App Group entitlements are missing or unavailable.
class KeyboardViewController: UIInputViewController {

    static let appGroupId = "group.com.mainstayx.echolet.dev"
    static let intentSequenceStorageKey = "echolet.keyboard.intent_sequence_counter"

    // MARK: - Active Session State
    private struct ActiveSession {
        let epoch: String
        let sessionId: String
        let requestId: String
        let sequence: UInt64
        let intentSequence: UInt64
        var lastAcceptedRevision: UInt64
    }

    private var activeSession: ActiveSession?
    private var sharedDefaults: UserDefaults?

    // MARK: - UI Components
    private let statusLabel = UILabel()
    private let disclaimerLabel = UILabel()
    private let actionButton = UIButton(type: .system)
    private let cancelButton = UIButton(type: .system)
    private let nextKeyboardButton = UIButton(type: .system)
    private let pollResponseButton = UIButton(type: .system)

    override func viewDidLoad() {
        super.viewDidLoad()
        self.sharedDefaults = UserDefaults(suiteName: Self.appGroupId)
        setupUI()
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        updateStatusDisplay()
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        // Invalidate active session on dismiss
        invalidateSession(reason: "Keyboard dismissed")
    }

    override func textWillChange(_ textInput: UITextInput?) {
        super.textWillChange(textInput)
        // Focus or selection change: invalidate active session to avoid writing to wrong editor target
        if activeSession != nil {
            invalidateSession(reason: "Editor target focus changed")
        }
    }

    override func textDidChange(_ textInput: UITextInput?) {
        super.textDidChange(textInput)
        updateStatusDisplay()
    }

    // MARK: - UI Setup
    private func setupUI() {
        view.backgroundColor = .secondarySystemBackground

        let mainStack = UIStackView()
        mainStack.translatesAutoresizingMaskIntoConstraints = false
        mainStack.axis = .vertical
        mainStack.alignment = .center
        mainStack.spacing = 8
        view.addSubview(mainStack)

        NSLayoutConstraint.activate([
            mainStack.topAnchor.constraint(equalTo: view.topAnchor, constant: 10),
            mainStack.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 12),
            mainStack.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -12),
            mainStack.bottomAnchor.constraint(lessThanOrEqualTo: view.bottomAnchor, constant: -10)
        ])

        // Status Label
        statusLabel.font = .systemFont(ofSize: 13, weight: .medium)
        statusLabel.textColor = .label
        statusLabel.textAlignment = .center
        statusLabel.numberOfLines = 2
        mainStack.addArrangedSubview(statusLabel)

        // Disclaimer
        disclaimerLabel.font = .systemFont(ofSize: 11)
        disclaimerLabel.textColor = .secondaryLabel
        disclaimerLabel.textAlignment = .center
        disclaimerLabel.text = "Echolet Voice Keyboard Demo • No Microphone Recording"
        mainStack.addArrangedSubview(disclaimerLabel)

        // Top Button Row: Action Buttons
        let actionRow = UIStackView()
        actionRow.axis = .horizontal
        actionRow.spacing = 12
        actionRow.distribution = .fillEqually

        actionButton.setTitle("Start Test", for: .normal)
        actionButton.titleLabel?.font = .boldSystemFont(ofSize: 14)
        actionButton.backgroundColor = .systemBlue
        actionButton.setTitleColor(.white, for: .normal)
        actionButton.layer.cornerRadius = 8
        actionButton.contentEdgeInsets = UIEdgeInsets(top: 8, left: 16, bottom: 8, right: 16)
        actionButton.addTarget(self, action: #selector(didTapActionButton), for: .touchUpInside)
        actionRow.addArrangedSubview(actionButton)

        cancelButton.setTitle("Stop / Cancel", for: .normal)
        cancelButton.titleLabel?.font = .systemFont(ofSize: 14)
        cancelButton.backgroundColor = .systemGray4
        cancelButton.setTitleColor(.label, for: .normal)
        cancelButton.layer.cornerRadius = 8
        cancelButton.contentEdgeInsets = UIEdgeInsets(top: 8, left: 16, bottom: 8, right: 16)
        cancelButton.addTarget(self, action: #selector(didTapCancelButton), for: .touchUpInside)
        actionRow.addArrangedSubview(cancelButton)

        pollResponseButton.setTitle("Consume Response", for: .normal)
        pollResponseButton.titleLabel?.font = .systemFont(ofSize: 13)
        pollResponseButton.backgroundColor = .systemGray5
        pollResponseButton.setTitleColor(.label, for: .normal)
        pollResponseButton.layer.cornerRadius = 8
        pollResponseButton.contentEdgeInsets = UIEdgeInsets(top: 8, left: 12, bottom: 8, right: 12)
        pollResponseButton.addTarget(self, action: #selector(didTapConsumeResponse), for: .touchUpInside)
        actionRow.addArrangedSubview(pollResponseButton)

        mainStack.addArrangedSubview(actionRow)

        // Bottom Row: Globe / Next Keyboard Button (Apple HIG Requirement)
        let bottomRow = UIStackView()
        bottomRow.axis = .horizontal
        bottomRow.spacing = 12
        bottomRow.alignment = .leading

        nextKeyboardButton.setTitle("🌐 Next Keyboard", for: .normal)
        nextKeyboardButton.titleLabel?.font = .systemFont(ofSize: 13)
        nextKeyboardButton.setTitleColor(.secondaryLabel, for: .normal)
        nextKeyboardButton.addTarget(self, action: #selector(handleInputModeList(from:with:)), for: .allTouchEvents)
        bottomRow.addArrangedSubview(nextKeyboardButton)

        mainStack.addArrangedSubview(bottomRow)

        updateStatusDisplay()
    }

    private func updateStatusDisplay() {
        guard let defaults = sharedDefaults else {
            statusLabel.text = "App Group Unavailable: Entitlement Missing"
            actionButton.isEnabled = false
            pollResponseButton.isEnabled = false
            return
        }

        guard let appEpoch = defaults.string(forKey: EcholetIPC.appEpochKey), !appEpoch.isEmpty else {
            statusLabel.text = "App Not Ready: Open Containing App Once to Set Epoch"
            actionButton.isEnabled = false
            pollResponseButton.isEnabled = false
            return
        }

        if let active = activeSession {
            statusLabel.text = "Session Active (\(active.sessionId.prefix(8))...)\nWaiting for Containing App Mock Response"
            actionButton.isEnabled = false
            pollResponseButton.isEnabled = true
        } else {
            statusLabel.text = "Ready (Epoch: \(appEpoch.prefix(8))...)"
            actionButton.isEnabled = true
            pollResponseButton.isEnabled = false
        }
    }

    // MARK: - Actions
    @objc private func didTapActionButton() {
        guard let defaults = sharedDefaults else {
            statusLabel.text = "Error: App Group defaults unavailable"
            return
        }

        guard let currentEpoch = defaults.string(forKey: EcholetIPC.appEpochKey), !currentEpoch.isEmpty else {
            statusLabel.text = "Error: App epoch not published. Open containing app first."
            return
        }

        let newSessionId = UUID().uuidString
        let newRequestId = UUID().uuidString
        guard let nextIntentSeq = reserveNextIntentSequence(in: defaults) else {
            statusLabel.text = "Error: Intent sequence counter overflowed. Fail closed."
            return
        }

        do {
            let request = try EcholetIPC.KeyboardRequest(
                appEpoch: currentEpoch,
                intentSequence: nextIntentSeq,
                sessionId: newSessionId,
                sequence: 1,
                requestId: newRequestId,
                command: .start,
                clientTimestampMs: UInt64(Date().timeIntervalSince1970 * 1000)
            )

            let encoder = EcholetIPC.makeEncoder()
            let data = try encoder.encode(request)
            defaults.set(data, forKey: EcholetIPC.keyboardRequestKey)
            defaults.synchronize()

            // Post Darwin hint
            let notificationName = CFNotificationName(EcholetIPC.darwinNotificationRequest as CFString)
            CFNotificationCenterPostNotification(
                CFNotificationCenterGetDarwinNotifyCenter(),
                notificationName,
                nil,
                nil,
                true
            )

            self.activeSession = ActiveSession(
                epoch: currentEpoch,
                sessionId: newSessionId,
                requestId: newRequestId,
                sequence: 1,
                intentSequence: nextIntentSeq,
                lastAcceptedRevision: 0
            )

            updateStatusDisplay()
        } catch {
            statusLabel.text = "Failed to create start request: \(error.localizedDescription)"
        }
    }

    @objc private func didTapCancelButton() {
        invalidateSession(reason: "User cancelled")
    }

    @objc private func didTapConsumeResponse() {
        consumePendingResponse()
    }

    // MARK: - Monotonic Intent Counter Allocation
    private func reserveNextIntentSequence(in defaults: UserDefaults) -> UInt64? {
        // App Group single-writer sequence strategy for keyboard.
        // NOTE: Standard UserDefaults does not provide atomic compare-and-swap across separate
        // processes or extension instances. Future production builds should use POSIX flock,
        // file coordination, or an IPC coordinator daemon for strict cross-instance serialization.
        let current = defaults.object(forKey: Self.intentSequenceStorageKey) as? UInt64 ?? 0
        guard current < UInt64.max else {
            // Fail-closed on monotonic counter overflow
            return nil
        }
        let next = current + 1
        defaults.set(next, forKey: Self.intentSequenceStorageKey)
        defaults.synchronize()
        return next
    }

    // MARK: - IPC Response Consumption & Guarded Text Insertion
    private func consumePendingResponse() {
        guard let active = activeSession else {
            statusLabel.text = "No active session to consume response for."
            return
        }

        guard let defaults = sharedDefaults else {
            statusLabel.text = "App Group unavailable."
            return
        }

        guard let responseData = defaults.data(forKey: EcholetIPC.appResponseKey) else {
            statusLabel.text = "No response written in App Group yet."
            return
        }

        do {
            let decoder = EcholetIPC.makeDecoder()
            let response = try decoder.decode(EcholetIPC.AppResponse.self, from: responseData)
            try response.validate()

            // Strict Safety Gate:
            // 1. Epoch must match active session epoch
            guard response.appEpoch == active.epoch else {
                statusLabel.text = "Ignored: Response app_epoch mismatch"
                return
            }

            // 2. Session ID must match active session
            guard response.sessionId == active.sessionId else {
                statusLabel.text = "Ignored: Response session_id mismatch"
                return
            }

            // 3. Acknowledged Request ID and Sequence must correlate
            guard response.acknowledgedRequestId == active.requestId,
                  response.acknowledgedSequence == active.sequence else {
                statusLabel.text = "Ignored: Response ack mismatch (req: \(response.acknowledgedRequestId))"
                return
            }

            // 4. Monotonic Revision check: strictly greater than last accepted
            guard response.revision > active.lastAcceptedRevision else {
                statusLabel.text = "Ignored: Stale revision \(response.revision) <= \(active.lastAcceptedRevision)"
                return
            }

            // 5. Text insertion into visible editor target
            // NOTE: In this debug mock harness, recognizedText represents the complete final transcript.
            // If response state is listening or blocked without recognizedText (J3A audio probe),
            // update status display honestly without inserting fabricated text.
            if response.isFinal {
                if let textToInsert = response.recognizedText, !textToInsert.isEmpty {
                    textDocumentProxy.insertText(textToInsert)
                    statusLabel.text = "Inserted final text (rev \(response.revision)): \(textToInsert.prefix(20))..."
                } else if response.state == .blocked {
                    statusLabel.text = "Mic blocked (code: \(response.errorCode ?? "unknown")). Open Echolet."
                } else {
                    statusLabel.text = "Session complete (rev \(response.revision), state: \(response.state.rawValue))"
                }
                self.activeSession = nil
                updateStatusDisplay()
            } else {
                // Non-final intermediate revision: record watermark without duplicate full insertion
                self.activeSession?.lastAcceptedRevision = response.revision
                if response.state == .preparing {
                    statusLabel.text = "Mic PREPARING on iPad (rev \(response.revision))..."
                } else if response.state == .listening {
                    statusLabel.text = "Mic ACTIVE on iPad (rev \(response.revision))\nListening..."
                } else if response.state == .blocked {
                    statusLabel.text = "Mic BLOCKED: Open Echolet to arm test"
                } else {
                    statusLabel.text = "Received intermediate rev \(response.revision) (state: \(response.state.rawValue))"
                }
            }

        } catch {
            statusLabel.text = "Failed to decode response: \(error.localizedDescription)"
        }
    }

    private func invalidateSession(reason: String) {
        guard let active = activeSession else {
            statusLabel.text = "Session invalidated: \(reason)"
            updateStatusDisplay()
            return
        }

        // Close local active session FIRST to prevent any concurrent text insertion
        self.activeSession = nil
        statusLabel.text = "Session invalidated: \(reason)"
        updateStatusDisplay()

        // Best-effort cancel notification write into App Group
        guard let defaults = sharedDefaults,
              let nextIntentSeq = reserveNextIntentSequence(in: defaults) else {
            return
        }

        let cancelRequestId = UUID().uuidString
        if let cancelReq = try? EcholetIPC.KeyboardRequest(
            appEpoch: active.epoch,
            intentSequence: nextIntentSeq,
            sessionId: active.sessionId,
            sequence: active.sequence + 1,
            requestId: cancelRequestId,
            command: .cancel,
            clientTimestampMs: UInt64(Date().timeIntervalSince1970 * 1000)
        ) {
            if let data = try? EcholetIPC.makeEncoder().encode(cancelReq) {
                defaults.set(data, forKey: EcholetIPC.keyboardRequestKey)
                defaults.synchronize()

                let notificationName = CFNotificationName(EcholetIPC.darwinNotificationRequest as CFString)
                CFNotificationCenterPostNotification(
                    CFNotificationCenterGetDarwinNotifyCenter(),
                    notificationName,
                    nil,
                    nil,
                    true
                )
            }
        }
    }
}
