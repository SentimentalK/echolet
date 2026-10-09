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
     * Constructs and starts one capture. Throws for an unsupported config,
     * device refusal or a revoked/failed OS start (any realistic unchecked
     * failure, e.g. IllegalStateException, SecurityException, RuntimeException);
     * the caller maps that to BLOCKED/Stop. A start that was overtaken by
     * [stopAndRelease] must not leave a live recording behind.
     */
    fun start()

    /** One blocking read of ~CHUNK_SHORTS shorts; returns null once stopped. */
    fun readChunkShorts(): ShortArray?

    /** Stops and releases EXACTLY once; safe from any thread, never throws. */
    fun stopAndRelease()
}

/**
 * Narrow JVM-testable port over the OS recorder used by [AndroidMicCapture]:
 * exactly the operations the adapter needs, so the lifecycle state machine
 * (Design 3) is exercised in plain-JUnit tests against a fake port instead
 * of the device-only `AudioRecord`.
 */
interface MicOsRecord {
    fun startRecording()
    /** One blocking read; returns the consumed count, negative on error. */
    fun read(out: ShortArray): Int
    fun halt()
    fun free()
}

/** Builds the OS recorder; throws when the config/device is unusable. */
fun interface MicOsRecordFactory {
    fun open(): MicOsRecord
}

/** Production port: exact pinned Phase 0-B capture config, unchanged. */
object RealMicOsRecordFactory : MicOsRecordFactory {
    @SuppressLint("MissingPermission")
    override fun open(): MicOsRecord {
        val buf = AudioRecord.getMinBufferSize(
            AndroidMicCapture.SAMPLE_RATE,
            AudioFormat.CHANNEL_IN_MONO,
            AudioFormat.ENCODING_PCM_16BIT,
        )
        if (buf <= 0) {
            throw IllegalStateException(
                "unsupported 16 kHz mono PCM16 capture config (minBufferSize=$buf)"
            )
        }
        val bufferBytes = maxOf(buf, AndroidMicCapture.CHUNK_SHORTS * 2 * 4)
        val mic = AudioRecord(
            MediaRecorder.AudioSource.VOICE_RECOGNITION,
            AndroidMicCapture.SAMPLE_RATE,
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
        return object : MicOsRecord {
            override fun startRecording() = mic.startRecording()
            override fun read(out: ShortArray): Int =
                mic.read(out, 0, out.size, AudioRecord.READ_BLOCKING)

            override fun halt() {
                try {
                    mic.stop()
                } catch (_: IllegalStateException) {
                    // already stopped; free() below is still required
                }
            }

            override fun free() {
                try {
                    mic.release()
                } catch (_: Throwable) {
                    // released or already released; nothing left to clean
                }
            }
        }
    }
}

/**
 * Phase 0-B microphone adapter: mono PCM16 16 kHz from VOICE_RECOGNITION
 * (the exact format the existing JNI Rust core consumes). No resampling, no
 * disk persistence, no network: recording happens only while the IME is
 * visible and Listening.
 *
 * Explicit per-capture lifecycle (Design 3): every instance walks
 * FRESH -> STARTING -> ACTIVE -> CLOSED with a stop fence raised BEFORE the
 * OS recorder is even built, so a Stop racing a still-unpublished start
 * leaves no live recording. The OS teardown handshake between a racing stop
 * thread and an in-flight start thread is deterministic: Stop during
 * STARTING defers teardown to the START thread, which re-checks the fence
 * right after startRecording returns; the stop thread never halts/frees an
 * instance whose Android startRecording may still execute. Freeing is
 * exactly-once and the instance then RESETS to FRESH, so the next lease gets
 * a clean, un-poisoned capture. Locks are only held across short state
 * flips, never across the OS build/start/halt/free or the blocking read.
 */
class AndroidMicCapture(
    private val factory: MicOsRecordFactory = RealMicOsRecordFactory,
) : MicCapture {
    private enum class Phase { FRESH, STARTING, ACTIVE, CLOSED }

    private val lock = Any()
    private var phase = Phase.FRESH
    /** Stop-and-release requested for THIS capture instance (the fence). */
    @Volatile private var stopRequested = false
    /** The OS recorder handed out to a reader; null once a stop consumed it. */
    private var record: MicOsRecord? = null
    /** The recorder whose stop/free an in-flight start thread still owes. */
    private var owedTeardown: MicOsRecord? = null

    companion object {
        const val SAMPLE_RATE = 16000
        const val CHUNK_SHORTS = 3200 // 200 ms at 16 kHz, mono
        const val MAX_FEED_SAMPLES = 32000 // JNI nativeFeed ceiling

        /** Bounded wait for an in-flight teardown to settle, then BLOCKED. */
        const val CLOSE_SETTLE_TIMEOUT_MILLIS = 5_000L
        private const val CLOSE_SETTLE_TIMEOUT_NANOS =
            CLOSE_SETTLE_TIMEOUT_MILLIS * 1_000_000
        private const val CLOSE_SETTLE_STEP_NANOS = 50_000_000L
    }

    override fun start() {
        synchronized(lock) {
            // A CLOSED phase here means the PREVIOUS lease's teardown is
            // still settling (marked CLOSED before its halt/free finishes).
            // That is a transient "closing" state, not a permanent mic
            // failure: wait bounded for the settle reset to FRESH, so a fast
            // valid Stop->Start never spuriously errors. If the underlying
            // OS teardown hangs, the bounded wait expires and the caller
            // surfaces BLOCKED instead of starting over an unsettled or
            //possibly live recorder.
            var remaining = CLOSE_SETTLE_TIMEOUT_NANOS
            while (phase == Phase.CLOSED && remaining > 0) {
                val step = CLOSE_SETTLE_STEP_NANOS
                try {
                    (lock as java.lang.Object).wait(step, (step % 1_000_000).toInt())
                } catch (_: InterruptedException) {
                    Thread.currentThread().interrupt()
                }
                remaining -= step
            }
            if (phase == Phase.CLOSED) {
                // Teardown did not settle in time: do NOT start a second
                // recorder over a possibly still-live or half-released one.
                throw IllegalStateException(
                    "mic capture still closing; previous teardown unsettled"
                )
            }
            if (phase != Phase.FRESH) {
                throw IllegalStateException(
                    "mic capture reused without a full stop (phase=$phase)"
                )
            }
            if (stopRequested) {
                // Stop beat start before anything OS-side: nothing to build,
                // nothing to start, nothing to leak.
                phase = Phase.CLOSED
                resetIfSettledLocked()
                return
            }
            phase = Phase.STARTING
        }
        val os = try {
            factory.open()
        } catch (t: Throwable) {
            synchronized(lock) {
                phase = Phase.CLOSED
                resetIfSettledLocked()
            }
            throw t
        }
        val lost = synchronized(lock) {
            if (stopRequested) {
                // Canceled before publish: WE free without starting.
                phase = Phase.CLOSED
                owedTeardown = os
                true
            } else {
                // Publish BEFORE startRecording so a racing stop sees the
                // instance it would otherwise miss.
                record = os
                false
            }
        }
        if (lost) {
            finishTeardown(os)
            return // canceled: no surviving recording, no error surfaced
        }
        try {
            os.startRecording()
        } catch (e: Throwable) {
            // EVERY realistic Android start failure (SecurityException when
            // RECORD_AUDIO was revoked, IllegalStateException, any runtime
            // glitch) must release the recorder EXACTLY ONCE and reset the
            // instance, then RETHROW the original failure unswallowed: the
            // caller maps the type/message to BLOCKED and never sees a
            // half-configured capture.
            synchronized(lock) {
                record = null
                phase = Phase.CLOSED
                owedTeardown = os
            }
            finishTeardown(os)
            throw e
        }
        val orphan = synchronized(lock) {
            if (stopRequested) {
                // Stop ran while startRecording was IN FLIGHT: the stop
                // thread deferred, so THIS start thread owns the teardown
                // and reports the capture ended, never Listening.
                record = null
                phase = Phase.CLOSED
                owedTeardown = os
                os
            } else {
                phase = Phase.ACTIVE
                null
            }
        }
        if (orphan != null) finishTeardown(orphan)
    }

    /** One blocking read of ~CHUNK_SHORTS shorts; returns null once stopped. */
    override fun readChunkShorts(): ShortArray? {
        val os = synchronized(lock) {
            if (phase != Phase.ACTIVE) return null
            record
        } ?: return null
        val out = ShortArray(CHUNK_SHORTS)
        val n = try {
            os.read(out)
        } catch (e: IllegalStateException) {
            // A read failure caused by our OWN stop (release'd mid-read) is
            // NOT an unexpected error: report it as the stopped adapter.
            if (synchronized(lock) { record !== os }) return null
            throw IllegalStateException("AudioRecord.read failed: $e")
        }
        if (n < 0) {
            if (synchronized(lock) { record !== os }) return null
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
     * thread and lifecycle paths stay idempotent. A pending blocking read is
     * unblocked without holding the lock through it. When startRecording may
     * still be IN FLIGHT on another thread, THIS thread only raises the stop
     * fence and that start thread performs the OS teardown (Design 3
     * handshake). Once fully settled the instance resets to FRESH, so the
     * next lease starts an un-canceled, un-poisoned capture.
     */
    override fun stopAndRelease() {
        val mine: MicOsRecord?
        synchronized(lock) {
            stopRequested = true
            when (phase) {
                Phase.STARTING -> return // start thread owns OS teardown now
                Phase.CLOSED -> return   // handled exactly-once elsewhere
                Phase.FRESH -> {
                    phase = Phase.CLOSED
                    record = null
                    mine = null
                }
                Phase.ACTIVE -> {
                    mine = record
                    record = null
                    phase = Phase.CLOSED
                }
            }
        }
        if (mine != null) {
            finishTeardown(mine)
        } else {
            synchronized(lock) { resetIfSettledLocked() } // FRESH stop: reset
        }
    }

    // ------------------------------------------------------------- internals

    /**
     * Halts and frees the OS recorder once, then clears any owed teardown so
     * the settle reset can mark this instance FRESH for the next lease.
     */
    private fun finishTeardown(os: MicOsRecord) {
        try {
            os.halt()
        } catch (_: Throwable) {
        }
        try {
            os.free()
        } catch (_: Throwable) {
        }
        synchronized(lock) {
            if (owedTeardown === os) owedTeardown = null
            resetIfSettledLocked()
        }
    }

    private fun resetIfSettledLocked() {
        if (record == null && owedTeardown == null && phase == Phase.CLOSED) {
            phase = Phase.FRESH
            stopRequested = false
            // Wake any start() waiting in the closing-settlement gate so the
            // next lease may begin the moment the OS recorder is truly free.
            (lock as java.lang.Object).notifyAll()
        }
    }
}
