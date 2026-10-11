import Foundation
import AVFoundation

/// App-side on-device ASR service (J4-E2E DEBUG/test implementation).
///
/// Serial ownership model:
/// - ONE background serial worker queue (`asrQueue`) exclusively owns the
///   native recognizer handle and every streaming session FFI call
///   (create / feed / read / flush / destroy). No inference, model mutation,
///   or recognizer access ever happens on the main thread, the AVAudioEngine
///   realtime tap, or `WarmIPCService.ipcQueue`.
/// - PCM moves from the realtime tap through the lock-free bounded SPSC ring
///   (`echolet_ios_ring_*`, roughly 2 s at the native hardware rate). The tap
///   performs ONLY an allocation-free mono downmix + ring push; the worker
///   queue drains the ring, converts native-rate mono → 16 kHz mono with a
///   session-owned `AVAudioConverter`, and feeds the recognizer.
/// - Every session is fenced by its exact capture generation and explicit
///   session id; stale rings/late stops fail closed. A session ends by
///   destroying its stream FIRST; the recognizer handle persists for reuse.
/// - Privacy: no raw PCM is written to disk, network, or logs; transcripts
///   are surfaced only through the delegate (UI / explicit smoke). Console
///   logs carry state and counts only.
///
/// The realtime tap NEVER calls this service; it pushes floats into the ring
/// handle it was handed at hardware start (memcpy + atomic release store —
/// no allocation, no lock, no FFI inference).
final class EcholetAsrService {

    static let shared = EcholetAsrService()

    enum ModelState: String {
        case notLoaded
        case loading
        case loaded
        case failed
    }

    enum SessionKind: String {
        case warmKeyboard
        case liveDictation
        case syntheticWav
    }

    /// Timestamps for optional rough observability (no promises).
    struct SessionObservability {
        var beganAtMs: UInt64 = 0
        var finalizedAfterMs: UInt64 = 0
        var loadMs: UInt64 = 0
    }

    protocol Delegate: AnyObject {
        func asrService(_ service: EcholetAsrService, didChangeModelState state: ModelState)
        func asrService(_ service: EcholetAsrService, didUpdatePartialText partial: String, sessionId: String)
        func asrService(_ service: EcholetAsrService, sessionDidStart sessionId: String, kind: SessionKind)
        func asrService(_ service: EcholetAsrService, sessionDidFail sessionId: String, errorCode: String)
        func asrService(_ service: EcholetAsrService, sessionDidDispose sessionId: String, reason: String)
    }

    weak var delegate: Delegate?

    /// Serial worker that owns ALL recognizer/stream state and FFI access.
    let asrQueue = DispatchQueue(label: "com.echolet.asr.worker", qos: .userInitiated)

    // Recognizer + model state are mutated only on asrQueue; the main thread
    // receives immutable state snapshots through the delegate only.
    private var modelState: ModelState = .notLoaded
    private var recognizer: UnsafeMutableRawPointer?
    private var recognizerLoadStartedMs: UInt64?
    private var loadInProgress = false

    /// Active streaming pipeline; nil when no session is active.
    private var session: AsrPipeline?

    /// Main-published mirror of the last final transcript (UI convenience).
    public private(set) var lastFinalTranscript: String?

    // MARK: - FFI (Rust bridge; linked into the app target only)

    @_silgen_name("echolet_ios_recognizer_create")
    private static func ffi_recognizer_create(_ modelDir: UnsafePointer<CChar>?) -> UnsafeMutableRawPointer?
    @_silgen_name("echolet_ios_recognizer_destroy")
    private static func ffi_recognizer_destroy(_ rec: UnsafeMutableRawPointer?)
    @_silgen_name("echolet_ios_stream_create")
    private static func ffi_stream_create(_ rec: UnsafeMutableRawPointer?) -> UnsafeMutableRawPointer?
    @_silgen_name("echolet_ios_stream_feed")
    private static func ffi_stream_feed(_ stream: UnsafeMutableRawPointer?, _ sampleRate: Int32, _ samples: UnsafePointer<Float>?, _ numSamples: Int32) -> Int32
    @_silgen_name("echolet_ios_stream_read")
    private static func ffi_stream_read(_ stream: UnsafeMutableRawPointer?, _ out: UnsafeMutablePointer<CChar>?, _ outLen: Int32) -> Int32
    @_silgen_name("echolet_ios_stream_flush")
    private static func ffi_stream_flush(_ stream: UnsafeMutableRawPointer?, _ out: UnsafeMutablePointer<CChar>?, _ outLen: Int32) -> Int32
    @_silgen_name("echolet_ios_stream_destroy")
    private static func ffi_stream_destroy(_ stream: UnsafeMutableRawPointer?)
    @_silgen_name("echolet_ios_ring_create")
    static func ffi_ring_create(_ capacitySamples: UInt64) -> UnsafeMutableRawPointer?
    @_silgen_name("echolet_ios_ring_push")
    static func ffi_ring_push(_ ring: UnsafeMutableRawPointer?, _ samples: UnsafePointer<Float>?, _ numSamples: Int) -> Int64
    @_silgen_name("echolet_ios_ring_pop")
    static func ffi_ring_pop(_ ring: UnsafeMutableRawPointer?, _ out: UnsafeMutablePointer<Float>?, _ maxSamples: Int) -> Int64
    @_silgen_name("echolet_ios_ring_clear")
    static func ffi_ring_clear(_ ring: UnsafeMutableRawPointer?)
    @_silgen_name("echolet_ios_ring_destroy")
    static func ffi_ring_destroy(_ ring: UnsafeMutableRawPointer?)
    @_silgen_name("echolet_ios_ring_dropped_total")
    static func ffi_ring_dropped_total(_ ring: UnsafeMutableRawPointer?) -> UInt64

    // MARK: - Bundled model resolution

    /// Path of the bundled (git-ignored) bilingual-zh-en model folder resource.
    static func bundledModelDirectory() -> String? {
        return Bundle.main.path(forResource: "bilingual-zh-en", ofType: nil, inDirectory: nil)
    }

    /// Main-thread-safe immutable snapshot of the current model state.
    func publishModelStateSnapshot(_ body: @escaping (ModelState) -> Void) {
        asrQueue.async { [weak self] in
            guard let self = self else { return }
            let state = self.modelState
            DispatchQueue.main.async { body(state) }
        }
    }

    // MARK: - Active capture tap ring exposure (off-realtime only)

    /// Tap-side ring metadata for the currently-running PCM capture.
    /// The retained context keeps the ring alive until the ASR pipeline that
    /// adopted it releases its reference (stop/flush consumption happens
    /// strictly AFTER the tap is detached, so the ring must not die with it).
    struct TapRingState {
        let context: EcholetCapturePCMContext
        let generation: UInt64
        let nativeSampleRate: Double

        var ring: UnsafeMutableRawPointer { context.ring }
    }

    private let tapRingLock = NSLock()
    private var tapRingState: TapRingState?

    /// Called by AudioCaptureController on start/stop of PCM-gated captures.
    func publishTapRing(_ state: TapRingState?) {
        tapRingLock.lock()
        tapRingState = state
        tapRingLock.unlock()
    }

    func currentTapRing() -> TapRingState? {
        tapRingLock.lock()
        defer { tapRingLock.unlock() }
        return tapRingState
    }

    // MARK: - Model loading

    /// Asynchronously loads the pinned offline recognizer bundle. Never
    /// blocks the caller thread; completion runs on an arbitrary queue.
    func loadModelIfNeeded(completion: @escaping (Bool, String?) -> Void) {
        asrQueue.async { [weak self] in
            guard let self = self else {
                completion(false, "asr service unavailable")
                return
            }
            if self.modelState == .loaded, self.recognizer != nil {
                completion(true, nil)
                return
            }
            guard !self.loadInProgress else {
                completion(false, "model_load_in_progress")
                return
            }

            guard let modelDir = Self.bundledModelDirectory() else {
                print("[EcholetAsr] bundled model resource missing: bilingual-zh-en")
                completion(false, "bundled_model_missing")
                return
            }

            self.loadInProgress = true
            self.modelState = .loading
            self.recognizerLoadStartedMs = nowMs()
            self.notifyModelState()

            guard let rec = Self.ffi_recognizer_create((modelDir as NSString).utf8String) else {
                self.loadInProgress = false
                self.modelState = .failed
                self.notifyModelState()
                print("[EcholetAsr] recognizer_create FAILED (invalid model or sherpa init error)")
                completion(false, "recognizer_create_failed")
                return
            }
            self.recognizer = rec
            self.loadInProgress = false
            self.modelState = .loaded
            if let started = self.recognizerLoadStartedMs {
                print("[EcholetAsr] offline model loaded load_ms=\(nowMs() - started) (on-device only)")
            }
            self.notifyModelState()
            completion(true, nil)
        }
    }

    private func notifyModelState() {
        let state = modelState
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            self.delegate?.asrService(self, didChangeModelState: state)
        }
    }

    // MARK: - Tap-side realtime mono downmix + bounded ring push

    /// Called by `AudioCaptureController`'s tap context on the realtime render
    /// thread. Allocation-free mono downmix (average of first 2 channels)
    /// into the tap-owned preallocated scratch, then bounded lock-free push.
    /// Surplus is dropped inside the ring (never blocks).
    static func tapPushMonoToRing(
        ring: UnsafeMutableRawPointer?,
        scratch: UnsafeMutablePointer<Float>,
        scratchCapacity: Int,
        buffer: AVAudioPCMBuffer
    ) {
        let frames = Int(buffer.frameLength)
        guard let channels = Int(exactly: buffer.format.channelCount), frames > 0, frames <= scratchCapacity else { return }
        guard let channelData = buffer.floatChannelData else { return }
        let ch0 = channelData[0]
        if channels == 1 {
            scratch.update(from: ch0, count: frames)
        } else {
            let ch1 = channelData[1]
            for i in 0..<frames {
                scratch[i] = (ch0[i] + ch1[i]) * 0.5
            }
        }
        _ = Self.ffi_ring_push(ring, scratch, frames)
    }

    // MARK: - Streaming session pipeline

    final class AsrPipeline {
        let sessionId: String
        let kind: SessionKind
        let generation: UInt64
        /// Strong retention: the tap ring survives tap teardown until this
        /// pipeline's finalize/dispose drains it, then both die together.
        let tapContext: EcholetCapturePCMContext
        var ring: UnsafeMutableRawPointer { tapContext.ring }
        let stream: UnsafeMutableRawPointer
        let nativeSampleRate: Double
        /// Deterministic block-average decimator (e.g. 3:1 for a 48 kHz tap);
        /// nil only when the tap already delivers exactly 16 kHz.
        let resampler: EcholetDeterministicResampler?

        // Preallocated worker buffers (owned by this pipeline; asrQueue only).
        static let maxNativeChunk = 9600  // 200 ms at 48 kHz
        static let maxOutChunk = 4800     // 300 ms at 16 kHz
        var nativeChunk = [Float]()
        var resampledChunk = [Float](repeating: 0, count: maxNativeChunk)
        var cStringBuffer = [CChar](repeating: 0, count: 8192)

        var ringDroppedAtBegin: UInt64 = 0
        var framesPoppedTotal = 0
        var framesConvertedTotal = 0
        var framesFedTotal = 0
        var lastObservedDropped: UInt64 = 0
        var pumpCycleCount = 0
        var ended = false
        var pumpTimer: DispatchSourceTimer?
        var lastLoggedCycle = 0

        init(
            sessionId: String,
            kind: SessionKind,
            generation: UInt64,
            tapContext: EcholetCapturePCMContext,
            stream: UnsafeMutableRawPointer,
            nativeSampleRate: Double,
            resampler: EcholetDeterministicResampler?
        ) {
            self.sessionId = sessionId
            self.kind = kind
            self.generation = generation
            self.tapContext = tapContext
            self.stream = stream
            self.nativeSampleRate = nativeSampleRate
            self.resampler = resampler
            self.nativeChunk = [Float](repeating: 0, count: AsrPipeline.maxNativeChunk)
        }
    }

    /// Published (main-mirrored) session observability for the UI.
    private(set) var observability = SessionObservability()

    // MARK: - Session begin (warm keyboard adoption / live dictation)

    /// Begins a streaming session bound to the ACTIVE tap ring of exactly
    /// `expectedGeneration`. Pre-START ring backlog is discarded at the ring
    /// boundary (fresh utterance session). Runs wholly on `asrQueue`.
    func beginSession(
        sessionId: String,
        kind: SessionKind,
        expectedGeneration: UInt64,
        completion: ((Bool, String?) -> Void)? = nil
    ) {
        asrQueue.async { [weak self] in
            guard let self = self else { completion?(false, "asr service unavailable"); return }
            guard self.modelState == .loaded, let recognizer = self.recognizer else {
                print("[EcholetAsr] beginSession rejected: model not loaded")
                self.mainSessionFailure(sessionId: sessionId, errorCode: "asr_model_not_loaded")
                completion?(false, "asr_model_not_loaded")
                return
            }

            // Fail closed over an existing session: it ends dispased before a
            // new one may open (no double ownership of the pipeline).
            if let existing = self.session {
                self.disposePipelineLocked(existing, reason: "superseded_by_new_session")
            }

            guard let tapRing = self.currentTapRing(), tapRing.generation == expectedGeneration else {
                print("[EcholetAsr] beginSession fenced: tap ring generation mismatch expected=\(expectedGeneration)")
                self.mainSessionFailure(sessionId: sessionId, errorCode: "asr_capture_generation_mismatch")
                completion?(false, "asr_capture_generation_mismatch")
                return
            }

            // Discard pre-START backlog strictly at the ring boundary.
            Self.ffi_ring_clear(tapRing.ring)
            let droppedAtBegin = Self.ffi_ring_dropped_total(tapRing.ring)

            guard let stream = Self.ffi_stream_create(recognizer) else {
                print("[EcholetAsr] stream_create FAILED (recognizer retained)")
                self.mainSessionFailure(sessionId: sessionId, errorCode: "asr_stream_create_failed")
                completion?(false, "asr_stream_create_failed")
                return
            }

            // Deterministic resampler for the hardware tap rate → 16 kHz.
            // A 48 kHz iPad mic gives factor 3 (block mean of 3 samples);
            // an exact-16 kHz tap (never produced by the hardware-format
            // tap today) would skip resampling entirely.
            var resampler: EcholetDeterministicResampler?
            if abs(tapRing.nativeSampleRate - 16000.0) > 0.5 {
                guard let built = EcholetDeterministicResampler(
                    nativeSampleRate: tapRing.nativeSampleRate,
                    targetSampleRate: 16000.0
                ) else {
                    Self.ffi_stream_destroy(stream)
                    print("[EcholetAsr] unsupported tap sample rate for deterministic resampler: \(tapRing.nativeSampleRate)")
                    self.mainSessionFailure(sessionId: sessionId, errorCode: "asr_unsupported_capture_rate")
                    completion?(false, "asr_unsupported_capture_rate")
                    return
                }
                resampler = built
            }

            let pipeline = AsrPipeline(
                sessionId: sessionId,
                kind: kind,
                generation: expectedGeneration,
                tapContext: tapRing.context,
                stream: stream,
                nativeSampleRate: tapRing.nativeSampleRate,
                resampler: resampler
            )
            pipeline.ringDroppedAtBegin = droppedAtBegin
            self.session = pipeline
            self.observability.beganAtMs = nowMs()

            pipeline.pumpTimer = DispatchSource.makeTimerSource(queue: self.asrQueue)
            pipeline.pumpTimer?.schedule(
                deadline: .now() + DispatchTimeInterval.milliseconds(50),
                repeating: .milliseconds(50)
            )
            pipeline.pumpTimer?.setEventHandler { [weak self, weak pipeline] in
                if let pipeline = pipeline {
                    self?.pumpPipeline(pipeline)
                }
            }
            pipeline.pumpTimer?.resume()

            print("[EcholetAsr] session began id_prefix=\(sessionId.prefix(8)) kind=\(kind.rawValue) gen=\(expectedGeneration) native_rate=\(tapRing.nativeSampleRate)")
            DispatchQueue.main.async { [weak self] in
                guard let self = self else { return }
                self.delegate?.asrService(self, sessionDidStart: sessionId, kind: kind)
            }
            completion?(true, nil)
        }
    }

    // MARK: - Worker pump (asrQueue only)

    private func pumpPipeline(_ pipeline: AsrPipeline) {
        pipeline.pumpCycleCount += 1
        guard !pipeline.ended else { return }

        // Drain the ring fully this cycle (bounded by ring capacity).
        var poppedAny = false
        var guardCycles = 0
        while guardCycles < 60 {
            guardCycles += 1
            let popped = popRingChunk(pipeline)
            if popped <= 0 {
                break
            }
            poppedAny = true
            pipeline.framesPoppedTotal += Int(popped)
            let accepted = consumedIntoConverter(pipeline, poppedCount: Int(popped))
            if accepted < 0 {
                // Real error (converter error / feed rejected) → fail closed.
                sessionDidFailLocked(pipeline, errorCode: "asr_convert_chain_failed")
                return
            }
            // accepted == 0: converter latency-buffered this chunk; continue
            // draining the rest of the ring (never kill the session here).
        }

        if poppedAny {
            // Fresh audio arrived → bounded partial read. `read` is
            // idempotent (decode-all-ready + current result); the first pass
            // queries the required size, the second copies into the bounded
            // buffer.
            let required = Self.ffi_stream_read(pipeline.stream, nil, 0)
            if required < 0 {
                sessionDidFailLocked(pipeline, errorCode: "asr_partial_read_failed")
            } else if required < Int32(pipeline.cStringBuffer.count) {
                let got = pipeline.cStringBuffer.withUnsafeMutableBufferPointer { buf -> Int32 in
                    Self.ffi_stream_read(pipeline.stream, buf.baseAddress, Int32(buf.count))
                }
                if got >= 0 {
                    let text = String(cString: pipeline.cStringBuffer)
                    if !text.isEmpty {
                        DispatchQueue.main.async { [weak self] in
                            guard let self = self else { return }
                            self.delegate?.asrService(self, didUpdatePartialText: text, sessionId: pipeline.sessionId)
                        }
                    }
                }
            } else {
                // Oversize partial: no valid bounded copy exists without
                // dropping the streaming envelope → fail closed.
                sessionDidFailLocked(pipeline, errorCode: "asr_partial_oversize")
            }
        }

        // Rough integrability observability: state counts only, ~1 Hz.
        if pipeline.pumpCycleCount - pipeline.lastLoggedCycle >= 20 {
            pipeline.lastLoggedCycle = pipeline.pumpCycleCount
            print("[EcholetAsr] pump id_prefix=\(pipeline.sessionId.prefix(8)) cycles=\(pipeline.pumpCycleCount) popped_total=\(pipeline.framesPoppedTotal) converted_16k=\(pipeline.framesConvertedTotal) fed_total=\(pipeline.framesFedTotal) rate=\(pipeline.nativeSampleRate) tap_max_buf=\(pipeline.tapContext.maxTapBufferFrames) tap_oversize_drops=\(pipeline.tapContext.rejectedOversizePushes)")
        }
    }

    /// Must run on asrQueue. Pops at most `maxNativeChunk` floats into the
    /// pipeline-owned preallocated chunk and returns the popped count.
    private func popRingChunk(_ pipeline: AsrPipeline) -> Int {
        pipeline.nativeChunk.withUnsafeMutableBufferPointer { buf -> Int in
            guard let base = buf.baseAddress else { return -1 }
            return Int(Self.ffi_ring_pop(pipeline.ring, base, buf.count))
        }
    }

    /// Reduces one native chunk → 16k mono via the deterministic resampler
    /// and feeds the recognizer stream. Returns the number of frames fed at
    /// 16 kHz, or -1 on a real error.
    private func consumedIntoConverter(_ pipeline: AsrPipeline, poppedCount: Int) -> Int {
        guard poppedCount > 0 else { return 0 }

        // Zero-copy handoff through the preallocated resampled buffer.
        var fed = 0
        func feedChunk(pointer: UnsafePointer<Float>, count: Int) {
            // feedStream() itself records the fed count; converted mirrors it
            // at the 16 kHz boundary exactly once (no double counting).
            feedStream(pipeline, samples: UnsafeMutablePointer(mutating: pointer), count: count, sr: 16000)
            pipeline.framesConvertedTotal += count
            fed = count
        }

        if let resampler = pipeline.resampler {
            pipeline.nativeChunk.withUnsafeBufferPointer { chunkBuf in
                guard let base = chunkBuf.baseAddress else { return }
                pipeline.resampledChunk.withUnsafeMutableBufferPointer { outBuf in
                    let produced = resampler.process(chunkBuf, srcCount: poppedCount, out: outBuf)
                    if produced > 0, let outBase = outBuf.baseAddress {
                        feedChunk(pointer: outBase, count: produced)
                    }
                }
            }
        } else {
            // Tap already at 16 kHz: feed the popped chunk as-is.
            pipeline.nativeChunk.withUnsafeBufferPointer { chunkBuf in
                if let base = chunkBuf.baseAddress {
                    feedChunk(pointer: base, count: poppedCount)
                }
            }
        }

        let dropped = Self.ffi_ring_dropped_total(pipeline.ring)
        if dropped > pipeline.lastObservedDropped {
            print("[EcholetAsr] ring overflow drop detected delta=\(dropped - pipeline.lastObservedDropped)")
            pipeline.lastObservedDropped = dropped
        }
        return fed
    }

    private func feedStream(_ pipeline: AsrPipeline, samples: UnsafeMutablePointer<Float>, count: Int, sr: Int32) {
        guard count > 0 else { return }
        let rc = Self.ffi_stream_feed(pipeline.stream, sr, samples, Int32(count))
        if rc == 0 {
            pipeline.framesFedTotal += count
        } else {
            print("[EcholetAsr] stream_feed rejected rc=\(rc) count=\(count)")
            sessionDidFailLocked(pipeline, errorCode: "asr_feed_rejected")
        }
    }

    // MARK: - Session termination

    /// Drains remaining ring audio, flushes with trailing silence, reads the
    /// FINAL text, then destroys the stream. Single serial path; text is
    /// returned to the caller (nil on failure). The recognizer is kept loaded.
    func finalizeSession(
        sessionId: String,
        completion: @escaping (String?) -> Void
    ) {
        asrQueue.async { [weak self] in
            guard let self = self else { completion(nil); return }
            guard let pipeline = self.session, pipeline.sessionId == sessionId, !pipeline.ended else {
                print("[EcholetAsr] finalize fenced: no active pipeline match for id_prefix=\(sessionId.prefix(8))")
                completion(nil)
                return
            }
            let finalText = self.finalizePipelineLocked(pipeline)
            completion(finalText)
        }
    }

    /// Tears down the session without collecting a transcript (cancel,
    /// disarm, redesign replacement paths).
    func disposeSession(sessionId: String, reason: String) {
        asrQueue.async { [weak self] in
            guard let self = self else { return }
            guard let pipeline = self.session, pipeline.sessionId == sessionId else { return }
            self.disposePipelineLocked(pipeline, reason: reason)
        }
    }

    /// Dispose whatever session is active (no transcript).
    func disposeActiveSession(reason: String) {
        asrQueue.async { [weak self] in
            guard let self = self else { return }
            guard let pipeline = self.session else { return }
            self.disposePipelineLocked(pipeline, reason: reason)
        }
    }

    /// Must run on asrQueue. Returns the final transcript or nil.
    private func finalizePipelineLocked(_ pipeline: AsrPipeline) -> String? {
        pipeline.ended = true
        pipeline.pumpTimer?.cancel()
        pipeline.pumpTimer = nil

        // 1. Drain any remaining ring content (bounded).
        var guardCycles = 0
        let drainGuard = 200
        while guardCycles < drainGuard {
            let popped = popRingChunk(pipeline)
            if popped <= 0 { break }
            pipeline.framesPoppedTotal += Int(popped)
            let accepted = self.consumedIntoConverter(pipeline, poppedCount: Int(popped))
            if accepted < 0 {
                print("[EcholetAsr] finalize drain-conference failed; completing with partial text")
                if let text = self.readResultLocked(pipeline, flush: false) {
                    return text
                }
                return nil
            }
            guardCycles += 1
        }

        // 2. Trailing-silence flush → decoder emits buffered final features.
        guard let text = readResultLocked(pipeline, flush: true) else {
            Self.ffi_stream_destroy(pipeline.stream)
            self.session = nil
            return nil
        }

        // 3. Destroy the stream FIRST, keeping the recognizer handle.
        Self.ffi_stream_destroy(pipeline.stream)
        self.session = nil
        self.observability.finalizedAfterMs = nowMs() - self.observability.beganAtMs
        print("[EcholetAsr] session finalized id_prefix=\(pipeline.sessionId.prefix(8)) kind=\(pipeline.kind.rawValue) pump_cycles=\(pipeline.pumpCycleCount) native_popped=\(pipeline.framesPoppedTotal) converted_16k=\(pipeline.framesConvertedTotal) elapsed_ms=\(self.observability.finalizedAfterMs) (no PCM persisted)")
        if pipeline.kind == .warmKeyboard || pipeline.kind == .liveDictation {
            self.lastFinalTranscript = text
        }
        return text
    }

    private func readResultLocked(_ pipeline: AsrPipeline, flush: Bool) -> String? {
        let stream = pipeline.stream
        if flush {
            let silenceLen = 6400
            var silence = [Float](repeating: 0, count: silenceLen)
            silence.withUnsafeBufferPointer { buf in
                if let base = buf.baseAddress {
                    _ = Self.ffi_stream_feed(stream, 16000, base, Int32(silenceLen))
                }
            }
        }
        let required = flush
            ? Self.ffi_stream_flush(stream, nil, 0)
            : Self.ffi_stream_read(stream, nil, 0)
        if required < 0 {
            print("[EcholetAsr] result read failed (rc=\(required))")
            return nil
        }
        var buf = [CChar](repeating: 0, count: max(64, Int(required) + 1))
        let rc = flush
            ? Self.ffi_stream_flush(stream, &buf, Int32(buf.count))
            : Self.ffi_stream_read(stream, &buf, Int32(buf.count))
        guard rc >= 0 else {
            print("[EcholetAsr] result copy failed (rc=\(rc))")
            return nil
        }
        return String(cString: buf)
    }

    /// Must run on asrQueue. Destroys the stream; recognizer survives.
    private func disposePipelineLocked(_ pipeline: AsrPipeline, reason: String) {
        pipeline.ended = true
        pipeline.pumpTimer?.cancel()
        pipeline.pumpTimer = nil
        Self.ffi_stream_destroy(pipeline.stream)
        if self.session === pipeline {
            self.session = nil
        }
        print("[EcholetAsr] session disposed id_prefix=\(pipeline.sessionId.prefix(8)) reason=\(reason) (stream destroyed first, recognizer kept)")
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            self.delegate?.asrService(self, sessionDidDispose: pipeline.sessionId, reason: reason)
        }
    }

    private func sessionDidFailLocked(_ pipeline: AsrPipeline, errorCode: String) {
        guard !pipeline.ended else { return }
        pipeline.ended = true
        pipeline.pumpTimer?.cancel()
        pipeline.pumpTimer = nil
        Self.ffi_stream_destroy(pipeline.stream)
        if self.session === pipeline {
            self.session = nil
        }
        print("[EcholetAsr] session FAILED id_prefix=\(pipeline.sessionId.prefix(8)) code=\(errorCode)")
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            self.delegate?.asrService(self, sessionDidFail: pipeline.sessionId, errorCode: errorCode)
        }
    }

    private func mainSessionFailure(sessionId: String, errorCode: String) {
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            self.delegate?.asrService(self, sessionDidFail: sessionId, errorCode: errorCode)
        }
    }

    // MARK: - Deterministic synthetic-WAV regression (no mic)

    /// Feeds the given 16 kHz mono samples through a fresh stream of the
    /// SAME recognizer used for live sessions and returns the final
    /// transcript. Deterministic; no mic involved.
    func runPinnedWavRegression(
        samples16k: [Float],
        completion: @escaping (String?, String?) -> Void
    ) {
        asrQueue.async { [weak self] in
            guard let self = self else { completion(nil, "asr service unavailable"); return }
            guard self.modelState == .loaded, let recognizer = self.recognizer else {
                completion(nil, "asr_model_not_loaded")
                return
            }
            guard let stream = Self.ffi_stream_create(recognizer) else {
                completion(nil, "asr_stream_create_failed")
                return
            }
            // Feed chunked in bounded bundles; validate each C-call return.
            var fed = 0
            let chunkSize = 800  // 50 ms @ 16 kHz, same as the J4A probe
            samples16k.withUnsafeBufferPointer { buf in
                guard let base = buf.baseAddress else { return }
                while fed < samples16k.count {
                    let n = min(chunkSize, samples16k.count - fed)
                    let rc = Self.ffi_stream_feed(stream, 16000, base + fed, Int32(n))
                    if rc != 0 { break }
                    fed += n
                }
            }
            guard fed == samples16k.count, !samples16k.isEmpty else {
                Self.ffi_stream_destroy(stream)
                completion(nil, "asr_feed_rejected (\(fed)/\(samples16k.count))")
                return
            }
            let text = Self.finalTextForStream(stream)
            Self.ffi_stream_destroy(stream)
            print("[EcholetAsr] wav regression complete samples=\(fed) bytes=\(text?.utf8.count ?? 0)")
            completion(text, nil)
        }
    }

    private static func finalTextForStream(_ stream: UnsafeMutableRawPointer) -> String? {
        var silence = [Float](repeating: 0, count: 6400)
        _ = silence.withUnsafeBufferPointer { buf -> Bool in
            Self.ffi_stream_feed(stream, 16000, buf.baseAddress, 6400) == 0
        }
        let required = Self.ffi_stream_flush(stream, nil, 0)
        guard required >= 0 else { return nil }
        var buf = [CChar](repeating: 0, count: Int(required) + 1)
        let rc = Self.ffi_stream_flush(stream, &buf, Int32(buf.count))
        guard rc >= 0 else { return nil }
        return String(cString: buf)
    }
}

private func nowMs() -> UInt64 {
    return UInt64(DispatchTime.now().uptimeNanoseconds / 1_000_000)
}
