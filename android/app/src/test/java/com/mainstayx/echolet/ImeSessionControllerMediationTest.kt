package com.mainstayx.echolet

import android.view.inputmethod.EditorInfo
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.Queue
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executor
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * Plain-JVM JUnit tests (no Android runtime) of the REAL controller
 * mediation: the same [ImeSessionController] the IME runs, with deterministic
 * fakes for main scheduling, the background lane, the JNI surface, mic
 * capture and the editor connection. The composition-ownership lifecycle,
 * Stop finalize ordering, stale-generation isolation, mic ownership and
 * fail-closed paths are exercised end to end — NOT via a bypassing fake.
 */
class ImeSessionControllerMediationTest {

    // ------------------------------------------------------------ test fakes

    /** Single-threaded test main: posts run inline on the calling thread. */
    private class InlineMainRunner : MainRunner {
        override fun isOnMain(): Boolean = false // posts run immediately below
        override fun post(body: () -> Unit) = body()
    }

    /** Async main for threaded gate tests: queue drained by the test thread. */
    private class QueuedMainRunner : MainRunner {
        private val q: Queue<() -> Unit> = ConcurrentLinkedQueue()
        override fun isOnMain(): Boolean = false
        override fun post(body: () -> Unit) {
            q.add(body)
        }

        fun pendingCount(): Int = q.size

        fun drainMain() {
            while (true) {
                val body = q.poll() ?: return
                body()
            }
        }
    }

    /** Deterministic FIFO lane drained by the test thread. */
    private class SequencedLane : Executor {
        private val q = ArrayDeque<Runnable>()
        override fun execute(command: Runnable) {
            q.addLast(command)
        }

        fun runAll() {
            while (true) {
                val task = q.removeFirstOrNull() ?: return
                task.run()
            }
        }
    }

    private fun awaitCondition(reason: String, millis: Long = 2000, done: () -> Boolean) {
        val deadline = System.currentTimeMillis() + millis
        while (!done()) {
            if (System.currentTimeMillis() > deadline) {
                throw AssertionError("condition not met in $millis ms: $reason")
            }
            Thread.sleep(5)
        }
    }

    /** Releases a fake gate by COUNTING DOWN, then clears it. */
    private fun releaseGate(gate: CountDownLatch?) {
        gate?.countDown()
    }

    /** Waits for a queued (threaded) outcome then applies it synchronously. */
    private fun awaitQueuedOutcome(main: QueuedMainRunner, reason: String) {
        awaitCondition(reason) { main.pendingCount() > 0 }
        main.drainMain()
    }

    /**
     * Drains the queued main runner UNTIL the expected state is observable,
     * tolerating that a single drain can consume several queued bodies (a
     * lane between two posts) — deterministic regardless of interleaving.
     */
    private fun awaitApplied(main: QueuedMainRunner, reason: String, done: () -> Boolean) {
        val deadline = System.currentTimeMillis() + 2000
        while (true) {
            main.drainMain()
            if (done()) return
            if (System.currentTimeMillis() > deadline) {
                throw AssertionError("condition not met in 2000 ms: $reason")
            }
            Thread.sleep(5)
        }
    }

    private class FakeNativeApi : NativeApi {
        val openedHandles = mutableListOf<Long>()
        val closedHandles = mutableListOf<Long>()

        /** Parked INSIDE open() to emulate "generation still preparing". */
        @Volatile var openGate: CountDownLatch? = null
        val openEntered = CountDownLatch(1)
        val firstHandleClosed = CountDownLatch(1)

        private val lock = Any()
        private val pendingFeeds = ArrayDeque<List<String>>()
        private val byHandle = HashMap<Long, ArrayDeque<String>>()
        private var counter = 1000L

        fun planOpen(feedScript: List<String>) {
            pendingFeeds.addLast(feedScript)
        }

        override fun open(modelDir: String): Long {
            openEntered.countDown()
            openGate?.await()
            synchronized(lock) {
                val handle = ++counter
                openedHandles.add(handle)
                byHandle[handle] = ArrayDeque(pendingFeeds.removeFirstOrNull() ?: emptyList())
                return handle
            }
        }

        /**
         * Pops the planned feed script; once exhausted it returns "[]" so a
         * live session keeps producing without replaying stale events.
         */
        override fun feed(handle: Long, samples: FloatArray, sampleRate: Int): String {
            synchronized(lock) {
                val scriptQueue = byHandle[handle] ?: return "[]"
                val next = if (scriptQueue.isEmpty()) null else scriptQueue.removeFirst()
                return next ?: "[]"
            }
        }

        override fun close(handle: Long) {
            synchronized(lock) {
                if (!openedHandles.contains(handle)) return
                if (byHandle.remove(handle) == null) return // duplicate: no-op
                closedHandles.add(handle)
            }
            firstHandleClosed.countDown()
        }
    }

    private class FakeMic : MicCapture {
        /** Parked INSIDE start() to emulate "start still preparing". */
        @Volatile var startGate: CountDownLatch? = null
        val startEntered = CountDownLatch(1)

        /**
         * The producer emits this many finite reads, then returns null from
         * read (exactly the stopped-device semantics that terminate a
         * producer loop quietly).
         */
        @Volatile var plannedReads: Long = Long.MAX_VALUE

        private val lock = Any()
        private var live = emptySet<Int>()
        private val released = mutableSetOf<Int>()
        private var nextId = 1

        fun limitPlannedReads(n: Long) {
            plannedReads = n
        }

        fun startCount(): Int = synchronized(lock) { nextId - 1 }
        fun liveCount(): Int = synchronized(lock) { live.size }

        fun allReleasedExactlyOnce(): Boolean = synchronized(lock) {
            live.isEmpty() && released.size == nextId - 1
        }

        override fun start() {
            startEntered.countDown()
            startGate?.await()
            synchronized(lock) {
                val id = nextId++
                live = live + id
            }
        }

        override fun readChunkShorts(): ShortArray? = synchronized(lock) {
            if (live.isEmpty()) return null
            if (plannedReads <= 0L) return null
            plannedReads -= 1
            ShortArray(AndroidMicCapture.CHUNK_SHORTS)
        }

        override fun stopAndRelease() {
            synchronized(lock) {
                if (live.isEmpty()) return
                val taken = live
                live = emptySet()
                released.addAll(taken) // never released twice
            }
        }
    }

    private class FakeEditor(val id: String) : ComposingEditor {
        var settleCount = 0
        var setCount = 0
        var finishCount = 0
        var composed = ""
        var committed = ""
        var refuseNext = false
        var throwNext = false

        fun visible(): String = committed + composed

        override fun settlePreexisting(): Boolean {
            if (refuseNext) return false
            if (throwNext) {
                throwNext = false
                throw RuntimeException("editor threw at settle")
            }
            settleCount++
            return true
        }

        override fun setComposing(text: String): Boolean {
            if (refuseNext) return false
            if (throwNext) {
                throwNext = false
                throw RuntimeException("editor threw at setComposing")
            }
            setCount++
            composed = text
            return true
        }

        override fun finish(): Boolean {
            if (refuseNext) return false
            if (throwNext) {
                throwNext = false
                throw RuntimeException("editor threw at finish")
            }
            finishCount++
            committed += composed
            composed = ""
            return true
        }
    }

    // -------------------------------------------------------- shared helpers

    private class EditorToken(val name: String)

    private val editors = HashMap<Any?, FakeEditor>()

    @Volatile private var activeEditorToken: Any? = null

    private class Harness(
        val model: ImeSessionModel,
        val native: FakeNativeApi,
        val mic: FakeMic,
        val main: MainRunner,
        val lane: Executor,
        val controller: ImeSessionController,
        val queuedMain: QueuedMainRunner?,
        val sequencedLane: SequencedLane?,
    )

    private fun sequencedHarness(): Harness {
        editors.clear()
        val native = FakeNativeApi()
        val mic = FakeMic()
        val model = ImeSessionModel()
        val lane = SequencedLane()
        val main = InlineMainRunner()
        val controller = ImeSessionController(
            dirProvider = { "/models/test" },
            model = model,
            onState = { _, _ -> },
            main = main,
            native = native,
            mic = mic,
            liveEditorProvider = { activeEditorToken },
            editorAdapterFactory = { token ->
                editors.getOrPut(token) { FakeEditor(token.toString()) }
            },
            lane = lane,
        )
        return Harness(model, native, mic, main, lane, controller, null, lane)
    }

    private fun threadedHarness(): Harness {
        editors.clear()
        val native = FakeNativeApi()
        val mic = FakeMic()
        val model = ImeSessionModel()
        val lane = Executors.newSingleThreadExecutor { r ->
            Thread({
                try {
                    r.run()
                } catch (e: Throwable) {
                    System.err.println("LANE TASK FAILED")
                    e.printStackTrace()
                }
            }, "test-ime-lane").apply { isDaemon = true }
        }
        val main = QueuedMainRunner()
        val controller = ImeSessionController(
            dirProvider = { "/models/test" },
            model = model,
            onState = { _, _ -> },
            main = main,
            native = native,
            mic = mic,
            liveEditorProvider = { activeEditorToken },
            editorAdapterFactory = { token ->
                editors.getOrPut(token) { FakeEditor(token.toString()) }
            },
            lane = lane,
        )
        return Harness(model, native, mic, main, lane, controller, main, null)
    }

    private fun info(
        pkg: String = "com.notes",
        fieldId: Int = 42,
        inputType: Int = 0x80000,
    ): EditorInfo = EditorInfo().apply {
        packageName = pkg
        this.fieldId = fieldId
        this.inputType = inputType
    }

    private fun partial(revision: Long, text: String) =
        """[{"kind":"partial","session":1,"revision":$revision,"backspaces":0,"suffix":"x","text":"$text"}]"""

    private fun endpoint(text: String) =
        """[{"kind":"endpoint","session":1,"text":"$text"}]"""

    /** One FRESH visibility: shown → rebind → view started (auto lease). */
    private fun startVisibility(harness: Harness, token: EditorToken, pkgFieldReuse: EditorInfo = info()) {
        activeEditorToken = token
        harness.controller.onWindowShown()
        harness.controller.onInputStarted(pkgFieldReuse, restarting = false, viewVisible = true)
        harness.controller.onInputViewStarted(pkgFieldReuse, token, ready = true, blockedReason = null)
        harness.queuedMain?.drainMain()
    }

    private fun drainLane(harness: Harness) {
        harness.sequencedLane?.runAll()
    }

    // ----------------------------------------------------------------- tests

    /** Designs 1+4: two partial batches replace once; genuine LISTENING. */
    @Test
    fun two_partials_in_distinct_batches_replace_without_duplicating() {
        val harness = sequencedHarness()
        val token = EditorToken("A")
        harness.native.planOpen(listOf(partial(1L, "你好"), partial(2L, "你们好")))
        harness.mic.limitPlannedReads(2)
        startVisibility(harness, token)
        drainLane(harness)

        val editor = editors.getValue(token)
        assertEquals("你们好", editor.visible()) // ONCE; never "你好你们好"
        assertEquals("", editor.committed)
        assertEquals(0, editor.finishCount)
        assertEquals(1, editor.settleCount) // pre-existing settled exactly once
        assertTrue(harness.controller.ownsComposingRightNow())
        assertEquals(ImeSessionModel.ImeState.LISTENING, harness.model.currentState)
        assertEquals("Listening…", harness.controller.lastStatusText)
        assertEquals(1, harness.native.openedHandles.size)
        assertEquals(0, harness.native.closedHandles.size)
    }

    @Test
    fun blank_partial_clears_only_owned_span_prefix_untouched() {
        val harness = sequencedHarness()
        val token = EditorToken("A")
        harness.native.planOpen(listOf(partial(1L, "你好"), partial(2L, "")))
        harness.mic.limitPlannedReads(2)
        startVisibility(harness, token)
        drainLane(harness)

        val editor = editors.getValue(token)
        assertEquals("", editor.visible()) // owned span cleared
        assertEquals("", editor.committed)
    }

    @Test
    fun endpoint_finalizes_once_then_next_utterance_restarts() {
        val harness = sequencedHarness()
        val token = EditorToken("A")
        harness.native.planOpen(
            listOf(partial(1L, "第一"), endpoint("第一"), partial(2L, "第二"), endpoint("第二"))
        )
        harness.mic.limitPlannedReads(4)
        startVisibility(harness, token)
        drainLane(harness)

        val editor = editors.getValue(token)
        assertEquals("第一第二", editor.visible())
        assertEquals(2, editor.finishCount) // exact-once per owned span
        assertTrue(harness.model.isLeaseCurrent(harness.controller.currentLease()))
    }

    @Test
    fun stop_mid_partial_preserves_exact_visible_text_and_finalizes_valid_ic() {
        val harness = sequencedHarness()
        val token = EditorToken("A")
        harness.native.planOpen(listOf(partial(1L, "说到一半")))
        harness.mic.limitPlannedReads(1)
        startVisibility(harness, token)
        drainLane(harness)
        val editor = editors.getValue(token)
        assertEquals("说到一半", editor.visible())
        assertTrue(harness.controller.ownsComposingRightNow())
        val preStopEpoch = harness.model.currentEpoch()

        harness.controller.stop("user stop mid partial")
        drainLane(harness) // drain the queued native close

        assertEquals("说到一半", editor.visible()) // exact partial preserved
        assertEquals(1, editor.finishCount) // commit-once on the SAME editor
        assertFalse(harness.controller.ownsComposingRightNow())
        assertTrue(harness.model.currentEpoch() > preStopEpoch)
        assertTrue(harness.native.closedHandles.containsAll(harness.native.openedHandles))
    }

    @Test
    fun stop_twice_and_hide_reopen_add_no_text_and_get_fresh_epoch() {
        val harness = sequencedHarness()
        val tokenA = EditorToken("A")
        val tokenB = EditorToken("B")
        harness.native.planOpen(listOf(partial(1L, "半句")))
        harness.mic.limitPlannedReads(1)
        startVisibility(harness, tokenA)
        drainLane(harness)
        harness.controller.stop("first stop")
        drainLane(harness)
        harness.controller.stop("second stop (idempotent)")
        drainLane(harness)
        assertEquals("半句", editors.getValue(tokenA).visible()) // no extra commit
        val epochAfterStopA = harness.model.currentEpoch()

        // Hide + reopen on a different editor: fresh epoch, fresh lease.
        harness.controller.onWindowHidden()
        harness.controller.onWindowShown()
        harness.native.planOpen(listOf(partial(1L, "")))
        harness.mic.limitPlannedReads(1)
        startVisibility(harness, tokenB)
        drainLane(harness)
        val leaseB = harness.controller.currentLease()

        assertNotNull(leaseB)
        assertTrue(harness.model.currentEpoch() > epochAfterStopA)
        assertEquals("", editors.getValue(tokenB).visible()) // no cross-editor text
        assertEquals(2, harness.native.openedHandles.size)
        assertEquals(1, harness.native.closedHandles.size) // A only; B alive
    }

    @Test
    fun manual_pause_persists_only_within_same_visibility() {
        val harness = sequencedHarness()
        val token = EditorToken("A")
        harness.native.planOpen(listOf(partial(1L, "第一")))
        harness.mic.limitPlannedReads(1)
        startVisibility(harness, token)
        drainLane(harness)
        assertEquals(ImeSessionModel.ImeState.LISTENING, harness.model.currentState)

        harness.controller.onControlTap(ready = true, blockedReason = null, ic = token)
        drainLane(harness)
        assertEquals(ImeSessionModel.ImeState.PAUSED, harness.model.currentState)
        assertNull(harness.controller.currentLease())
        val opensAfterPause = harness.native.openedHandles.size

        // (i) SAME visible redraw: manual pause holds; no restart.
        harness.controller.onInputViewStarted(
            info(), token, ready = true, blockedReason = null,
        )
        harness.queuedMain?.drainMain() // inline for the sequenced harness
        drainLane(harness)
        assertEquals(ImeSessionModel.ImeState.PAUSED, harness.model.currentState)
        assertNull(harness.controller.currentLease())
        assertEquals(opensAfterPause, harness.native.openedHandles.size)

        // (ii) hide -> fresh visible session: auto-start again.
        harness.controller.onWindowHidden()
        harness.controller.onWindowShown()
        harness.native.planOpen(listOf(partial(1L, "重新开始")))
        harness.mic.limitPlannedReads(1)
        startVisibility(harness, token)
        drainLane(harness)
        assertEquals(ImeSessionModel.ImeState.LISTENING, harness.model.currentState)
        assertNotNull(harness.controller.currentLease())
        assertEquals(opensAfterPause + 1, harness.native.openedHandles.size)
    }

    /** Designs 3+7: B live; late old-A events touch nothing of B's. */
    @Test
    fun late_old_generation_open_and_failure_cannot_touch_active_b() {
        val harness = threadedHarness()
        val main = harness.queuedMain!!
        val tokenA = EditorToken("A")
        val tokenB = EditorToken("B")
        val native = harness.native

        native.planOpen(listOf(partial(1L, "A1")))
        native.planOpen(listOf(partial(1L, "B1")))
        harness.mic.limitPlannedReads(Long.MAX_VALUE)
        native.openGate = CountDownLatch(1)

        // Begin A (real lane); its nativeOpen parks inside the gate.
        startVisibility(harness, tokenA)
        assertTrue(native.openEntered.await(2, TimeUnit.SECONDS))
        assertTrue(native.openedHandles.isEmpty()) // still blocked inside open

        val leaseA = harness.controller.currentLease()
        // Stop A, then begin B (fresh lease, same visible view).
        harness.controller.stop("stop A while preparing")
        main.drainMain()
        assertFalse(harness.model.isLeaseCurrent(leaseA))

        harness.controller.onInputStarted(info(), restarting = false, viewVisible = true)
        activeEditorToken = tokenB
        harness.controller.onInputViewStarted(info(), tokenB, ready = true, blockedReason = null)
        main.drainMain()
        val leaseB = harness.controller.currentLease()
        assertNotNull(leaseB)

        // Late A open completes: close ONLY A; never publish or feed B.
        native.openGate?.countDown()
        native.openGate = null
        assertTrue(native.firstHandleClosed.await(2, TimeUnit.SECONDS))
        assertTrue(native.closedHandles.contains(native.openedHandles[0]))

        // B runs after A's close finished on the serial lane: queued
        // Listening outcome, then B's own projected batch arrives through main.
        val editorA = editors.getOrPut(tokenA) { FakeEditor(tokenA.name) }
        awaitApplied(main, "B listening + B1 projected") {
            harness.model.currentState == ImeSessionModel.ImeState.LISTENING &&
                editors.getOrPut(tokenB) { FakeEditor(tokenB.name) }.visible() == "B1"
        }
        assertTrue(harness.model.isLeaseCurrent(leaseB))
        assertEquals("B1", editors.getValue(tokenB).visible())
        assertEquals("", editorA.visible())
        assertEquals(1, harness.mic.liveCount()) // only B's record is live

        // (i) Late failure from OLD A: strict no-op.
        val microStartsBefore = harness.mic.startCount()
        val lateFailure = harness.model.generationFailed(leaseA, "late A mic error")
        assertTrue(lateFailure.noSideEffects)
        assertTrue(harness.model.isLeaseCurrent(leaseB))

        // (ii) A stale QUEUED stop outcome (epoch behind B) is skipped.
        val epochBefore = harness.model.currentEpoch()
        harness.controller.applyDecision(
            ImeSessionModel.Outcome(
                state = ImeSessionModel.ImeState.PAUSED,
                status = "late stop",
                requestStop = true,
                closeNative = true,
                atEpoch = epochBefore - 1,
            )
        )
        assertTrue(harness.model.isLeaseCurrent(leaseB))
        assertEquals(epochBefore, harness.model.currentEpoch())
        assertEquals(ImeSessionModel.ImeState.LISTENING, harness.model.currentState)
        assertEquals(microStartsBefore, harness.mic.startCount())
        assertEquals(1, harness.mic.liveCount())

        harness.controller.stop("teardown")
        awaitCondition("teardown drain") { harness.mic.allReleasedExactlyOnce() }
        awaitCondition("all handles closed") { native.closedHandles.containsAll(native.openedHandles) }
    }

    /** Repeated editor field identity with a DISTINCT nonce fences old A. */
    @Test
    fun repeated_field_identity_with_distinct_nonce_rejects_old_callbacks() {
        val harness = threadedHarness()
        val main = harness.queuedMain!!
        val tokenA = EditorToken("field")
        val tokenB = EditorToken("field")
        val native = harness.native

        native.planOpen(listOf(partial(1L, "A1")))
        native.planOpen(listOf(partial(1L, "B1")))
        harness.mic.limitPlannedReads(Long.MAX_VALUE)
        native.openGate = CountDownLatch(1)

        val sameInfo = info()
        harness.controller.onWindowShown()
        harness.controller.onInputStarted(sameInfo, restarting = false, viewVisible = true)
        activeEditorToken = tokenA
        harness.controller.onInputViewStarted(sameInfo, tokenA, ready = true, blockedReason = null)
        main.drainMain()
        val leaseA = harness.controller.currentLease()
        assertTrue(native.openEntered.await(2, TimeUnit.SECONDS))

        // Identical pkg/fieldId/inputType; only the lease nonce differs.
        harness.controller.onInputStarted(sameInfo, restarting = false, viewVisible = true)
        activeEditorToken = tokenB
        harness.controller.onInputViewStarted(sameInfo, tokenB, ready = true, blockedReason = null)
        main.drainMain()
        val leaseB = harness.controller.currentLease()

        assertNotNull(leaseB)
        assertNotEquals(leaseA!!, leaseB)
        assertEquals(leaseA.fieldId, leaseB!!.fieldId)

        native.openGate?.countDown()
        native.openGate = null
        assertTrue(native.firstHandleClosed.await(2, TimeUnit.SECONDS))
        val editorA = editors.getOrPut(tokenA) { FakeEditor(tokenA.name) }
        awaitApplied(main, "B listening + B1 projected") {
            harness.model.currentState == ImeSessionModel.ImeState.LISTENING &&
                editors.getOrPut(tokenB) { FakeEditor(tokenB.name) }.visible() == "B1"
        }
        assertTrue(harness.model.isLeaseCurrent(leaseB))
        assertEquals("B1", editors.getValue(tokenB).visible())
        assertEquals("", editorA.visible())
        assertTrue(harness.model.generationFailed(leaseA, "late A failure").noSideEffects)
        harness.controller.stop("teardown")
        awaitCondition("teardown drain") { harness.mic.allReleasedExactlyOnce() }
    }

    /** Design 6: a thrown InputConnection method is captured; fail closed. */
    @Test
    fun thrown_editor_exception_fails_closed_without_uncaught_crash() {
        val harness = sequencedHarness()
        val token = EditorToken("throwing")
        val throwingEditor = FakeEditor(token.toString()).apply { throwNext = true }
        editors[token] = throwingEditor
        harness.native.planOpen(listOf(partial(1L, "one-shot")))
        harness.mic.limitPlannedReads(1)
        val epochBefore = harness.model.currentEpoch()

        startVisibility(harness, token)
        drainLane(harness) // no exception ever escapes the controller

        assertEquals("", throwingEditor.visible())
        assertEquals(0, throwingEditor.setCount)
        assertFalse(throwingEditor.throwNext) // the throw was consumed once
        assertTrue(harness.model.currentEpoch() > epochBefore)
        assertEquals(ImeSessionModel.ImeState.PAUSED, harness.model.currentState)
        assertFalse(harness.controller.ownsComposingRightNow())
        assertTrue(harness.native.closedHandles.containsAll(harness.native.openedHandles))
    }

    /** Design 5: begin A -> Stop A (start preparing) -> begin B -> late start. */
    @Test
    fun mic_start_stop_overlap_releases_late_record_without_touching_b() {
        val harness = threadedHarness()
        val main = harness.queuedMain!!
        val tokenA = EditorToken("A")
        val tokenB = EditorToken("B")
        val mic = harness.mic

        harness.native.planOpen(listOf(partial(1L, "never projected")))
        harness.native.planOpen(listOf(partial(1L, "real B")))
        mic.limitPlannedReads(Long.MAX_VALUE)
        mic.startGate = CountDownLatch(1)

        // Begin A; mic.start parks inside the gate.
        startVisibility(harness, tokenA)
        assertTrue(mic.startEntered.await(2, TimeUnit.SECONDS))
        assertEquals(0, mic.liveCount()) // start still preparing

        // Stop A while its start is preparing; then release the late start.
        harness.controller.stop("stop A during mic prep")
        main.drainMain()
        mic.startGate?.countDown()
        mic.startGate = null
        awaitCondition("late A record released") { mic.allReleasedExactlyOnce() }
        assertEquals(0, mic.liveCount())

        // Begin B: no leaked A record, fresh generation untouched by A.
        harness.controller.onInputStarted(info(), restarting = false, viewVisible = true)
        activeEditorToken = tokenB
        harness.controller.onInputViewStarted(info(), tokenB, ready = true, blockedReason = null)
        main.drainMain()
        val leaseB = harness.controller.currentLease()

        awaitApplied(main, "B listening + live record") {
            harness.model.currentState == ImeSessionModel.ImeState.LISTENING && mic.liveCount() == 1
        }
        assertTrue(harness.model.isLeaseCurrent(leaseB))
        assertEquals(1, mic.liveCount())

        harness.controller.stop("teardown")
        awaitCondition("teardown drain") { mic.allReleasedExactlyOnce() }
        awaitCondition("all handles closed") {
            harness.native.closedHandles.containsAll(harness.native.openedHandles)
        }
    }

    /** Design 8 (12): start/stop + editor-switch cycles stay leak-free. */
    @Test
    fun ten_start_stop_and_editor_switch_cycles_have_no_duplicate_sessions() {
        val harness = sequencedHarness()
        var micStartsSeen = 0
        repeat(10) { cycle ->
            val token = EditorToken("cycle$cycle")
            harness.native.planOpen(listOf(partial(1L, "cycle $cycle")))
            harness.mic.limitPlannedReads(1)
            startVisibility(harness, token)
            drainLane(harness)
            micStartsSeen = harness.mic.startCount()
            assertEquals(ImeSessionModel.ImeState.LISTENING, harness.model.currentState)
            assertEquals("cycle $cycle", editors.getValue(token).visible())
            harness.controller.stop("cycle end $cycle")
            drainLane(harness)
            assertNull(harness.controller.currentLease())
        }
        assertEquals(10, micStartsSeen)
        assertEquals(10, harness.native.openedHandles.size)
        assertEquals(10, harness.native.closedHandles.size) // each handle closed once
        assertTrue(harness.mic.allReleasedExactlyOnce())
    }
}
