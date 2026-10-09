package com.mainstayx.echolet

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/**
 * Plain-JVM tests of the REAL [AndroidMicCapture] lifecycle state machine
 * (Design 3) against an injected fake OS recorder port: the stop fence beats
 * construction, an in-flight startRecording is torn down exactly once by the
 * start thread, an OS instance is never freed twice, a failed start releases
 * and resets, a stop racing a blocking read is NOT reclassified as an
 * unexpected read error, and a fully closed instance is restarted fresh.
 * Device-only AudioRecord behavior beyond this port (real buffer math, HAL
 * latency) remains an UNVERIFIED hardware-only gate.
 */
class AndroidMicCaptureLifecycleTest {

    private class FakePort : MicOsRecord {
        val startCount = AtomicInteger()
        val haltCount = AtomicInteger()
        val freeCount = AtomicInteger()
        val readCount = AtomicInteger()

        /** Parks INSIDE startRecording to emulate the Android call in flight. */
        @Volatile var startGate: CountDownLatch? = null
        val startEntered = CountDownLatch(1)

        /** Parks INSIDE read so a stop can tear down while the read blocks. */
        @Volatile var readGate: CountDownLatch? = null
        val readEntered = CountDownLatch(1)

        /** When non-null, read throws this (emulating a released recorder). */
        @Volatile var readFailure: IllegalStateException? = null

        override fun startRecording() {
            startEntered.countDown()
            startGate?.await()
            startCount.incrementAndGet()
        }

        override fun read(out: ShortArray): Int {
            readEntered.countDown()
            readGate?.await()
            readCount.incrementAndGet()
            readFailure?.let { throw it }
            return out.size
        }

        override fun halt() {
            haltCount.incrementAndGet()
        }

        override fun free() {
            freeCount.incrementAndGet()
        }
    }

    private class RecordingFactory : MicOsRecordFactory {
        val built = mutableListOf<FakePort>()
        private val lock = Any()

        /** Parks INSIDE open() to emulate OS construction still in flight. */
        @Volatile var openGate: CountDownLatch? = null
        val openEntered = CountDownLatch(1)

        /** Installed on every built port: parks its startRecording call. */
        @Volatile var pendingStartGate: CountDownLatch? = null

        @Volatile var readGateForNextPort: CountDownLatch? = null

        override fun open(): MicOsRecord {
            openEntered.countDown()
            openGate?.await()
            synchronized(lock) {
                val port = FakePort().apply {
                    startGate = pendingStartGate
                    readGate = readGateForNextPort
                }
                built.add(port)
                return port
            }
        }
    }

    private fun awaitPort(factory: RecordingFactory): FakePort {
        val deadline = System.currentTimeMillis() + 2000
        while (System.currentTimeMillis() < deadline) {
            synchronized(factory) { factory.built.firstOrNull() }?.let { return it }
            Thread.sleep(5)
        }
        throw AssertionError("OS recorder never built")
    }

    private fun awaitFreeCount(port: FakePort, expected: Int) {
        val deadline = System.currentTimeMillis() + 2000
        while (port.freeCount.get() < expected && System.currentTimeMillis() < deadline) {
            Thread.sleep(5)
        }
        assertEquals(expected, port.freeCount.get())
    }

    private fun startInThread(capture: AndroidMicCapture): Thread =
        Thread({ capture.start() }, "capture-start").apply {
            isDaemon = true
            start()
        }

    @Test
    fun active_capture_halts_and_frees_exactly_once_then_resets_fresh() {
        val factory = RecordingFactory()
        val capture = AndroidMicCapture(factory)
        capture.start()
        val port = awaitPort(factory)
        assertEquals(1, port.startCount.get())

        capture.stopAndRelease()
        capture.stopAndRelease() // repeated stop: an OS instance frees ONCE
        assertEquals(1, port.haltCount.get())
        assertEquals(1, port.freeCount.get())
        assertNull(capture.readChunkShorts()) // once stopped, reads yield null

        // The NEXT lease obtains a fresh, un-poisoned capture (no lingering
        // cancel fence, no poisoned STOP_REQUESTED gate).
        capture.start()
        val second = awaitPort(factory)
        assertEquals(2, factory.built.size)
        assertEquals(1, second.startCount.get())
        capture.stopAndRelease()
        assertEquals(1, second.haltCount.get())
        assertEquals(1, second.freeCount.get())
    }

    @Test
    fun stop_before_any_start_is_a_noop_and_next_start_is_clean() {
        val factory = RecordingFactory()
        val capture = AndroidMicCapture(factory)
        capture.stopAndRelease() // stop with nothing started: nothing leaked
        assertEquals(0, factory.built.size)
        assertNull(capture.readChunkShorts())

        // The fully settled instance resets fresh: the next start constructs
        // a clean recorder with a cleared cancel fence (not poisoned).
        capture.start()
        val port = awaitPort(factory)
        assertEquals(1, factory.built.size)
        assertEquals(1, port.startCount.get())
        capture.stopAndRelease()
        assertEquals(1, port.haltCount.get())
        assertEquals(1, port.freeCount.get())
    }

    @Test
    fun stop_while_start_recording_in_flight_tears_down_exactly_once() {
        val factory = RecordingFactory()
        val inFlight = CountDownLatch(1)
        factory.pendingStartGate = inFlight
        val capture = AndroidMicCapture(factory)
        val starter = startInThread(capture)
        val port = awaitPort(factory)
        assertTrue(port.startEntered.await(2, TimeUnit.SECONDS))
        assertEquals(0, port.startCount.get()) // startRecording IS in flight

        // Stop while the Android startRecording call is executing: the stop
        // thread must NOT free an instance another thread may still start;
        // teardown is transferred to the start thread, exactly once, and the
        // capture is NEVER exposed as surviving/started.
        capture.stopAndRelease()
        assertEquals(0, port.haltCount.get()) // stop deferred, not released early
        inFlight.countDown()

        starter.join(2000)
        assertTrue(!starter.isAlive)
        assertEquals(1, port.startCount.get()) // the single in-flight start
        assertEquals(1, port.haltCount.get())
        awaitFreeCount(port, 1)
        capture.stopAndRelease() // repeated stop: still exactly once
        awaitFreeCount(port, 1)
        assertNull(capture.readChunkShorts()) // no post-stop live capture
    }

    @Test
    fun stop_during_unpublished_construction_frees_without_starting() {
        val factory = RecordingFactory()
        val capture = AndroidMicCapture(factory)
        val building = CountDownLatch(1)
        factory.openGate = building
        val starter = startInThread(capture)
        assertTrue(factory.openEntered.await(2, TimeUnit.SECONDS))
        assertEquals(0, factory.built.size) // construction not yet returned

        capture.stopAndRelease() // the fence beats the in-flight build
        building.countDown()
        starter.join(2000)
        assertTrue(!starter.isAlive)

        val port = awaitPort(factory)
        assertEquals(0, port.startCount.get()) // NEVER started
        assertEquals(1, port.haltCount.get())
        assertEquals(1, port.freeCount.get()) // freed exactly once by the starter
        assertNull(capture.readChunkShorts())
    }

    @Test
    fun start_recording_failure_releases_once_and_resets() {
        val halted = AtomicInteger()
        val freed = AtomicInteger()
        val failingFactory = MicOsRecordFactory {
            object : MicOsRecord {
                override fun startRecording(): Nothing =
                    throw IllegalStateException("device refuses recording")

                override fun read(out: ShortArray): Int = out.size
                override fun halt() {
                    halted.incrementAndGet()
                }

                override fun free() {
                    freed.incrementAndGet()
                }
            }
        }
        val failing = AndroidMicCapture(failingFactory)
        val thrown = try {
            failing.start()
            null
        } catch (e: IllegalStateException) {
            e
        }
        assertNotNull(thrown)
        assertEquals(1, halted.get())
        assertEquals(1, freed.get()) // exactly once despite the throw

        // Reset: a following lease on the SAME instance can start cleanly.
        val factory = RecordingFactory()
        val next = AndroidMicCapture(factory)
        next.start()
        val port = awaitPort(factory)
        assertEquals(1, port.startCount.get())
        next.stopAndRelease()
        assertEquals(1, port.freeCount.get())
    }

    @Test
    fun own_stop_racing_a_blocking_read_is_not_an_error() {
        val factory = RecordingFactory()
        val blocked = CountDownLatch(1)
        factory.readGateForNextPort = blocked
        val capture = AndroidMicCapture(factory)
        capture.start()
        val port = awaitPort(factory)

        val results = arrayOf<ShortArray?>(null as ShortArray?)
        val fault = arrayOf<Throwable?>(null)
        val reader = Thread({
            try {
                results[0] = capture.readChunkShorts()
            } catch (t: Throwable) {
                fault[0] = t
            }
        }, "capture-read").apply {
            isDaemon = true
            start()
        }
        assertTrue(port.readEntered.await(2, TimeUnit.SECONDS)) // read is blocked

        // Stop while the blocking READ is in flight; the freed recorder's
        // read resolves with the released-recorder failure, and the adapter
        // must report it QUIETLY (null), never as an unexpected read error.
        port.readFailure = IllegalStateException("already released")
        capture.stopAndRelease()
        blocked.countDown()
        reader.join(2000)
        assertTrue(!reader.isAlive)
        assertNull(fault[0]) // a read failure caused by our OWN stop is not an error
        assertNull(results[0])
        assertEquals(1, port.haltCount.get())
        assertEquals(1, port.freeCount.get())
    }
}
