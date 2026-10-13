import UIKit
import AVFoundation
import Foundation

/// Echolet product Home panel (containing app, Increment B).
///
/// A clean scrollable control panel visually aligned with the Desktop
/// Slint panel: narrow honest header, dark Start / red Disarm warm-mic
/// card, the full offline model catalog (download, select, and load,
/// same actions as desktop), keyboard setup guidance, and an on-device
/// privacy footer. No harness controls, epoch values, IPC internals,
/// mock buttons or navigation experiments in the
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
    private let catalogStatusLabel = UILabel()
    private let modelRowsStack = UIStackView()
    /// Row identity for the grouped catalog (id -> row view).
    private var modelRowViews: [String: ModelRowView] = [:]
    /// Model id whose Select tap is still loading the recognizer.
    private var pendingSelectId: String?

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
        EcholetModelStore.shared.bootstrap()
        EcholetModelStore.shared.snapshotObserver = { [weak self] _ in
            self?.refreshProjection()
        }
        EcholetModelStore.shared.changeObserver = { [weak self] in
            self?.refreshProjection()
        }
    }

    // MARK: - UI construction

    private func setupUI() {
        scrollView.translatesAutoresizingMaskIntoConstraints = false
        // A short tap on Download / Select must not be swallowed as a scroll.
        scrollView.delaysContentTouches = false
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

        // MODELS card — every registry entry, same actions as desktop.
        modelSectionLabel.text = "MODELS"
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

        catalogStatusLabel.font = .systemFont(ofSize: 13)
        catalogStatusLabel.textColor = .secondaryLabel
        catalogStatusLabel.numberOfLines = 0
        catalogStatusLabel.text = "Loading models…"
        stack.addArrangedSubview(catalogStatusLabel)

        modelRowsStack.axis = .vertical
        modelRowsStack.spacing = 12
        stack.addArrangedSubview(modelRowsStack)
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
            // Snapshot not yet read back: honest No-Model state until Rust
            // truth lands (nothing is assumed installed).
            modelInstalled: EcholetModelStore.shared.snapshot.map { $0.anyInstalled } ?? false,
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

        renderModelCatalogRows()
    }

    /// Rebuilds the catalog rows from the latest Rust snapshot plus observed
    /// transfer state. The loading notice is a label above the rows and is
    /// hidden once the snapshot arrives — it must not stay in the list.
    private func renderModelCatalogRows() {
        let recognizerLoading = modelState == EcholetAsrService.ModelState.loading.rawValue
        guard let snapshot = EcholetModelStore.shared.snapshot else {
            catalogStatusLabel.isHidden = false
            catalogStatusLabel.text = EcholetModelStore.shared.catalogUnavailableReason
                ?? "Loading models…"
            clearModelRows()
            return
        }
        catalogStatusLabel.isHidden = true

        let live = Set(snapshot.entries.map { $0.id })
        for id in modelRowViews.keys.filter({ !live.contains($0) }) {
            if let row = modelRowViews.removeValue(forKey: id) {
                modelRowsStack.removeArrangedSubview(row)
                row.removeFromSuperview()
            }
        }
        for view in modelRowsStack.arrangedSubviews {
            guard let row = view as? ModelRowView, modelRowViews[row.modelId] === row else {
                modelRowsStack.removeArrangedSubview(view)
                view.removeFromSuperview()
                continue
            }
        }

        for (index, entry) in snapshot.entries.enumerated() {
            let transfer = EcholetModelStore.shared.transferStates[entry.id] ?? .idle
            let projection = EcholetHomePresenter.projectModelRow(
                presenterEntry(from: entry),
                transfer: transfer,
                selectInProgress: pendingSelectId == entry.id,
                recognizerLoading: recognizerLoading
            )
            let row = modelRowViews[entry.id] ?? ModelRowView(frame: .zero)
            row.apply(projection: projection, modelId: entry.id, controller: self)
            if row.superview == nil {
                modelRowViews[entry.id] = row
                let insertAt = min(index, modelRowsStack.arrangedSubviews.count)
                modelRowsStack.insertArrangedSubview(row, at: insertAt)
            }
        }
    }

    private func clearModelRows() {
        modelRowViews.removeAll()
        for view in modelRowsStack.arrangedSubviews {
            modelRowsStack.removeArrangedSubview(view)
            view.removeFromSuperview()
        }
    }

    /// Maps the store's Rust-mirrored entry to the presenter's pure type.
    private func presenterEntry(from entry: EcholetModelStore.Entry) -> EcholetHomePresenter.ModelEntry {
        return EcholetHomePresenter.ModelEntry(
            id: entry.id,
            displayName: entry.displayName,
            version: entry.version,
            languages: entry.languages,
            verificationStatus: entry.verificationStatus,
            runtimeCompatible: entry.runtimeCompatible,
            installed: entry.installed,
            active: entry.active,
            downloadSizeBytes: entry.downloadSizeBytes,
            installedSizeBytes: entry.installedSizeBytes
        )
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

    @objc func didTapModelAction(_ sender: ModelActionButton) {
        switch sender.rowAction {
        case .download, .retryDownload:
            // Honest pipeline: Download → Verify → Install. The row switches
            // to a visible progress control immediately inside download().
            EcholetModelDownloadCoordinator.shared.download(modelId: sender.modelId)
        case .select:
            pendingSelectId = sender.modelId
            refreshProjection()
            EcholetModelStore.shared.select(modelId: sender.modelId) { [weak self] outcome in
                guard let self = self else { return }
                self.pendingSelectId = nil
                switch outcome {
                case .switched:
                    self.advisory = nil
                case .denied(let guidance):
                    self.advisory = "Model switch blocked: \(guidance)"
                case .failed(let message):
                    self.advisory = "Model switch failed: \(message)"
                }
                self.refreshProjection()
            }
        case .selected, .busy:
            break
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

// MARK: - Catalog row rendering

/// Trailing action control for one catalog row. The model id and the
/// projected action travel with the button so the tap handler does not
/// guess from the title.
final class ModelActionButton: UIButton {
    var modelId: String = ""
    var rowAction: EcholetHomePresenter.ModelRowAction = .download
}

/// One catalog row: name + verification badge + status on the left, a
/// fixed 44pt action control on the right (Download / Select / Selected /
/// progress). The control stays in place while work is in progress so the
/// row does not collapse into an untappable text block.
final class ModelRowView: UIView, UIGestureRecognizerDelegate {
    private let titleLabel = UILabel()
    private let badgeLabel = UILabel()
    private let stateLabel = UILabel()
    private let detailLabel = UILabel()
    private let actionButton = ModelActionButton(type: .system)
    private var wired = false

    var modelId: String { actionButton.modelId }

    override init(frame: CGRect) {
        super.init(frame: frame)

        titleLabel.font = .boldSystemFont(ofSize: 15)
        titleLabel.numberOfLines = 2
        titleLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        badgeLabel.font = .systemFont(ofSize: 11, weight: .semibold)
        badgeLabel.setContentHuggingPriority(.required, for: .horizontal)
        badgeLabel.setContentCompressionResistancePriority(.required, for: .horizontal)

        stateLabel.font = .systemFont(ofSize: 13, weight: .semibold)
        stateLabel.numberOfLines = 2

        detailLabel.font = .systemFont(ofSize: 12)
        detailLabel.textColor = .secondaryLabel
        detailLabel.numberOfLines = 2

        actionButton.translatesAutoresizingMaskIntoConstraints = false
        actionButton.titleLabel?.font = .boldSystemFont(ofSize: 14)
        actionButton.titleLabel?.adjustsFontSizeToFitWidth = true
        actionButton.titleLabel?.minimumScaleFactor = 0.7
        actionButton.layer.cornerRadius = 10
        actionButton.clipsToBounds = true
        actionButton.setContentHuggingPriority(.required, for: .horizontal)
        actionButton.setContentCompressionResistancePriority(.required, for: .horizontal)
        NSLayoutConstraint.activate([
            actionButton.widthAnchor.constraint(equalToConstant: 112),
            actionButton.heightAnchor.constraint(equalToConstant: 44)
        ])

        let titleRow = UIStackView(arrangedSubviews: [titleLabel, badgeLabel])
        titleRow.axis = .horizontal
        titleRow.spacing = 6
        titleRow.alignment = .center

        let textStack = UIStackView(arrangedSubviews: [titleRow, stateLabel, detailLabel])
        textStack.axis = .vertical
        textStack.spacing = 2
        textStack.alignment = .fill

        let row = UIStackView(arrangedSubviews: [textStack, actionButton])
        row.translatesAutoresizingMaskIntoConstraints = false
        row.axis = .horizontal
        row.spacing = 12
        row.alignment = .center
        addSubview(row)
        NSLayoutConstraint.activate([
            row.topAnchor.constraint(equalTo: topAnchor, constant: 4),
            row.leadingAnchor.constraint(equalTo: leadingAnchor),
            row.trailingAnchor.constraint(equalTo: trailingAnchor),
            row.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -4)
        ])

        let tap = UITapGestureRecognizer(target: self, action: #selector(didTapRow))
        tap.delegate = self
        addGestureRecognizer(tap)
    }

    required init?(coder: NSCoder) { fatalError("unsupported") }

    func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
        guard actionButton.isUserInteractionEnabled else { return false }
        let point = touch.location(in: actionButton)
        return !actionButton.point(inside: point, with: nil)
    }

    @objc private func didTapRow() {
        guard actionButton.isUserInteractionEnabled else { return }
        actionButton.sendActions(for: .touchUpInside)
    }

    /// Updates one truthful projection in place. The button is never removed,
    /// so a download or select tap keeps a stable 44pt hit target.
    func apply(
        projection: EcholetHomePresenter.ModelRowProjection,
        modelId: String,
        controller: EcholetHomeViewController
    ) {
        if !wired {
            actionButton.addTarget(
                controller,
                action: #selector(EcholetHomeViewController.didTapModelAction(_:)),
                for: .touchUpInside
            )
            wired = true
        }
        actionButton.modelId = modelId
        actionButton.rowAction = projection.action
        titleLabel.text = projection.title
        badgeLabel.text = projection.badge
        badgeLabel.isHidden = projection.badge.isEmpty
        badgeLabel.textColor = projection.badge == "Verified" ? .systemGreen : .secondaryLabel
        stateLabel.text = projection.stateLine
        detailLabel.text = projection.detail

        actionButton.setTitle(projection.actionTitle, for: .normal)
        actionButton.accessibilityLabel = "\(projection.actionTitle) \(projection.title)"
        let enabled = projection.actionEnabled
        actionButton.isUserInteractionEnabled = enabled
        // Keep isEnabled so UIKit does not fade the title; interaction is
        // gated above. Selected stays a solid badge, progress stays readable.
        actionButton.isEnabled = true
        switch projection.action {
        case .selected:
            actionButton.backgroundColor = Self.filledColor
            actionButton.setTitleColor(Self.filledTitle, for: .normal)
        case .busy:
            actionButton.backgroundColor = .tertiarySystemFill
            actionButton.setTitleColor(.secondaryLabel, for: .normal)
        case .download, .retryDownload, .select:
            actionButton.backgroundColor = Self.filledColor
            actionButton.setTitleColor(Self.filledTitle, for: .normal)
        }
    }

    private static let filledColor = UIColor { trait in
        trait.userInterfaceStyle == .dark
            ? UIColor(white: 0.92, alpha: 1)
            : UIColor(white: 0.10, alpha: 1)
    }

    private static let filledTitle = UIColor { trait in
        trait.userInterfaceStyle == .dark ? .black : .white
    }
}
