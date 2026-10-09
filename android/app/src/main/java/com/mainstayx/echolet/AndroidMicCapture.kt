package com.mainstayx.echolet

import android.annotation.SuppressLint
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder

/**
 * Phase 0-B microphone adapter: mono PCM16 16 kHz from VOICE_RECOGNITION
 * (the exact format the existing JNI Rust core consumes). No resampling, no
 * disk persistence, no foreground service: recording happens only while the
 * IME is visible and Listening. All methods must be called from the single
 * background capture lane only.
 */
class AndroidMicCapture {
    private var record: AudioRecord? = null

    companion object {
        const val SAMPLE_RATE = 16000
        const val CHUNK_SHORTS = 3200 // 200 ms at 16 kHz, mono
        const val MAX_FEED_SAMPLES = 32000 // JNI nativeFeed ceiling

        private fun fatal(buf: Int, why: String): Nothing =
            throw IllegalStateException("$why (minBufferSize=$buf)")
    }

    /**
     * Constructs and starts one AudioRecord instance. Throws
     * IllegalStateException for unsupported 16 kHz config or device refusal;
     * the caller maps that to BLOCKED/Stop. Recording starts only after
     * capture is armed by the controller.
     */
    @SuppressLint("MissingPermission")
    fun start() {
        val buf = AudioRecord.getMinBufferSize(
            SAMPLE_RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT
        )
        if (buf <= 0) fatal(buf, "unsupported 16 kHz mono PCM16 capture config")
        val bufferBytes = maxOf(buf, CHUNK_SHORTS * 2 * 4)
        val mic = AudioRecord(
            MediaRecorder.AudioSource.VOICE_RECOGNITION,
            SAMPLE_RATE,
            AudioFormat.CHANNEL_IN_MONO,
            AudioFormat.ENCODING_PCM_16BIT,
            bufferBytes,
        )
        if (mic.state != AudioRecord.STATE_INITIALIZED) {
            mic.release()
            throw IllegalStateException(
                "AudioRecord not initialized (state=${mic.state}); " +
                    "microphone unavailable or permission revoked"
            )
        }
        try {
            mic.startRecording()
        } catch (e: IllegalStateException) {
            mic.release()
            throw IllegalStateException("AudioRecord.startRecording failed: $e")
        }
        record = mic
    }

    /** One blocking read of ~CHUNK_SHORTS shorts; returns null once stopped. */
    fun readChunkShorts(): ShortArray? {
        val mic = record ?: return null
        val out = ShortArray(CHUNK_SHORTS)
        val n = try {
            mic.read(out, 0, out.size, AudioRecord.READ_BLOCKING)
        } catch (e: IllegalStateException) {
            throw IllegalStateException("AudioRecord.read failed: $e")
        }
        if (n < 0) {
            throw IllegalStateException("AudioRecord.read returned $n; capture is dead")
        }
        if (n != out.size) {
            // Short tail (stop()/drain or device hiccup): trim, keep finite set.
            if (n == 0) return null
            return out.copyOf(n)
        }
        return out
    }

    /**
     * Stops recording and releases the microphone EXACTLY once; safe to call
     * repeatedly and never throws, so lane cleanup stays idempotent.
     * `stop()` also unblocks any pending READ_BLOCKING call.
     */
    fun stopAndRelease() {
        val mic = record ?: return
        record = null
        try {
            mic.stop()
        } catch (_: IllegalStateException) {
            // already stopped; releasing below is still required
        }
        try {
            mic.release()
        } catch (_: Throwable) {
            // released or already released; nothing left to clean
        }
    }
}
