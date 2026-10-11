import Foundation

/// Deterministic block-average resampler (pure Foundation, J4-E2E).
///
/// Converts the native hardware capture rate (e.g. 48 kHz) to the recognizer
/// target rate (16 kHz) by averaging equal blocks of `factor` successive
/// samples (block mean). The transform is deterministic (same input always
/// yields the same output), reuses a small internal carry buffer for
/// incomplete trailing blocks across calls, and has no AVFoundation
/// dependency. Runs ONLY on the app-side ASR worker queue — never on the
/// realtime tap thread, and never inside the recognition FFI path.
public final class EcholetDeterministicResampler {

    /// Integer decimation factor (native rate / target rate). Required >= 1.
    public let factor: Int

    /// Leftover incomplete block samples carried to the next call.
    var carry: [Float] = []

    public init?(nativeSampleRate: Double, targetSampleRate: Double) {
        guard nativeSampleRate > 0, targetSampleRate > 0 else { return nil }
        let ratio = nativeSampleRate / targetSampleRate
        guard ratio >= 1.0, abs(ratio - ratio.rounded()) < 0.01 else { return nil }
        let factorInt = Int(ratio.rounded())
        guard factorInt >= 1, factorInt <= 32 else { return nil }
        self.factor = factorInt
        self.carry = []
    }

    /// Clears carry state (fresh session).
    public func reset() {
        carry.removeAll(keepingCapacity: true)
    }

    /// Consumes `srcCount` native-rate samples from `src`, appends
    /// block-averaged target-rate frames to `out`, returns the number of
    /// frames appended. Incomplete trailing blocks are carried internally
    /// into the following call. Expects the pipeline to drain responsibly;
    /// the carry never exceeds factor-1 samples between calls.
    public func process(
        _ src: UnsafeBufferPointer<Float>,
        srcCount: Int,
        out: UnsafeMutableBufferPointer<Float>
    ) -> Int {
        guard factor >= 1, let outBase = out.baseAddress, srcCount >= 0 else { return 0 }
        var written = 0
        var cursor = 0

        func appendBlock(sum: Float) {
            guard written < out.count else { return }
            outBase.advanced(by: written).initialize(to: sum / Float(factor))
            written += 1
        }

        // 1. Complete any pending carry block out of src.
        while !carry.isEmpty && carry.count < factor && cursor < srcCount {
            carry.append(src[cursor])
            cursor += 1
        }
        if carry.count == factor {
            appendBlock(sum: carry.reduce(0, +))
            carry.removeAll(keepingCapacity: true)
        }

        // 2. Stream whole blocks straight out of src.
        while cursor + factor <= srcCount {
            var blockSum: Float = 0
            for i in 0..<factor {
                blockSum += src[cursor + i]
            }
            cursor += factor
            appendBlock(sum: blockSum)
        }

        // 3. Carry the trailing partial block.
        if cursor < srcCount {
            carry.append(contentsOf: src[cursor..<srcCount])
        }
        return written
    }
}
