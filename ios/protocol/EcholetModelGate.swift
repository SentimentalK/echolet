import Foundation

/// Pure Foundation gate for iOS model selection (containing app only).
///
/// UIKit never decides policy: the model store feeds OBSERVED truth (ASR
/// service busy flags, WarmIPC queue facts, manual capture status) into
/// `EcholetModelGate.evaluate` and executes the returned decision verbatim.
///
/// Truth rules:
/// * Select is DENIED (not racy) while a keyboard session is active, a
///   keyboard START activation is pending, a manual capture is recording,
///   warm mic hardware is live, the user-armed flag has not been disarmed,
///   or the ASR model/recognizer itself is busy (load or swap in progress).
/// * Denial guidance names the EXACT user actions ("Stop Listening" and/or
///   "Disarm Warm Mic"); select never implicitly terminates dictation.
public enum EcholetModelGate {

    /// Immutable snapshot of all blocking truths at gate-evaluation time.
    public struct Inputs: Equatable {
        public var keyboardSessionActive: Bool
        public var keyboardActivationPending: Bool
        public var warmHardwareActive: Bool
        public var micUserArmed: Bool
        public var manualCaptureRecording: Bool
        public var asrBusy: Bool
        public var selectionAlreadyActive: Bool

        public init(
            keyboardSessionActive: Bool = false,
            keyboardActivationPending: Bool = false,
            warmHardwareActive: Bool = false,
            micUserArmed: Bool = false,
            manualCaptureRecording: Bool = false,
            asrBusy: Bool = false,
            selectionAlreadyActive: Bool = false
        ) {
            self.keyboardSessionActive = keyboardSessionActive
            self.keyboardActivationPending = keyboardActivationPending
            self.warmHardwareActive = warmHardwareActive
            self.micUserArmed = micUserArmed
            self.manualCaptureRecording = manualCaptureRecording
            self.asrBusy = asrBusy
            self.selectionAlreadyActive = selectionAlreadyActive
        }
    }

    public enum Decision: Equatable {
        /// Safe to swap; an exclusive switch epoch is open.
        case allow
        /// Denied with honest user-action guidance (never auto-stops audio).
        case deny(String)
    }

    public static func evaluate(_ inputs: Inputs) -> Decision {
        if inputs.selectionAlreadyActive {
            return .deny("A model selection is already in progress; wait for it to finish.")
        }
        if inputs.keyboardSessionActive {
            return .deny("Stop Listening (keyboard Stop) before switching models.")
        }
        if inputs.keyboardActivationPending {
            return .deny("A keyboard start is still preparing; wait for it to finish or Disarm Warm Mic first.")
        }
        if inputs.warmHardwareActive {
            return .deny("Warm mic hardware is active; Disarm Warm Mic before switching models.")
        }
        if inputs.micUserArmed {
            return .deny("The microphone is armed; Disarm Warm Mic before switching models.")
        }
        if inputs.manualCaptureRecording {
            return .deny("Manual capture is recording; stop it before switching models.")
        }
        if inputs.asrBusy {
            return .deny("Model load or switch is still in progress; wait for it to finish.")
        }
        return .allow
    }

    /// Honest guidance helper for the ASR side: what the user must do first.
    public static let stopFirstGuidance =
        "Stop listening and Disarm Warm Mic before switching models."
}
