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
/// Complies with Apple audio requirements:
/// - Uses `AVAudioEngine` and `AVAudioSession`.
/// - Input tap installed on `engine.inputNode`.
/// - Explicit user permission prompt handled truthfully on iOS 15+.
/// - Category: `.playAndRecord` with `.defaultToSpeaker` and `.allowBluetooth`.
/// - Session activated ONLY when capture starts; deactivated on terminal stop.
/// - Handles audio interruptions and route changes cleanly.
/// - Fail-closed on all hardware/audio exceptions; no orphaned taps or active mics.
public final class AudioCaptureController: NSObject {

    public static let shared = AudioCaptureController()

    public enum Status: String, Equatable {
        case idle
        case requestingPermission
        case ready
        case recording
        case interrupted
        case stopped
        case blocked
        case failed
    }

    public struct Metrics: Equatable {
        public let frameCount: UInt64
        public let peakPower: Float
        public let rmsPower: Float
        public let elapsedSeconds: Double
        public let sampleRate: Double
        public let channelCount: UInt32
    }

    public enum CaptureError: LocalizedError, Equatable {
        case permissionDenied
        case hardwareUnavailable
        case engineConfigurationFailed(String)
        case audioSessionActivationFailed(String)
        case alreadyRecording

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
            }
        }
    }

    public weak var delegate: AudioCaptureDelegate?

    public private(set) var status: Status = .idle {
        didSet {
            guard oldValue != status else { return }
            DispatchQueue.main.async { [weak self] in
                guard let self = self else { return }
                self.delegate?.audioCaptureController(self, didUpdateStatus: self.status)
            }
        }
    }

    public private(set) var currentMetrics = Metrics(
        frameCount: 0,
        peakPower: -160.0,
        rmsPower: -160.0,
        elapsedSeconds: 0.0,
        sampleRate: 0.0,
        channelCount: 0
    )

    private var audioEngine: AVAudioEngine?
    private var isTapInstalled = false
    private let stateQueue = DispatchQueue(label: "com.echolet.audiocapture.state", qos: .userInitiated)

    // Rate-limiting for audio tap UI updates (~4Hz / 250ms interval)
    private var lastMetricsDispatchTime: UInt64 = 0
    private var totalFramesRecorded: UInt64 = 0
    private var recordingStartTime: DispatchTime?

    // Active generation token to reject stale asynchronous callbacks
    private var activeGeneration: UInt64 = 0

    override init() {
        super.init()
        setupNotificationObservers()
    }

    deinit {
        NotificationCenter.default.removeObserver(self)
        stopInternal(targetStatus: .stopped)
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
                if self.status == .recording {
                    self.stopInternal(targetStatus: .interrupted)
                }
            case .ended:
                // Require affirmative user interaction to resume recording after interruption
                if self.status == .interrupted {
                    self.status = .ready
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

            if reason == .oldDeviceUnavailable && self.status == .recording {
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
            self.status = (self.status == .recording) ? .recording : .ready
            completion(true)
        case .denied:
            self.status = .blocked
            completion(false)
        case .undetermined:
            self.status = .requestingPermission
            session.requestRecordPermission { [weak self] granted in
                DispatchQueue.main.async {
                    guard let self = self else { return }
                    self.status = granted ? .ready : .blocked
                    completion(granted)
                }
            }
        @unknown default:
            self.status = .blocked
            completion(false)
        }
    }

    // MARK: - Start Recording
    public func startCapture() {
        stateQueue.async { [weak self] in
            guard let self = self else { return }

            guard self.status != .recording else {
                self.notifyError(.alreadyRecording)
                return
            }

            let audioSession = AVAudioSession.sharedInstance()
            guard audioSession.recordPermission == .granted else {
                self.status = .blocked
                self.notifyError(.permissionDenied)
                return
            }

            self.activeGeneration += 1
            let currentGen = self.activeGeneration

            // 1. Configure and activate AVAudioSession
            do {
                try audioSession.setCategory(
                    .playAndRecord,
                    mode: .default,
                    options: [.defaultToSpeaker, .allowBluetooth]
                )
                try audioSession.setActive(true, options: .notifyOthersOnDeactivation)
            } catch {
                self.status = .failed
                self.notifyError(.audioSessionActivationFailed(error.localizedDescription))
                return
            }

            // 2. Setup AVAudioEngine
            let engine = AVAudioEngine()
            let inputNode = engine.inputNode
            let inputFormat = inputNode.outputFormat(forBus: 0)

            guard inputFormat.sampleRate > 0 && inputFormat.channelCount > 0 else {
                self.status = .failed
                self.stopInternal(targetStatus: .failed)
                self.notifyError(.hardwareUnavailable)
                return
            }

            // 3. Reset metering counters
            self.totalFramesRecorded = 0
            self.recordingStartTime = DispatchTime.now()
            self.lastMetricsDispatchTime = 0
            let sampleRate = inputFormat.sampleRate
            let channels = inputFormat.channelCount

            // 4. Install input tap (buffer size 1024 frames)
            let bufferSize: AVAudioFrameCount = 1024
            inputNode.removeTap(onBus: 0) // Defensive cleanup
            inputNode.installTap(onBus: 0, bufferSize: bufferSize, format: inputFormat) { [weak self] buffer, _ in
                guard let self = self else { return }
                self.processAudioBuffer(buffer, generation: currentGen, sampleRate: sampleRate, channelCount: channels)
            }
            self.isTapInstalled = true
            self.audioEngine = engine

            // 5. Start engine
            do {
                try engine.start()
                self.status = .recording
            } catch {
                self.stopInternal(targetStatus: .failed)
                self.notifyError(.engineConfigurationFailed(error.localizedDescription))
            }
        }
    }

    // MARK: - Stop Recording
    public func stopCapture() {
        stateQueue.async { [weak self] in
            guard let self = self else { return }
            self.stopInternal(targetStatus: .stopped)
        }
    }

    private func stopInternal(targetStatus: Status) {
        activeGeneration += 1

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

        // Deactivate audio session to release hardware
        let audioSession = AVAudioSession.sharedInstance()
        do {
            try audioSession.setActive(false, options: .notifyOthersOnDeactivation)
        } catch {
            // Log or ignore non-fatal deactivation error
        }

        self.status = targetStatus

        // Reset real-time power levels, preserve total recorded frame count
        let finalMetrics = Metrics(
            frameCount: self.totalFramesRecorded,
            peakPower: -160.0,
            rmsPower: -160.0,
            elapsedSeconds: self.currentMetrics.elapsedSeconds,
            sampleRate: self.currentMetrics.sampleRate,
            channelCount: self.currentMetrics.channelCount
        )
        self.currentMetrics = finalMetrics

        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            self.delegate?.audioCaptureController(self, didUpdateMetrics: finalMetrics)
        }
    }

    // MARK: - Allocation-Free Metering (Audio Thread)
    private func processAudioBuffer(
        _ buffer: AVAudioPCMBuffer,
        generation: UInt64,
        sampleRate: Double,
        channelCount: UInt32
    ) {
        guard generation == self.activeGeneration else { return }

        let frameLength = UInt64(buffer.frameLength)
        guard frameLength > 0 else { return }

        totalFramesRecorded += frameLength

        guard let channelData = buffer.floatChannelData else { return }
        let channelSamples = channelData[0] // Primary channel

        var maxSample: Float = 0.0
        var sumSquares: Float = 0.0

        for i in 0..<Int(buffer.frameLength) {
            let sample = channelSamples[i]
            let absSample = abs(sample)
            if absSample > maxSample {
                maxSample = absSample
            }
            sumSquares += sample * sample
        }

        let meanSquare = sumSquares / Float(buffer.frameLength)
        let rms = sqrt(meanSquare)

        // Convert to dBFS
        let peakDb: Float = (maxSample > 0.0000001) ? 20.0 * log10(maxSample) : -160.0
        let rmsDb: Float = (rms > 0.0000001) ? 20.0 * log10(rms) : -160.0

        let now = DispatchTime.now().uptimeNanoseconds
        // Rate-limit UI dispatch to ~4Hz (every 250_000_000 ns)
        if now - lastMetricsDispatchTime > 250_000_000 {
            lastMetricsDispatchTime = now

            var elapsed: Double = 0.0
            if let start = recordingStartTime {
                elapsed = Double(now - start.uptimeNanoseconds) / 1_000_000_000.0
            }

            let metrics = Metrics(
                frameCount: totalFramesRecorded,
                peakPower: max(peakDb, -160.0),
                rmsPower: max(rmsDb, -160.0),
                elapsedSeconds: elapsed,
                sampleRate: sampleRate,
                channelCount: channelCount
            )
            self.currentMetrics = metrics

            DispatchQueue.main.async { [weak self] in
                guard let self = self else { return }
                guard generation == self.activeGeneration else { return }
                self.delegate?.audioCaptureController(self, didUpdateMetrics: metrics)
            }
        }
    }

    private func notifyError(_ error: CaptureError) {
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            self.delegate?.audioCaptureController(self, didFailWithError: error)
        }
    }
}
