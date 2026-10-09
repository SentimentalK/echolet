package com.mainstayx.echolet

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Plain-JVM JUnit tests (no Android runtime) of the Phase 0-B EDITOR
 * projection: strict nativeFeed JSON validation, contiguous revision
 * watermarking, endpoint guarding and composing-only mapping. The batches are
 * applied through the REAL per-lease [CompositionOwner] over a fake
 * [ComposingEditor] implementing the same contract the Android
 * [InputConnectionSink] fulfils against the OS editor — so the persistent
 * one-owner-per-lease lifecycle is genuinely exercised (a fresh writer per
 * batch reproduces the `partial partial…` commit duplication bug this file
 * historically prevents). Real InputConnection/AudioRecord behavior is
 * validated on device smoke instead of JVM.
 */
class ProjectionReducerTest {
    private val lease = ImeSessionModel.EditorLease(
        epoch = 7L, packageName = "com.notes", fieldId = 3, inputType = 1, nonce = 5L,
    )

    private fun lease2() = ImeSessionModel.EditorLease(1L, null, 0, 0, 1L)

    /**
     * Simulated editor: pre-existing prefix text + Echolet-owned composing
     * span; beginOrReplaceComposing REPLACES the owned span only.
     */
    private class FakeEditor : ComposingEditor {
        var prefix = "existing "
        var composed = ""
        var disposes = 0
        var refuseNext = false
        var throwNext = false

        fun visible(): String = prefix + composed

        override fun settlePreexisting(): Boolean {
            if (refuseNext) return false
            if (throwNext) {
                throwNext = false
                throw RuntimeException("editor threw at settle")
            }
            // Finishing the pre-existing composition commits into prefix.
            prefix += composed
            composed = ""
            return true
        }

        override fun setComposing(text: String): Boolean {
            if (refuseNext) return false
            if (throwNext) {
                throwNext = false
                throw RuntimeException("editor threw at setComposing")
            }
            composed = text
            return true
        }

        override fun finish(): Boolean {
            if (refuseNext) return false
            if (throwNext) {
                throwNext = false
                throw RuntimeException("editor threw at finish")
            }
            prefix += composed
            composed = ""
            disposes += 1
            return true
        }
    }

    /** One owner per lease, spanning every batch (real lifecycle). */
    private fun owner(editor: FakeEditor) = ProjectionReducerOwnerProxy(editor)

    /** Applies a validated batch through the persistent composition owner. */
    private fun applyAll(
        reducer: ProjectionReducer,
        editor: FakeEditor,
        memo: ProjectionReducerOwnerProxy,
        json: String,
    ) {
        assertTrue(memo.applyVia(reducer.accept(json)))
    }

    /** Small wrapper so tests go through CompositionOwner, not raw ops. */
    private class ProjectionReducerOwnerProxy(private val editor: ComposingEditor) {
        private val owner = CompositionOwner(editor)

        fun applyVia(ops: List<EditorOp>): Boolean {
            return owner.apply(ops)
        }

        fun owns(): Boolean = owner.ownsComposition()
        fun finishOwnedNow(): Boolean = owner.finishOwnedIfAny()
    }

    private fun partial(revision: Long, text: String) =
        """[{"kind":"partial","session":1,"revision":$revision,"backspaces":0,"suffix":"x","text":"$text"}]"""

    private fun endpoint(session: Long = 1, text: String) =
        """[{"kind":"endpoint","session":$session,"text":"$text"}]"""

    @Test
    fun accepted_full_text_replaces_only_owned_span_prefix_survives() {
        val reducer = ProjectionReducer(lease)
        val editor = FakeEditor()
        val chip = owner(editor)
        applyAll(reducer, editor, chip, partial(1L, "你好"))
        assertEquals("existing 你好", editor.visible())
        assertEquals("existing ", editor.prefix) // prefix never mutated by partials
        assertTrue(chip.owns())
        // SECOND partial in a DISTINCT batch must REPLACE, never duplicate.
        applyAll(reducer, editor, chip, partial(2L, "你们好"))
        assertEquals("existing 你们好", editor.visible())
        assertEquals(0, editor.disposes) // nothing committed until endpoint
    }

    @Test
    fun two_batches_through_one_owner_must_not_duplicate() {
        // The wire sends batches through ONE lease-scoped reducer + owner.
        // Two DISTINCT batches arrive: reducer revision watermark continues
        // and the persistent owner replaces, never duplicates.
        val reducer = ProjectionReducer(lease)
        val editor = FakeEditor()
        val chip = owner(editor)
        applyAll(reducer, editor, chip, partial(1L, "你好"))
        applyAll(reducer, editor, chip, partial(2L, "你们好"))
        assertEquals("existing 你们好", editor.visible())
        assertEquals("existing ", editor.prefix)
        assertEquals(0, editor.disposes)
    }

    @Test
    fun accepted_empty_text_clears_only_owned_composing_region() {
        val reducer = ProjectionReducer(lease)
        val editor = FakeEditor()
        val chip = owner(editor)
        applyAll(reducer, editor, chip, partial(1L, "你好"))
        applyAll(reducer, editor, chip, partial(2L, ""))
        assertTrue(editor.composed.isEmpty()) // owned span cleared
        assertEquals("existing ", editor.prefix) // prefix preserved exactly
        assertEquals("existing ", editor.visible())
    }

    @Test
    fun endpoint_finishes_once_and_guards_stale_duplicates() {
        val reducer = ProjectionReducer(lease)
        val editor = FakeEditor()
        val chip = owner(editor)
        applyAll(reducer, editor, chip, partial(1L, "好的"))
        applyAll(reducer, editor, chip, endpoint(text = "好的"))
        assertEquals("existing 好的", editor.visible()) // finalized once, no
        // extra commit duplication
        assertFalse(chip.owns())
        assertEquals(1, editor.disposes)

        // A REAL same-session duplicate endpoint: guarded Noop only when
        // there is no owned composition to finish.
        val lateOps = reducer.accept(endpoint(text = "好的"))
        assertEquals(1, lateOps.size)
        assertTrue(lateOps[0] is EditorOp.Noop)
        applyAll(reducer, editor, chip, "[]")
        assertEquals("existing 好的", editor.visible()) // untouched
        assertEquals(1, editor.disposes) // second finish never happened
    }

    @Test
    fun multiple_endpoints_in_one_session_are_not_globally_suppressed() {
        val reducer = ProjectionReducer(lease)
        val editor = FakeEditor()
        val chip = owner(editor)
        applyAll(reducer, editor, chip, partial(1L, "第一"))
        applyAll(reducer, editor, chip, endpoint(text = "第一"))
        assertEquals("existing 第一", editor.visible())
        // Next utterance: revision keeps ascending, composing span restarts.
        applyAll(reducer, editor, chip, partial(2L, "第二"))
        applyAll(reducer, editor, chip, endpoint(text = "第二"))
        assertEquals("existing 第一第二", editor.visible())
        assertEquals(2, editor.disposes)
    }

    @Test
    fun stop_mid_partial_preserves_exact_visible_text_without_extra_commit() {
        val reducer = ProjectionReducer(lease)
        val editor = FakeEditor()
        val chip = owner(editor)
        applyAll(reducer, editor, chip, partial(1L, "说到一半"))
        assertEquals("existing 说到一半", editor.visible())
        assertTrue(chip.owns())
        // Stop: finalize ONLY what's already visible; appends nothing.
        assertTrue(chip.finishOwnedNow())
        assertEquals("existing 说到一半", editor.visible())
        assertTrue(editor.composed.isEmpty())
        assertEquals(1, editor.disposes) // exactly one settle-free commit
        assertTrue(chip.finishOwnedNow()) // idempotent: no second commit
        assertEquals(1, editor.disposes)
        assertEquals("existing 说到一半", editor.visible())
    }

    @Test
    fun monotonic_contiguous_revisions_enforced() {
        val reducer = ProjectionReducer(lease)
        reducer.accept(partial(1L, "a"))
        // duplicate revision
        assertThrows(IllegalStateException::class.java) { reducer.accept(partial(1L, "b")) }
        // out-of-order across separate reducers
        val out = ProjectionReducer(lease2())
        out.accept(partial(1L, "a"))
        assertThrows(IllegalStateException::class.java) { out.accept(partial(3L, "b")) }
        assertThrows(IllegalStateException::class.java) { out.accept(partial(1L, "dup")) }
    }

    @Test
    fun interleaved_native_session_ids_rejected() {
        val reducer = ProjectionReducer(lease)
        reducer.accept(partial(1L, "a"))
        val alien =
            "[{\"kind\":\"partial\",\"session\":2,\"revision\":2,\"backspaces\":0,\"suffix\":\"\",\"text\":\"x\"}]"
        assertThrows(IllegalStateException::class.java) { reducer.accept(alien) }
    }

    @Test
    fun malformed_wire_payloads_fail_closed_without_writes() {
        val reducer = ProjectionReducer(lease)
        val editor = FakeEditor()
        val chip = owner(editor)
        val malformed = listOf(
            """[{"kind":"weird"}]""",
            """[{"kind":"partial","session":0,"revision":1,"backspaces":0,"suffix":"","text":"a"}]""",
            """[{"kind":"partial","session":1,"revision":0,"backspaces":0,"suffix":"","text":"a"}]""",
            """[{"kind":"partial","session":1}]""",
            """[{"kind":"partial","session":1,"revision":1,"backspaces":-2,"suffix":"","text":"a"}]""",
            """[{"kind":"partial","session":1,"revision":1,"suffix":"","text":"a"}]""",
            """[{"kind":"partial","session":1,"revision":1,"backspaces":0,"text":"a"}]""",
            """[{"kind":"partial","session":1,"revision":1,"backspaces":0,"suffix":""}]""",
            """[{"kind":"endpoint","session":1,"text":""}]""",
        )
        for (json in malformed) {
            val before = editor.visible()
            assertThrows(IllegalArgumentException::class.java) {
                reducer.accept(json)
            }
            assertEquals(before, editor.visible()) // fail closed, no editor write
        }
        assertTrue(reducer.accept("[]").isEmpty())
    }

    @Test
    fun surrogate_characters_handled_as_full_text() {
        val reducer = ProjectionReducer(lease)
        val editor = FakeEditor()
        val chip = owner(editor)
        applyAll(reducer, editor, chip, partial(1L, "说🤖🈚️你好"))
        applyAll(reducer, editor, chip, partial(2L, "说🤖🈚️再見"))
        assertEquals("existing 说🤖🈚️再見", editor.visible())
    }

    @Test
    fun editor_refusal_fails_closed_without_any_owner_write() {
        val reducer = ProjectionReducer(lease)
        val editor = FakeEditor().apply { refuseNext = true }
        val chip = owner(editor)
        // Editor refuses the settle step: fail closed, no fallback writes.
        val ops = reducer.accept(partial(1L, "x"))
        assertFalse(chip.applyVia(ops))
        assertEquals("existing ", editor.visible())
        assertFalse(chip.owns())
    }

    @Test
    fun editor_thrown_exception_is_refusal_never_a_crash() {
        val reducer = ProjectionReducer(lease)
        val editor = FakeEditor().apply { throwNext = true }
        val chip = owner(editor)
        val ops = reducer.accept(partial(1L, "y"))
        assertFalse(chip.applyVia(ops)) // caught, mapped to refusal
        assertEquals("existing ", editor.visible()) // nothing was written
    }

    @Test
    fun first_write_of_a_generation_targets_the_owner_lease() {
        val reducer = ProjectionReducer(lease)
        assertEquals(lease, reducer.lease())
        assertEquals(-1L, reducer.nativeSession())
        reducer.accept("""[{"kind":"partial","session":9,"revision":1,"backspaces":0,"suffix":"","text":"初次"}]""")
        assertEquals(9L, reducer.nativeSession()) // stable session id locked in
        val ops2 = reducer.accept("""[{"kind":"partial","session":9,"revision":2,"backspaces":0,"suffix":"","text":"初次第"}]""")
        assertEquals(1, ops2.size)
    }

    @Test
    fun stop_keeps_last_visible_partial_and_emits_no_late_final() {
        val model = ImeSessionModel()
        // Issue the lease through the REAL model, mirroring controller flow.
        val issued = model.onVisible(true, null, "com.notes", 3, 1).lease!!
        val reducer = ProjectionReducer(issued)
        val editor = FakeEditor()
        val chip = owner(editor)
        applyAll(reducer, editor, chip, partial(1L, "说到一半"))
        assertEquals("existing 说到一半", editor.visible())
        // Stop fences the lease: late events for it are dropped upstream
        // (controller isLeaseCurrent), the visible partial is NEVER drained.
        model.stop("stop mid-utterance")
        assertFalse(model.isLeaseCurrent(issued))
        assertEquals("existing 说到一半", editor.visible()) // exact partial kept
    }
}
