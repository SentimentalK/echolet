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
        /// Most recent honest advisory (blocked/rejected IPC guidance).
        public var advisory: String?

        public init(
            warmHardwareActive: Bool = false,
            dictationSessionActive: Bool = false,
            micUserArmed: Bool = false,
            modelState: String? = nil,
            advisory: String? = nil
        ) {
            self.warmHardwareActive = warmHardwareActive
            self.dictationSessionActive = dictationSessionActive
            self.micUserArmed = micUserArmed
            self.modelState = modelState
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

        var advisoryLine = "Microphone off. Prepare warm dictation to start speaking quickly."
        if let advisory = inputs.advisory, !advisory.isEmpty {
            advisoryLine += " Last notice: \(advisory)"
        }
        return Projection(
            headline: "Ready",
            micDetail: advisoryLine,
            prepareEnabled: true,
            disarmEnabled: false,
            micHardwareOn: false
        )
    }

    // MARK: - Model projection (installed pack only; no switching backend)

    public struct ModelProjection: Equatable {
        public let title: String
        public let capabilityLine: String
        public let stateLine: String
        public let loadButtonEnabled: Bool
    }

    /// Deliberate scope boundary, surfaced verbatim by the Home UI.
    public static let modelSwitchNotice =
        "Model switching and downloads are not implemented in this iOS build; the installed offline pack below is the current model."

    /// Projects real bundled model metadata + live service state. There is
    /// exactly one truthfully installed X-ASR bilingual offline pack; no
    /// select/download/download-retry entries are produced here.
    public static func projectModel(modelState modelStateRaw: String?, modelName: String?) -> ModelProjection {
        let title = modelName ?? "X-ASR bilingual (installed offline pack)"
        let state: (line: String, loadEnabled: Bool)
        switch modelStateRaw {
        case "loaded":
            state = ("Loaded · ready for offline dictation", false)
        case "loading":
            state = ("Loading…", false)
        case "failed":
            state = ("Load failed — retry from the Load button or Prepare.", true)
        case "notLoaded", "", nil:
            state = ("Not loaded yet — loads on first Prepare.", true)
        default:
            state = (modelStateRaw ?? "unknown state", false)
        }
        return ModelProjection(
            title: title,
            capabilityLine: "Chinese + English · Offline · On-device only",
            stateLine: state.line,
            loadButtonEnabled: state.loadEnabled
        )
    }
}
