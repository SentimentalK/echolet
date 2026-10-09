package com.mainstayx.echolet

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/**
 * Plain-JVM JUnit tests (no Android runtime) of the PURE lease/state-chart
 * decisions in [ImeSessionModel]. Each test owns a fresh instance — a new
 * service/controller never inherits sticky process-global state.
 */
class ImeSessionModelTest {
    private val PKG: String? = "com.notes"
    private val FIELD = 42
    private val INPUT = 0x80000

    private lateinit var model: ImeSessionModel

    @Before
    fun freshModel() {
        model = ImeSessionModel()
    }

    private fun leaseOf(outcome: ImeSessionModel.Outcome): ImeSessionModel.EditorLease {
        assertNotNull(outcome.lease)
        return outcome.lease!!
    }

    @Test
    fun fresh_instances_do_not_share_sticky_process_state() {
        val first = ImeSessionModel()
        first.onVisible(true, null, PKG, FIELD, INPUT)
        first.onTapControl(ready = true, reason = null, hasView = true, pkg = PKG, fieldId = FIELD, inputType = INPUT)
        assertEquals(ImeSessionModel.ImeState.PAUSED, first.currentState)
        // A new service lifetime starts clean: auto-listening when visible.
        val second = ImeSessionModel()
        assertEquals(ImeSessionModel.ImeState.HIDDEN, second.currentState)
    }

    @Test
    fun visibility_auto_starts_when_ready() {
        val outcome = model.onVisible(true, null, PKG, FIELD, INPUT)
        assertEquals(ImeSessionModel.ImeState.PREPARING, outcome.state)
        val lease = leaseOf(outcome)
        assertEquals("Preparing…", outcome.status)
        assertTrue(model.isLeaseCurrent(lease))
        val listening = model.generationListening(lease)
        assertEquals(ImeSessionModel.ImeState.LISTENING, listening.state)
        assertFalse(listening.noSideEffects)
    }

    @Test
    fun missing_prerequisite_blocks_no_mic_no_editor() {
        val outcome = model.onVisible(false, "Mic permission missing", PKG, FIELD, INPUT)
        assertEquals(ImeSessionModel.ImeState.BLOCKED, outcome.state)
        assertNull(outcome.lease)
        assertEquals("Mic permission missing", outcome.status)
        assertTrue(outcome.requestStop)
    }

    @Test
    fun repeated_hide_focus_stop_is_idempotent_and_always_fences() {
        val first = model.onVisible(true, null, PKG, FIELD, INPUT)
        val lease1 = leaseOf(first)
        val hidden = model.onHidden()
        assertEquals(ImeSessionModel.ImeState.HIDDEN, hidden.state)
        assertTrue(hidden.requestStop && hidden.closeNative)
        assertFalse(model.isLeaseCurrent(lease1))
        val before = model.currentEpoch()
        repeat(5) {
            val repeatHide = model.onHidden()
            assertTrue(repeatHide.requestStop && repeatHide.closeNative)
            assertEquals(ImeSessionModel.ImeState.HIDDEN, repeatHide.state)
        }
        assertTrue(model.currentEpoch() > before)
    }

    @Test
    fun manual_stop_persists_only_within_same_visibility_redraw() {
        val start = model.onVisible(true, null, PKG, FIELD, INPUT)
        model.generationListening(leaseOf(start))
        val stopped = model.onTapControl(
            ready = true, reason = null, hasView = true,
            pkg = PKG, fieldId = FIELD, inputType = INPUT,
        )
        assertEquals(ImeSessionModel.ImeState.PAUSED, stopped.state)
        assertTrue(stopped.requestStop)
        assertNull(stopped.lease)

        // Redraw of the SAME visible keyboard: manual pause persists.
        val redraw = model.onVisible(true, null, PKG, FIELD, INPUT)
        assertEquals(ImeSessionModel.ImeState.PAUSED, redraw.state)
        assertNull(redraw.lease) // NO auto start within this visibility

        // Hide/reopen is a FRESH visibility: manual pause does NOT persist.
        model.onHidden()
        val reopened = model.onVisible(true, null, PKG, FIELD, INPUT)
        assertEquals(ImeSessionModel.ImeState.PREPARING, reopened.state)
        assertTrue(model.isLeaseCurrent(reopened.lease)) // auto-start when ready
    }

    @Test
    fun fresh_visibility_reset_is_also_honoured_via_onFreshVisibility() {
        model.onVisible(true, null, PKG, FIELD, INPUT)
        model.onTapControl(ready = true, reason = null, hasView = true, pkg = PKG, fieldId = FIELD, inputType = INPUT)
        // Service marks a new visibility (window shown) even before onVisible.
        model.onFreshVisibility()
        val outcome = model.onVisible(true, null, PKG, FIELD, INPUT)
        assertEquals(ImeSessionModel.ImeState.PREPARING, outcome.state)
    }

    @Test
    fun stale_generationListening_is_a_noop_without_side_effects() {
        val start = model.onVisible(true, null, PKG, FIELD, INPUT)
        val lease1 = leaseOf(start)
        model.onHidden()
        val lease2 = leaseOf(model.onVisible(true, null, PKG, FIELD, INPUT))
        val stateBefore = model.currentState
        val late = model.generationListening(lease1)
        assertTrue(late.noSideEffects)
        assertEquals(stateBefore, late.state)
        assertTrue(model.currentState == ImeSessionModel.ImeState.PREPARING)
        assertTrue(model.isLeaseCurrent(lease2))
        assertFalse(model.isLeaseCurrent(lease1))
    }

    @Test
    fun preparing_canceled_before_native_open_finishes() {
        val start = model.onVisible(true, null, PKG, FIELD, INPUT)
        val lease1 = leaseOf(start)
        val cancel = model.stop("user stop while preparing")
        assertFalse(model.isLeaseCurrent(lease1))
        assertTrue(cancel.requestStop && cancel.closeNative)
    }

    @Test
    fun old_lease_delayed_events_rejected_after_new_editor() {
        val first = model.onVisible(true, null, PKG, FIELD, INPUT)
        val lease1 = leaseOf(first)
        model.onEditorRebinding(hasView = true, pkg = PKG, fieldId = FIELD, inputType = INPUT)
        assertFalse(model.isLeaseCurrent(lease1))

        val second = model.onVisible(true, null, PKG, FIELD, INPUT)
        val lease2 = leaseOf(second)
        assertTrue(model.isLeaseCurrent(lease2))
        assertFalse(model.isLeaseCurrent(lease1))
        assertTrue(lease1.epoch != lease2.epoch && lease1.nonce != lease2.nonce)
    }

    @Test
    fun field_id_reuse_keeps_leases_distinct() {
        val leaseA = leaseOf(model.onVisible(true, null, PKG, FIELD, INPUT))
        model.stop("switch")
        val b = model.onVisible(true, null, PKG, FIELD, INPUT)
        val leaseB = leaseOf(b)
        assertEquals(leaseA.fieldId, leaseB.fieldId) // SAME reused fieldId
        assertTrue(leaseA.nonce != leaseB.nonce && leaseA.epoch != leaseB.epoch)
        assertTrue(model.isLeaseCurrent(leaseB))
        assertFalse(model.isLeaseCurrent(leaseA))
    }

    @Test
    fun ten_start_stop_cycles_never_reuse_a_stale_generation() {
        val issued = HashSet<ImeSessionModel.EditorLease>()
        var previousEpoch = model.currentEpoch()
        repeat(10) {
            val lease = leaseOf(model.onVisible(true, null, PKG, FIELD, INPUT))
            assertTrue(issued.none { it.epoch == lease.epoch && it.nonce == lease.nonce })
            issued.add(lease)
            assertTrue(model.currentEpoch() > previousEpoch)
            previousEpoch = model.currentEpoch()
            val stop = model.stop("cycle end")
            assertFalse(model.isLeaseCurrent(lease))
            assertTrue(stop.requestStop && stop.closeNative)
        }
    }

    @Test
    fun editor_switch_counts_as_stop_even_without_finish_input_view() {
        val lease = leaseOf(model.onVisible(true, null, PKG, FIELD, INPUT))
        val rebind = model.onEditorRebinding(
            hasView = true, pkg = PKG, fieldId = FIELD, inputType = INPUT,
        )
        assertTrue(rebind.requestStop && rebind.closeNative)
        assertFalse(model.isLeaseCurrent(lease))
    }

    @Test
    fun late_generationFailure_from_old_generation_changes_nothing() {
        val first = model.onVisible(true, null, PKG, FIELD, INPUT)
        val lease1 = leaseOf(first)
        model.stop("stop A")
        val second = model.onVisible(true, null, PKG, FIELD, INPUT)
        val lease2 = leaseOf(second)
        assertTrue(model.isLeaseCurrent(lease2))
        val failed = model.generationFailed(lease2, "microphone read failed")
        assertEquals(ImeSessionModel.ImeState.BLOCKED, failed.state)
        assertFalse(failed.noSideEffects)
        assertFalse(model.isLeaseCurrent(lease2))

        val stateBefore = model.currentState
        val epochBefore = model.currentEpoch()
        // A LATE failure from the OLD generation: strict no-op.
        val stale = model.generationFailed(lease1, "native feed failed")
        assertTrue(stale.noSideEffects)
        assertEquals(stateBefore, stale.state)
        assertEquals(epochBefore, model.currentEpoch())
        assertEquals(stateBefore, model.currentState)
        assertFalse(model.isLeaseCurrent(lease1))
    }

    @Test
    fun error_marks_blocked_when_visible_hidden_when_not() {
        val lease = leaseOf(model.onVisible(true, null, PKG, FIELD, INPUT))
        val failed = model.generationFailed(lease, "microphone read failed")
        assertEquals(ImeSessionModel.ImeState.BLOCKED, failed.state)
        assertTrue(failed.requestStop && failed.closeNative)
        assertFalse(model.isLeaseCurrent(lease))

        val lease2 = leaseOf(model.onVisible(true, null, PKG, FIELD, INPUT))
        model.onHidden()
        val failed2 = model.generationFailed(lease2, "native feed failed")
        assertEquals(ImeSessionModel.ImeState.HIDDEN, failed2.state)
    }

    @Test
    fun hidden_invalidates_even_when_paused() {
        model.onVisible(true, null, PKG, FIELD, INPUT)
        val pause = model.onTapControl(
            ready = true, reason = null, hasView = true,
            pkg = PKG, fieldId = FIELD, inputType = INPUT,
        )
        assertEquals(ImeSessionModel.ImeState.PAUSED, pause.state)
        val hidden = model.onHidden()
        assertEquals(ImeSessionModel.ImeState.HIDDEN, hidden.state)
        assertTrue(hidden.requestStop) // mic released regardless of PAUSED
        // Manual pause cleared for the NEXT visibility.
        val reopened = model.onVisible(true, null, PKG, FIELD, INPUT)
        assertEquals(ImeSessionModel.ImeState.PREPARING, reopened.state)
    }
}
