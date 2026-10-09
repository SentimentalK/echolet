package com.mainstayx.echolet

import android.annotation.SuppressLint
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder

/**
 * JVM-testable seam for one mic capture lane: the controller holds THIS
 * interface per-perlease-generation and may call start/read/stop from both
 * the background lane and the main thread. Implementations must make the
 * lifecycle itself thread-safe: publish-start atomically, stop at most once,
 * never hold a lock across the blocking read, and never throw across callers
 * beyond the documented start failures.
 */
interface MicCapture {
    /**
     * Constructs and starts one capture. Throws IllegalStateException for an
     * unsupported config or device refusal; the caller maps that to
     * BLOCKED/Stop. A start that was overtaken by [stopAndRelease] must not
     * leave a live recording behind.
     */
    fun start()

    /** One blocking read of ~CHUNK_SHORTS shorts; returns null once stopped. */
    fun readChunkShorts(): ShortArray?

    /** Stops and releases EXACTLY once; safe from any thread, never throws. */
    fun stopAndRelease()
}

/**
 * Phase 0-B microphone adapter: mono PCM16 16 kHz from VOICE_RECOGNITION
 * (the exact format the existing JNI Rust core consumes). No resampling, no
 * disk persistence, no network: recording happens only while the IME is
 * visible and Listening. Lifecycle is guarded by a SHORT critical section:
 * stop/switch between lane and main threads is safe, and the lock is never
 * held across the blocking read or the OS stop/release calls.
 */
class AndroidMicCapture : MicCapture {
    private val lock = Any()
    private var record: AudioRecord? = null

    companion object {
        const val SAMPLE_RATE = 16000
        const val CHUNK_SHORTS = 3200 // 200 ms at 16 kHz, mono
        const val MAX_FEED_SAMPLES = 32000 // JNI nativeFeed ceiling

        private fun fatal(buf: Int, why: String): Nothing =
            throw IllegalStateException("$why (minBufferSize=$buf)")
    }

    /** Atomically publishes the live record under the short lock. */
    private fun publish(mic: AudioRecord): Unit = synchronized(lock) {
        record = mic
    }

    /**
     * Constructs and starts one AudioRecord instance, then publishes it
     * atomically. Throws IllegalStateException for an unsupported 16 kHz
     * config or device refusal (device released defensively first); the
     * caller maps the failure to BLOCKED/Stop.
     */
    @SuppressLint("MissingPermission")
    override fun start() {
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
        // Publish BEFORE startRecording so a racing stopAndRelease can see
        // (and stop) the instance it is about to own.
        publish(mic)
        try {
            mic.startRecording()
        } catch (e: IllegalStateException) {
            // Overtaken by stopAndRelease, or device refusal: release the
            // instance we hold; stopAndRelease remains a safe no-op if it
            // already consumed this same instance (extracted the pointer).
            try {
                mic.release()
            } catch (_: Throwable) {
            }
            throw IllegalStateException("AudioRecord.startRecording failed: $e")
        }
    }

    /** One blocking read of ~CHUNK_SHORTS shorts; returns null once stopped. */
    override fun readChunkShorts(): ShortArray? {
        val mic = synchronized(lock) { record } ?: return null
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
     * repeatedly from any thread and never throws, so lane cleanup, main
     * thread and lifecycle paths stay idempotent. `stop()` also unblocks any
     * pending READ_BLOCKING call without holding the lock through it.
     */
    override fun stopAndRelease() {
        val extracted: AudioRecord? = synchronized(lock) {
            val mic = record
            record = null
            mic
        }
        if (extracted == null) return
        try {
            extracted.stop()
        } catch (_: IllegalStateException) {
            // already stopped; releasing below is still required
        }
        try {
            extracted.release()
        } catch (_: Throwable) {
            // released or already released; nothing left to clean
        }
    }
}
