import UIKit
import AVFoundation

/// Minimal status and test harness view controller for the Containing App.
///
/// Responsibilities:
/// 1. Mint and publish a unique launch app_epoch UUID to App Group UserDefaults (`echolet.app.epoch.v2`).
/// 2. Display clear instructions for enabling the custom keyboard in Settings.
/// 3. Provide real microphone testing controls with permission preflight, live PCM metering (~4Hz), and frame counters.
/// 4. Provide Warm IPC coordination for keyboard requests while app is active.
/// 5. Preserve DEBUG-only mock transcription button for IPC regression testing.
/// 6. Surface explicit status/error warnings if App Group entitlements are missing or unconfigured.
class AppStatusViewController: UIViewController, AudioCaptureDelegate, WarmIPCServiceDelegate {

    static let appGroupId = "group.com.mainstayx.echolet.dev"

    // MARK: - Process State
    private var currentEpoch: String { AppDelegate.sharedEpoch }
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

    // Real Microphone Capture Test UI (J3A)
    private let micSectionLabel = UILabel()
    private let micStatusLabel = UILabel()
    private let micLevelMeterLabel = UILabel()
    private let micLevelProgress = UIProgressView(progressViewStyle: .default)
    private let micArmSwitchContainer = UIStackView()
    private let micArmSwitchLabel = UILabel()
    private let micArmSwitch = UISwitch()
    private let micActionRow = UIStackView()
    private let startMicButton = UIButton(type: .system)
    private let stopMicButton = UIButton(type: .system)

    // Contained In-App Editor Target
    private let testTextViewLabel = UILabel()
    private let testTextView = UITextView()

    // Last Request Display
    private let lastRequestLabel = UILabel()
    private let refreshButton = UIButton(type: .system)
    private let demoResponseButton = UIButton(type: .system)
    private let statusNoteLabel = UILabel()

    override func viewDidLoad() {
        super.viewDidLoad()
        setupUI()
        initializeIPC()
        AudioCaptureController.shared.delegate = self
        WarmIPCService.shared.delegate = self
        refreshIPCStatus()
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        refreshIPCStatus()
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        // Note: Polling lifecycle is app-owned (AppDelegate/SceneDelegate), not tied to VC presentation.
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
        2. Select "Echolet", then tap "Echolet" and turn on "Allow Full Access" (needed for local App Group IPC).
        3. Real Microphone Capture (J3A):
           - Tap "Enable/Arm Microphone Test" switch below to authorize warm capture.
           - Tap "Start Audio Test" to capture real PCM frames from iPad mic.
           - Speak into iPad: watch RMS level and frame count advance in real time.
           - Tap "Stop Audio Test": microphone releases cleanly and frame count stops.
        4. In-App Contained Test Flow:
           - Tap the text editor box below so the keyboard appears.
           - Switch to Echolet keyboard using the Globe (🌐) key.
           - Tap "Start Test" on the Echolet keyboard.
           - Tap "DEBUG: Send Mock Transcription" below.
           - Tap "Consume Response" on the Echolet keyboard to insert mock text into the editor.
        """
        stackView.addArrangedSubview(instructionsLabel)

        // MARK: Real Microphone Section (J3A)
        let micContainer = UIView()
        micContainer.backgroundColor = .secondarySystemBackground
        micContainer.layer.cornerRadius = 10
        micContainer.layer.masksToBounds = true

        let micStack = UIStackView()
        micStack.translatesAutoresizingMaskIntoConstraints = false
        micStack.axis = .vertical
        micStack.spacing = 10
        micContainer.addSubview(micStack)

        NSLayoutConstraint.activate([
            micStack.topAnchor.constraint(equalTo: micContainer.topAnchor, constant: 12),
            micStack.leadingAnchor.constraint(equalTo: micContainer.leadingAnchor, constant: 12),
            micStack.trailingAnchor.constraint(equalTo: micContainer.trailingAnchor, constant: -12),
            micStack.bottomAnchor.constraint(equalTo: micContainer.bottomAnchor, constant: -12)
        ])

        micSectionLabel.text = "iPad Microphone Capture Probe (J3A)"
        micSectionLabel.font = .boldSystemFont(ofSize: 15)
        micStack.addArrangedSubview(micSectionLabel)

        micStatusLabel.font = .systemFont(ofSize: 13, weight: .semibold)
        micStatusLabel.textColor = .secondaryLabel
        micStatusLabel.text = "Microphone Status: NOT RECORDING"
        micStack.addArrangedSubview(micStatusLabel)

        micLevelMeterLabel.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        micLevelMeterLabel.numberOfLines = 0
        micLevelMeterLabel.text = "Frames: 0 | RMS: -160 dB | Peak: -160 dB | Time: 0.0s"
        micStack.addArrangedSubview(micLevelMeterLabel)

        micLevelProgress.progress = 0.0
        micStack.addArrangedSubview(micLevelProgress)

        // Arm Switch
        micArmSwitchContainer.axis = .horizontal
        micArmSwitchContainer.spacing = 8
        micArmSwitchContainer.alignment = .center

        micArmSwitchLabel.text = "Enable / Arm Microphone Test:"
        micArmSwitchLabel.font = .systemFont(ofSize: 13)
        micArmSwitchContainer.addArrangedSubview(micArmSwitchLabel)

        micArmSwitch.isOn = false
        micArmSwitch.addTarget(self, action: #selector(didToggleArmSwitch(_:)), for: .valueChanged)
        micArmSwitchContainer.addArrangedSubview(micArmSwitch)
        micStack.addArrangedSubview(micArmSwitchContainer)

        // Capture Action Buttons
        micActionRow.axis = .horizontal
        micActionRow.spacing = 10
        micActionRow.distribution = .fillEqually

        startMicButton.setTitle("Start Audio Test", for: .normal)
        startMicButton.titleLabel?.font = .boldSystemFont(ofSize: 14)
        startMicButton.backgroundColor = .systemGreen
        startMicButton.setTitleColor(.white, for: .normal)
        startMicButton.layer.cornerRadius = 8
        startMicButton.addTarget(self, action: #selector(didTapStartMic), for: .touchUpInside)
        micActionRow.addArrangedSubview(startMicButton)

        stopMicButton.setTitle("Stop Audio Test", for: .normal)
        stopMicButton.titleLabel?.font = .boldSystemFont(ofSize: 14)
        stopMicButton.backgroundColor = .systemRed
        stopMicButton.setTitleColor(.white, for: .normal)
        stopMicButton.layer.cornerRadius = 8
        stopMicButton.addTarget(self, action: #selector(didTapStopMic), for: .touchUpInside)
        stopMicButton.isEnabled = false
        micActionRow.addArrangedSubview(stopMicButton)

        micStack.addArrangedSubview(micActionRow)
        stackView.addArrangedSubview(micContainer)

        // Contained In-App Editor Target
        testTextViewLabel.text = "In-App Test Editor (Tap here to summon Echolet keyboard):"
        testTextViewLabel.font = .boldSystemFont(ofSize: 14)
        stackView.addArrangedSubview(testTextViewLabel)

        testTextView.font = .systemFont(ofSize: 15)
        testTextView.layer.borderColor = UIColor.systemGray3.cgColor
        testTextView.layer.borderWidth = 1.0
        testTextView.layer.cornerRadius = 8
        testTextView.layer.masksToBounds = true
        testTextView.text = "Tap here to test keyboard input..."
        testTextView.translatesAutoresizingMaskIntoConstraints = false
        testTextView.heightAnchor.constraint(equalToConstant: 90).isActive = true
        stackView.addArrangedSubview(testTextView)

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
        statusNoteLabel.text = "Audio capture runs entirely on-device; PCM frames are measured for level/duration only and never saved or transmitted."
        stackView.addArrangedSubview(statusNoteLabel)
    }

    private func initializeIPC() {
        let defaults = UserDefaults(suiteName: Self.appGroupId)
        self.sharedDefaults = defaults

        if let defaults = defaults {
            defaults.set(currentEpoch, forKey: EcholetIPC.appEpochKey)
            defaults.synchronize()
            appGroupStatusLabel.text = "App Group: Connected (\(Self.appGroupId))"
            appGroupStatusLabel.textColor = .systemGreen
        } else {
            appGroupStatusLabel.text = "App Group: FAILED / BLOCKED. Provisioning entitlement missing or invalid for group '\(Self.appGroupId)'."
            appGroupStatusLabel.textColor = .systemRed
        }
    }

    // Monotonic generation for local UI permission checks
    private var uiPermissionGeneration: UInt64 = 0

    // MARK: - Real Microphone Controls (J3A)
    @objc private func didToggleArmSwitch(_ sender: UISwitch) {
        uiPermissionGeneration &+= 1
        let currentUIGen = uiPermissionGeneration

        WarmIPCService.shared.setUserArmMicrophone(sender.isOn)
        if sender.isOn {
            AudioCaptureController.shared.requestMicrophonePermission { [weak self] granted in
                DispatchQueue.main.async {
                    guard let self = self, self.uiPermissionGeneration == currentUIGen else { return }
                    if !granted {
                        sender.isOn = false
                        WarmIPCService.shared.setUserArmMicrophone(false)
                        self.micStatusLabel.text = "Microphone Status: PERMISSION DENIED (Blocked)"
                        self.micStatusLabel.textColor = .systemRed
                    }
                }
            }
        } else {
            // Disarming mic: stop capture if actively recording
            WarmIPCService.shared.registerManualCaptureStopped()
        }
    }

    @objc private func didTapStartMic() {
        uiPermissionGeneration &+= 1
        let currentUIGen = uiPermissionGeneration

        AudioCaptureController.shared.requestMicrophonePermission { [weak self] granted in
            guard let self = self else { return }
            DispatchQueue.main.async {
                guard self.uiPermissionGeneration == currentUIGen else { return }
                if granted {
                    self.micArmSwitch.isOn = true
                    WarmIPCService.shared.setUserArmMicrophone(true)
                    // Register manual capture owner with WarmIPCService
                    WarmIPCService.shared.registerManualStartCapture { shouldProceed, busyReason in
                        if shouldProceed {
                            AudioCaptureController.shared.startCapture()
                        } else {
                            if let reason = busyReason {
                                self.micStatusLabel.text = "Microphone Status: BUSY — \(reason). Retry after the keyboard session stops."
                            } else {
                                self.micStatusLabel.text = "Microphone Status: BUSY — keyboard capture is active. Retry after it stops."
                            }
                            self.micStatusLabel.textColor = .systemOrange
                        }
                    }
                } else {
                    self.micStatusLabel.text = "Microphone Status: PERMISSION DENIED"
                    self.micStatusLabel.textColor = .systemRed
                }
            }
        }
    }

    @objc private func didTapStopMic() {
        uiPermissionGeneration &+= 1
        WarmIPCService.shared.registerManualCaptureStopped()
    }

    // MARK: - AudioCaptureDelegate
    func audioCaptureController(_ controller: AudioCaptureController, didUpdateStatus status: AudioCaptureController.Status) {
        switch status {
        case .recording:
            micStatusLabel.text = "Microphone Status: ACTIVE RECORDING (Hardware Tap On)"
            micStatusLabel.textColor = .systemGreen
            startMicButton.isEnabled = false
            stopMicButton.isEnabled = true
        case .stopped:
            micStatusLabel.text = "Microphone Status: STOPPED (Hardware Tap Released)"
            micStatusLabel.textColor = .systemGray
            startMicButton.isEnabled = true
            stopMicButton.isEnabled = false
        case .interrupted:
            micStatusLabel.text = "Microphone Status: INTERRUPTED (Audio Session Preempted)"
            micStatusLabel.textColor = .systemOrange
            startMicButton.isEnabled = true
            stopMicButton.isEnabled = false
        case .blocked:
            micStatusLabel.text = "Microphone Status: BLOCKED (Permission Denied)"
            micStatusLabel.textColor = .systemRed
            startMicButton.isEnabled = true
            stopMicButton.isEnabled = false
        case .failed:
            micStatusLabel.text = "Microphone Status: FAILED (AudioEngine Exception)"
            micStatusLabel.textColor = .systemRed
            startMicButton.isEnabled = true
            stopMicButton.isEnabled = false
        case .ready:
            micStatusLabel.text = "Microphone Status: READY (Permission Granted)"
            micStatusLabel.textColor = .systemBlue
            startMicButton.isEnabled = true
            stopMicButton.isEnabled = false
        case .idle:
            micStatusLabel.text = "Microphone Status: NOT RECORDING (Idle)"
            micStatusLabel.textColor = .secondaryLabel
            startMicButton.isEnabled = true
            stopMicButton.isEnabled = false
        case .requestingPermission:
            micStatusLabel.text = "Microphone Status: REQUESTING PERMISSION..."
            micStatusLabel.textColor = .systemOrange
            startMicButton.isEnabled = false
            stopMicButton.isEnabled = false
        case .starting:
            micStatusLabel.text = "Microphone Status: STARTING ENGINE..."
            micStatusLabel.textColor = .systemOrange
            startMicButton.isEnabled = false
            stopMicButton.isEnabled = true
        }
    }

    func audioCaptureController(_ controller: AudioCaptureController, didUpdateMetrics metrics: AudioCaptureController.Metrics) {
        let text = String(
            format: "Frames: %llu | RMS: %.1f dB | Peak: %.1f dB | Time: %.1fs (%.0f Hz)",
            metrics.frameCount,
            metrics.rmsPower,
            metrics.peakPower,
            metrics.elapsedSeconds,
            metrics.sampleRate
        )
        micLevelMeterLabel.text = text

        // Normalize RMS (-60 dB to 0 dB) to [0.0, 1.0] for level meter
        let clampedRms = max(-60.0, min(0.0, metrics.rmsPower))
        let normalized = (clampedRms + 60.0) / 60.0
        micLevelProgress.setProgress(normalized, animated: true)
    }

    func audioCaptureController(_ controller: AudioCaptureController, didFailWithError error: AudioCaptureController.CaptureError) {
        micStatusLabel.text = "Error: \(error.localizedDescription)"
        micStatusLabel.textColor = .systemRed
    }

    // MARK: - WarmIPCServiceDelegate
    func warmIPCService(_ service: WarmIPCService, didAdmitStartSession sessionId: String, intentSequence: UInt64) {
        lastRequestLabel.text = "Warm IPC Admitted START:\nSession: \(sessionId)\nIntent Seq: \(intentSequence)"
        lastRequestLabel.textColor = .systemGreen
    }

    func warmIPCService(_ service: WarmIPCService, didAdmitStopSession sessionId: String, intentSequence: UInt64) {
        lastRequestLabel.text = "Warm IPC Admitted STOP:\nSession: \(sessionId)\nIntent Seq: \(intentSequence)"
        lastRequestLabel.textColor = .systemBlue
    }

    func warmIPCService(_ service: WarmIPCService, didRejectRequest description: String) {
        lastRequestLabel.text = "Warm IPC Rejected Request:\n\(description)"
        lastRequestLabel.textColor = .systemOrange
    }

    func warmIPCService(_ service: WarmIPCService, didEncounterBlockedState reason: String) {
        lastRequestLabel.text = "Warm IPC Blocked:\n\(reason)"
        lastRequestLabel.textColor = .systemRed
    }

    // MARK: - Manual IPC Refresh & Mock Response
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

            let mockText = "[Echolet Test Demo: App Group IPC OK]"

            WarmIPCService.shared.writeResponseSnapshot(
                sessionId: request.sessionId,
                acknowledgedRequestId: request.requestId,
                acknowledgedSequence: request.sequence,
                state: .completed,
                recognizedText: mockText,
                isFinal: true,
                errorCode: nil
            )

            lastRequestLabel.text = """
            Mock Response Written via WarmIPCService:
            Session: \(request.sessionId)
            Ack Request: \(request.requestId)
            Text: \(mockText)
            State: completed (final)
            """
        } catch {
            lastRequestLabel.text = "Failed to write mock response: \(error.localizedDescription)"
        }
    }
}
