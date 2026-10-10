import Foundation

/// Pure Foundation lifecycle gate and state machine for audio capture session management.
///
/// Designed to be driven exclusively by a serialized state executor (or lock-free synchronized context).
/// Fences against stale start acks, out-of-order hardware responses, duplicate requests,
/// counter overflow, and post-stop late tap callbacks.
public final class CaptureLifecycleGate {

    public enum State: String, Equatable {
        case idle
        case requestingPermission
        case ready
        case starting
        case recording
        case interrupted
        case stopped
        case blocked
        case failed
    }

    public enum LifecycleError: LocalizedError, Equatable {
        case generationOverflow
        case alreadyRecording
        case staleGeneration(issued: UInt64, active: UInt64)
        case cancelledBeforeStart
        case permissionDenied
        case hardwareUnavailable
        case engineConfigurationFailed(String)
        case audioSessionActivationFailed(String)
        case sessionInterrupted
        case invalidStateTransition(from: State, action: String)

        public var errorDescription: String? {
            switch self {
            case .generationOverflow:
                return "Capture generation counter reached maximum value; failing closed."
            case .alreadyRecording:
                return "Microphone capture is already actively recording."
            case .staleGeneration(let issued, let active):
                return "Stale generation token: issued \(issued) != active \(active)."
            case .cancelledBeforeStart:
                return "Audio capture request was cancelled before start completion."
            case .permissionDenied:
                return "Microphone permission was denied by the user."
            case .hardwareUnavailable:
                return "No audio input hardware is available on this device."
            case .engineConfigurationFailed(let details):
                return "AudioEngine configuration failed: \(details)"
            case .audioSessionActivationFailed(let details):
                return "AVAudioSession activation failed: \(details)"
            case .sessionInterrupted:
                return "Audio session was interrupted by the system."
            case .invalidStateTransition(let from, let action):
                return "Invalid state transition from \(from) for action \(action)."
            }
        }
    }

    /// Immutable metrics snapshot dispatched from rate-limited audio tap
    public struct MetricSnapshot: Equatable {
        public let generation: UInt64
        public let frameIncrement: UInt64
        public let peakPower: Float
        public let rmsPower: Float
        public let elapsedSeconds: Double
        public let sampleRate: Double
        public let channelCount: UInt32

        public init(
            generation: UInt64,
            frameIncrement: UInt64,
            peakPower: Float,
            rmsPower: Float,
            elapsedSeconds: Double,
            sampleRate: Double,
            channelCount: UInt32
        ) {
            self.generation = generation
            self.frameIncrement = frameIncrement
            self.peakPower = peakPower
            self.rmsPower = rmsPower
            self.elapsedSeconds = elapsedSeconds
            self.sampleRate = sampleRate
            self.channelCount = channelCount
        }
    }

    public struct CumulativeMetrics: Equatable {
        public let frameCount: UInt64
        public let peakPower: Float
        public let rmsPower: Float
        public let elapsedSeconds: Double
        public let sampleRate: Double
        public let channelCount: UInt32

        public init(
            frameCount: UInt64 = 0,
            peakPower: Float = -160.0,
            rmsPower: Float = -160.0,
            elapsedSeconds: Double = 0.0,
            sampleRate: Double = 0.0,
            channelCount: UInt32 = 0
        ) {
            self.frameCount = frameCount
            self.peakPower = peakPower
            self.rmsPower = rmsPower
            self.elapsedSeconds = elapsedSeconds
            self.sampleRate = sampleRate
            self.channelCount = channelCount
        }
    }

    // MARK: - State Properties
    public private(set) var state: State = .idle
    public private(set) var activeGeneration: UInt64 = 0
    public private(set) var currentMetrics: CumulativeMetrics = CumulativeMetrics()
    public private(set) var totalFramesRecorded: UInt64 = 0

    public init() {}

    // MARK: - Generation & Start Initiation
    /// Issues a new unique monotonic generation token to initiate capture preparation.
    /// Fails closed if counter would overflow UInt64.max.
    /// Caller can pass `expectedGeneration` to ensure caller intent is not stale.
    public func issueStartToken(expectedGeneration: UInt64? = nil) -> Result<UInt64, LifecycleError> {
        guard state != .recording && state != .starting else {
            return .failure(.alreadyRecording)
        }

        if let expected = expectedGeneration, expected != activeGeneration {
            return .failure(.staleGeneration(issued: expected, active: activeGeneration))
        }

        guard activeGeneration < UInt64.max else {
            state = .failed
            return .failure(.generationOverflow)
        }

        activeGeneration += 1
        state = .starting
        totalFramesRecorded = 0
        currentMetrics = CumulativeMetrics()
        return .success(activeGeneration)
    }

    // MARK: - Hardware Acknowledgment & Activation
    /// Called when native engine/session start returns successfully for the given generation token.
    public func acknowledgeStartSuccess(for generation: UInt64) -> Result<UInt64, LifecycleError> {
        guard generation == activeGeneration else {
            // Superseded or stopped in flight
            return .failure(.staleGeneration(issued: generation, active: activeGeneration))
        }

        guard state == .starting else {
            return .failure(.cancelledBeforeStart)
        }

        state = .recording
        return .success(generation)
    }

    /// Called when native engine/session start fails or is aborted.
    public func acknowledgeStartFailure(for generation: UInt64, error: LifecycleError) -> Result<Void, LifecycleError> {
        guard generation == activeGeneration else {
            // Stale failure from previously cancelled/superseded generation
            return .failure(.staleGeneration(issued: generation, active: activeGeneration))
        }

        state = (error == .permissionDenied) ? .blocked : .failed
        return .success(())
    }

    // MARK: - Stopping & Invalidation
    /// Transitions state to stopped/interrupted/failed and invalidates active generation.
    /// Advances generation so any in-flight start or queued tap callbacks are instantly stale.
    @discardableResult
    public func stop(targetState: State = .stopped) -> UInt64 {
        if activeGeneration < UInt64.max {
            activeGeneration += 1
        }
        state = targetState

        // Freeze frame count, reset power levels to -160 dB
        currentMetrics = CumulativeMetrics(
            frameCount: totalFramesRecorded,
            peakPower: -160.0,
            rmsPower: -160.0,
            elapsedSeconds: currentMetrics.elapsedSeconds,
            sampleRate: currentMetrics.sampleRate,
            channelCount: currentMetrics.channelCount
        )

        return activeGeneration
    }

    // MARK: - Metric Intake
    /// Accepts a rate-limited immutable snapshot from a tap context.
    /// Dropped if snapshot generation != activeGeneration or state != .recording.
    @discardableResult
    public func acceptMetricSnapshot(_ snapshot: MetricSnapshot) -> Bool {
        guard snapshot.generation == activeGeneration else {
            // Stale snapshot from prior generation; drop silently
            return false
        }

        guard state == .recording else {
            // Dropped: not in recording state
            return false
        }

        // Check for frame counter overflow
        let (newTotal, overflow) = totalFramesRecorded.addingReportingOverflow(snapshot.frameIncrement)
        if overflow {
            totalFramesRecorded = UInt64.max
        } else {
            totalFramesRecorded = newTotal
        }

        currentMetrics = CumulativeMetrics(
            frameCount: totalFramesRecorded,
            peakPower: snapshot.peakPower,
            rmsPower: snapshot.rmsPower,
            elapsedSeconds: snapshot.elapsedSeconds,
            sampleRate: snapshot.sampleRate,
            channelCount: snapshot.channelCount
        )

        return true
    }

    // MARK: - State Update Helpers
    public func setPermissionStatus(granted: Bool) {
        if granted {
            if state != .recording && state != .starting {
                state = .ready
            }
        } else {
            state = .blocked
        }
    }

    public func markRequestingPermission() {
        if state != .recording && state != .starting {
            state = .requestingPermission
        }
    }
}
