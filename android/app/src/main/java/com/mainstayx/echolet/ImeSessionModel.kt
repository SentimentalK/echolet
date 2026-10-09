package com.mainstayx.echolet

/**
 * Phase 0-B PURE IME decision model (JUnit-testable, no Android runtime).
 *
 * Owns epochs (checked overflow), editor-lease identity (never fieldId alone)
 * and the HIDDEN/PREPARING/LISTENING/PAUSED/BLOCKED state chart. Android
 * execution lives in [EcholetInputMethodService] + [ImeSessionController]:
 * they only ever execute decided values, never re-derive them.
 *
 * Instantiable and owned exclusively by one [ImeSessionController]/service
 * lifetime: a fresh service never inherits a previous instance's process state
 * (in particular, a manual pause never survives hide/reopen of the keyboard).
 */
class ImeSessionModel {

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
        /** nativeClose of the handle owned by THIS generation, on the lane. */
        val closeNative: Boolean = false,
        /**
         * The epoch the decision was made under (decided inside the model
         * lock). The controller refuses to apply queued outcomes whose epoch
         * is already superseded, so a late stop/failure can never overwrite a
         * newer generation's state or side effects.
         */
        val atEpoch: Long = 0L,
        /**
         * True when the call was a stale candidate no-op: NO state/UI change,
         * NO mic stop, NO native close. The controller checks this before any
         * side effect.
         */
        val noSideEffects: Boolean = false,
    )

    // ------------------------------------------------------------------ state

    private val stateLock = Any()

    private var state: ImeState = ImeState.HIDDEN
    private var visible: Boolean = false
    /**
     * Manual Stop blocks auto-restart for THIS visible activation only;
     * persisting across a redraw of the same keyboard, but cleared when the
     * keyboard hides / a fresh visibility begins.
     */
    private var pausedByUser: Boolean = false
    private var blockedReason: String? = null

    /** Monotonic epoch; every stop/rebind/new lease increments it. */
    private var epoch: Long = 0L
    private var nonceCounter: Long = 0L
    private var lease: EditorLease? = null

    val currentState: ImeState
        get() = synchronized(stateLock) { state }

    fun currentEpoch(): Long = synchronized(stateLock) { epoch }

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

    private fun clearLeaseLocked() {
        bumpEpochLocked()
        lease = null
    }

    private fun fenceOutcomeLocked(
        next: ImeState,
        status: String?,
        blocked: String? = null,
    ): Outcome {
        clearLeaseLocked()
        state = next
        if (blocked != null) blockedReason = blocked
        return Outcome(next, null, status, requestStop = true, closeNative = true, atEpoch = epoch)
    }

    // -------------------------------------------------------- visibility path

    /**
     * onStartInputView. Auto-starts when visible + permission + staged model +
     * loadable libs; a manual pause from THIS activation needs an explicit
     * tap; missing prerequisites show BLOCKED without any microphone or
     * editor write. A fresh visibility (after hide) always auto-starts when
     * ready — a manual pause never outlives the hidden keyboard.
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
                // Same visibility, manual pause: stay explicit-tap-only.
                state = ImeState.PAUSED
                clearLeaseLocked()
                Outcome(state, null, "Paused — tap Start", atEpoch = epoch)
            }
            !ready -> {
                state = ImeState.BLOCKED
                blockedReason = reason ?: "Not ready"
                clearLeaseLocked()
                Outcome(state, null, blockedReason, requestStop = true, closeNative = true, atEpoch = epoch)
            }
            else -> {
                blockedReason = null
                val created = newLeaseLocked(pkg, fieldId, inputType)
                state = ImeState.PREPARING
                Outcome(state, created, "Preparing…", requestStop = true, closeNative = true, atEpoch = epoch)
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
        // Keyboard still visible: land in PAUSED (not user-paused, redraw can
        // still auto-start when the next visibility decision comes); gone:
        // full HIDDEN bookkeeping.
        fenceOutcomeLocked(
            if (hasView) ImeState.PAUSED else ImeState.HIDDEN.also { visible = false },
            status = null,
        )
    }

    /** onStartInput with no editor info: fence everything, hidden bookkeeping. */
    fun onNoEditor(): Outcome = synchronized(stateLock) {
        visible = false
        fenceOutcomeLocked(ImeState.HIDDEN, status = null)
    }

    /** Lifecycle hide (view or window): ALWAYS invalidate and free resources. */
    fun onHidden(): Outcome = synchronized(stateLock) {
        visible = false
        // Manual pause does NOT outlive a hidden keyboard: the next fresh
        // visibility auto-starts when prerequisites are ready.
        pausedByUser = false
        state = ImeState.HIDDEN
        fenceOutcomeLocked(ImeState.HIDDEN, status = null)
    }

    /** A FRESH visibility begins: manual pause was per-activation only. */
    fun onFreshVisibility() = synchronized(stateLock) {
        pausedByUser = false
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
            !hasView -> Outcome(state, null, "Keyboard not visible", atEpoch = epoch)
            state == ImeState.LISTENING || state == ImeState.PREPARING -> {
                // Manual STOP: freeze text + mic, set PAUSED for THIS visible
                // keyboard; persists across redraws of the same visibility,
                // never across hide/reopen. Old editor composing finish is
                // controller-wired.
                state = ImeState.PAUSED
                pausedByUser = true
                fenceOutcomeLocked(ImeState.PAUSED, status = "Paused — tap Start")
            }
            !ready -> {
                state = ImeState.BLOCKED
                blockedReason = reason ?: "Not ready"
                fenceOutcomeLocked(ImeState.BLOCKED, status = blockedReason)
            }
            else -> {
                // PAUSED-explicit-resume, BLOCKED, or HIDDEN-with-view.
                blockedReason = null
                pausedByUser = false
                val created = newLeaseLocked(pkg, fieldId, inputType)
                state = ImeState.PREPARING
                Outcome(state, created, "Preparing…", requestStop = true, closeNative = true, atEpoch = epoch)
            }
        }
    }

    // --------------------------------------------------- generation callbacks

    /** Lane confirms recognizer + mic are live for this exact lease. */
    fun generationListening(candidate: EditorLease): Outcome = synchronized(stateLock) {
        if (candidate != lease) {
            // Stale candidate: no-op, no state/UI change, no side effects.
            return Outcome(state, null, null, noSideEffects = true, atEpoch = epoch)
        }
        state = ImeState.LISTENING
        Outcome(state, null, "Listening…", atEpoch = epoch)
    }

    /**
     * Failure/interruption: fence + stop ONLY when the candidate is STILL the
     * current lease; a late failure from a previous generation changes
     * nothing (no epoch bump, no mic stop on a newer lease, no UI churn).
     */
    fun generationFailed(candidate: EditorLease?, message: String): Outcome =
        synchronized(stateLock) {
            if (candidate == null || candidate != lease) {
                return Outcome(state, null, null, noSideEffects = true, atEpoch = epoch)
            }
            val next = if (visible) ImeState.BLOCKED else ImeState.HIDDEN
            fenceOutcomeLocked(next, status = message, blocked = message)
        }

    /** Deterministic stop from any lifecycle path. */
    fun stop(reason: String?): Outcome = synchronized(stateLock) {
        val next = if (visible) ImeState.PAUSED else ImeState.HIDDEN
        fenceOutcomeLocked(next, status = reason)
    }
}
