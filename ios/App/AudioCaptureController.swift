import Foundation
import AVFoundation

/// Delegate protocol for observing real microphone capture events from `AudioCaptureController`.
///
/// NOTE: Status callbacks are rate-limited (~4Hz) and always dispatched to the main thread.
/// The raw audio render callback itself performs ONLY bounded, allocation-free RMS and peak metering.
/// No raw audio buffers are persisted, logged, or sent to cloud/UserDefaults.
public protocol AudioCaptureDelegate: AnyObject {
    func audioCaptureController(_ controller: AudioCaptureController, didUpdateStatus status: AudioCaptureController.Status)
    func audioCaptureController(_ controller: AudioCaptureController, didUpdateMetrics metrics: AudioCaptureController.Metrics)
    func audioCaptureController(_ controller: AudioCaptureController, didFailWithError error: AudioCaptureController.CaptureError)
}

/// Centralized, containing-app-owned real microphone capture controller.
///
/// Complies with Apple audio requirements & production concurrency invariants:
/// - Uses `AVAudioEngine` and `AVAudioSession`.
/// - Input tap installed on `engine.inputNode`.
/// - Explicit user permission prompt handled truthfully on iOS 15+.
/// - Category: `.playAndRecord` with `.defaultToSpeaker` and `.allowBluetooth`.
/// - Session activated ONLY when capture starts; deactivated on terminal stop.
/// - Handles audio interruptions and route changes cleanly.
/// - Fail-closed on all hardware/audio exceptions; no orphaned taps or active mics.
/// - All native engine/session state transitions, generation fencing, and counters are serialized
///   on `stateQueue` and governed by production `CaptureLifecycleGate`.
/// - Nonblocking, allocation-free audio tap metering uses dedicated `MeterContext` with immutable
///   session tokens; rate-limited (~4Hz / 250ms) immutable snapshots dispatched to `stateQueue`.
public final class AudioCaptureController: NSObject {

    public static let shared = AudioCaptureController()

    public typealias Status = CaptureLifecycleGate.State
    public typealias Metrics = CaptureLifecycleGate.CumulativeMetrics

    public enum CaptureError: LocalizedError, Equatable {
        case permissionDenied
        case hardwareUnavailable
        case engineConfigurationFailed(String)
        case audioSessionActivationFailed(String)
        case alreadyRecording
        case cancelledBeforeStart
        case sessionInterrupted
        case generationOverflow

        public var errorDescription: String? {
            switch self {
            case .permissionDenied:
                return "Microphone permission was denied by the user."
            case .hardwareUnavailable:
                return "No audio input hardware is available on this device."
            case .engineConfigurationFailed(let details):
                return "AudioEngine configuration failed: \(details)"
            case .audioSessionActivationFailed(let details):
                return "AVAudioSession activation failed: \(details)"
            case .alreadyRecording:
                return "Microphone is already actively recording."
            case .cancelledBeforeStart:
                return "Audio capture request was cancelled before start completion."
            case .sessionInterrupted:
                return "Audio session was interrupted by the system."
            case .generationOverflow:
                return "Capture generation counter reached maximum value; failing closed."
            }
        }

        init(from lifecycleError: CaptureLifecycleGate.LifecycleError) {
            switch lifecycleError {
            case .permissionDenied:
                self = .permissionDenied
            case .hardwareUnavailable:
                self = .hardwareUnavailable
            case .engineConfigurationFailed(let msg):
                self = .engineConfigurationFailed(msg)
            case .audioSessionActivationFailed(let msg):
                self = .audioSessionActivationFailed(msg)
            case .alreadyRecording:
                self = .alreadyRecording
            case .cancelledBeforeStart, .staleGeneration:
                self = .cancelledBeforeStart
            case .sessionInterrupted:
                self = .sessionInterrupted
            case .generationOverflow:
                self = .generationOverflow
            case .invalidStateTransition(_, let action):
                self = .engineConfigurationFailed(action)
            }
        }
    }

    /// Dedicated per-tap metering context owned by the tap closure.
    ///
    /// Invariants:
    /// 1. Holds immutable session token `generation`.
    /// 2. Accumulators (`accumulatedFrames`, `lastDispatchUptimeNanoseconds`) are mutated ONLY
    ///    on the realtime audio render thread within the tap callback.
    /// 3. NEVER reads or mutates mutable `AudioCaptureController` fields.
    /// 4. Does zero heap allocations, zero locks, zero sleeps, zero Foundation or disk/network I/O.
    /// 5. Rate-limits dispatches to `stateQueue` to ~4Hz (250ms), avoiding ~47 tasks/sec dispatch flood.
    /// MeterContext aliases the pure Foundation AudioCaptureTapContext.
    public typealias MeterContext = AudioCaptureTapContext


    public weak var delegate: AudioCaptureDelegate?

    public var status: Status {
        if Thread.isMainThread {
            return _publishedStatus
        } else {
            return stateQueue.sync { lifecycleGate.state }
        }
    }

    public var currentMetrics: Metrics {
        if Thread.isMainThread {
            return _publishedMetrics
        } else {
            return stateQueue.sync { lifecycleGate.currentMetrics }
        }
    }

    public var activeGeneration: UInt64 {
        if Thread.isMainThread {
            return _publishedGeneration
        } else {
            return stateQueue.sync { lifecycleGate.activeGeneration }
        }
    }

    // Published values cached for main thread reads
    private var _publishedStatus: Status = .idle
    private var _publishedMetrics: Metrics = Metrics()
    private var _publishedGeneration: UInt64 = 0

    // Specific key to detect execution on stateQueue and prevent deinit deadlock
    private static let stateQueueSpecificKey = DispatchSpecificKey<Void>()

    // Serial executor for all native engine, session, tap, and lifecycle gate mutations
    public let stateQueue = DispatchQueue(label: "com.echolet.audiocapture.state", qos: .userInitiated)

    // Production lifecycle gate state machine (accessed ONLY on stateQueue)
    private let lifecycleGate = CaptureLifecycleGate()

    private var audioEngine: AVAudioEngine?
    private var isTapInstalled = false

    override init() {
        super.init()
        stateQueue.setSpecific(key: Self.stateQueueSpecificKey, value: ())
        setupNotificationObservers()
    }

    deinit {
        NotificationCenter.default.removeObserver(self)
        // Ensure cleanup runs on serial executor without reentrant deadlock
        if DispatchQueue.getSpecific(key: Self.stateQueueSpecificKey) != nil {
            self.stopInternal(targetStatus: .stopped)
        } else {
            stateQueue.sync { [weak self] in
                guard let self = self else { return }
                self.stopInternal(targetStatus: .stopped)
            }
        }
    }

    // MARK: - Notification Observers
    private func setupNotificationObservers() {
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(handleAudioInterruption(_:)),
            name: AVAudioSession.interruptionNotification,
            object: nil
        )
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(handleRouteChange(_:)),
            name: AVAudioSession.routeChangeNotification,
            object: nil
        )
    }

    @objc private func handleAudioInterruption(_ notification: Notification) {
        stateQueue.async { [weak self] in
            guard let self = self else { return }
            guard let userInfo = notification.userInfo,
                  let typeValue = userInfo[AVAudioSessionInterruptionTypeKey] as? UInt,
                  let type = AVAudioSession.InterruptionType(rawValue: typeValue) else {
                return
            }

            switch type {
            case .began:
                if self.lifecycleGate.state == .recording || self.lifecycleGate.state == .starting {
                    self.stopInternal(targetStatus: .interrupted)
                }
            case .ended:
                // Apple HIG / Security: Require affirmative user interaction to resume recording after interruption.
                // Never auto-restart recording.
                if self.lifecycleGate.state == .interrupted {
                    self.lifecycleGate.setPermissionStatus(granted: true)
                    self.publishStateChange()
                }
            @unknown default:
                break
            }
        }
    }

    @objc private func handleRouteChange(_ notification: Notification) {
        stateQueue.async { [weak self] in
            guard let self = self else { return }
            guard let userInfo = notification.userInfo,
                  let reasonValue = userInfo[AVAudioSessionRouteChangeReasonKey] as? UInt,
                  let reason = AVAudioSession.RouteChangeReason(rawValue: reasonValue) else {
                return
            }

            if reason == .oldDeviceUnavailable && (self.lifecycleGate.state == .recording || self.lifecycleGate.state == .starting) {
                // Audio route pulled (e.g. headset unplugged / bluetooth disconnected)
                self.stopInternal(targetStatus: .interrupted)
            }
        }
    }

    // MARK: - Permission Preflight
    public func requestMicrophonePermission(completion: @escaping (Bool) -> Void) {
        let session = AVAudioSession.sharedInstance()
        switch session.recordPermission {
        case .granted:
            stateQueue.async { [weak self] in
                guard let self = self else { return }
                self.lifecycleGate.setPermissionStatus(granted: true)
                self.publishStateChange()
                DispatchQueue.main.async { completion(true) }
            }
        case .denied:
            stateQueue.async { [weak self] in
                guard let self = self else { return }
                self.lifecycleGate.setPermissionStatus(granted: false)
                self.publishStateChange()
                DispatchQueue.main.async { completion(false) }
            }
        case .undetermined:
            stateQueue.async { [weak self] in
                guard let self = self else { return }
                self.lifecycleGate.markRequestingPermission()
                self.publishStateChange()
                session.requestRecordPermission { granted in
                    self.stateQueue.async {
                        self.lifecycleGate.setPermissionStatus(granted: granted)
                        self.publishStateChange()
                        DispatchQueue.main.async { completion(granted) }
                    }
                }
            }
        @unknown default:
            stateQueue.async { [weak self] in
                guard let self = self else { return }
                self.lifecycleGate.setPermissionStatus(granted: false)
                self.publishStateChange()
                DispatchQueue.main.async { completion(false) }
            }
        }
    }

    // MARK: - Start Recording with Generation & Completion
    /// Begins native audio capture under a strictly serialized monotonic generation token.
    /// Entire initiation, configuration, and engine start executes on `stateQueue` asynchronously.
    /// Returns immediately without synchronous caller re-entry or deadlock.
    ///
    /// - Parameters:
    ///   - expectedGeneration: Optional generation to fence against stale caller triggers.
    ///   - completion: Callback executed on stateQueue indicating whether hardware start succeeded.
    public func startCapture(
        expectedGeneration: UInt64? = nil,
        completion: ((Result<UInt64, CaptureError>) -> Void)? = nil
    ) {
        stateQueue.async { [weak self] in
            guard let self = self else {
                completion?(.failure(.cancelledBeforeStart))
                return
            }

            // Issue unique monotonic generation token via production lifecycle gate
            let tokenResult = self.lifecycleGate.issueStartToken(expectedGeneration: expectedGeneration)
            let issuedGen: UInt64
            switch tokenResult {
            case .success(let gen):
                issuedGen = gen
            case .failure(let err):
                let captureErr = CaptureError(from: err)
                if captureErr != .alreadyRecording {
                    self.notifyError(captureErr)
                }
                self.publishStateChange()
                completion?(.failure(captureErr))
                return
            }

            self.publishStateChange()

            let audioSession = AVAudioSession.sharedInstance()
            guard audioSession.recordPermission == .granted else {
                _ = self.lifecycleGate.acknowledgeStartFailure(for: issuedGen, error: .permissionDenied)
                self.publishStateChange()
                self.notifyError(.permissionDenied)
                completion?(.failure(.permissionDenied))
                return
            }

            // 1. Configure and activate AVAudioSession
            do {
                try audioSession.setCategory(
                    .playAndRecord,
                    mode: .default,
                    options: [.defaultToSpeaker, .allowBluetooth]
                )
                try audioSession.setActive(true, options: .notifyOthersOnDeactivation)
            } catch {
                _ = self.lifecycleGate.acknowledgeStartFailure(for: issuedGen, error: .audioSessionActivationFailed(error.localizedDescription))
                self.publishStateChange()
                self.notifyError(.audioSessionActivationFailed(error.localizedDescription))
                completion?(.failure(.audioSessionActivationFailed(error.localizedDescription)))
                return
            }

            // Check generation fence after audioSession activation (e.g. stop occurred in flight)
            guard self.lifecycleGate.activeGeneration == issuedGen && self.lifecycleGate.state == .starting else {
                _ = try? audioSession.setActive(false, options: .notifyOthersOnDeactivation)
                completion?(.failure(.cancelledBeforeStart))
                return
            }

            // 2. Setup AVAudioEngine
            let engine = AVAudioEngine()
            let inputNode = engine.inputNode
            let inputFormat = inputNode.outputFormat(forBus: 0)

            guard inputFormat.sampleRate > 0 && inputFormat.channelCount > 0 else {
                self.stopInternal(targetStatus: .failed)
                self.notifyError(.hardwareUnavailable)
                completion?(.failure(.hardwareUnavailable))
                return
            }

            // 3. Preallocate per-tap MeterContext with immutable session token
            let sampleRate = inputFormat.sampleRate
            let channelCount = inputFormat.channelCount
            let startUptime = DispatchTime.now().uptimeNanoseconds

            let meterContext = MeterContext(
                generation: issuedGen,
                sampleRate: sampleRate,
                channelCount: channelCount,
                startUptimeNanoseconds: startUptime
            )

            // 4. Install input tap (buffer size 1024 frames)
            let bufferSize: AVAudioFrameCount = 1024
            inputNode.removeTap(onBus: 0) // Defensive cleanup

            // Closure captures dedicated meterContext strongly for the tap lifetime and weak self, NEVER reads mutable controller fields
            let tapHandler = MeterContext.makeTapHandler(
                context: meterContext,
                owner: self,
                stateQueue: self.stateQueue,
                dispatchAction: { controller, snapshot in
                    controller.handleMetricSnapshot(snapshot)
                }
            )
            inputNode.installTap(onBus: 0, bufferSize: bufferSize, format: inputFormat) { buffer, _ in
                AudioCaptureController.processAudioBuffer(
                    buffer: buffer,
                    tapHandler: tapHandler
                )
            }

            self.isTapInstalled = true
            self.audioEngine = engine

            // 5. Start engine
            do {
                try engine.start()

                // Acknowledge start success on gate
                let ackResult = self.lifecycleGate.acknowledgeStartSuccess(for: issuedGen)
                switch ackResult {
                case .success(let activeGen):
                    self.publishStateChange()
                    completion?(.success(activeGen))
                case .failure(let err):
                    // Stopped or superseded while engine.start was executing
                    self.stopInternal(targetStatus: .stopped)
                    completion?(.failure(CaptureError(from: err)))
                }
            } catch {
                self.stopInternal(targetStatus: .failed)
                let errorDesc = error.localizedDescription
                self.notifyError(.engineConfigurationFailed(errorDesc))
                completion?(.failure(.engineConfigurationFailed(errorDesc)))
            }
        }
    }

    // MARK: - Stop Recording with Completion
    /// Hardware stop completion only after tap removed, engine stopped, and session deactivate attempted.
    public func stopCapture(completion: (() -> Void)? = nil) {
        stateQueue.async { [weak self] in
            guard let self = self else {
                completion?()
                return
            }
            self.stopInternal(targetStatus: .stopped)
            completion?()
        }
    }

    // MARK: - Generation-Bound Conditional Stop
    /// Conditional cleanup for stale start completions.
    ///
    /// Decides against the REAL `CaptureLifecycleGate.activeGeneration` and state on
    /// the SAME serialized `stateQueue` that owns all stop/start mutations — never
    /// against the main-thread published generation. The compare-and-stop is atomic
    /// with respect to any newer engine start or stop:
    /// - If the gate is genuinely active (starting/recording) at exactly
    ///   `expectedGeneration`, the hardware stops (tap removed, engine stopped,
    ///   session deactivated) and completion(true) fires.
    /// - If a newer generation (session B, manual) now owns the hardware, or the
    ///   expected capture already stopped (generation advanced past expectation),
    ///   this is a strict NO-OP and completion(false) fires.
    public func stopCaptureIfGeneration(expectedGeneration: UInt64, completion: ((Bool) -> Void)? = nil) {
        stateQueue.async { [weak self] in
            guard let self = self else {
                completion?(false)
                return
            }
            let stopped = self.lifecycleGate.stopIfGeneration(expectedGeneration: expectedGeneration)
            if stopped {
                self.stopInternal(targetStatus: .stopped)
            }
            completion?(stopped)
        }
    }

    /// Internal synchronous stop executed exclusively on `stateQueue`.
    private func stopInternal(targetStatus: Status) {
        // Advance generation on lifecycle gate first so any in-flight callbacks/snapshots are instantly fenced
        _ = self.lifecycleGate.stop(targetState: targetStatus)

        // Halt native engine and remove tap
        if let engine = audioEngine {
            if isTapInstalled {
                engine.inputNode.removeTap(onBus: 0)
                isTapInstalled = false
            }
            if engine.isRunning {
                engine.stop()
            }
        }
        audioEngine = nil

        // Deactivate audio session to release hardware cleanly
        let audioSession = AVAudioSession.sharedInstance()
        do {
            try audioSession.setActive(false, options: .notifyOthersOnDeactivation)
        } catch {
            // Non-fatal deactivation error
        }

        publishStateChange()
    }

    // MARK: - Allocation-Free Metering (Audio Render Context)
    /// Real-time audio render callback processing.
    ///
    /// Rules:
    /// - Mutates ONLY `context.accumulatedFrames` and `context.lastDispatchUptimeNanoseconds`.
    /// - Reads ZERO mutable fields on `controller`.
    /// - Rate-limits before dispatching: dispatches snapshot to `stateQueue` only when >= 250ms elapsed.
    /// - Performs zero heap allocations, zero locks, zero sleeps, zero Foundation/disk/network I/O.
    private static func processAudioBuffer(
        buffer: AVAudioPCMBuffer,
        tapHandler: (_ frameLength: UInt64, _ maxSample: Float, _ rms: Float, _ nowUptimeNanoseconds: UInt64) -> Void
    ) {
        let frameLength = UInt64(buffer.frameLength)
        guard frameLength > 0 else { return }

        guard let channelData = buffer.floatChannelData else { return }
        let channelSamples = channelData[0] // Primary channel

        var maxSample: Float = 0.0
        var sumSquares: Float = 0.0

        let count = Int(buffer.frameLength)
        for i in 0..<count {
            let sample = channelSamples[i]
            let absSample = abs(sample)
            if absSample > maxSample {
                maxSample = absSample
            }
            sumSquares += sample * sample
        }

        let meanSquare = sumSquares / Float(buffer.frameLength)
        let rms = sqrt(meanSquare)
        let now = DispatchTime.now().uptimeNanoseconds

        tapHandler(frameLength, maxSample, rms, now)
    }

    /// Executed exclusively on `stateQueue` to accept snapshot and update UI
    private func handleMetricSnapshot(_ snapshot: CaptureLifecycleGate.MetricSnapshot) {
        let accepted = lifecycleGate.acceptMetricSnapshot(snapshot)
        guard accepted else { return }

        let latestMetrics = lifecycleGate.currentMetrics
        let currentGen = lifecycleGate.activeGeneration

        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            // Verify generation hasn't changed before updating published metrics & UI
            guard self._publishedGeneration == currentGen else { return }
            self._publishedMetrics = latestMetrics
            self.delegate?.audioCaptureController(self, didUpdateMetrics: latestMetrics)
        }
    }

    /// Updates main-thread published values and notifies delegate
    /// MUST be called on `stateQueue`.
    private func publishStateChange() {
        let newState = lifecycleGate.state
        let newMetrics = lifecycleGate.currentMetrics
        let newGen = lifecycleGate.activeGeneration

        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            let stateChanged = (self._publishedStatus != newState)
            self._publishedStatus = newState
            self._publishedMetrics = newMetrics
            self._publishedGeneration = newGen

            if stateChanged {
                self.delegate?.audioCaptureController(self, didUpdateStatus: newState)
            }
            self.delegate?.audioCaptureController(self, didUpdateMetrics: newMetrics)
        }
    }

    private func notifyError(_ error: CaptureError) {
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            self.delegate?.audioCaptureController(self, didFailWithError: error)
        }
    }
}
