package com.mainstayx.echolet

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong

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

        /** Parks INSIDE halt to emulate an OS teardown in flight (Design C). */
        @Volatile var haltGate: CountDownLatch? = null
        val haltEntered = CountDownLatch(1)

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
            haltEntered.countDown()
            haltGate?.await()
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

    private fun awaitPort(factory: RecordingFactory, index: Int = 0): FakePort {
        val deadline = System.currentTimeMillis() + 2000
        while (System.currentTimeMillis() < deadline) {
            synchronized(factory) {
                if (factory.built.size > index) return factory.built[index]
            }
            Thread.sleep(5)
        }
        throw AssertionError("OS recorder #$index never built")
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
        val second = awaitPort(factory, index = 1)
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

    /**
     * Design B(4): EVERY unchecked start failure — not only
     * IllegalStateException, but a revoked-permission SecurityException or
     * any RuntimeException — must free the recorder EXACTLY ONCE, start
     * NOTHING, never record, and reset the instance so a subsequent
     * independent start on the SAME capture works. The underlying failure is
     * rethrown unswallowed (not converted into a generic wrapper).
     */
    @Test
    fun any_unchecked_start_failure_releases_once_and_restarts_clean() {
        for (failure in listOf(
            SecurityException("RECORD_AUDIO permission revoked"),
            RuntimeException("HAL recorder glitch"),
        )) {
            val halted = AtomicInteger()
            val freed = AtomicInteger()
            val started = AtomicInteger()
            val startsRemaining = AtomicInteger(1)
            val factory = MicOsRecordFactory {
                if (startsRemaining.getAndDecrement() > 0) {
                    object : MicOsRecord {
                        override fun startRecording(): Nothing = throw failure
                        override fun read(out: ShortArray): Int = out.size
                        override fun halt() {
                            halted.incrementAndGet()
                        }

                        override fun free() {
                            freed.incrementAndGet()
                        }
                    }
                } else {
                    object : MicOsRecord {
                        override fun startRecording() {
                            started.incrementAndGet()
                        }

                        override fun read(out: ShortArray): Int = out.size
                        override fun halt() {
                            halted.incrementAndGet()
                        }

                        override fun free() {
                            freed.incrementAndGet()
                        }
                    }
                }
            }
            val capture = AndroidMicCapture(factory)
            val thrown = try {
                capture.start()
                null
            } catch (t: Throwable) {
                t
            }
            assertEquals(failure, thrown) // underlying failure NOT swallowed
            assertEquals(0, started.get()) // NOTHING ever recorded
            assertEquals(1, halted.get()) // teardown exactly once
            assertEquals(1, freed.get())

            // The reset instance takes a clean subsequent start.
            capture.start()
            assertEquals(1, started.get())
            capture.stopAndRelease()
            assertEquals(2, halted.get())
            assertEquals(2, freed.get())
            assertNull(capture.readChunkShorts())
        }
    }

    /**
     * Design C(5): with halt/free deliberately BLOCKED (teardown in flight),
     * a fast immediate Start request must WAIT for settlement on the same
     * lane — it must never build a concurrent OS recorder, never report the
     * unrelated permanent "reused" failure while the close is merely in
     * progress, and it starts cleanly once the prior recorder is released.
     */
    @Test
    fun start_during_ongoing_close_waits_for_settlement_then_starts_clean() {
        val factory = RecordingFactory()
        val capture = AndroidMicCapture(factory)
        capture.start()
        val port = awaitPort(factory)
        assertEquals(1, port.startCount.get())
        port.haltGate = CountDownLatch(1)

        val stopErrors = arrayOf<Throwable?>(null)
        val stopper = Thread({
            try {
                capture.stopAndRelease()
            } catch (t: Throwable) {
                stopErrors[0] = t
            }
        }, "capture-stop")
        stopper.isDaemon = true
        stopper.start()
        assertTrue(port.haltEntered.await(2, TimeUnit.SECONDS)) // halt IS blocked

        val startErrors = arrayOf<Throwable?>(null)
        val starter = Thread({
            try {
                capture.start()
            } catch (t: Throwable) {
                startErrors[0] = t
            }
        }, "capture-start-during-close")
        starter.isDaemon = true
        starter.start()

        // While the prior halt is parked: the deferred start must sit in the
        // closing-settlement wait (deterministic thread state proof) and
        // must NOT build a second concurrent OS recorder.
        val deadline = System.currentTimeMillis() + 2000
        while (!((starter.state == Thread.State.TIMED_WAITING ||
                starter.state == Thread.State.WAITING) &&
                synchronized(factory) { factory.built.size == 1 })
        ) {
            if (System.currentTimeMillis() > deadline) {
                throw AssertionError(
                    "deferred start did not wait for settlement: " +
                        "state=${starter.state} built=${factory.built.size}"
                )
            }
            assertFalse(startErrors[0].let { it != null })
            Thread.sleep(5)
        }
        port.haltGate?.countDown() // release the parked halt
        port.haltGate = null

        // Release the teardown: settlement must un-block the deferred start
        // naturally (no BLOCKED error, no concurrent recorder).
        stopper.join(2000)
        assertTrue(!stopper.isAlive)
        assertNull(stopErrors[0])
        starter.join(2000)
        assertTrue(!starter.isAlive)
        assertNull(startErrors[0])
        assertEquals(1, port.haltCount.get())
        assertEquals(1, port.freeCount.get()) // first recorder freed ONCE

        val second = awaitPort(factory, index = 1)
        assertEquals(1, second.startCount.get()) // started cleanly after settle
        capture.stopAndRelease()
        assertEquals(1, second.haltCount.get())
        assertEquals(1, second.freeCount.get())
        assertNull(capture.readChunkShorts())
    }

    /**
     * Design E(8) hung-teardown regression: with the PREVIOUS recorder's
     * halt parked FOREVER (no notifier can fire), a fresh start must still
     * fail closed at the BOUNDED settlement deadline — the actionable
     * unsettled-close error around the deadline (monotonic, unit-correct
     * Object.wait), never 13.9-hour nanoseconds-as-milliseconds-style hangs,
     * never a second concurrent OS recorder, and once the parked halt is
     * finally released the SAME instance recovers to a full clean cycle.
     */
    @Test
    fun start_during_unsettled_close_times_out_fail_closed_without_second_mic() {
        val factory = RecordingFactory()
        val settleTimeoutMillis = 150L
        val capture = AndroidMicCapture(factory, closeSettleTimeoutMillis = settleTimeoutMillis)
        capture.start()
        val port = awaitPort(factory, index = 0)
        assertEquals(1, port.startCount.get())

        // A is ACTIVE; park the OS teardown INSIDE halt, so after Stop A is
        // CLOSED but NOT yet freed and nothing can notify a settling start.
        port.haltGate = CountDownLatch(1)
        val stopErrors = arrayOf<Throwable?>(null)
        val stopper = Thread({
            try {
                capture.stopAndRelease()
            } catch (t: Throwable) {
                stopErrors[0] = t
            }
        }, "capture-stop-hung-teardown").apply {
            isDaemon = true
            start()
        }
        assertTrue(port.haltEntered.await(2, TimeUnit.SECONDS)) // halt IS parked

        val startErrors = arrayOf<Throwable?>(null)
        val startElapsed = AtomicLong(-1)
        val starter = Thread({
            val began = System.nanoTime()
            try {
                capture.start()
            } catch (t: Throwable) {
                startErrors[0] = t
            } finally {
                startElapsed.set((System.nanoTime() - began) / 1_000_000)
            }
        }, "capture-start-vs-hung-teardown").apply {
            isDaemon = true
            start()
        }

        try {
            // Deterministic parking proof: the deferred start sits inside
            // the settlement wait and NEVER builds a concurrent recorder.
            val parkedMark = System.currentTimeMillis() + 2000
            while (!((starter.state == Thread.State.TIMED_WAITING ||
                    starter.state == Thread.State.WAITING) &&
                    synchronized(factory) { factory.built.size == 1 })
            ) {
                if (System.currentTimeMillis() > parkedMark) {
                    throw AssertionError(
                        "deferred start did not park in the settlement wait: " +
                            "state=${starter.state} built=${factory.built.size}"
                    )
                }
                assertNull(startErrors[0]) // not prematurely failed while parked
                Thread.sleep(5)
            }

            starter.join(2000) // prints baseline hang instead of sleeping 13.9 h
            assertTrue("start thread still hung past bounded join", !starter.isAlive)
            val unsettled = startErrors[0]
            assertTrue(
                "expected the unsettled-close failure, got $unsettled",
                unsettled is IllegalStateException
            )
            assertTrue(
                "unsettled-close failure must stay actionable: $unsettled",
                unsettled?.message?.contains("still closing") == true
            )
            // KEY bounded-deadline assertion: measured MONOTONIC elapsed time
            // must be AT the deadline (not early, not unbounded).
            val elapsedMillis = startElapsed.get()
            assertTrue(
                "start released BEFORE the bounded deadline: $elapsedMillis ms",
                elapsedMillis >= 100
            )
            assertTrue(
                "start ignored the bounded deadline: $elapsedMillis ms",
                elapsedMillis <= 2000
            )
        } finally {
            port.haltGate?.countDown() // ALWAYS unblock the parked halt
        }

        stopper.join(2000)
        assertTrue(!stopper.isAlive)
        assertNull(stopErrors[0])
        assertEquals(1, factory.built.size) // B built NO concurrent recorder
        assertEquals(1, port.startCount.get()) // B never started
        awaitFreeCount(port, 1) // A freed EXACTLY once after its halt unblocked

        // Post-settle recovery: a LATER fresh start succeeds on the SAME
        // instance at the exact expected port index.
        capture.start()
        val next = awaitPort(factory, index = 1)
        assertEquals(2, factory.built.size)
        assertEquals(1, next.startCount.get())
        capture.stopAndRelease()
        assertEquals(1, next.haltCount.get())
        assertEquals(1, next.freeCount.get())
        assertNull(capture.readChunkShorts())
    }

    /**
     * Design E(9) optional belt: an interrupt arriving during the CLOSED
     * settlement wait (BEFORE any deadline) must fail closed PROMPTLY
     * through the actionable unsettled-close exception (not a busy
     * InterruptedException loop), PRESERVE the interrupted status, build no
     * second recorder, and leave the instance recoverable once the hovered
     * teardown settles.
     */
    @Test
    fun interrupted_during_unsettled_close_wait_fails_closed_without_second_mic() {
        val factory = RecordingFactory()
        val capture = AndroidMicCapture(factory) // production 5 s deadline
        capture.start()
        val port = awaitPort(factory, index = 0)
        assertEquals(1, port.startCount.get())

        port.haltGate = CountDownLatch(1) // teardown parks INSIDE halt
        val stopErrors = arrayOf<Throwable?>(null)
        val stopper = Thread({
            try {
                capture.stopAndRelease()
            } catch (t: Throwable) {
                stopErrors[0] = t
            }
        }, "capture-stop-interrupted-wait").apply {
            isDaemon = true
            start()
        }
        assertTrue(port.haltEntered.await(2, TimeUnit.SECONDS)) // halt IS parked

        val startErrors = arrayOf<Throwable?>(null)
        val interruptedOnExit = AtomicBoolean(false)
        val starter = Thread({
            try {
                capture.start()
            } catch (t: Throwable) {
                startErrors[0] = t
            } finally {
                interruptedOnExit.set(Thread.currentThread().isInterrupted)
            }
        }, "capture-start-interrupted-wait").apply {
            isDaemon = true
            start()
        }

        try {
            val parkedMark = System.currentTimeMillis() + 2000
            while (!((starter.state == Thread.State.TIMED_WAITING ||
                    starter.state == Thread.State.WAITING) &&
                    synchronized(factory) { factory.built.size == 1 })
            ) {
                if (System.currentTimeMillis() > parkedMark) {
                    throw AssertionError(
                        "deferred start did not park in the settlement wait: " +
                            "state=${starter.state} built=${factory.built.size}"
                    )
                }
                assertNull(startErrors[0])
                Thread.sleep(5)
            }

            starter.interrupt() // arrives BEFORE the 5 s deadline
            starter.join(1000) // must fail closed PROMPTLY, not at 5 s
            assertTrue("interrupted start thread still hung", !starter.isAlive)
            val unsettled = startErrors[0]
            assertTrue(
                "expected a fail-closed exception, got $unsettled",
                unsettled is IllegalStateException
            )
            assertTrue(
                "interrupt status must be preserved on exit",
                interruptedOnExit.get()
            )
            // No concurrent recorder was built by the interrupted start.
            assertEquals(1, synchronized(factory) { factory.built.size })
        } finally {
            port.haltGate?.countDown() // ALWAYS unblock the parked halt
        }

        stopper.join(2000)
        assertTrue(!stopper.isAlive)
        assertNull(stopErrors[0])
        awaitFreeCount(port, 1)

        // Recovery: after the settle the SAME instance restarts completely.
        capture.start()
        val next = awaitPort(factory, index = 1)
        assertEquals(2, factory.built.size)
        assertEquals(1, next.startCount.get())
        capture.stopAndRelease()
        assertEquals(1, next.haltCount.get())
        assertEquals(1, next.freeCount.get())
        assertNull(capture.readChunkShorts())
    }

    /**
     * Design D(6): two concurrent Stops racing an in-flight startRecording:
     * the fence is raised exactly once, the start thread performs the
     * teardown EXACTLY once, no recorder survives and nothing double-frees.
     */
    @Test
    fun concurrent_stops_during_in_flight_start_tear_down_exactly_once() {
        val factory = RecordingFactory()
        val inFlight = CountDownLatch(1)
        factory.pendingStartGate = inFlight
        val capture = AndroidMicCapture(factory)
        val starter = startInThread(capture)
        val port = awaitPort(factory)
        assertTrue(port.startEntered.await(2, TimeUnit.SECONDS))

        val stopDone = CountDownLatch(2)
        repeat(2) { n ->
            Thread({
                capture.stopAndRelease()
                stopDone.countDown()
            }, "capture-stop-$n").apply {
                isDaemon = true
                start()
            }
        }
        assertTrue(stopDone.await(2, TimeUnit.SECONDS))
        assertEquals(0, port.haltCount.get()) // deferred: start in flight
        inFlight.countDown()
        starter.join(2000)
        assertTrue(!starter.isAlive)
        assertEquals(1, port.startCount.get())
        assertEquals(1, port.haltCount.get())
        awaitFreeCount(port, 1)
        assertNull(capture.readChunkShorts())
    }

    /**
     * Design E(7): ten rapid Stop->Start transitions on ONE instance — every
     * OS recorder is started, halted and freed EXACTLY once, none is left
     * live, and the settled instance never poisons the next lease.
     */
    @Test
    fun ten_rapid_stop_then_start_cycles_release_each_port_exactly_once() {
        val factory = RecordingFactory()
        val capture = AndroidMicCapture(factory)
        repeat(10) {
            capture.start()
            capture.stopAndRelease()
        }
        assertEquals(10, factory.built.size)
        factory.built.forEachIndexed { index, port ->
            assertEquals("port $index start", 1, port.startCount.get())
            assertEquals("port $index halt", 1, port.haltCount.get())
            assertEquals("port $index free", 1, port.freeCount.get())
        }
        assertNull(capture.readChunkShorts())
    }
}
