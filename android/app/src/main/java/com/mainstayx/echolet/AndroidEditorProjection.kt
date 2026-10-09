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
 * Pure projection reducer for one editor lease/native generation.
 * Throws IllegalArgumentException on malformed wire payloads (fail closed:
 * caller stops capture rather than writing anything unvalidated).
 */
class ProjectionReducer(private val lease: ImeSessionModel.EditorLease) {
    private var session: Long = -1
    private var lastRevision: Long = -1
    private var hasOwnedComposition = false
    private var sawEndpointText = false

    fun lease(): ImeSessionModel.EditorLease = lease
    fun nativeSession(): Long = session
    fun lastRevision(): Long = lastRevision
    fun ownsComposition(): Boolean = hasOwnedComposition

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
                    sawEndpointText = false
                    hasOwnedComposition = true
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
                    // guarded so a stale/double endpoint with no owned
                    // composition is a Noop instead of a duplicate finish.
                    if (sawEndpointText || !hasOwnedComposition) {
                        ops.add(EditorOp.Noop)
                    } else {
                        sawEndpointText = true
                        hasOwnedComposition = false
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
 * the Android main thread. Every method returns false on editor refusal so the
 * caller fails closed and stops (no destructive backspace fallback).
 */
class InputConnectionSink(
    private val lease: ImeSessionModel.EditorLease,
    private val ic: InputConnection,
) : EditorConnection {
    private var settledPreexisting = false

    override fun beginOrReplaceComposing(text: String): Boolean {
        if (!settledPreexisting) {
            // First write: settle any pre-existing composition WITHOUT
            // deleting prefix text (finishComposingText keeps the text).
            settledPreexisting = true
            ic.finishComposingText()
        }
        // The sink targets exactly the binding this lease captured; the
        // controller re-validates the live current InputConnection before
        // calling in, so no cross-editor write can originate here.
        return ic.setComposingText(text, 1)
    }

    override fun finishComposing(): Boolean = ic.finishComposingText()
}

/**
 * Thinner interface used by both the controller and the JUnit fake: the sink
 * is the only OS boundary, everything above it is pure.
 */
interface EditorConnection {
    fun beginOrReplaceComposing(text: String): Boolean
    fun finishComposing(): Boolean
}
