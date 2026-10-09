package com.mainstayx.echolet

/**
 * Phase 0-B PURE IME decision model (JUnit-testable, no Android runtime).
 *
 * Owns epochs (checked overflow), editor-lease identity (never fieldId alone)
 * and the HIDDEN/PREPARING/LISTENING/PAUSED/BLOCKED state chart. Android
 * execution lives in [EcholetInputMethodService] + [ImeSessionController]:
 * they only ever execute decided values, never re-derive them.
 */
object ImeSessionModel {

    enum class ImeState { HIDDEN, PREPARING, LISTENING, PAUSED, BLOCKED }

    /**
     * Editor binding identity: packageName + fieldId + inputType PLUS a unique
     * locally issued binding nonce and the issuing epoch, because fields can
     * reuse IDs across rebinding.
     */
    data class EditorLease(
        val epoch: Long,
        val packageName: String?,
        val fieldId: Int,
        val inputType: Int,
        val nonce: Long,
    )

    /** Decided action bundle the controller executes on Android. */
    data class Outcome(
        val state: ImeState,
        /** Newly issued lease (require a NEW JNI generation) or null. */
        val lease: EditorLease? = null,
        val status: String? = null,
        /** Immediate synchronous AudioRecord stop on the stop initiator. */
        val requestStop: Boolean = false,
        /** nativeClose of currently held handle, on the single lane. */
        val closeNative: Boolean = false,
    )

    // ------------------------------------------------------------------ state

    private val stateLock = Any()

    private var state: ImeState = ImeState.HIDDEN
    private var visible: Boolean = false
    /** Manual Stop blocks auto-restart until an explicit tap for that keyboard. */
    private var pausedByUser: Boolean = false
    private var blockedReason: String? = null

    /** Monotonic epoch; every stop/rebind/new lease increments it. */
    private var epoch: Long = 0L
    private var nonceCounter: Long = 0L
    private var lease: EditorLease? = null

    val currentState: ImeState
        get() = synchronized(stateLock) { state }

    fun currentEpoch(): Long = synchronized(stateLock) { epoch }

    fun issueNonce(): Long = synchronized(stateLock) { nextNonceLocked() }

    fun isLeaseCurrent(candidate: EditorLease?): Boolean =
        synchronized(stateLock) { candidate != null && candidate == lease }

    private fun bumpEpochLocked() {
        if (epoch == Long.MAX_VALUE) {
            throw IllegalStateException("epoch overflow after $epoch fences")
        }
        epoch += 1
    }

    private fun nextNonceLocked(): Long {
        if (nonceCounter == Long.MAX_VALUE) {
            throw IllegalStateException("binding nonce overflow")
        }
        nonceCounter += 1
        return nonceCounter
    }

    private fun newLeaseLocked(pkg: String?, fieldId: Int, inputType: Int): EditorLease {
        bumpEpochLocked()
        val created = EditorLease(epoch, pkg, fieldId, inputType, nextNonceLocked())
        lease = created
        return created
    }

    // -------------------------------------------------------- visibility path

    /**
     * onStartInputView. Auto-starts when visible + permission + staged model +
     * loadable libs; a sticky manual pause needs an explicit tap; missing
     * prerequisites show BLOCKED without any microphone or editor write.
     */
    fun onVisible(
        ready: Boolean,
        reason: String?,
        pkg: String?,
        fieldId: Int,
        inputType: Int,
    ): Outcome = synchronized(stateLock) {
        visible = true
        when {
            pausedByUser -> {
                state = ImeState.PAUSED
                bumpEpochLocked()
                lease = null
                Outcome(state, null, "Paused — tap Start")
            }
            !ready -> {
                state = ImeState.BLOCKED
                bumpEpochLocked()
                lease = null
                blockedReason = reason ?: "Not ready"
                Outcome(state, null, blockedReason, requestStop = true, closeNative = true)
            }
            else -> {
                blockedReason = null
                val created = newLeaseLocked(pkg, fieldId, inputType)
                state = ImeState.PREPARING
                Outcome(state, created, "Preparing…", requestStop = true, closeNative = true)
            }
        }
    }

    /**
     * onStartInput/restart: external editor switch or field detach counts as
     * Stop even when onFinishInputView has not arrived. Fences the old lease
     * immediately; the upcoming onStartInputView (or explicit tap for
     * BLOCKED/PAUSED) decides the next generation.
     */
    fun onEditorRebinding(
        hasView: Boolean,
        pkg: String?,
        fieldId: Int,
        inputType: Int,
    ): Outcome = synchronized(stateLock) {
        bumpEpochLocked()
        lease = null
        if (!hasView) {
            visible = false
            state = ImeState.HIDDEN
        } else if (pausedByUser) {
            state = ImeState.PAUSED
        }
        Outcome(state, null, null, requestStop = true, closeNative = true)
    }

    /** onStartInput with no editor info: fence everything, hidden bookkeeping. */
    fun onNoEditor(): Outcome = synchronized(stateLock) {
        bumpEpochLocked()
        lease = null
        state = ImeState.HIDDEN
        Outcome(state, null, null, requestStop = true, closeNative = true)
    }

    /** Lifecycle hide (view or window): ALWAYS invalidate and free resources. */
    fun onHidden(): Outcome = synchronized(stateLock) {
        visible = false
        bumpEpochLocked()
        lease = null
        state = ImeState.HIDDEN
        // pausedByUser survives so reopen stays manual-explicit.
        Outcome(state, null, null, requestStop = true, closeNative = true)
    }

    /** Service destruction. */
    fun onServiceDestroyed(): Outcome = onHidden()

    // ----------------------------------------------------------- user control

    /** The one wide Start/Stop control on the keyboard view. */
    fun onTapControl(
        ready: Boolean,
        reason: String?,
        hasView: Boolean,
        pkg: String?,
        fieldId: Int,
        inputType: Int,
    ): Outcome = synchronized(stateLock) {
        when {
            !hasView -> Outcome(state, null, "Keyboard not visible")
            state == ImeState.LISTENING || state == ImeState.PREPARING -> {
                // Manual STOP: freeze text + mic, set PAUSED; no auto-restart
                // on redraw. Old editor composing finish is controller-wired.
                bumpEpochLocked()
                lease = null
                state = ImeState.PAUSED
                pausedByUser = true
                Outcome(state, null, "Paused — tap Start", requestStop = true, closeNative = true)
            }
            pausedByUser -> {
                // Explicit resume for THIS visible keyboard: new generation.
                if (!ready) {
                    state = ImeState.BLOCKED
                    blockedReason = reason ?: "Not ready"
                    bumpEpochLocked()
                    lease = null
                    Outcome(state, null, blockedReason, requestStop = true, closeNative = true)
                } else {
                    blockedReason = null
                    pausedByUser = false
                    val created = newLeaseLocked(pkg, fieldId, inputType)
                    state = ImeState.PREPARING
                    Outcome(state, created, "Preparing…", requestStop = true, closeNative = true)
                }
            }
            else -> {
                // BLOCKED or HIDDEN-with-view: explicit tap may start.
                if (!ready) {
                    state = ImeState.BLOCKED
                    blockedReason = reason ?: "Not ready"
                    bumpEpochLocked()
                    lease = null
                    Outcome(state, null, blockedReason, requestStop = true, closeNative = true)
                } else {
                    blockedReason = null
                    val created = newLeaseLocked(pkg, fieldId, inputType)
                    state = ImeState.PREPARING
                    Outcome(state, created, "Preparing…")
                }
            }
        }
    }

    // --------------------------------------------------- generation callbacks

    /** Lane confirms recognizer + mic are live for this exact lease. */
    fun generationListening(candidate: EditorLease): Outcome = synchronized(stateLock) {
        if (candidate != lease) {
            return Outcome(state, null, null)
        }
        state = ImeState.LISTENING
        Outcome(state, null, "Listening…")
    }

    /** Failure/interruption: fence + stop; BLOCKED when visible, else HIDDEN. */
    fun generationFailed(candidate: EditorLease?, message: String): Outcome =
        synchronized(stateLock) {
            bumpEpochLocked()
            lease = null
            state = if (visible) ImeState.BLOCKED else ImeState.HIDDEN
            blockedReason = if (visible) message else blockedReason
            Outcome(state, null, message, requestStop = true, closeNative = true)
        }

    /** Deterministic stop from any lifecycle path. */
    fun stop(reason: String?): Outcome = synchronized(stateLock) {
        bumpEpochLocked()
        lease = null
        state = if (visible) ImeState.PAUSED else ImeState.HIDDEN
        Outcome(state, null, reason, requestStop = true, closeNative = true)
    }

    /** JUnit-only: restore construction state (never called on Android). */
    fun resetForJUnit() = synchronized(stateLock) {
        state = ImeState.HIDDEN
        visible = false
        pausedByUser = false
        blockedReason = null
        epoch = 0L
        nonceCounter = 0L
        lease = null
        Unit
    }
}
