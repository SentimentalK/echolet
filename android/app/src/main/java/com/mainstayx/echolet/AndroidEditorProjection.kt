package com.mainstayx.echolet

import android.view.inputmethod.InputConnection
import org.json.JSONArray
import org.json.JSONObject

/**
 * Phase 0-B editor-side projection: strictly parses the existing JNI
 * `nativeFeed` event array, validates editor-lease freshness, revision
 * monotonicity and native-session stability in a PURE reducer (JUnit-testable,
 * no Android runtime), and maps accepted full text onto the CURRENT
 * InputConnection via composing operations only.
 *
 * The editor binding identity type is [ImeSessionModel.EditorLease] — the
 * single owner of leases. Rules built in elsewhere remain: Kotlin never
 * applies backspaces/suffix to an external editor (diff wire metadata is
 * diagnostic only) and never uses deleteSurroundingText.
 * `setComposingText(full, 1)` replaces ONLY the owned composing region;
 * endpoint finishes it ONCE without also committing text.
 */
sealed interface EditorOp {
    /** Replace ONLY the Echolet-owned composing region with full text. */
    data class SetComposing(val text: String) : EditorOp

    /** Finalize the owned composing region (utterance endpoint). */
    data object FinishComposing : EditorOp

    /** Validated but nothing to project (e.g. guarded double endpoint). */
    data object Noop : EditorOp
}

/**
 * JVM-testable seam over the LIVE editor writing surface. The Android
 * implementation is [InputConnectionSink], created ONLY on Android with a real
 * InputConnection; JVM tests inject fakes here. Every method returns false on
 * editor refusal / thrown editor exception, so callers fail closed with no
 * destructive fallback and no uncaught exception on the Android main thread.
 */
interface ComposingEditor {
    /**
     * Settle the PRE-EXISTING editor composition exactly once (before the
     * first Echolet write): finishComposingText keeps already-visible text
     * and commits nothing new; refusals/exceptions are refused (false).
     */
    fun settlePreexisting(): Boolean

    /** Replace ONLY the owned composing region with the full recognized text. */
    fun setComposing(text: String): Boolean

    /** Finalize the owned composing region, leaving the text visible/intact. */
    fun finish(): Boolean
}

/**
 * Persistent per-generation composition OWNER: one instance spans ALL
 * partial/endpoint batches of one lease, so the pre-existing editor
 * composition is settled exactly ONCE at the first accepted write and
 * subsequent partials merely replace the same owned composing span. Creating
 * a fresh writer per batch would re-run the settle step and prematurely
 * commit the previous partial, then re-compose from scratch — the exact bug
 * this class exists to prevent (visible `partial partial…` duplication).
 *
 * Also owns the "stop finalize" transition: [finishOwnedIfAny] commits the
 * already-visible owned composition exactly once (Stop mid-partial keeps the
 * exact visible text), and is safe/idempotent to call repeatedly.
 */
class CompositionOwner(private val editor: ComposingEditor) {
    private var settledPreexisting = false
    private var hasOwnedComposition = false

    fun ownsComposition(): Boolean = hasOwnedComposition

    /** Applies one validated op batch; false = refused/failed, no fallback. */
    fun apply(ops: List<EditorOp>): Boolean {
        for (op in ops) {
            val accepted = try {
                when (op) {
                    is EditorOp.SetComposing -> {
                        if (!settledPreexisting) {
                            // Settle the pre-existing composition ONCE, on the
                            // first ACCEPTED write of this lease. Retried by
                            // later batches if the editor refuses now.
                            if (!editor.settlePreexisting()) return false
                            settledPreexisting = true
                        }
                        hasOwnedComposition = true
                        editor.setComposing(op.text)
                    }
                    EditorOp.FinishComposing -> {
                        if (hasOwnedComposition) {
                            hasOwnedComposition = false
                            editor.finish()
                        } else {
                            true // duplicate endpoint: nothing owned to finish
                        }
                    }
                    EditorOp.Noop -> true
                }
            } catch (_: Throwable) {
                // A thrown editor method must never crash the Android main
                // thread: treat as refusal, fail closed.
                false
            }
            if (!accepted) return false
        }
        return true
    }

    /**
     * Stop/Hide/ServiceDestroy finalize on the STILL-VALID captured editor:
     * commits the owned, already-visible composition exactly once without
     * appending, deleting or re-committing anything. Returns true when
     * nothing was owned (nothing to do).
     */
    fun finishOwnedIfAny(): Boolean {
        if (!hasOwnedComposition) return true
        val finished = try {
            settledPreexisting = true
            hasOwnedComposition = false
            editor.finish()
        } catch (_: Throwable) {
            false
        }
        return finished
    }
}

/**
 * Pure projection reducer for one editor lease/native generation.
 * Throws IllegalArgumentException on malformed wire payloads (fail closed:
 * caller stops capture rather than writing anything unvalidated).
 */
class ProjectionReducer(private val lease: ImeSessionModel.EditorLease) {
    private var session: Long = -1
    private var lastRevision: Long = -1
    private var sawEndpointSincePartial = false

    fun lease(): ImeSessionModel.EditorLease = lease
    fun nativeSession(): Long = session
    fun lastRevision(): Long = lastRevision

    private fun requireSessionEvent(event: JSONObject, kind: String): Long {
        val sessionValue = event.optLong("session", -1L)
        if (sessionValue <= 0L) {
            throw IllegalArgumentException(
                "malformed wire event: $kind requires positive session: $event"
            )
        }
        if (session != -1L && sessionValue != session) {
            throw IllegalStateException(
                "wire event interleaved native session $sessionValue into " +
                    "generation bound to session $session (lease epoch ${lease.epoch})"
            )
        }
        return sessionValue
    }

    /** Validates + reduces one complete nativeFeed JSON response (may be []). */
    fun accept(jsonText: String): List<EditorOp> {
        val events = JSONArray(jsonText)
        val ops = ArrayList<EditorOp>()
        for (i in 0 until events.length()) {
            val event = events.getJSONObject(i)
            when (val kind = event.optString("kind")) {
                "partial" -> {
                    val sessionValue = requireSessionEvent(event, kind)
                    val revisionValue = event.optLong("revision", -1L)
                    if (revisionValue <= 0L) {
                        throw IllegalArgumentException(
                            "malformed wire event: partial requires positive revision: $event"
                        )
                    }
                    if (!event.has("backspaces") || event.optInt("backspaces", -1) < 0) {
                        throw IllegalArgumentException(
                            "malformed wire event: partial requires nonnegative backspaces: $event"
                        )
                    }
                    if (!event.has("suffix") || !event.has("text")) {
                        throw IllegalArgumentException(
                            "malformed wire event: partial requires suffix+text: $event"
                        )
                    }
                    if (session == -1L) {
                        session = sessionValue
                    }
                    // Contiguous ascending watermark, mirroring Rust
                    // accept_delivery: the first partial of a native session
                    // MUST be revision 1 and every next one exactly +1;
                    // duplicates and out-of-order are rejected.
                    val expected = if (lastRevision == -1L) 1L else lastRevision + 1L
                    if (revisionValue != expected) {
                        throw IllegalStateException(
                            "stale/out-of-order partial revision $revisionValue " +
                                "(expected $expected, epoch ${lease.epoch})"
                        )
                    }
                    lastRevision = revisionValue
                    sawEndpointSincePartial = false
                    ops.add(EditorOp.SetComposing(event.optString("text")))
                }
                "endpoint" -> {
                    val sessionValue = requireSessionEvent(event, kind)
                    val text = event.optString("text")
                    if (text.isEmpty()) {
                        throw IllegalArgumentException(
                            "malformed wire event: endpoint carries empty text: $event"
                        )
                    }
                    if (session == -1L) {
                        session = sessionValue
                    }
                    // A session CAN emit several endpoints (per utterance);
                    // guarded so a stale/double endpoint between partials is
                    // a Noop instead of a duplicate finish. A partial after an
                    // endpoint opens the NEXT utterance's fresh owned span.
                    if (sawEndpointSincePartial) {
                        ops.add(EditorOp.Noop)
                    } else {
                        sawEndpointSincePartial = true
                        ops.add(EditorOp.FinishComposing)
                    }
                }
                else -> throw IllegalArgumentException(
                    "malformed wire event: unknown kind $kind: $event"
                )
            }
        }
        return ops
    }
}

/**
 * The one place that touches the OS InputConnection. Must be invoked ONLY on
 * the Android main thread. Wraps EVERY InputConnection call in an exception
 * barrier: a thrown/null/stale editor method returns false (refusal) instead
 * of crashing the IME UI; the controller fails closed and stops that lease.
 */
class InputConnectionSink(private val ic: InputConnection) : ComposingEditor {
    private var settledPreexistingForIc = false

    override fun settlePreexisting(): Boolean {
        if (settledPreexistingForIc) return true
        settledPreexistingForIc = true
        return try {
            ic.finishComposingText()
        } catch (_: Throwable) {
            settledPreexistingForIc = false
            false
        }
    }

    override fun setComposing(text: String): Boolean = try {
        ic.setComposingText(text, 1)
    } catch (_: Throwable) {
        false
    }

    override fun finish(): Boolean = try {
        ic.finishComposingText()
    } catch (_: Throwable) {
        false
    }
}
