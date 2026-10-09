package com.mainstayx.echolet

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * Phase 0-B IME control owner, constructed by [EcholetInputMethodService] on
 * the main thread. All lifecycle entry points and the Start/Stop control route
 * through deterministic begin/fence/stop; pure decisions come from
 * [ImeSessionModel], the single background lane serializes native
 * open/feed/close, and editor writes happen ONLY on the main thread behind a
 * bounded worker->main projection ACK.
 */
class ImeSessionController(
    private val context: Context,
    private val onState: (ImeSessionModel.ImeState, String) -> Unit,
) {
    private val main = Handler(Looper.getMainLooper())
    /** Pure decision singleton object, shared by service + tests. */
    val model: ImeSessionModel = ImeSessionModel

    /** THE single background lane: no overlapping nativeOpen, no races. */
    private val lane = Executors.newSingleThreadExecutor { r ->
        Thread(r, "echolet-ime-lane").apply { isDaemon = true }
    }

    private val mic = AndroidMicCapture()

    @Volatile private var nativeHandle: Long = 0L
    @Volatile private var active: ActiveLease? = null
    @Volatile private var serviceVisible: Boolean = false
    @Volatile private var lastStatus: String = ""

    /** Current editor identity, re-read at projection time. */
    @Volatile private var currentInfo: EditorInfo? = null

    private class ActiveLease(
        val lease: ImeSessionModel.EditorLease,
        @Volatile var ic: InputConnection?,
    )

    // ------------------------------------------------------------ bindings/UI

    fun onServiceCreated() {
        onState(ImeSessionModel.ImeState.HIDDEN, "Not listening")
    }

    /** onStartInput: any editor (re)binding fences the previous lease. */
    fun onInputStarted(info: EditorInfo?, restarting: Boolean, viewVisible: Boolean) {
        runOnMain {
            val identity = info
            val outcome = if (identity == null) {
                model.onNoEditor()
            } else {
                model.onEditorRebinding(
                    hasView = viewVisible,
                    pkg = identity.packageName?.toString(),
                    fieldId = identity.fieldId,
                    inputType = identity.inputType,
                )
            }
            apply(outcome)
        }
    }

    /** onStartInputView: gate readiness and maybe auto-start a generation. */
    fun onInputViewStarted(
        info: EditorInfo,
        ic: InputConnection?,
        ready: Boolean,
        blockedReason: String?,
    ) {
        runOnMain {
            currentInfo = info
            val outcome = model.onVisible(
                ready = ready,
                reason = blockedReason,
                pkg = info.packageName?.toString(),
                fieldId = info.fieldId,
                inputType = info.inputType,
            )
            apply(outcome)
            val lease = outcome.lease
            if (lease != null) {
                active = ActiveLease(lease, ic)
                startGeneration(lease)
            } else {
                active = null
            }
        }
    }

    fun onFinishInputView(finishingInput: Boolean) {
        runOnMain { apply(model.stop("input view finished")) }
    }

    fun onFinishInput() {
        runOnMain { apply(model.stop("input finished")) }
    }

    fun onWindowHidden() {
        runOnMain {
            serviceVisible = false
            apply(model.onHidden())
        }
    }

    fun onWindowShown() {
        runOnMain { serviceVisible = true }
    }

    fun onDestroyed() {
        runOnMain {
            currentInfo = null
            apply(model.onServiceDestroyed())
        }
        // Lane drains its queued tasks; each is fingerprinted by epoch and
        // stale ones reject at head/idempotently close.
        lane.execute { }
        lane.shutdown()
    }

    fun onCurrentInputConnection(ic: InputConnection?) {
        active?.ic = ic
    }

    // -------------------------------------------------------- the wide button

    fun onControlTap(ready: Boolean, blockedReason: String?, ic: InputConnection?) {
        runOnMain {
            val info = currentInfo
            val outcome = model.onTapControl(
                ready = ready,
                reason = blockedReason,
                hasView = serviceVisible && info != null,
                pkg = info?.packageName?.toString(),
                fieldId = info?.fieldId ?: 0,
                inputType = info?.inputType ?: 0,
            )
            apply(outcome)
            val lease = outcome.lease
            if (lease != null) {
                active = ActiveLease(lease, ic)
                startGeneration(lease)
            }
        }
    }

    // -------------------------------------------------------- deterministic stop

    /**
     * Stop path (thread-safe: callable from main or the lane). Order per
     * design 4: fence epoch + invalidate lease synchronously, immediately stop
     * AudioRecord (ignoring IllegalStateException), then nativeClose once on
     * the lane; visible partial text is never drained by late finals.
     */
    fun stop(reason: String?) {
        val outcome = model.stop(reason)
        // Immediate producer stop: unblocks a hung READ_BLOCKING read.
        mic.stopAndRelease()
        applyOnCallerThread(outcome)
    }

    // ------------------------------------------------------------------ impl

    private fun runOnMain(body: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) body() else main.post(body)
    }

    /** Applies an Outcome: fence mic, close native, refresh status/lease refs. */
    private fun apply(outcome: ImeSessionModel.Outcome) {
        if (outcome.lease == null) {
            active = null
        }
        lastStatus = outcome.status ?: lastStatus
        onState(outcome.state, outcome.status ?: lastStatus)
        applySideEffects(outcome)
    }

    private fun applyOnCallerThread(outcome: ImeSessionModel.Outcome) {
        main.post { apply(outcome) }
        // Side-effect ordering is preserved: fence + mic stop now, lane close
        // next, UI refresh posted.
        applySideEffects(outcome)
    }

    private fun applySideEffects(outcome: ImeSessionModel.Outcome) {
        if (outcome.requestStop) {
            mic.stopAndRelease()
        }
        if (outcome.closeNative) {
            val handle = nativeHandle
            if (handle != 0L) {
                nativeHandle = 0L
                lane.execute {
                    try {
                        NativeBridge.nativeClose(handle)
                    } catch (t: Throwable) {
                        Log.w(TAG, "nativeClose failed", t)
                    }
                }
            }
        }
    }

    private fun startGeneration(lease: ImeSessionModel.EditorLease) {
        val leased = active
        val modelDir = modelDirectory()
        lane.execute { generationTask(lease, leased?.ic, modelDir) }
    }

    fun modelDirectory(): String {
        val base = context.getExternalFilesDir("models")
            ?: throw IllegalStateException("app-specific external files dir unavailable")
        return java.io.File(base, "bilingual-zh-en").absolutePath
    }

    // ------------------------------- generation task (single background lane)

    private fun generationTask(
        lease: ImeSessionModel.EditorLease,
        ic: InputConnection?,
        modelDir: String,
    ) {
        // Head-of-lane stale rejection: tasks queued before a stop are cheap.
        if (!model.isLeaseCurrent(lease)) return
        val handle = try {
            NativeBridge.nativeOpen(modelDir)
        } catch (t: Throwable) {
            fail(lease, "native open failed: ${t.message ?: t.javaClass.simpleName}")
            return
        }
        // Stop raced the delayed open: close WITHOUT opening microphone and
        // WITHOUT posting Listening/UI results.
        if (!model.isLeaseCurrent(lease)) {
            closeQuietly(handle)
            return
        }
        nativeHandle = handle
        try {
            mic.start()
        } catch (t: Throwable) {
            nativeHandle = 0L
            closeQuietly(handle)
            fail(lease, "microphone start failed: ${t.message ?: t.javaClass.simpleName}")
            return
        }
        if (!model.isLeaseCurrent(lease)) {
            // Last-chance fence (onWindowHidden raced mic start).
            nativeHandle = 0L
            mic.stopAndRelease()
            closeQuietly(handle)
            return
        }
        main.post { statusFor(lease, "Listening…") }
        try {
            feedLoop(lease, handle, ic)
        } finally {
            // Loop-local mic release: exact-once via the adapter.
            mic.stopAndRelease()
        }
    }

    private fun feedLoop(
        lease: ImeSessionModel.EditorLease,
        handle: Long,
        ic: InputConnection?,
    ) {
        val reducer = ProjectionReducer(lease)
        while (model.isLeaseCurrent(lease)) {
            val shorts = try {
                mic.readChunkShorts()
            } catch (t: Throwable) {
                fail(lease, "microphone read failed: ${t.message ?: t.javaClass.simpleName}")
                return
            }
            if (shorts == null) {
                // stopAndRelease de-armed the adapter: terminate quietly.
                return
            }
            if (!model.isLeaseCurrent(lease)) return
            if (shorts.isEmpty()) continue
            val floats = FloatArray(shorts.size) { i -> shorts[i] / 32768f }
            if (floats.size > AndroidMicCapture.MAX_FEED_SAMPLES) {
                fail(lease, "oversized audio chunk rejected")
                return
            }
            val json = try {
                NativeBridge.nativeFeed(handle, floats, AndroidMicCapture.SAMPLE_RATE)
            } catch (t: Throwable) {
                fail(lease, "native feed failed: ${t.message ?: t.javaClass.simpleName}")
                return
            }
            if (!model.isLeaseCurrent(lease)) return
            // Validate strictly BEFORE dispatch: a malformed wire payload is a
            // native contract break -> fail closed, no editor write.
            val ops = try {
                reducer.accept(json)
            } catch (t: Throwable) {
                fail(lease, "native wire validation failed: ${t.message ?: t.javaClass.simpleName}")
                return
            }
            if (ops.isEmpty()) continue
            if (!projectWithAck(lease, reducer, ops)) {
                // Stale/refused/timeout: the projection path already fenced +
                // scheduled Stop; drop subsequent audio either way.
                return
            }
        }
    }

    /** One batch, one bounded main-thread ACK. Returns false to terminate. */
    private fun projectWithAck(
        lease: ImeSessionModel.EditorLease,
        reducer: ProjectionReducer,
        ops: List<EditorOp>,
    ): Boolean {
        val latch = CountDownLatch(1)
        val ack = booleanArrayOf(false)
        main.post {
            try {
                ack[0] = projectOnMain(lease, reducer, ops)
            } finally {
                latch.countDown() // ALWAYS completes the ACK
            }
        }
        val timedOut = try {
            !latch.await(PROJECTION_ACK_SECONDS, TimeUnit.SECONDS)
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
            true
        }
        if (timedOut) {
            failFromLane(lease, "projection ACK timed out")
            return false
        }
        if (!ack[0]) {
            failFromLane(lease, "editor projection refused")
            return false
        }
        return true
    }

    /**
     * Main thread: re-validates epoch/lease/visibility/binding identity ON the
     * CURRENT InputConnection, then applies composing ops. Never blocks on the
     * worker; a mismatch drops the batch and schedules Stop without writing.
     */
    private fun projectOnMain(
        lease: ImeSessionModel.EditorLease,
        reducer: ProjectionReducer,
        ops: List<EditorOp>,
    ): Boolean {
        val current = active
        val info = currentInfo
        val ic = current?.ic
        val editorMatches = ic != null &&
            info != null &&
            info.packageName?.toString() == lease.packageName &&
            info.fieldId == lease.fieldId &&
            info.inputType == lease.inputType
        if (!model.isLeaseCurrent(lease) || !serviceVisible || !editorMatches || current !== active) {
            // Stale UI callback / editor mismatch: DROP and schedule Stop.
            runOnMain { stop("projection dropped for stale editor or lease") }
            return false
        }
        if (model.currentState != ImeSessionModel.ImeState.LISTENING &&
            model.currentState != ImeSessionModel.ImeState.PREPARING
        ) {
            runOnMain { stop("projection while not listening") }
            return false
        }
        val sink = InputConnectionSink(lease, ic!!)
        for (op in ops) {
            val accepted = when (op) {
                is EditorOp.SetComposing ->
                    sink.beginOrReplaceComposing(op.text)
                EditorOp.FinishComposing -> sink.finishComposing()
                EditorOp.Noop -> true
            }
            if (!accepted) {
                // Editor refused the composing API: fail closed, no destructive
                // backspace fallback.
                runOnMain { stop("editor refused composing API") }
                return false
            }
        }
        return true
    }

    private fun fail(lease: ImeSessionModel.EditorLease, message: String) {
        Log.i(TAG, "generation fenced: $message")
        val outcome = model.generationFailed(lease, message)
        mic.stopAndRelease()
        main.post { apply(outcome) }
        applySideEffects(outcome)
    }

    private fun failFromLane(lease: ImeSessionModel.EditorLease, message: String) = fail(lease, message)

    private fun statusFor(lease: ImeSessionModel.EditorLease, text: String) {
        if (model.isLeaseCurrent(lease)) {
            lastStatus = text
            onState(ImeSessionModel.ImeState.LISTENING, text)
        }
    }

    private fun closeQuietly(handle: Long) {
        try {
            NativeBridge.nativeClose(handle)
        } catch (t: Throwable) {
            Log.w(TAG, "nativeClose (cancel) failed", t)
        }
    }

    companion object {
        private const val TAG = "EcholetIme"
        private const val PROJECTION_ACK_SECONDS = 5L
    }
}
