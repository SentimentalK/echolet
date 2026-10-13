import Foundation

/// Pure Foundation projection of the Echolet containing-app Home panel
/// (Increment B). UIKit never reproduces product policy: the Home view
/// controller feeds observed service truth into `EcholetHomePresenter`
/// and renders the returned projection verbatim.
///
/// Truth rules encoded here:
/// * Warm mic HARDWARE state and an active ASR dictation session are
///   different booleans. Physical warm capture can stay live after a
///   keyboard Stop/hide (the orange privacy indicator stays on); the
///   projection must never claim the microphone is off while warm
///   hardware is genuinely active.
/// * App opening alone never starts a microphone. There is no projection
///   state that invites an automatic session: `prepare` becomes enabled
///   only from cold/mic-idle truth, and every state change requires a
///   real user control or a production service callback.
public enum EcholetHomePresenter {

    // MARK: - Input snapshot

    public struct Inputs: Equatable {
        /// Physical warm recorder genuinely running (WarmIPCService warm
        /// standby hardware-active truth).
        public var warmHardwareActive: Bool
        /// A real ASR dictation session is live (keyboard adopted or
        /// in-app live dictation).
        public var dictationSessionActive: Bool
        /// User arming requested but hardware confirmation not yet observed.
        public var micUserArmed: Bool
        /// `EcholetAsrService.ModelState.rawValue` (nil-safe decorative fallback).
        public var modelState: String?
        /// Whether at least one model entry is installed (from the Rust
        /// canonical snapshot). No-model keeps Prepare honestly disabled.
        public var modelInstalled: Bool
        /// Most recent honest advisory (blocked/rejected IPC guidance).
        public var advisory: String?

        public init(
            warmHardwareActive: Bool = false,
            dictationSessionActive: Bool = false,
            micUserArmed: Bool = false,
            modelState: String? = nil,
            modelInstalled: Bool = true,
            advisory: String? = nil
        ) {
            self.warmHardwareActive = warmHardwareActive
            self.dictationSessionActive = dictationSessionActive
            self.micUserArmed = micUserArmed
            self.modelState = modelState
            self.modelInstalled = modelInstalled
            self.advisory = advisory
        }
    }

    // MARK: - Projection

    public struct Projection: Equatable {
        public let headline: String
        public let micDetail: String
        public let prepareEnabled: Bool
        public let disarmEnabled: Bool
        public /// Explicit hardware on-mic truth for unconditional privacy display.
        let micHardwareOn: Bool

        public static func == (lhs: Projection, rhs: Projection) -> Bool {
            return lhs.headline == rhs.headline
                && lhs.micDetail == rhs.micDetail
                && lhs.prepareEnabled == rhs.prepareEnabled
                && lhs.disarmEnabled == rhs.disarmEnabled
                && lhs.micHardwareOn == rhs.micHardwareOn
        }
    }

    public static func project(_ inputs: Inputs) -> Projection {
        // Dictation session wins display: real speech actively processed.
        if inputs.dictationSessionActive {
            return Projection(
                headline: "Listening",
                micDetail: "Dictation session active; recognized speech becomes text on your device.",
                prepareEnabled: false,
                disarmEnabled: true,
                micHardwareOn: true
            )
        }

        // Warm hardware genuinely live: the ONLY state where "Mic off"
        // claims are forbidden. The orange indicator legitimately stays on.
        if inputs.warmHardwareActive {
            return Projection(
                headline: "Warm mic active",
                micDetail: "Microphone hardware is on standby for fast Start. The orange indicator stays on until you Disarm.",
                prepareEnabled: false,
                disarmEnabled: true,
                micHardwareOn: true
            )
        }

        // Arming without confirmed hardware: honest pending state, not "ready".
        if inputs.micUserArmed {
            return Projection(
                headline: "Preparing…",
                micDetail: "Echolet is arming the microphone; hardware is not confirmed yet.",
                prepareEnabled: false,
                disarmEnabled: true,
                micHardwareOn: false
            )
        }

        // No model at all: honest No-Model home state — Prepare stays
        // disabled until the user actively downloads and selects a model.
        // Never an auto-download: each model's Download control is user-owned.
        if !inputs.modelInstalled {
            let advisoryLine = "Download an offline model below to enable dictation."
                + advisorySuffix(inputs.advisory)
            return Projection(
                headline: "No Model",
                micDetail: advisoryLine,
                prepareEnabled: false,
                disarmEnabled: false,
                micHardwareOn: false
            )
        }

        var advisoryLine = "Microphone off. Prepare warm dictation to start speaking quickly."
            + advisorySuffix(inputs.advisory)
        return Projection(
            headline: "Ready",
            micDetail: advisoryLine,
            prepareEnabled: true,
            disarmEnabled: false,
            micHardwareOn: false
        )
    }

    private static func advisorySuffix(_ advisory: String?) -> String {
        guard let advisory = advisory, !advisory.isEmpty else { return "" }
        return " Last notice: \(advisory)"
    }

    // MARK: - Model catalog projection (shared Rust canonical registry)

    /// Immutable per-row truth mirrored verbatim from the Rust canonical
    /// model snapshot (no Swift-side invented catalog).
    public struct ModelEntry: Equatable {
        public let id: String
        public let displayName: String
        public let version: String
        public let languages: [String]
        public let verificationStatus: String
        public let runtimeCompatible: Bool
        public let installed: Bool
        public let active: Bool
        public let downloadSizeBytes: UInt64?
        public let installedSizeBytes: UInt64?

        public init(
            id: String,
            displayName: String,
            version: String,
            languages: [String],
            verificationStatus: String,
            runtimeCompatible: Bool,
            installed: Bool,
            active: Bool,
            downloadSizeBytes: UInt64?,
            installedSizeBytes: UInt64?
        ) {
            self.id = id
            self.displayName = displayName
            self.version = version
            self.languages = languages
            self.verificationStatus = verificationStatus
            self.runtimeCompatible = runtimeCompatible
            self.installed = installed
            self.active = active
            self.downloadSizeBytes = downloadSizeBytes
            self.installedSizeBytes = installedSizeBytes
        }
    }

    /// Per-model transfer state (owned by the App model store, observed).
    public enum ModelTransferState: Equatable {
        case idle
        case downloading(downloadedBytes: UInt64, totalBytes: UInt64?)
        case verifying
        case installing
        case installed
        case failed(message: String)
    }

    /// Action available for a catalog row right now.
    ///
    /// Every registry model can be downloaded and selected, matching desktop.
    /// Verification is a badge, not a lock. In-progress rows keep a visible
    /// control (percent / Verifying / Installing / Loading) so the button
    /// does not disappear while work is happening.
    public enum ModelRowAction: Equatable {
        case download
        case retryDownload
        case select
        case selected
        case busy(label: String)
    }

    public struct ModelRowProjection: Equatable {
        public let title: String
        public let badge: String
        public let stateLine: String
        public let detail: String
        public let action: ModelRowAction

        public var actionTitle: String {
            switch action {
            case .download: return "Download"
            case .retryDownload: return "Retry"
            case .select: return "Select"
            case .selected: return "Selected"
            case .busy(let label): return label
            }
        }

        public var actionEnabled: Bool {
            switch action {
            case .download, .retryDownload, .select: return true
            case .selected, .busy: return false
            }
        }

        public static func == (l: ModelRowProjection, r: ModelRowProjection) -> Bool {
            l.title == r.title && l.badge == r.badge && l.stateLine == r.stateLine
                && l.detail == r.detail && l.action == r.action
        }
    }

    /// Short badge next to the model name. Desktop shows "Verified"; iOS
    /// also names Experimental so the row is identifiable without hiding it.
    public static func badgeLabel(for verificationStatus: String) -> String {
        let lower = verificationStatus.lowercased()
        if lower.contains("verified") { return "Verified" }
        if lower.contains("experimental") { return "Experimental" }
        return verificationStatus
    }

    /// Projects ONE catalog row. `selectInProgress` covers the gap before
    /// the persisted active id flips; `recognizerLoading` covers the active
    /// row while the on-device recognizer is actually loading.
    public static func projectModelRow(
        _ entry: ModelEntry,
        transfer: ModelTransferState,
        selectInProgress: Bool = false,
        recognizerLoading: Bool = false
    ) -> ModelRowProjection {
        let badge = badgeLabel(for: entry.verificationStatus)
        let detail = detailLine(entry, badge: badge)

        if selectInProgress || (entry.active && recognizerLoading) {
            return ModelRowProjection(
                title: entry.displayName,
                badge: badge,
                stateLine: "Loading model…",
                detail: detail,
                action: .busy(label: "Loading…")
            )
        }

        switch transfer {
        case .downloading(let done, let total):
            var line = "Downloading… \(byteLabel(done))"
            var button = "Downloading"
            if let total = total, total > 0 {
                let pct = Int(done * 100 / total)
                line += " / \(byteLabel(total)) (\(pct)%)"
                button = "\(pct)%"
            }
            return ModelRowProjection(
                title: entry.displayName,
                badge: badge,
                stateLine: line,
                detail: "Wi-Fi required · checksum verified before install",
                action: .busy(label: button)
            )
        case .verifying:
            return ModelRowProjection(
                title: entry.displayName,
                badge: badge,
                stateLine: "Verifying checksum…",
                detail: detail,
                action: .busy(label: "Verifying")
            )
        case .installing:
            return ModelRowProjection(
                title: entry.displayName,
                badge: badge,
                stateLine: "Installing…",
                detail: detail,
                action: .busy(label: "Installing")
            )
        case .failed(let message):
            return ModelRowProjection(
                title: entry.displayName,
                badge: badge,
                stateLine: "Failed — \(message)",
                detail: detail,
                action: .retryDownload
            )
        case .installed, .idle:
            // Installed truth comes from the Rust read-back. A transfer-only
            // `.installed` flag must not override Selected, and must not
            // proclaim installed when Rust says the model is absent.
            return idleProjection(entry, badge: badge, detail: detail)
        }
    }

    private static func detailLine(_ entry: ModelEntry, badge: String) -> String {
        let size: String
        if entry.installed, let bytes = entry.installedSizeBytes {
            size = "\(byteLabel(bytes)) installed"
        } else if let bytes = entry.downloadSizeBytes {
            size = "\(byteLabel(bytes)) download"
        } else if entry.installed {
            size = "Installed on this device"
        } else {
            size = "On-device offline dictation"
        }
        if badge.isEmpty { return size }
        return "\(badge) · \(size)"
    }

    private static func idleProjection(
        _ entry: ModelEntry,
        badge: String,
        detail: String
    ) -> ModelRowProjection {
        if entry.active {
            return ModelRowProjection(
                title: entry.displayName,
                badge: badge,
                stateLine: "Selected",
                detail: detail,
                action: .selected
            )
        }
        if entry.installed {
            return ModelRowProjection(
                title: entry.displayName,
                badge: badge,
                stateLine: "Installed",
                detail: detail,
                action: .select
            )
        }
        return ModelRowProjection(
            title: entry.displayName,
            badge: badge,
            stateLine: "Not installed",
            detail: detail,
            action: .download
        )
    }

    public static func byteLabel(_ bytes: UInt64) -> String {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .file
        return formatter.string(fromByteCount: Int64(clamping: bytes))
    }

    /// Whether any model entry is installed (NoModel home state input).
    public static func anyInstalled(_ entries: [ModelEntry]) -> Bool {
        return entries.contains { $0.installed }
    }
}
