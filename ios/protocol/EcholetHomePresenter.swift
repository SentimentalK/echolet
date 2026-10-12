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
        // Never an auto-download: the single Download CTA is user-owned.
        if !inputs.modelInstalled {
            let advisoryLine = "Download the verified offline model below to enable dictation."
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

    /// Action available for a catalog row right now (honest gating).
    public enum ModelRowAction: Equatable {
        case download           // verified model, not installed → Download CTA
        case retryDownload      // failed transfer → Download/Retry
        case select             // installed, not active/loaded → Select
        case selected           // active row, read-only confirmation
        case none               // experimental rows: read-only only
    }

    public struct ModelRowProjection: Equatable {
        public let title: String
        public let stateLine: String
        public let detail: String
        public let action: ModelRowAction
        public static func == (l: ModelRowProjection, r: ModelRowProjection) -> Bool {
            l.title == r.title && l.stateLine == r.stateLine
                && l.detail == r.detail && l.action == r.action
        }
    }

    public static let experimentalNote =
        "Experimental: shown for visibility; not yet verified on iOS."

    /// Projects ONE catalog row truthfully. Selection is disabled entirely
    /// for rows whose runtime compatibility was not proven on iOS.
    public static func projectModelRow(
        _ entry: ModelEntry,
        transfer: ModelTransferState
    ) -> ModelRowProjection {
        let sizeLine: String
        if entry.installed {
            if let s = entry.installedSizeBytes {
                sizeLine = "Installed · \(byteLabel(s))"
            } else {
                sizeLine = "Installed"
            }
        } else if let s = entry.downloadSizeBytes {
            sizeLine = "Not installed · \(byteLabel(s)) download"
        } else {
            sizeLine = "Not installed"
        }

        let compatibility = entry.runtimeCompatible
            ? "Echolet Verified · On-device offline dictation"
            : Self.experimentalNote

        switch transfer {
        case .downloading(let done, let total):
            var line = "Downloading… \(byteLabel(done))"
            if let total = total, total > 0 {
                line += " / \(byteLabel(total)) (\(Int(done * 100 / total))%)"
            }
            return ModelRowProjection(
                title: entry.displayName,
                stateLine: line,
                detail: "Wi-Fi required · Verified SHA256 on completion",
                action: .none
            )
        case .verifying:
            return ModelRowProjection(
                title: entry.displayName,
                stateLine: "Verifying checksum…",
                detail: compatibility,
                action: .none
            )
        case .installing:
            return ModelRowProjection(
                title: entry.displayName,
                stateLine: "Installing…",
                detail: compatibility,
                action: .none
            )
        case .failed(let message):
            return ModelRowProjection(
                title: entry.displayName,
                stateLine: "Failed — \(message)",
                detail: sizeLine,
                action: .retryDownload
            )
        case .installed:
            // Installed truth comes from the RUST read-back; a transfer-only
            // state must never proclaim installed.
            guard entry.installed else {
                return idleProjection(entry, sizeLine: sizeLine, compatibility: compatibility)
            }
            if entry.runtimeCompatible {
                return ModelRowProjection(
                    title: entry.displayName,
                    stateLine: "Installed — ready to select",
                    detail: compatibility,
                    action: .select
                )
            }
            return ModelRowProjection(
                title: entry.displayName,
                stateLine: "Installed (experimental)",
                detail: compatibility,
                action: .none
            )
        case .idle:
            return idleProjection(entry, sizeLine: sizeLine, compatibility: compatibility)
        }
    }

    private static func idleProjection(
        _ entry: ModelEntry,
        sizeLine: String,
        compatibility: String
    ) -> ModelRowProjection {
        if !entry.runtimeCompatible {
            return ModelRowProjection(
                title: entry.displayName,
                stateLine: entry.installed ? "Installed (experimental)" : sizeLine,
                detail: experimentalNote,
                action: .none
            )
        }
        if entry.active {
            return ModelRowProjection(
                title: entry.displayName,
                stateLine: "Selected",
                detail: compatibility,
                action: .selected
            )
        }
        if entry.installed {
            return ModelRowProjection(
                title: entry.displayName,
                stateLine: "Installed",
                detail: compatibility,
                action: .select
            )
        }
        return ModelRowProjection(
            title: entry.displayName,
            stateLine: "Not installed",
            detail: compatibility,
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
