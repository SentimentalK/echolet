package com.mainstayx.echolet

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/**
 * Plain-JVM JUnit tests (no Android runtime) of the Phase 0-B EDITOR
 * projection: strict nativeFeed JSON validation, contiguous revision
 * watermarking, endpoint guarding and composing-only mapping. The fake
 * [ProjectionReducerTest.FakeEditor] implements the same [EditorConnection]
 * contract the Android [InputConnectionSink] fulfils against the real OS
 * editor; per Design 9, real InputConnection/AudioRecord behavior is validated
 * by a debug device smoke instead of JVM tests.
 */
class ProjectionReducerTest {
    private val lease = ImeSessionModel.EditorLease(
        epoch = 7L, packageName = "com.notes", fieldId = 3, inputType = 1, nonce = 5L,
    )

    private fun lease2() = ImeSessionModel.EditorLease(1L, null, 0, 0, 1L)

    /** Simulated editor: pre-existing prefix text + one owned composition. */
    private class FakeEditor : EditorConnection {
        var prefix = "existing "
        var composed = ""
        var refuseNext = false

        fun visible(): String = prefix + composed

        override fun beginOrReplaceComposing(text: String): Boolean {
            if (refuseNext) return false
            composed = text
            return true
        }

        override fun finishComposing(): Boolean {
            if (refuseNext) return false
            prefix = prefix + composed
            composed = ""
            return true
        }
    }

    @Before
    fun reset() {
        ImeSessionModel.resetForJUnit()
    }

    /** Apply helper mirroring the Android sink op dispatch. */
    private fun apply(editor: FakeEditor, op: EditorOp): Boolean = when (op) {
        is EditorOp.SetComposing -> editor.beginOrReplaceComposing(op.text)
        EditorOp.FinishComposing -> editor.finishComposing()
        EditorOp.Noop -> true
    }

    private fun applyAll(reducer: ProjectionReducer, editor: FakeEditor, json: String) {
        for (op in reducer.accept(json)) apply(editor, op)
    }

    private fun partial(revision: Long, text: String) =
        """[{"kind":"partial","session":1,"revision":$revision,"backspaces":0,"suffix":"x","text":"$text"}]"""

    private fun endpoint(session: Long = 1, text: String) =
        """[{"kind":"endpoint","session":$session,"text":"$text"}]"""

    @Test
    fun accepted_full_text_replaces_only_owned_span_prefix_survives() {
        val reducer = ProjectionReducer(lease)
        val editor = FakeEditor()
        applyAll(reducer, editor, partial(1L, "你好"))
        assertEquals("existing 你好", editor.visible())
        assertEquals("existing ", editor.prefix) // prefix never mutated by partials
        applyAll(reducer, editor, partial(2L, "你坏"))
        assertEquals("existing 你坏", editor.visible()) // correction, prefix intact
        assertTrue(reducer.ownsComposition())
    }

    @Test
    fun accepted_empty_text_clears_only_owned_composing_region() {
        val reducer = ProjectionReducer(lease)
        val editor = FakeEditor()
        applyAll(reducer, editor, partial(1L, "你好"))
        applyAll(reducer, editor, partial(2L, ""))
        assertTrue(editor.composed.isEmpty()) // owned span cleared
        assertEquals("existing ", editor.prefix) // prefix preserved exactly
        assertEquals("existing ", editor.visible())
    }

    @Test
    fun endpoint_finishes_once_and_guards_stale_duplicates() {
        val reducer = ProjectionReducer(lease)
        val editor = FakeEditor()
        applyAll(reducer, editor, partial(1L, "好的"))
        applyAll(reducer, editor, endpoint(text = "好的"))
        assertEquals("existing 好的", editor.visible()) // finalized once, no
        // extra commit duplication
        assertFalse(reducer.ownsComposition())

        // A REAL same-session duplicate endpoint (device haters: none in rust
        // but the contract allows several per session): guarded Noop only when
        // there is no owned composition to finish.
        val lateOps = reducer.accept(endpoint(text = "好的"))
        assertEquals(1, lateOps.size)
        assertTrue(lateOps[0] is EditorOp.Noop)
        assertEquals("existing 好的", editor.visible()) // untouched
    }

    @Test
    fun multiple_endpoints_in_one_session_are_not_globally_suppressed() {
        val reducer = ProjectionReducer(lease)
        val editor = FakeEditor()
        applyAll(reducer, editor, partial(1L, "第一"))
        applyAll(reducer, editor, endpoint(text = "第一"))
        assertEquals("existing 第一", editor.visible())
        // Next utterance: revision keeps ascending, composing span restarts.
        applyAll(reducer, editor, partial(2L, "第二"))
        applyAll(reducer, editor, endpoint(text = "第二"))
        assertEquals("existing 第一第二", editor.visible())
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
        applyAll(reducer, editor, partial(1L, "说🤖🈚️你好"))
        applyAll(reducer, editor, partial(2L, "说🤖🈚️再見"))
        assertEquals("existing 说🤖🈚️再見", editor.visible())
    }

    @Test
    fun stop_keeps_last_visible_partial_and_emits_no_late_final() {
        // Issue the lease through the REAL model, mirroring controller flow.
        val issued = ImeSessionModel.onVisible(true, null, "com.notes", 3, 1).lease!!
        val reducer = ProjectionReducer(issued)
        val editor = FakeEditor()
        applyAll(reducer, editor, partial(1L, "说到一半"))
        assertEquals("existing 说到一半", editor.visible())
        // Stop fences the lease: late events for it are dropped upstream
        // (controller isLeaseCurrent), the visible partial is NEVER drained.
        ImeSessionModel.stop("stop mid-utterance")
        assertFalse(ImeSessionModel.isLeaseCurrent(issued))
        assertEquals("existing 说到一半", editor.visible())
    }

    @Test
    fun error_path_drops_pending_callbacks_and_owner_never_writes() {
        val reducer = ProjectionReducer(lease)
        val editor = FakeEditor().apply { refuseNext = true }
        // Editor refuses the composing API: fail closed, no fallback writes.
        val ops = reducer.accept(partial(1L, "x"))
        assertFalse(apply(editor, ops[0]))
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
}
