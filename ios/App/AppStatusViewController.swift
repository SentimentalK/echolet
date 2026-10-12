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

    // J4-E2E On-Device ASR section
    private let asrSectionLabel = UILabel()
    private let asrModelLabel = UILabel()
    private let loadModelButton = UIButton(type: .system)
    private let liveStatusView = UITextView()
    private let startLiveButton = UIButton(type: .system)
    private let stopLiveButton = UIButton(type: .system)
    private let prepareWarmButton = UIButton(type: .system)
    private let disarmWarmButton = UIButton(type: .system)
    private let wavRegressionButton = UIButton(type: .system)

    private var liveAsrSessionId: String?
    private var asrFinalTranscriptsShownCount = 0

    private let navSectionLabel = UILabel()
    private let returnHintLabel = UILabel()
    private let auditNavButton = UIButton(type: .system)

    override func viewDidLoad() {
        super.viewDidLoad()
        setupUI()
        setupAsrUI()
        setupNavUI()
        initializeIPC()
        AudioCaptureController.shared.delegate = self
        WarmIPCService.shared.delegate = self
        EcholetAsrService.shared.delegate = self
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

        returnHintLabel.numberOfLines = 0
        returnHintLabel.textAlignment = .center
        returnHintLabel.font = .boldSystemFont(ofSize: 18)
        returnHintLabel.textColor = .label
        returnHintLabel.backgroundColor = UIColor.systemYellow.withAlphaComponent(0.35)
        returnHintLabel.layer.cornerRadius = 8
        returnHintLabel.clipsToBounds = true
        returnHintLabel.isHidden = true
        stackView.addArrangedSubview(returnHintLabel)

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
        4. J5 Single-Button Keyboard Flow (primary path):
           - Tap "Prepare Warm Dictation" (or let the keyboard open the app automatically).
           - Tap the in-app editor below, switch to the Echolet keyboard via Globe.
           - The keyboard shows ONE button: dark "Start Listening", red "Stop Listening"
             only after the real mic/ASR ACK, then the recognized transcript is
             inserted once automatically (no Finish/Consume taps).
           - When Echolet auto-prepares for the keyboard, iOS shows no public
             automatic return; use the system Back/App Switcher to return to the
             host app — the keyboard sends START itself once the warm mic is live.
           - DEBUG diagnostics (Keep for IPC regression): "DEBUG: Send Mock Transcription".
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
    func audioCaptureController(_ controller: AudioCaptureController, didUpdateStatus status: AudioCaptureController.Status) {        switch status {
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
        // A blocked terminal state implies the warm/keyboard lifecycle settled;
        // recover the J4-E2E warm prepare affinity for an honest retry.
        prepareWarmButton.isEnabled = true
        disarmWarmButton.isEnabled = false
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
        // Mock submission is queue-owned: eligibility is validated and written
        // on WarmIPCService.ipcQueue (single writer). The completion mirrors the
        // real write/result honestly on the main thread.
        let mockText = "[Echolet Test Demo: App Group IPC OK]"

        WarmIPCService.shared.submitDebugMockResponse(recognizedText: mockText) { [weak self] succeeded, rejectionReason in
            guard let self = self else { return }

            if succeeded {
                self.lastRequestLabel.text = """
                Mock Response Written via WarmIPCService:
                Text: \(mockText)
                State: completed (final)
                """
            } else {
                self.lastRequestLabel.text = "Mock response NOT written: \(rejectionReason ?? "unknown rejection")"
            }
        }
    }

    // ============================================================
    // MARK: - J4-E2E On-Device ASR (single shared pipeline)
    // ============================================================

    private func setupAsrUI() {
        // Model + live dictation section
        asrSectionLabel.text = "On-Device ASR (J4-E2E offline)"
        asrSectionLabel.font = .boldSystemFont(ofSize: 15)
        asrSectionLabel.textAlignment = .center

        asrModelLabel.numberOfLines = 0
        asrModelLabel.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        asrModelLabel.text = "Model: bundling check pending..."

        loadModelButton.setTitle("Load Offline Model ( sherpa-onnx zh-en )", for: .normal)
        loadModelButton.titleLabel?.font = .boldSystemFont(ofSize: 13)
        loadModelButton.backgroundColor = .systemIndigo
        loadModelButton.setTitleColor(.white, for: .normal)
        loadModelButton.layer.cornerRadius = 8
        loadModelButton.addTarget(self, action: #selector(didTapLoadModel), for: .touchUpInside)

        liveStatusView.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        liveStatusView.isEditable = false
        liveStatusView.isScrollEnabled = true
        liveStatusView.layer.borderColor = UIColor.systemGray4.cgColor
        liveStatusView.layer.borderWidth = 1.0
        liveStatusView.layer.cornerRadius = 6
        liveStatusView.text = "ASR: no session. Recognized text appears here."
        liveStatusView.heightAnchor.constraint(equalToConstant: 120).isActive = true

        // Live Dictation row
        startLiveButton.setTitle("Start Live Dictation", for: .normal)
        startLiveButton.titleLabel?.font = .boldSystemFont(ofSize: 13)
        startLiveButton.backgroundColor = .systemTeal
        startLiveButton.setTitleColor(.white, for: .normal)
        startLiveButton.layer.cornerRadius = 8
        startLiveButton.addTarget(self, action: #selector(didTapStartLiveDictation), for: .touchUpInside)

        stopLiveButton.setTitle("Stop & Finalize Live", for: .normal)
        stopLiveButton.titleLabel?.font = .boldSystemFont(ofSize: 13)
        stopLiveButton.backgroundColor = .systemOrange
        stopLiveButton.setTitleColor(.white, for: .normal)
        stopLiveButton.layer.cornerRadius = 8
        stopLiveButton.addTarget(self, action: #selector(didTapStopLiveDictation), for: .touchUpInside)
        stopLiveButton.isEnabled = false

        let liveRow = UIStackView(arrangedSubviews: [startLiveButton, stopLiveButton])
        liveRow.axis = .horizontal
        liveRow.spacing = 10
        liveRow.distribution = .fillEqually

        // Warm dictation row
        prepareWarmButton.setTitle("Prepare Warm Dictation for Notes", for: .normal)
        prepareWarmButton.titleLabel?.font = .boldSystemFont(ofSize: 13)
        prepareWarmButton.backgroundColor = .systemBlue
        prepareWarmButton.setTitleColor(.white, for: .normal)
        prepareWarmButton.layer.cornerRadius = 8
        prepareWarmButton.addTarget(self, action: #selector(didTapPrepareWarm), for: .touchUpInside)

        disarmWarmButton.setTitle("Disarm / End Warm Mic", for: .normal)
        disarmWarmButton.titleLabel?.font = .boldSystemFont(ofSize: 13)
        disarmWarmButton.backgroundColor = .systemGray4
        disarmWarmButton.setTitleColor(.label, for: .normal)
        disarmWarmButton.layer.cornerRadius = 8
        disarmWarmButton.addTarget(self, action: #selector(didTapDisarmWarm), for: .touchUpInside)
        disarmWarmButton.isEnabled = false

        let warmRow = UIStackView(arrangedSubviews: [prepareWarmButton, disarmWarmButton])
        warmRow.axis = .horizontal
        warmRow.spacing = 10
        warmRow.distribution = .fillEqually

        // WAV regression row
        wavRegressionButton.setTitle("Run Pinned WAV Regression (no mic)", for: .normal)
        wavRegressionButton.titleLabel?.font = .boldSystemFont(ofSize: 13)
        wavRegressionButton.backgroundColor = .systemPurple
        wavRegressionButton.setTitleColor(.white, for: .normal)
        wavRegressionButton.layer.cornerRadius = 8
        wavRegressionButton.addTarget(self, action: #selector(didTapWavRegression), for: .touchUpInside)

        stackView.addArrangedSubview(asrSectionLabel)
        stackView.addArrangedSubview(asrModelLabel)
        stackView.addArrangedSubview(loadModelButton)
        stackView.addArrangedSubview(liveStatusView)
        stackView.addArrangedSubview(liveRow)
        stackView.addArrangedSubview(warmRow)
        stackView.addArrangedSubview(wavRegressionButton)
    }

    private func setupNavUI() {
        navSectionLabel.text = "Return experiments — one open each, then stay if it does not leave Echolet"
        navSectionLabel.font = .boldSystemFont(ofSize: 15)
        navSectionLabel.textAlignment = .center
        navSectionLabel.numberOfLines = 0

        auditNavButton.setTitle("Audit Runtime Navigation APIs", for: .normal)
        auditNavButton.titleLabel?.font = .boldSystemFont(ofSize: 13)
        auditNavButton.backgroundColor = .systemGray
        auditNavButton.setTitleColor(.white, for: .normal)
        auditNavButton.layer.cornerRadius = 8
        auditNavButton.addTarget(self, action: #selector(didTapAuditNav), for: .touchUpInside)

        stackView.addArrangedSubview(navSectionLabel)
        stackView.addArrangedSubview(auditNavButton)

        for host in EcholetLaunch.knownReturnHosts {
            stackView.addArrangedSubview(returnTestButton(
                title: "\(host.name) bundle id",
                color: .systemBlue
            ) { [weak self] in
                self?.appendLiveStatusLine("NAV-TEST: bundle \(host.bundleId)")
                WarmIPCService.shared.debugReturnByBundleId(host.bundleId)
            })
            stackView.addArrangedSubview(returnTestButton(
                title: "\(host.name) URL \(host.urlScheme)://",
                color: .systemOrange
            ) { [weak self] in
                self?.appendLiveStatusLine("NAV-TEST: url \(host.urlScheme)://")
                WarmIPCService.shared.debugReturnByURL(bundleId: host.bundleId)
            })
        }
    }

    private func returnTestButton(title: String, color: UIColor, action: @escaping () -> Void) -> UIButton {
        let button = UIButton(type: .system)
        button.setTitle(title, for: .normal)
        button.titleLabel?.font = .boldSystemFont(ofSize: 13)
        button.titleLabel?.numberOfLines = 2
        button.titleLabel?.textAlignment = .center
        button.backgroundColor = color
        button.setTitleColor(.white, for: .normal)
        button.layer.cornerRadius = 8
        button.heightAnchor.constraint(greaterThanOrEqualToConstant: 40).isActive = true
        button.addAction(UIAction { _ in action() }, for: .touchUpInside)
        return button
    }

    @objc private func didTapAuditNav() {
        auditReturnNavigationAPIs()
    }

    private func auditReturnNavigationAPIs() {
        print("[DIAG-NAV] ========== AUDIT RETURN NAVIGATION APIS ==========")

        // 1. LSApplicationWorkspace introspection
        if let wsCls = NSClassFromString("LSApplicationWorkspace") as? NSObject.Type {
            print("[DIAG-NAV] LSApplicationWorkspace class FOUND")
            let defSel = NSSelectorFromString("defaultWorkspace")
            if wsCls.responds(to: defSel), let ws = wsCls.perform(defSel)?.takeUnretainedValue() as? NSObject {
                print("[DIAG-NAV] defaultWorkspace instance ACQUIRED: \(type(of: ws))")
                var count: UInt32 = 0
                if let methods = class_copyMethodList(type(of: ws), &count) {
                    var openMethods: [String] = []
                    for i in 0..<Int(count) {
                        let name = NSStringFromSelector(method_getName(methods[i]))
                        if name.lowercased().contains("open") || name.lowercased().contains("application") {
                            openMethods.append(name)
                        }
                    }
                    free(methods)
                    print("[DIAG-NAV] LSApplicationWorkspace open/application methods (\(openMethods.count)): \(openMethods.joined(separator: ", "))")
                }
            } else {
                print("[DIAG-NAV] defaultWorkspace returns nil")
            }
        } else {
            print("[DIAG-NAV] LSApplicationWorkspace class NOT FOUND")
        }

        // 2. Priority C: UIApplication & breadcrumb / return introspection
        let app = UIApplication.shared
        var appMethodCount: UInt32 = 0
        if let methods = class_copyMethodList(type(of: app), &appMethodCount) {
            var navMethods: [String] = []
            for i in 0..<Int(appMethodCount) {
                let name = NSStringFromSelector(method_getName(methods[i]))
                let lower = name.lowercased()
                if lower.contains("breadcrumb") || lower.contains("backto") || lower.contains("previousapp") || lower.contains("return") {
                    navMethods.append(name)
                }
            }
            free(methods)
            print("[DIAG-NAV] UIApplication breadcrumb/nav methods (\(navMethods.count)): \(navMethods.joined(separator: ", "))")
        }

        // 3. Scan all runtime classes for Breadcrumb
        let numClasses = objc_getClassList(nil, 0)
        let classes = UnsafeMutablePointer<AnyClass?>.allocate(capacity: Int(numClasses))
        let realNum = objc_getClassList(AutoreleasingUnsafeMutablePointer(classes), numClasses)
        var breadcrumbClasses: [String] = []
        for i in 0..<Int(realNum) {
            if let cls = classes[i] {
                let name = NSStringFromClass(cls)
                if name.localizedCaseInsensitiveContains("breadcrumb") {
                    breadcrumbClasses.append(name)
                }
            }
        }
        classes.deallocate()
        print("[DIAG-NAV] Classes matching *Breadcrumb* in runtime (\(breadcrumbClasses.count)): \(breadcrumbClasses.joined(separator: ", "))")
        print("[DIAG-NAV] ==================================================")
    }

    // MARK: Shared ASR actions
    @objc private func didTapLoadModel() {
        appendLiveStatusLine("ASR: loading offline model (one-time on-device)...")
        loadModelButton.isEnabled = false
        EcholetAsrService.shared.loadModelIfNeeded { [weak self] ok, errorCode in
            DispatchQueue.main.async {
                guard let self = self else { return }
                if ok {
                    self.appendLiveStatusLine("ASR: OFFLINE MODEL LOADED (on-device only).")
                } else {
                    self.appendLiveStatusLine("ASR: MODEL LOAD FAIL — \(errorCode ?? "unknown")")
                    self.loadModelButton.isEnabled = true
                }
            }
        }
    }

    @objc private func didTapStartLiveDictation() {
        // Foreground REAL MIC → REAL ASR through the SHARED service pipeline
        // used by warm keyboard mode. Reserved manualTest ownership keeps the
        // microphone ownership check identical to the J3A manual test.
        startLiveButton.isEnabled = false
        loadModelButton.isEnabled = false
        EcholetAsrService.shared.loadModelIfNeeded { [weak self] ok, errorCode in
            DispatchQueue.main.async {
                guard let self = self else { return }
                guard ok else {
                    self.appendLiveStatusLine("ASR: model load fail — \(errorCode ?? "unknown")")
                    self.startLiveButton.isEnabled = true
                    self.loadModelButton.isEnabled = true
                    return
                }
                WarmIPCService.shared.registerManualStartCapture { [weak self] shouldProceed, busyReason in
                    guard let self = self else { return }
                    DispatchQueue.main.async {
                        if shouldProceed {
                            AudioCaptureController.shared.startCapture(includePCM: true) { [weak self] result in
                                guard let self = self else { return }
                                DispatchQueue.main.async {
                                    switch result {
                                    case .success(let generation):
                                        let sessionId = "live-\(UUID().uuidString.prefix(8))"
                                        self.liveAsrSessionId = sessionId
                                        EcholetAsrService.shared.beginSession(
                                            sessionId: sessionId,
                                            kind: .liveDictation,
                                            expectedGeneration: generation
                                        ) { ok, errorCode in
                                            DispatchQueue.main.async {
                                                if !ok {
                                                    self.appendLiveStatusLine("ASR session failed: \(errorCode ?? "unknown")")
                                                    self.stopLiveButton.isEnabled = false
                                                    self.startLiveButton.isEnabled = true
                                                }
                                            }
                                        }
                                        self.appendLiveStatusLine("ASR LIVE: dictation ACTIVE (session \(sessionId.prefix(8)), tap gen=\(generation)) — SPEAK NOW, then tap 'Stop & Finalize Live'.")
                                        self.stopLiveButton.isEnabled = true
                                        self.startMicButton.isEnabled = false
                                    case .failure(let failure):
                                        self.appendLiveStatusLine("Mic start failed: \(failure.localizedDescription)")
                                        self.stopLiveButton.isEnabled = false
                                        self.startLiveButton.isEnabled = true
                                        // Release manual ownership so retry works.
                                        WarmIPCService.shared.registerManualCaptureStopped()
                                    }
                                }
                            }
                        } else {
                            self.appendLiveStatusLine("Mic busy: \(busyReason ?? "unavailable")")
                            self.startLiveButton.isEnabled = true
                        }
                    }
                }
            }
        }
    }

    @objc private func didTapStopLiveDictation() {
        stopLiveButton.isEnabled = false
        stopMicButton.isEnabled = false
        guard let liveSessionId = liveAsrSessionId else {
            appendLiveStatusLine("ASR: no live session to finalize (bus error semantics).")
            startLiveButton.isEnabled = true
            appendLiveStatusLine("ASR: starting live session failed; mic released.")
            WarmIPCService.shared.registerManualCaptureStopped()
            return
        }
        appendLiveStatusLine("ASR: finalizing session id_prefix=\(String(liveSessionId.prefix(8)))...")
        WarmIPCService.shared.registerManualCaptureStopped()
        EcholetAsrService.shared.finalizeSession(sessionId: liveSessionId) { [weak self] finalText in
            DispatchQueue.main.async {
                guard let self = self else { return }
                self.sessionFinalizeCompletion(finalText)
            }
        }
    }

    private func sessionFinalizeCompletion(_ finalText: String?) {
        if let finalText = finalText, !finalText.isEmpty {
            appendLiveStatusLine("ASR FINAL (live): \(finalText)")
            asrFinalTranscriptsShownCount += 1
            // Insert into the contained test editor so the recognized phrase
            // is verifiable against manual expectations in one read-through.
            if testTextView.text.isEmpty || testTextView.text == "Tap here to test keyboard input..." {
                testTextView.text = finalText
            } else {
                testTextView.text = testTextView.text + "\n" + finalText
            }
        } else if let finalText = finalText {
            appendLiveStatusLine("ASR FINAL (live): empty/no speech — truthful empty transcript.")
        } else {
            appendLiveStatusLine("ASR FINAL (live): FAIL/CANCELLED (no final transcript available).")
        }
        liveAsrSessionId = nil
        startLiveButton.isEnabled = true
        loadModelButton.isEnabled = true
        startMicButton.isEnabled = true
    }

    @objc private func didTapPrepareWarm() {
        prepareWarmButton.isEnabled = false
        loadModelButton.isEnabled = false
        appendLiveStatusLine("WARM: preparing warm dictation for Notes (foreground-only)...")
        WarmIPCService.shared.prepareWarmDictation()
    }

    @objc private func didTapDisarmWarm() {
        WarmIPCService.shared.setUserArmMicrophone(false)
        appendLiveStatusLine("WARM: disarm requested. Mic FULL STOP after stop completes.")
        prepareWarmButton.isEnabled = true
        disarmWarmButton.isEnabled = false
    }

    @objc private func didTapWavRegression() {
        wavRegressionButton.isEnabled = false
        EcholetAsrService.shared.loadModelIfNeeded { [weak self] ok, errorCode in
            DispatchQueue.main.async {
                guard let self = self else { return }
                guard ok else {
                    self.appendLiveStatusLine("ASR: model load fail — \(errorCode ?? "unknown")")
                    self.wavRegressionButton.isEnabled = true
                    return
                }
                guard let wavPath = Bundle.main.path(forResource: "0", ofType: "wav", inDirectory: "bilingual-zh-en/test_wavs"),
                      let samples = Self.read16BitPcmMonoWav(path: wavPath) else {
                    self.appendLiveStatusLine("WAV regression: pinned wav not found or unparsable in bundled model dir.")
                    self.wavRegressionButton.isEnabled = true
                    return
                }
                EcholetAsrService.shared.runPinnedWavRegression(samples16k: samples) { text, err in
                    DispatchQueue.main.async {
                        if let text = text {
                            self.appendLiveStatusLine("WAV REGRESSION (offline deterministic): FINAL=\"\(text)\"")
                        } else {
                            self.appendLiveStatusLine("WAV REGRESSION FAIL: \(err ?? "unknown")")
                        }
                        self.wavRegressionButton.isEnabled = true
                    }
                }
            }
        }
    }

    private func appendLiveStatusLine(_ line: String) {
        let stamp = Date().addingTimeInterval(0)
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss"
        liveStatusView.text = liveStatusView.text + "\n[GEN \(formatter.string(from: stamp))] " + line
        let truncated = liveStatusView.text.count > 4000
        if truncated {
            let tailCount = liveStatusView.text.count - 4000
            liveStatusView.text = "...(older)... " + String(liveStatusView.text.suffix(tailCount))
        }
    }
}

// ============================================================
// MARK: - EcholetAsrServiceDelegate (main thread only)
// ============================================================

extension AppStatusViewController: EcholetAsrService.Delegate {
    func asrService(_ service: EcholetAsrService, didChangeModelState state: EcholetAsrService.ModelState) {
        asrModelLabel.text = "ASR model state: \(state.rawValue) (offline-only, bundled)"
        if state == .loaded {
            loadModelButton.isEnabled = false
        } else {
            loadModelButton.isEnabled = state != .loading
        }
        print("[ASR-UI] model state: \(state.rawValue)")
    }

    func asrService(_ service: EcholetAsrService, didUpdatePartialText partial: String, sessionId: String) {
        appendLiveStatusLine("ASR partial: \(partial)")
    }

    func asrService(_ service: EcholetAsrService, sessionDidStart sessionId: String, kind: EcholetAsrService.SessionKind) {
        if kind == .warmKeyboard {
            appendLiveStatusLine("ASR: WARM keyboard ASR stream session begun id_prefix=\(sessionId.prefix(8)) (one shared system used for both modes)")
        }
    }

    func asrService(_ service: EcholetAsrService, sessionDidFail sessionId: String, errorCode: String) {
        appendLiveStatusLine("ASR session FAILED: id_prefix=\(sessionId.prefix(8)) code=\(errorCode)")
        if liveAsrSessionId == sessionId {
            liveAsrSessionId = nil
            startLiveButton.isEnabled = true
            stopLiveButton.isEnabled = false
            WarmIPCService.shared.registerManualCaptureStopped()
        }
    }

    func asrService(_ service: EcholetAsrService, sessionDidDispose sessionId: String, reason: String) {
        appendLiveStatusLine("ASR session disposed: id_prefix=\(sessionId.prefix(8)) reason=\(reason)")
        if liveAsrSessionId == sessionId {
            liveAsrSessionId = nil
            startLiveButton.isEnabled = true
            stopLiveButton.isEnabled = false
        }
    }
}

// ============================================================
// MARK: - WarmIPCServiceDelegate warm standby notification
// ============================================================

extension AppStatusViewController {
    func warmIPCService(_ service: WarmIPCService, didChangeWarmStandbyActive active: Bool) {
        if active {
            appendLiveStatusLine("WARM: recording ACTIVE. \(EcholetLaunch.manualReturnHint)")
            disarmWarmButton.isEnabled = true
            prepareWarmButton.isEnabled = false
        } else {
            appendLiveStatusLine("WARM: warm dictation recorder INACTIVE.")
            disarmWarmButton.isEnabled = false
            prepareWarmButton.isEnabled = true
        }
    }

    func warmIPCService(_ service: WarmIPCService, didUpdateReturnGuidance text: String?) {
        returnHintLabel.text = text
        returnHintLabel.isHidden = (text == nil)
        if let text = text {
            appendLiveStatusLine(text)
        }
    }
}

// ============================================================
// MARK: - Pinned 16-bit PCM mono WAV reader (no framework deps)
// ============================================================

extension AppStatusViewController {
    static func read16BitPcmMonoWav(path: String) -> [Float]? {
        guard let data = FileManager.default.contents(atPath: path),
              data.count >= 44,
              data.starts(with: Array("RIFF".utf8)),
              data[8..<12].elementsEqual(Array("WAVE".utf8)) else { return nil }
        var pos = 12
        var pcmOffset: Int?
        var pcmLength = 0
        var blockAlign = 0
        while pos + 8 <= data.count {
            let chunkId = String(bytes: data[pos..<pos+4], encoding: .ascii)
            let chunkSize = Int(data[pos+4]) | Int(data[pos+5]) << 8
                | Int(data[pos+6]) << 16 | Int(data[pos+7]) << 24
            if chunkId == "fmt " {
                let audioFmt = Int(data[pos+8]) | Int(data[pos+9]) << 8
                let channels = Int(data[pos+10]) | Int(data[pos+11]) << 8
                blockAlign = Int(data[pos+16]) | Int(data[pos+17]) << 8
                let bits = Int(data[pos+22]) | Int(data[pos+23]) << 8
                guard audioFmt == 1, channels == 1, bits == 16 else { return nil }
            } else if chunkId == "data" {
                pcmOffset = pos + 8
                pcmLength = min(chunkSize, data.count - pos - 8)
            }
            pos += 8 + chunkSize + (chunkSize % 2)
        }
        guard let pcmOffset, blockAlign > 0,
              pcmLength > 0, data.count >= pcmOffset + pcmLength else { return nil }
        let count = pcmLength / 2
        var out = [Float](repeating: 0, count: count)
        let scale: Float = 1.0 / 32768.0
        for i in 0..<count {
            let lo = data[pcmOffset + 2 * i]
            let hi = data[pcmOffset + 2 * i + 1]
            let v = Int16(bitPattern: UInt16(lo) | (UInt16(hi) << 8))
            out[i] = Float(Int(v)) * scale
        }
        return out
    }
}
