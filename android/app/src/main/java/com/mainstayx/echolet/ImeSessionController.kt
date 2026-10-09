package com.mainstayx.echolet

import android.content.Context
import android.util.Log
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executor
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.TimeUnit

/**
 * Injectable Android main-thread plumbing: the controller NEVER references
 * Handler/Looper directly, so pure-JVM JUnit tests exercise the real
 * controller mediation with a deterministic fake scheduler.
 */
interface MainRunner {
    fun isOnMain(): Boolean
    fun post(body: () -> Unit)
}

/** Real Android main looper runner (default for the IME service). */
object AndroidMainRunner : MainRunner {
    private val handler = android.os.Handler(android.os.Looper.getMainLooper())
    override fun isOnMain(): Boolean =
        android.os.Looper.myLooper() == android.os.Looper.getMainLooper()

    override fun post(body: () -> Unit) {
        handler.post(body)
    }
}

/**
 * Injectable native session surface: the controller mediates open/feed/close
 * through THIS interface (the real Android implementation delegates to the
 * unchanged [NativeBridge]); JVM tests inject outcomes to exercise
 * generation-scoped closure and stale rejection.
 */
interface NativeApi {
    fun open(modelDir: String): Long
    fun feed(handle: Long, samples: FloatArray, sampleRate: Int): String
    fun close(handle: Long)
}

/** Real Android JNI delegate: unchanged NativeBridge single lane caller. */
object AndroidNativeApi : NativeApi {
    override fun open(modelDir: String): Long = NativeBridge.nativeOpen(modelDir)
    override fun feed(handle: Long, samples: FloatArray, sampleRate: Int): String =
        NativeBridge.nativeFeed(handle, samples, sampleRate)

    override fun close(handle: Long) {
        NativeBridge.nativeClose(handle)
    }
}

/**
 * Phase 0-B IME control owner, constructed by [EcholetInputMethodService] on
 * the main thread. All lifecycle entry points and the Start/Stop control route
 * through deterministic begin/fence/stop; PURE decisions come from the
 * per-controller [model] instance, the single background lane serializes
 * native open/feed/close, and editor writes happen ONLY on the main thread
 * behind a bounded worker->main projection ACK.
 *
 * Composition ownership: ONE [CompositionOwner] per lease spans ALL
 * partial/endpoint batches of that generation (never re-created per batch),
 * so the pre-existing editor composition is settled exactly once, partials
 * replace the same owned span, endpoints finalize once, and Stop/Hide
 * finalize the owned span on the STILL-VALID captured InputConnection.
 * Late/stale callbacks are matched against their source lease/epoch and
 * dropped before ANY side effect can touch a newer generation.
 */
class ImeSessionController internal constructor(
    private val dirProvider: () -> String,
    val model: ImeSessionModel,
    private val onState: (ImeSessionModel.ImeState, String) -> Unit,
    private val main: MainRunner,
    private val native: NativeApi,
    private val mic: MicCapture,
    /** Main-thread query of the service's CURRENT live editor connection. */
    private val liveEditorProvider: () -> Any?,
    /** Android-only: wraps the captured InputConnection into a ComposingEditor. */
    private val editorAdapterFactory: (Any?) -> ComposingEditor,
    /** THE single background lane: no overlapping nativeOpen, no races. */
    private val lane: Executor,
) {
    /** Android-facing constructor wired to the real OS surfaces. */
    constructor(
        context: Context,
        onState: (ImeSessionModel.ImeState, String) -> Unit,
        icProvider: () -> InputConnection?,
    ) : this(
        dirProvider = {
            val base = context.getExternalFilesDir("models")
                ?: throw IllegalStateException("app-specific external files dir unavailable")
            java.io.File(base, "bilingual-zh-en").absolutePath
        },
        model = ImeSessionModel(),
        onState = onState,
        main = AndroidMainRunner,
        native = AndroidNativeApi,
        mic = AndroidMicCapture(),
        liveEditorProvider = { icProvider() },
        editorAdapterFactory = { token -> InputConnectionSink(token as InputConnection) },
        lane = Executors.newSingleThreadExecutor { r ->
            Thread(r, "echolet-ime-lane").apply { isDaemon = true }
        },
    )

    /** The lease-owned generation's InputConnection + composition owner. */
    private class ActiveLease(
        val lease: ImeSessionModel.EditorLease,
        /**
         * The ONE editor connection captured for this lease — intentionally
         * NEVER rebound from a later handler notification (Design 1); a
         * changed editor fences the lease instead of silently re-pointing the
         * sink at it.
         */
        @Volatile var capturedEditor: Any?,
        /** Created at the FIRST accepted write of this lease; spans it. */
        @Volatile var owner: CompositionOwner? = null,
    )

    /** The native handle currently owned by an ALIVE generation. */
    private class NativeOwner(
        val lease: ImeSessionModel.EditorLease,
        val handle: Long,
    )

    private val nativeLock = Any()
    private var nativeOwner: NativeOwner? = null

    /**
     * Sticky fault when a native close threw (Design A): a Rust session may
     * still be alive, so every later generation fails BEFORE native.open.
     */
    @Volatile private var nativeCloseFault: String? = null

    /**
     * Deterministic test barrier (Java-Kotlin JVM mediation tests only):
     * invoked on the generation lane exactly between the stop precheck and
     * the nativeLock publish decision, letting tests hold Stop in the exact
     * Design-1 race window. NEVER set in production.
     */
    @Volatile internal var nativePublishGate: (() -> Unit)? = null

    /**
     * Deterministic test barrier (JVM mediation tests only): invoked on the
     * generation lane in [fail] exactly AFTER the model epoch fence and
     * BEFORE the lease's own native close executes, letting tests hold a
     * failed generation in the close-scheduling window while a newer
     * generation is issued from main. Null in production.
     */
    @Volatile internal var onAfterFailureFenceBeforeClose: (() -> Unit)? = null

    @Volatile private var active: ActiveLease? = null
    @Volatile private var serviceVisible: Boolean = false
    @Volatile private var lastStatus: String = ""

    /** Current editor identity, refreshed by the service on (re)binding. */
    @Volatile private var currentInfo: EditorInfo? = null

    // ------------------------------------------------------------ bindings/UI

    fun onServiceCreated() {
        onState(ImeSessionModel.ImeState.HIDDEN, "Not listening")
    }

    /** onStartInput: any editor (re)binding fences the previous lease. */
    fun onInputStarted(info: EditorInfo?, restarting: Boolean, viewVisible: Boolean) {
        decisionOnMain {
            val outcome = if (info == null) {
                model.onNoEditor()
            } else {
                model.onEditorRebinding(
                    hasView = viewVisible,
                    pkg = info.packageName?.toString(),
                    fieldId = info.fieldId,
                    inputType = info.inputType,
                )
            }
            applyDecision(outcome)
        }
    }

    /** onStartInputView: gate readiness and maybe auto-start a generation. */
    fun onInputViewStarted(
        info: EditorInfo,
        ic: Any?,
        ready: Boolean,
        blockedReason: String?,
    ) {
        decisionOnMain {
            currentInfo = info
            val outcome = model.onVisible(
                ready = ready,
                reason = blockedReason,
                pkg = info.packageName?.toString(),
                fieldId = info.fieldId,
                inputType = info.inputType,
            )
            applyDecision(outcome)
            if (outcome.lease != null) startGeneration(outcome.lease)
        }
    }

    fun onFinishInputView(finishingInput: Boolean) {
        decisionOnMain { applyDecision(model.stop("input view finished")) }
    }

    fun onFinishInput() {
        decisionOnMain { applyDecision(model.stop("input finished")) }
    }

    fun onWindowHidden() {
        decisionOnMain {
            serviceVisible = false
            applyDecision(model.onHidden())
        }
    }

    fun onWindowShown() {
        decisionOnMain {
            serviceVisible = true
            // A fresh visibility begins: a manual pause does NOT outlive a
            // hidden keyboard (per-activation only).
            model.onFreshVisibility()
        }
    }

    fun onDestroyed() {
        decisionOnMain {
            currentInfo = null
            applyDecision(model.onServiceDestroyed())
        }
        // Close any remaining registry owner IDENTIFIED by lease, never a
        // blind null-scope sweep: the registry may hold nothing (the fencing
        // task closes its own handle).
        onDestroyNativeSweep()
        // Lane drains its queued tasks; each is fingerprinted by epoch and
        // stale ones reject at head/idempotently close. Only the real
        // single-thread executor is shut down (test lanes stay drainable).
        executeOnLane {}
        (lane as? java.util.concurrent.ExecutorService)?.shutdown()
    }

    /** Teardown: enqueue close of whatever owner remains by identity. */
    private fun onDestroyNativeSweep() {
        val owner = synchronized(nativeLock) {
            val currentOwner = nativeOwner
            if (currentOwner == null) return
            nativeOwner = null
            currentOwner
        }
        executeOnLane { closeHandleQuietly(owner.handle) }
    }

    /**
     * Initial fill of the lease's captured editor. IMPORTANT: NEVER silently
     * re-points an existing owner at a different InputConnection — an editor
     * change fences the lease via rebinding, never an implicit rebind here.
     */
    fun onCurrentInputConnection(ic: Any?) {
        val current = active ?: return
        if (current.capturedEditor == null && ic != null) {
            current.capturedEditor = ic
        }
    }

    // -------------------------------------------------------- the wide button

    fun onControlTap(ready: Boolean, blockedReason: String?, ic: Any?) {
        decisionOnMain {
            val info = currentInfo
            val outcome = model.onTapControl(
                ready = ready,
                reason = blockedReason,
                hasView = serviceVisible && info != null,
                pkg = info?.packageName?.toString(),
                fieldId = info?.fieldId ?: 0,
                inputType = info?.inputType ?: 0,
            )
            applyDecision(outcome)
            if (outcome.lease != null) startGeneration(outcome.lease)
        }
    }

    // -------------------------------------------------------- deterministic stop

    /**
     * Stop path (thread-safe: callable from main or the lane). Order per
     * Design 2: fence epoch/lease synchronously FIRST, immediately request
     * the mic stop to unblock reads, then on the MAIN thread finalize the
     * owned composition on the still-valid captured editor and clear active;
     * nativeClose queues once on the serial lane behind running feeds.
     * Repeated calls are idempotent: no appended final, no leaked mic.
     */
    fun stop(reason: String?) {
        val outcome = model.stop(reason)
        mic.stopAndRelease()
        decisionOnMain { applyDecision(outcome) }
    }

    // ------------------------------------------------------------------ impl

    private fun decisionOnMain(body: () -> Unit) {
        if (main.isOnMain()) body() else main.post(body)
    }

    /**
     * THE single main-thread transition. Applies one decided Outcome:
     * freshness-gated (a queued outcome whose epoch is already superseded is
     * dropped BEFORE any active/UI reset), finalize-before-clear on stops,
     * lease-issued start capture, then UI status.
     */
    internal fun applyDecision(outcome: ImeSessionModel.Outcome) {
        if (outcome.noSideEffects) return
        // Queued-outcome freshness gate: stops/failures captured before a
        // newer generation began must not overwrite its state or side effects.
        if (model.currentEpoch() > outcome.atEpoch) return
        if (outcome.requestStop) {
            // Fence the CURRENT generation's producer side effects; when the
            // same outcome also carries a NEW lease (auto-start), the old
            // generation is finalized first and the new one captures fresh.
            mic.stopAndRelease()
            val pendingLease = active?.lease
            finalizeOwnedComposition()
            closeNativeForStop(pendingLease)
            active = null
        }
        if (outcome.lease != null) {
            active = ActiveLease(outcome.lease, liveEditorProvider())
        }
        lastStatus = outcome.status ?: lastStatus
        onState(outcome.state, outcome.status ?: lastStatus)
    }

    /**
     * Design 2 finalize: ON the main thread, IF the old editor binding is
     * still the live one AND Echolet owns a composition, commit the
     * already-visible owned span ONCE (no append, no delete, no re-commit).
     * On editor switch where the old binding is no longer live, touch
     * NOTHING (no methods on a foreign/new editor). Idempotent.
     */
    private fun finalizeOwnedComposition() {
        val current = active
        val owner = current?.owner
        if (owner == null || !owner.ownsComposition()) return
        val stillLiveEditor = current.capturedEditor != null &&
            current.capturedEditor === liveEditorProvider()
        if (!stillLiveEditor) {
            Log.i(TAG, "cannot finalize composition: old editor binding gone")
            return
        }
        if (!owner.finishOwnedIfAny()) {
            Log.w(TAG, "editor refused finalize at stop; no destructive fallback")
        }
    }

    /**
     * Marks the CURRENT native owner as the one to close for the stopping
     * lease, queues nativeClose ONCE on the serial lane behind running
     * open/feed. Never closes a handle owned by a NEWER generation.
     */
    private fun closeNativeForStop(stoppedLease: ImeSessionModel.EditorLease?) {
        // STRICT lease scope: a null/unknown lease must NEVER behave as "close
        // whatever owner is registered" — that could kill a newer generation's
        // handle. A fenced handle with no identified owner is closed by its own
        // generation task's fence release instead.
        if (stoppedLease == null) return
        val owner = synchronized(nativeLock) {
            val currentOwner = nativeOwner
            if (currentOwner == null) return
            if (currentOwner.lease != stoppedLease) return
            nativeOwner = null
            currentOwner
        }
        executeOnLane { closeHandleQuietly(owner.handle) }
    }

    /** On-lane cleanup fallback for a task releasing its own handle. */
    private fun releaseOwnNativeOnFence(lease: ImeSessionModel.EditorLease) {
        // A STILL-CURRENT lease keeps its owner alive (loop ended because the
        // producer stopped for another reason); only fenced leases release.
        if (model.isLeaseCurrent(lease)) return
        val owner = synchronized(nativeLock) {
            val currentOwner = nativeOwner
            if (currentOwner == null) null
            else if (currentOwner.lease == lease) {
                nativeOwner = null
                currentOwner
            } else null
        }
        if (owner != null) closeHandleQuietly(owner.handle)
    }

    private fun closeHandleQuietly(handle: Long) {
        try {
            native.close(handle)
        } catch (t: Throwable) {
            Log.w(TAG, "nativeClose failed", t)
        }
    }

    /**
     * Lane-ordered own-close (Design A): failure runs ON the generation lane,
     * so the lease's native handle is closed SYNCHRONOUSLY HERE — never
     * enqueued at the lane tailbehind an already-queued newer generation's
     * native.open. This makes the single serial lane itself the close-before-
     * next-open barrier. Exactly-once owner transfer preserved: once claimed,
     * the owner registry slot is gone, so main-Stop closeNativeForStop and
     * the task's own releaseOwnNativeOnFence are both no-ops afterwards.
     * Returns true when a close was claimed and executed for this lease.
     */
    private fun takeAndCloseOwnNativeNow(lease: ImeSessionModel.EditorLease): Boolean {
        val owner = synchronized(nativeLock) {
            if (nativeOwner?.lease != lease) return false
            val taken = nativeOwner!!
            nativeOwner = null
            taken
        }
        try {
            native.close(owner.handle)
            return true
        } catch (t: Throwable) {
            // A close that failed leaves a MAYBE-LIVE Rust session: newer
            // generations MUST NOT open until ownership is reconciled, so a
            // sticky fault blocks every subsequent generation's native.open.
            Log.w(TAG, "own nativeClose failed; native opens disabled", t)
            nativeCloseFault =
                "native session close unsettled: ${t.message ?: t.javaClass.simpleName}"
            return true
        }
    }

    private fun executeOnLane(action: () -> Unit) {
        try {
            lane.execute(action)
        } catch (_: RejectedExecutionException) {
            // Service destroyed and lane already down: queued cleanup is moot.
        }
    }

    // ------------------------------------------------------- generation task

    private fun startGeneration(lease: ImeSessionModel.EditorLease) {
        val modelDir = dirProvider()
        executeOnLane { generationTask(lease, modelDir) }
    }

    /** Test seam: queues the generation lane for a lease already issued. */
    internal fun beginGenerationForTest(lease: ImeSessionModel.EditorLease) =
        startGeneration(lease)

    // ------------------------------- generation task (single background lane)

    private fun generationTask(lease: ImeSessionModel.EditorLease, modelDir: String) {
        // Head-of-lane stale rejection: tasks queued before a stop are cheap.
        if (!model.isLeaseCurrent(lease)) return
        // Design A barrier: an earlier native close that THREW leaves a
        // possibly live Rust session; opening a new one here could raise
        // AlreadyActive and silently double-own the engine. Fail closed.
        nativeCloseFault?.let { fault ->
            fail(lease, fault)
            return
        }
        val handle = try {
            native.open(modelDir)
        } catch (t: Throwable) {
            fail(lease, "native open failed: ${t.message ?: t.javaClass.simpleName}")
            return
        }
        // Stop raced the delayed open: close WITHOUT opening a microphone and
        // WITHOUT posting Listening/UI results; the handle was never owned.
        if (!model.isLeaseCurrent(lease)) {
            closeHandleQuietly(handle)
            return
        }
        // Design 1 barrier: tests hold Stop exactly here (precheck passed,
        // publication not yet decided). Production leaves this unset.
        nativePublishGate?.invoke()
        // Atomic open/publish decision: EITHER the still-current lease takes
        // sole ownership of the fresh handle, OR the handle is refused and
        // closed exactly once right here — never started against the mic,
        // never orphaned, never overwriting a competing owner. Holding the
        // lock is microseconds: no native open/feed/close happens inside.
        val published = synchronized(nativeLock) {
            if (model.isLeaseCurrent(lease) && nativeOwner == null) {
                nativeOwner = NativeOwner(lease, handle)
                true
            } else {
                false
            }
        }
        if (!published) {
            // Stop raced the precheck, or a conflicting owner is registered:
            // fail closed for THIS handle only. No mic, no UI, no newer-owner
            // mutation; the newer generation's own lifecycle continues.
            closeHandleQuietly(handle)
            return
        }
        try {
            mic.start()
        } catch (t: Throwable) {
            releaseOwnNativeOnFence(lease)
            fail(lease, "microphone start failed: ${t.message ?: t.javaClass.simpleName}")
            return
        }
        if (!model.isLeaseCurrent(lease)) {
            // Last-chance fence (stop raced mic start): stop our own record
            // and publish NO Listening; the stop owner often captures close.
            mic.stopAndRelease()
            releaseOwnNativeOnFence(lease)
            return
        }
        // ACTUAL model transition to LISTENING (not on-screen text only).
        val listening = model.generationListening(lease)
        postOutcome(listening)
        try {
            feedLoop(lease, handle)
        } finally {
            // Loop-local mic release: exact-once via the adapter.
            mic.stopAndRelease()
            // Self-release our OWN handle when nobody else captured the close
            // (a queued stop's closeNativeForStop beats this — exactly one
            // path owns the close either way).
            releaseOwnNativeOnFence(lease)
        }
    }

    private fun feedLoop(lease: ImeSessionModel.EditorLease, handle: Long) {
        val reducer = ProjectionReducer(lease)
        while (model.isLeaseCurrent(lease)) {
            val shorts = try {
                mic.readChunkShorts()
            } catch (t: Throwable) {
                fail(lease, "microphone read failed: ${t.message ?: t.javaClass.simpleName}")
                return
            }
            if (shorts == null) {
                // A null read is normal ONLY for an already-fenced lease
                // (stopAndRelease de-armed the adapter). With the lease STILL
                // current the audio ended unexpectedly: fail THIS generation
                // closed (Design 2) instead of going silent — never remain
                // LISTENING without a live microphone.
                if (model.isLeaseCurrent(lease)) {
                    fail(lease, "audio stopped unexpectedly")
                }
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
                native.feed(handle, floats, AndroidMicCapture.SAMPLE_RATE)
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
            if (!projectWithAck(lease, ops)) {
                // Stale/refused/timeout: the projection path already fenced +
                // scheduled Stop; drop subsequent audio either way.
                return
            }
        }
    }

    /** One batch, one bounded main-thread ACK. Returns false to terminate. */
    private fun projectWithAck(
        lease: ImeSessionModel.EditorLease,
        ops: List<EditorOp>,
    ): Boolean {
        val latch = CountDownLatch(1)
        val ack = booleanArrayOf(false)
        main.post {
            try {
                ack[0] = try {
                    projectOnMain(lease, ops)
                } catch (t: Throwable) {
                    // A projection crash must never crash the IME UI thread:
                    // fail closed for THIS lease.
                    Log.w(TAG, "projection crashed on main", t)
                    try {
                        stop("projection crashed")
                    } catch (_: Throwable) {
                        // already fenced; nothing further
                    }
                    false
                }
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
            fail(lease, "projection ACK timed out")
            return false
        }
        if (!ack[0]) {
            fail(lease, "editor projection refused")
            return false
        }
        return true
    }

    /**
     * Main thread: re-validates epoch/lease/visibility/binding identity plus
     * the LIVE current editor connection against the lease's CAPTURED one,
     * then applies composing ops through the lease-owned
     * [CompositionOwner]. A mismatch drops the batch and fences the lease —
     * it never writes to a foreign editor and never re-points the owner.
     */
    private fun projectOnMain(
        lease: ImeSessionModel.EditorLease,
        ops: List<EditorOp>,
    ): Boolean {
        val current = active
        if (current == null || current.lease != lease) return false // stale queued
        if (!model.isLeaseCurrent(lease) || !serviceVisible) {
            stop("projection dropped for stale lease")
            return false
        }
        val liveEditor = liveEditorProvider()
        val info = currentInfo
        val identityMatches = info != null &&
            info.packageName?.toString() == lease.packageName &&
            info.fieldId == lease.fieldId &&
            info.inputType == lease.inputType
        val editorMatches = identityMatches &&
            current.capturedEditor != null &&
            liveEditor != null &&
            liveEditor === current.capturedEditor
        if (!editorMatches) {
            // Changed editor identity under identical field identifiers:
            // fence and stop rather than rebind the owner implicitly.
            stop("projection dropped for stale editor or lease")
            return false
        }
        if (model.currentState != ImeSessionModel.ImeState.LISTENING &&
            model.currentState != ImeSessionModel.ImeState.PREPARING
        ) {
            stop("projection while not listening")
            return false
        }
        val owner: CompositionOwner
        val existing = current.owner
        if (existing != null) {
            owner = existing
        } else {
            owner = CompositionOwner(editorAdapterFactory(current.capturedEditor!!))
            current.owner = owner
        }
        if (!owner.apply(ops)) {
            // Editor refused/failed the composing API: fail closed, unload the
            // batch (no destructive backspace fallback), fence the lease.
            stop("editor refused composing API")
            return false
        }
        return true
    }

    // ------------------------------------------------------------- outcomes

    /** Applies an outcome decided off-main, freshness-gated on arrival. */
    private fun postOutcome(outcome: ImeSessionModel.Outcome) {
        if (outcome.noSideEffects) return
        main.post { applyDecision(outcome) }
    }

    /**
     * Generation failure: the model guards candidate==lease under its lock and
     * returns a NO-OP outcome for stale candidates (Design 3), so a late
     * failure from a previous generation changes NOTHING here: the fresh
     * generation's mic, native handle and state are untouched.
     */
    private fun fail(lease: ImeSessionModel.EditorLease, message: String) {
        Log.i(TAG, "generation fenced: $message")
        val outcome = model.generationFailed(lease, message)
        if (outcome.noSideEffects) return
        // Time-of-check/time-of-use guard: the fence above was valid at its
        // own epoch; if a NEWER lease was issued in the meantime, its own
        // start path performs the stop/close — executing ours now would stop
        // or close the newer generation's resources. Safe to skip: the fenced
        // lease's generation task settles its own handle afterwards.
        if (model.currentEpoch() != outcome.atEpoch) return
        // Still-current: immediate side effects like a stop, then fenced state.
        mic.stopAndRelease()
        // Test seam (null in production): park BEFORE the close executes.
        onAfterFailureFenceBeforeClose?.invoke()
        // Design A: we are ON the lane; close our OWN handle HERE so any
        // newer generation queued on this lane can only native.open AFTER
        // our handle is fully closed — closeNativeForStop's tail-enqueued
        // close could land behind a queued generationTask(B) otherwise.
        takeAndCloseOwnNativeNow(lease)
        main.post { applyDecision(outcome) }
    }

    // --------------------------------------------------- test seam (JVM only)

    /** Emulates a competing registered owner for publish-refusal tests. */
    internal fun plantNativeOwnerForTest(lease: ImeSessionModel.EditorLease, handle: Long) {
        synchronized(nativeLock) { nativeOwner = NativeOwner(lease, handle) }
    }

    internal fun clearedNativeOwnerForTest(): Boolean = synchronized(nativeLock) {
        nativeOwner == null
    }

    /** Test-friendly current lease accessor. */
    internal fun currentLease(): ImeSessionModel.EditorLease? = active?.lease

    internal fun ownsComposingRightNow(): Boolean = active?.owner?.ownsComposition() == true

    val lastStatusText: String
        get() = lastStatus

    companion object {
        private const val TAG = "EcholetIme"
        private const val PROJECTION_ACK_SECONDS = 5L
    }
}
