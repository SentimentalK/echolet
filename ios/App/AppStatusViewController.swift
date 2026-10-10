import UIKit

/// Minimal status and test harness view controller for the Containing App.
///
/// Responsibilities:
/// 1. Mint and publish a unique launch app_epoch UUID to App Group UserDefaults (`echolet.app.epoch.v2`).
/// 2. Display clear instructions for enabling the custom keyboard in Settings.
/// 3. Provide a DEBUG-only test responder button that monitors for valid pending Keyboard requests
///    in the App Group defaults and generates an explicit mock response snapshot.
/// 4. Surface explicit status/error warnings if App Group entitlements are missing or unconfigured.
class AppStatusViewController: UIViewController {

    static let appGroupId = "group.com.mainstayx.echolet.dev"

    // MARK: - Process State
    private let currentEpoch: String = UUID().uuidString
    private var sharedDefaults: UserDefaults?
    private var lastObservedRequestId: String?
    private var responseRevision: UInt64 = 0

    // MARK: - UI Elements
    private let scrollView = UIScrollView()
    private let stackView = UIStackView()

    private let titleLabel = UILabel()
    private let epochStatusLabel = UILabel()
    private let appGroupStatusLabel = UILabel()
    private let instructionsLabel = UILabel()
    private let lastRequestLabel = UILabel()
    private let demoResponseButton = UIButton(type: .system)
    private let refreshButton = UIButton(type: .system)
    private let statusNoteLabel = UILabel()

    override func viewDidLoad() {
        super.viewDidLoad()
        setupUI()
        initializeIPC()
        refreshIPCStatus()
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        refreshIPCStatus()
    }

    private func setupUI() {
        view.backgroundColor = .systemBackground

        scrollView.translatesAutoresizingMaskIntoConstraints = false
        stackView.translatesAutoresizingMaskIntoConstraints = false
        stackView.axis = .vertical
        stackView.spacing = 16
        stackView.alignment = .fill

        view.addSubview(scrollView)
        scrollView.addSubview(stackView)

        NSLayoutConstraint.activate([
            scrollView.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            scrollView.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 20),
            scrollView.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -20),
            scrollView.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor),

            stackView.topAnchor.constraint(equalTo: scrollView.topAnchor, constant: 16),
            stackView.leadingAnchor.constraint(equalTo: scrollView.leadingAnchor),
            stackView.trailingAnchor.constraint(equalTo: scrollView.trailingAnchor),
            stackView.bottomAnchor.constraint(equalTo: scrollView.bottomAnchor, constant: -16),
            stackView.widthAnchor.constraint(equalTo: scrollView.widthAnchor)
        ])

        // Title
        titleLabel.text = "Echolet Voice Keyboard (Harness)"
        titleLabel.font = .boldSystemFont(ofSize: 22)
        titleLabel.numberOfLines = 0
        stackView.addArrangedSubview(titleLabel)

        // Epoch
        epochStatusLabel.numberOfLines = 0
        epochStatusLabel.font = .monospacedSystemFont(ofSize: 13, weight: .regular)
        epochStatusLabel.textColor = .secondaryLabel
        epochStatusLabel.text = "App Epoch: \(currentEpoch)"
        stackView.addArrangedSubview(epochStatusLabel)

        // App Group Status
        appGroupStatusLabel.numberOfLines = 0
        appGroupStatusLabel.font = .systemFont(ofSize: 14, weight: .medium)
        stackView.addArrangedSubview(appGroupStatusLabel)

        // Instructions
        instructionsLabel.numberOfLines = 0
        instructionsLabel.font = .systemFont(ofSize: 14)
        instructionsLabel.text = """
        Keyboard Setup Guide:
        1. Open Settings -> General -> Keyboard -> Keyboards -> Add New Keyboard.
        2. Select "Echolet".
        3. Note: If using an Apple Personal Team / Free Account, App Groups entitlement may not be provisioned by Apple. If App Group defaults are unavailable, cross-process IPC will fail-closed.
        4. Test Flow: Open any typing editor, switch to Echolet keyboard, tap "Start Test". Switch back to this app (must be foreground/warm), tap "Send Mock Transcription", then return to the editor to observe inserted text.
        """
        stackView.addArrangedSubview(instructionsLabel)

        // Last Request Display
        lastRequestLabel.numberOfLines = 0
        lastRequestLabel.font = .monospacedSystemFont(ofSize: 13, weight: .regular)
        lastRequestLabel.backgroundColor = .secondarySystemBackground
        lastRequestLabel.layer.cornerRadius = 8
        lastRequestLabel.layer.masksToBounds = true
        lastRequestLabel.text = "No pending request detected."
        stackView.addArrangedSubview(lastRequestLabel)

        // Actions
        refreshButton.setTitle("Check for Incoming Request", for: .normal)
        refreshButton.addTarget(self, action: #selector(didTapRefresh), for: .touchUpInside)
        stackView.addArrangedSubview(refreshButton)

        demoResponseButton.setTitle("DEBUG: Send Mock Transcription", for: .normal)
        demoResponseButton.titleLabel?.font = .boldSystemFont(ofSize: 16)
        demoResponseButton.addTarget(self, action: #selector(didTapDemoResponse), for: .touchUpInside)
        demoResponseButton.isEnabled = false
        stackView.addArrangedSubview(demoResponseButton)

        // Warning/Note
        statusNoteLabel.numberOfLines = 0
        statusNoteLabel.font = .systemFont(ofSize: 12)
        statusNoteLabel.textColor = .tertiaryLabel
        statusNoteLabel.text = "DEMO ONLY: No microphone recording or real ASR models run in this scaffold."
        stackView.addArrangedSubview(statusNoteLabel)
    }

    private func initializeIPC() {
        let defaults = UserDefaults(suiteName: Self.appGroupId)
        self.sharedDefaults = defaults

        if let defaults = defaults {
            // Write our fresh process epoch
            defaults.set(currentEpoch, forKey: EcholetIPC.appEpochKey)
            defaults.synchronize()
            appGroupStatusLabel.text = "App Group: Connected (\(Self.appGroupId))"
            appGroupStatusLabel.textColor = .systemGreen
        } else {
            appGroupStatusLabel.text = "App Group: FAILED / BLOCKED. Provisioning entitlement missing or invalid for group '\(Self.appGroupId)'."
            appGroupStatusLabel.textColor = .systemRed
        }
    }

    @objc private func didTapRefresh() {
        refreshIPCStatus()
    }

    private func refreshIPCStatus() {
        guard let defaults = sharedDefaults else {
            lastRequestLabel.text = "Error: App Group defaults unavailable. Free Personal Team entitlement block suspected."
            demoResponseButton.isEnabled = false
            return
        }

        guard let requestData = defaults.data(forKey: EcholetIPC.keyboardRequestKey) else {
            lastRequestLabel.text = "App Group: Connected.\nNo keyboard request written yet."
            demoResponseButton.isEnabled = false
            return
        }

        do {
            let decoder = EcholetIPC.makeDecoder()
            let request = try decoder.decode(EcholetIPC.KeyboardRequest.self, from: requestData)
            try request.validate()

            // Epoch validation
            if request.appEpoch != currentEpoch {
                lastRequestLabel.text = """
                Pending Request: STALE EPOCH REJECTED
                Request ID: \(request.requestId)
                Command: \(request.command.rawValue)
                Expected Epoch: \(currentEpoch)
                Request Epoch: \(request.appEpoch)
                """
                demoResponseButton.isEnabled = false
                return
            }

            self.lastObservedRequestId = request.requestId
            lastRequestLabel.text = """
            Pending Valid Request:
            Session ID: \(request.sessionId)
            Sequence: \(request.sequence)
            Intent Seq: \(request.intentSequence)
            Request ID: \(request.requestId)
            Command: \(request.command.rawValue)
            Timestamp: \(request.clientTimestampMs ?? 0)
            """
            demoResponseButton.isEnabled = (request.command == .start)
        } catch {
            lastRequestLabel.text = "Error decoding request: \(error.localizedDescription)"
            demoResponseButton.isEnabled = false
        }
    }

    @objc private func didTapDemoResponse() {
        guard let defaults = sharedDefaults else { return }
        guard let requestData = defaults.data(forKey: EcholetIPC.keyboardRequestKey) else { return }

        do {
            let decoder = EcholetIPC.makeDecoder()
            let request = try decoder.decode(EcholetIPC.KeyboardRequest.self, from: requestData)
            try request.validate()

            guard request.appEpoch == currentEpoch else {
                lastRequestLabel.text = "Cannot respond: Request belongs to different app epoch."
                return
            }

            responseRevision += 1
            let mockText = "[Echolet Test Demo: App Group IPC OK revision \(responseRevision)]"

            let response = try EcholetIPC.AppResponse(
                appEpoch: currentEpoch,
                sessionId: request.sessionId,
                acknowledgedRequestId: request.requestId,
                acknowledgedSequence: request.sequence,
                revision: responseRevision,
                state: .completed,
                recognizedText: mockText,
                isFinal: true,
                errorCode: nil,
                serverTimestampMs: UInt64(Date().timeIntervalSince1970 * 1000)
            )

            let encoder = EcholetIPC.makeEncoder()
            let encodedData = try encoder.encode(response)
            defaults.set(encodedData, forKey: EcholetIPC.appResponseKey)
            defaults.synchronize()

            // Optional Darwin notification hint
            let notificationName = CFNotificationName(EcholetIPC.darwinNotificationResponse as CFString)
            CFNotificationCenterPostNotification(
                CFNotificationCenterGetDarwinNotifyCenter(),
                notificationName,
                nil,
                nil,
                true
            )

            lastRequestLabel.text = """
            Mock Response Written:
            Session: \(request.sessionId)
            Ack Request: \(request.requestId)
            Revision: \(responseRevision)
            Text: \(mockText)
            State: completed (final)
            """
        } catch {
            lastRequestLabel.text = "Failed to write mock response: \(error.localizedDescription)"
        }
    }
}
