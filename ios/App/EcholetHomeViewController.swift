import UIKit
import AVFoundation
import Foundation

/// Echolet product Home panel (containing app, Increment B).
///
/// A clean scrollable control panel visually aligned with the Desktop
/// Slint panel: narrow honest header, dark Start / red Disarm warm-mic
/// card, the one truthfully installed offline model card, keyboard setup
/// guidance, and an on-device privacy footer. No harness controls, epoch
/// values, IPC internals, mock buttons or navigation experiments in the
/// product surface — production logic lives in `WarmIPCService` /
/// `EcholetAsrService` and this controller merely mirrors their published
/// truth through the pure Foundation `EcholetHomePresenter`.
///
/// DEBUG builds retain the full development harness
/// (`AppStatusViewController`) behind a small Diagnostics entry.
class EcholetHomeViewController: UIViewController, WarmIPCServiceDelegate, EcholetAsrService.Delegate, AudioCaptureDelegate {

    // MARK: - Observed service truth (delegate-fed + bounded refresh)

    private var warmHardwareActive = false
    private var dictationSessionActive = false
    private var manualCaptureActive = false
    private var modelState: String?
    private var advisory: String?

    // MARK: - UI

    private let scrollView = UIScrollView()
    private let contentStack = UIStackView()

    private let titleLabel = UILabel()
    private let headlineLabel = UILabel()
    private let guidanceLabel = UILabel()

    private let warmSectionLabel = UILabel()
    private let warmCard = UIView()
    private let warmDetailLabel = UILabel()
    private let prepareWarmButton = UIButton(type: .system)
    private let disarmWarmButton = UIButton(type: .system)

    private let modelSectionLabel = UILabel()
    private let modelCard = UIView()
    private let modelTitleLabel = UILabel()
    private let modelCapabilityLabel = UILabel()
    private let modelStateLabel = UILabel()
    private let loadModelButton = UIButton(type: .system)
    private let modelSwitchNoticeLabel = UILabel()

    private let keyboardSectionLabel = UILabel()
    private let keyboardCard = UIView()
    private let keyboardStepsLabel = UILabel()
    private let openSettingsButton = UIButton(type: .system)
    private let settingsNoteLabel = UILabel()

    private let footerLabel = UILabel()

    private var refreshTimer: Timer?

    private static let cardCornerRadius: CGFloat = 12

    // MARK: - Lifecycle

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemGroupedBackground
        setupUI()
        registerServiceObservers()
        loadInstalledModelInfo()
        refreshProjection()
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        registerServiceObservers()
        startBoundedRefresh()
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        refreshTimer?.invalidate()
        refreshTimer = nil
    }

    private func registerServiceObservers() {
        WarmIPCService.shared.delegate = self
        EcholetAsrService.shared.delegate = self
        AudioCaptureController.shared.delegate = self
        EcholetAsrService.shared.publishModelStateSnapshot { [weak self] state in
            guard let self = self else { return }
            self.modelState = state.rawValue
            self.refreshProjection()
        }
    }

    // MARK: - Truthful installed model metadata

    private func loadInstalledModelInfo() {
        guard let directory = EcholetAsrService.bundledModelDirectory(),
              let manifestData = FileManager.default.contents(atPath: directory + "/model.json"),
              let manifest = try? JSONSerialization.jsonObject(with: manifestData) as? [String: Any] else {
            return // presenter falls back to the generic truthful title
        }
        if let name = manifest["display_name"] as? String, !name.isEmpty {
            modelTitleLabel.text = name
        }
    }

    // MARK: - UI construction

    private func setupUI() {
        scrollView.translatesAutoresizingMaskIntoConstraints = false
        contentStack.translatesAutoresizingMaskIntoConstraints = false
        contentStack.axis = .vertical
        contentStack.spacing = 16
        contentStack.alignment = .fill

        view.addSubview(scrollView)
        scrollView.addSubview(contentStack)

        NSLayoutConstraint.activate([
            scrollView.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            scrollView.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 16),
            scrollView.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -16),
            scrollView.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor),

            contentStack.topAnchor.constraint(equalTo: scrollView.contentLayoutGuide.topAnchor, constant: 16),
            contentStack.leadingAnchor.constraint(equalTo: scrollView.contentLayoutGuide.leadingAnchor),
            contentStack.trailingAnchor.constraint(equalTo: scrollView.contentLayoutGuide.trailingAnchor),
            contentStack.bottomAnchor.constraint(equalTo: scrollView.contentLayoutGuide.bottomAnchor, constant: -16),
            contentStack.widthAnchor.constraint(equalTo: scrollView.frameLayoutGuide.widthAnchor)
        ])

        // Header (mirrors StatusHeader: big name + narrow status line)
        titleLabel.text = "Echolet"
        titleLabel.font = .boldSystemFont(ofSize: 28)
        contentStack.addArrangedSubview(titleLabel)

        headlineLabel.font = .systemFont(ofSize: 15, weight: .semibold)
        headlineLabel.textColor = .secondaryLabel
        headlineLabel.numberOfLines = 0
        contentStack.addArrangedSubview(headlineLabel)

        guidanceLabel.font = .systemFont(ofSize: 13)
        guidanceLabel.numberOfLines = 0
        guidanceLabel.backgroundColor = UIColor.systemYellow.withAlphaComponent(0.3)
        guidanceLabel.layer.cornerRadius = 8
        guidanceLabel.clipsToBounds = true
        guidanceLabel.isHidden = true
        contentStack.addArrangedSubview(guidanceLabel)

        // WARM DICTATION card
        warmSectionLabel.text = "WARM DICTATION"
        styleSectionLabel(warmSectionLabel)
        contentStack.addArrangedSubview(warmSectionLabel)

        buildWarmCard()
        contentStack.addArrangedSubview(warmCard)

        // MODEL card
        modelSectionLabel.text = "MODEL"
        styleSectionLabel(modelSectionLabel)
        contentStack.addArrangedSubview(modelSectionLabel)

        buildModelCard()
        contentStack.addArrangedSubview(modelCard)

        // KEYBOARD SETUP card
        keyboardSectionLabel.text = "KEYBOARD SETUP"
        styleSectionLabel(keyboardSectionLabel)
        contentStack.addArrangedSubview(keyboardSectionLabel)

        buildKeyboardCard()
        contentStack.addArrangedSubview(keyboardCard)

        footerLabel.text = "Local dictation. Audio never leaves this device."
        footerLabel.font = .monospacedSystemFont(ofSize: 10, weight: .regular)
        footerLabel.textColor = .tertiaryLabel
        footerLabel.textAlignment = .center
        footerLabel.numberOfLines = 0
        contentStack.addArrangedSubview(footerLabel)

        #if DEBUG
        let diagnosticsButton = UIButton(type: .system)
        diagnosticsButton.setTitle("Development Diagnostics (Harness)", for: .normal)
        diagnosticsButton.accessibilityLabel = "Open Development Diagnostics Harness"
        diagnosticsButton.addTarget(self, action: #selector(didTapDiagnostics), for: .touchUpInside)
        contentStack.addArrangedSubview(diagnosticsButton)
        #endif
    }

    private func styleSectionLabel(_ label: UILabel) {
        label.font = .monospacedSystemFont(ofSize: 11, weight: .bold)
        label.textColor = .secondaryLabel
    }

    private func buildWarmCard() {
        warmCard.backgroundColor = .secondarySystemBackground
        warmCard.layer.cornerRadius = Self.cardCornerRadius

        let stack = UIStackView()
        stack.translatesAutoresizingMaskIntoConstraints = false
        stack.axis = .vertical
        stack.spacing = 12
        warmCard.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: warmCard.topAnchor, constant: 14),
            stack.leadingAnchor.constraint(equalTo: warmCard.leadingAnchor, constant: 14),
            stack.trailingAnchor.constraint(equalTo: warmCard.trailingAnchor, constant: -14),
            stack.bottomAnchor.constraint(equalTo: warmCard.bottomAnchor, constant: -14)
        ])

        warmDetailLabel.font = .systemFont(ofSize: 13)
        warmDetailLabel.textColor = .secondaryLabel
        warmDetailLabel.numberOfLines = 0
        stack.addArrangedSubview(warmDetailLabel)

        prepareWarmButton.setTitle("Prepare Warm Dictation", for: .normal)
        prepareWarmButton.titleLabel?.font = .boldSystemFont(ofSize: 15)
        prepareWarmButton.setTitleColor(.white, for: .normal)
        prepareWarmButton.backgroundColor = Self.darkStartColor
        prepareWarmButton.layer.cornerRadius = 10
        prepareWarmButton.heightAnchor.constraint(greaterThanOrEqualToConstant: 44).isActive = true
        prepareWarmButton.accessibilityLabel = "Prepare Warm Dictation"
        prepareWarmButton.addTarget(self, action: #selector(didTapPrepareWarm), for: .touchUpInside)

        disarmWarmButton.setTitle("Disarm Warm Mic", for: .normal)
        disarmWarmButton.titleLabel?.font = .boldSystemFont(ofSize: 15)
        disarmWarmButton.setTitleColor(.white, for: .normal)
        disarmWarmButton.backgroundColor = .systemRed
        disarmWarmButton.layer.cornerRadius = 10
        disarmWarmButton.heightAnchor.constraint(greaterThanOrEqualToConstant: 44).isActive = true
        disarmWarmButton.accessibilityLabel = "Disarm Warm Microphone"
        disarmWarmButton.addTarget(self, action: #selector(didTapDisarmWarm), for: .touchUpInside)

        let buttonRow = UIStackView(arrangedSubviews: [prepareWarmButton, disarmWarmButton])
        buttonRow.axis = .horizontal
        buttonRow.spacing = 10
        buttonRow.distribution = .fillEqually
        stack.addArrangedSubview(buttonRow)
    }

    private func buildModelCard() {
        modelCard.backgroundColor = .secondarySystemBackground
        modelCard.layer.cornerRadius = Self.cardCornerRadius

        let stack = UIStackView()
        stack.translatesAutoresizingMaskIntoConstraints = false
        stack.axis = .vertical
        stack.spacing = 8
        modelCard.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: modelCard.topAnchor, constant: 14),
            stack.leadingAnchor.constraint(equalTo: modelCard.leadingAnchor, constant: 14),
            stack.trailingAnchor.constraint(equalTo: modelCard.trailingAnchor, constant: -14),
            stack.bottomAnchor.constraint(equalTo: modelCard.bottomAnchor, constant: -14)
        ])

        modelTitleLabel.font = .boldSystemFont(ofSize: 15)
        modelTitleLabel.numberOfLines = 0
        modelTitleLabel.text = "X-ASR bilingual (installed offline pack)"
        stack.addArrangedSubview(modelTitleLabel)

        modelCapabilityLabel.font = .systemFont(ofSize: 12)
        modelCapabilityLabel.textColor = .secondaryLabel
        stack.addArrangedSubview(modelCapabilityLabel)

        modelStateLabel.font = .systemFont(ofSize: 12, weight: .semibold)
        modelStateLabel.numberOfLines = 0
        stack.addArrangedSubview(modelStateLabel)

        loadModelButton.setTitle("Load Model Now", for: .normal)
        loadModelButton.titleLabel?.font = .boldSystemFont(ofSize: 14)
        loadModelButton.setTitleColor(.label, for: .normal)
        loadModelButton.backgroundColor = .tertiarySystemFill
        loadModelButton.layer.cornerRadius = 8
        loadModelButton.heightAnchor.constraint(greaterThanOrEqualToConstant: 40).isActive = true
        loadModelButton.accessibilityLabel = "Load Offline Model"
        loadModelButton.addTarget(self, action: #selector(didTapLoadModel), for: .touchUpInside)
        stack.addArrangedSubview(loadModelButton)

        modelSwitchNoticeLabel.font = .systemFont(ofSize: 11)
        modelSwitchNoticeLabel.textColor = .tertiaryLabel
        modelSwitchNoticeLabel.numberOfLines = 0
        modelSwitchNoticeLabel.text = EcholetHomePresenter.modelSwitchNotice
        stack.addArrangedSubview(modelSwitchNoticeLabel)
    }

    private func buildKeyboardCard() {
        keyboardCard.backgroundColor = .secondarySystemBackground
        keyboardCard.layer.cornerRadius = Self.cardCornerRadius

        let stack = UIStackView()
        stack.translatesAutoresizingMaskIntoConstraints = false
        stack.axis = .vertical
        stack.spacing = 8
        keyboardCard.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: keyboardCard.topAnchor, constant: 14),
            stack.leadingAnchor.constraint(equalTo: keyboardCard.leadingAnchor, constant: 14),
            stack.trailingAnchor.constraint(equalTo: keyboardCard.trailingAnchor, constant: -14),
            stack.bottomAnchor.constraint(equalTo: keyboardCard.bottomAnchor, constant: -14)
        ])

        keyboardStepsLabel.numberOfLines = 0
        keyboardStepsLabel.font = .systemFont(ofSize: 13)
        keyboardStepsLabel.text = """
        Enable the Echolet keyboard once:
        1. iOS Settings → General → Keyboard → Keyboards → Add New Keyboard…
        2. Choose "Echolet", then tap it and enable Allow Full Access.
        Then tap any text field and switch keyboards with the Globe key.
        """
        stack.addArrangedSubview(keyboardStepsLabel)

        openSettingsButton.setTitle("Open Echolet iOS Settings Page", for: .normal)
        openSettingsButton.titleLabel?.font = .boldSystemFont(ofSize: 14)
        openSettingsButton.setTitleColor(.label, for: .normal)
        openSettingsButton.backgroundColor = .tertiarySystemFill
        openSettingsButton.layer.cornerRadius = 8
        openSettingsButton.heightAnchor.constraint(greaterThanOrEqualToConstant: 40).isActive = true
        openSettingsButton.accessibilityLabel = "Open Echolet's iOS Settings page"
        openSettingsButton.addTarget(self, action: #selector(didTapOpenSettings), for: .touchUpInside)
        stack.addArrangedSubview(openSettingsButton)

        settingsNoteLabel.numberOfLines = 0
        settingsNoteLabel.font = .systemFont(ofSize: 11)
        settingsNoteLabel.textColor = .tertiaryLabel
        settingsNoteLabel.text = "This opens Echolet's own app settings page; iOS offers no public link to the Keyboard settings screen itself."
        stack.addArrangedSubview(settingsNoteLabel)
    }

    private static let darkStartColor = UIColor { trait in
        trait.userInterfaceStyle == .dark
            ? UIColor(red: 0.30, green: 0.30, blue: 0.33, alpha: 1.0)
            : UIColor(red: 0.09, green: 0.09, blue: 0.11, alpha: 1.0)
    }

    // MARK: - Projection rendering

    private func refreshProjection() {
        let inputs = EcholetHomePresenter.Inputs(
            warmHardwareActive: warmHardwareActive,
            dictationSessionActive: dictationSessionActive,
            micUserArmed: WarmIPCService.shared.isUserArmed,
            modelState: modelState,
            advisory: advisory
        )
        let projection = EcholetHomePresenter.project(inputs)

        if manualCaptureActive {
            headlineLabel.text = "Mic busy — manual capture (Diagnostics)"
        } else {
            headlineLabel.text = "Status: \(projection.headline)"
        }
        warmDetailLabel.text = projection.micDetail
        prepareWarmButton.isEnabled = projection.prepareEnabled && !manualCaptureActive
        disarmWarmButton.isEnabled = projection.disarmEnabled
        prepareWarmButton.backgroundColor = prepareWarmButton.isEnabled
            ? Self.darkStartColor
            : .tertiarySystemFill
        disarmWarmButton.backgroundColor = disarmWarmButton.isEnabled ? .systemRed : .tertiarySystemFill

        let model = EcholetHomePresenter.projectModel(modelState: modelState, modelName: modelTitleLabel.text)
        modelCapabilityLabel.text = model.capabilityLine
        modelStateLabel.text = "Model state: \(model.stateLine)"
        loadModelButton.isHidden = !model.loadButtonEnabled
        loadModelButton.isEnabled = model.loadButtonEnabled
    }

    private func startBoundedRefresh() {
        refreshTimer?.invalidate()
        refreshTimer = Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { [weak self] _ in
            guard let self = self else { return }
            // Bounded truth re-read: hardware-flag drift without a fresh
            // delegate callback (e.g. the flag flipped while this screen
            // was busy) still lands within one second. Reads use the
            // queue-serialized accessors only.
            self.warmHardwareActive = WarmIPCService.shared.warmStandbyHardwareActive
            self.dictationSessionActive = self.dictationSessionActive
                || WarmIPCService.shared.hasActiveKeyboardSession
            if !self.warmHardwareActive && !WarmIPCService.shared.hasActiveKeyboardSession {
                self.dictationSessionActive = false
            }
            self.refreshProjection()
        }
    }

    // MARK: - Actions

    @objc private func didTapPrepareWarm() {
        prepareWarmButton.isEnabled = false
        // Production path only: model load + ordered warm standby prepare on
        // WarmIPCService's single-writer queue. No second engine anywhere.
        WarmIPCService.shared.prepareWarmDictation()
    }

    @objc private func didTapDisarmWarm() {
        disarmWarmButton.isEnabled = false
        WarmIPCService.shared.setUserArmMicrophone(false)
        WarmIPCService.shared.registerManualCaptureStopped()
        // Projection re-syncs via delegate callback + bounded refresh.
    }

    @objc private func didTapLoadModel() {
        loadModelButton.isEnabled = false
        EcholetAsrService.shared.loadModelIfNeeded { [weak self] _, _ in
            DispatchQueue.main.async {
                self?.loadModelButton.isHidden = true
                self?.refreshProjection()
            }
        }
    }

    @objc private func didTapOpenSettings() {
        guard let url = URL(string: UIApplication.openSettingsURLString) else { return }
        UIApplication.shared.open(url)
    }

    #if DEBUG
    @objc private func didTapDiagnostics() {
        let statusVC = AppStatusViewController()
        let nav = UINavigationController(rootViewController: statusVC)
        statusVC.navigationItem.rightBarButtonItem = UIBarButtonItem(
            title: "Done",
            style: .done,
            target: self,
            action: #selector(didTapDismissDiagnostics)
        )
        if UIDevice.current.userInterfaceIdiom == .pad {
            nav.modalPresentationStyle = .formSheet
        } else {
            nav.modalPresentationStyle = .fullScreen
        }
        present(nav, animated: true)
    }

    @objc private func didTapDismissDiagnostics() {
        presentedViewController?.dismiss(animated: true)
    }
    #endif

    // MARK: - EcholetAsrService.Delegate

    func asrService(_ service: EcholetAsrService, didChangeModelState state: EcholetAsrService.ModelState) {
        modelState = state.rawValue
        refreshProjection()
    }

    func asrService(_ service: EcholetAsrService, didUpdatePartialText partial: String, sessionId: String) {
        // Live partial rendering is the keyboard's contract; Home shows only
        // session state, never transcript content.
    }

    func asrService(_ service: EcholetAsrService, sessionDidStart sessionId: String, kind: EcholetAsrService.SessionKind) {
        dictationSessionActive = true
        refreshProjection()
    }

    func asrService(_ service: EcholetAsrService, sessionDidFail sessionId: String, errorCode: String) {
        if WarmIPCService.shared.hasActiveKeyboardSession { return }
        dictationSessionActive = false
        advisory = "ASR session failed: \(errorCode)"
        refreshProjection()
    }

    func asrService(_ service: EcholetAsrService, sessionDidDispose sessionId: String, reason: String) {
        if WarmIPCService.shared.hasActiveKeyboardSession { return }
        dictationSessionActive = false
        refreshProjection()
    }

    // MARK: - WarmIPCServiceDelegate

    func warmIPCService(_ service: WarmIPCService, didAdmitStartSession sessionId: String, intentSequence: UInt64) {
        dictationSessionActive = true
        refreshProjection()
    }

    func warmIPCService(_ service: WarmIPCService, didAdmitStopSession sessionId: String, intentSequence: UInt64) {
        refreshProjection()
    }

    func warmIPCService(_ service: WarmIPCService, didRejectRequest description: String) {
        advisory = description
        refreshProjection()
    }

    func warmIPCService(_ service: WarmIPCService, didEncounterBlockedState reason: String) {
        advisory = reason
        refreshProjection()
    }

    func warmIPCService(_ service: WarmIPCService, didChangeWarmStandbyActive active: Bool) {
        warmHardwareActive = active
        if !active {
            dictationSessionActive = WarmIPCService.shared.hasActiveKeyboardSession
        }
        refreshProjection()
    }

    func warmIPCService(_ service: WarmIPCService, didUpdateReturnGuidance text: String?) {
        guidanceLabel.text = text
        guidanceLabel.isHidden = (text == nil)
    }

    // MARK: - AudioCaptureDelegate

    func audioCaptureController(_ controller: AudioCaptureController, didUpdateStatus status: AudioCaptureController.Status) {
        manualCaptureActive = (status == .recording) && !warmHardwareActive
            && !WarmIPCService.shared.hasActiveKeyboardSession
        refreshProjection()
    }

    func audioCaptureController(_ controller: AudioCaptureController, didUpdateMetrics metrics: AudioCaptureController.Metrics) {
        // Level metering is harness detail; the product Home stays calm.
    }

    func audioCaptureController(_ controller: AudioCaptureController, didFailWithError error: AudioCaptureController.CaptureError) {
        advisory = error.localizedDescription
        refreshProjection()
    }
}
