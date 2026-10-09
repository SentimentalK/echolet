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
 * decisions in [ImeSessionModel].
 */
class ImeSessionModelTest {
    private val PKG: String? = "com.notes"
    private val FIELD = 42
    private val INPUT = 0x80000

    @Before
    fun reset() {
        ImeSessionModel.resetForJUnit()
    }

    private fun leaseOf(outcome: ImeSessionModel.Outcome): ImeSessionModel.EditorLease {
        assertNotNull(outcome.lease)
        return outcome.lease!!
    }

    @Test
    fun visibility_auto_starts_when_ready() {
        val outcome = ImeSessionModel.onVisible(true, null, PKG, FIELD, INPUT)
        assertEquals(ImeSessionModel.ImeState.PREPARING, outcome.state)
        val lease = leaseOf(outcome)
        assertEquals("Preparing…", outcome.status)
        assertTrue(ImeSessionModel.isLeaseCurrent(lease))
        val listening = ImeSessionModel.generationListening(lease)
        assertEquals(ImeSessionModel.ImeState.LISTENING, listening.state)
    }

    @Test
    fun missing_prerequisite_blocks_no_mic_no_editor() {
        val outcome = ImeSessionModel.onVisible(false, "Mic permission missing", PKG, FIELD, INPUT)
        assertEquals(ImeSessionModel.ImeState.BLOCKED, outcome.state)
        assertNull(outcome.lease)
        assertEquals("Mic permission missing", outcome.status)
        assertTrue(outcome.requestStop)
    }

    @Test
    fun repeated_hide_focus_stop_is_idempotent_and_always_fences() {
        val first = ImeSessionModel.onVisible(true, null, PKG, FIELD, INPUT)
        val lease1 = leaseOf(first)
        val hidden = ImeSessionModel.onHidden()
        assertEquals(ImeSessionModel.ImeState.HIDDEN, hidden.state)
        assertTrue(hidden.requestStop && hidden.closeNative)
        assertFalse(ImeSessionModel.isLeaseCurrent(lease1))
        val before = ImeSessionModel.currentEpoch()
        repeat(5) {
            val repeatHide = ImeSessionModel.onHidden()
            assertTrue(repeatHide.requestStop && repeatHide.closeNative)
            assertEquals(ImeSessionModel.ImeState.HIDDEN, repeatHide.state)
        }
        assertTrue(ImeSessionModel.currentEpoch() > before)
    }

    @Test
    fun paused_does_not_auto_resume_on_next_visibility() {
        val start = ImeSessionModel.onVisible(true, null, PKG, FIELD, INPUT)
        ImeSessionModel.generationListening(leaseOf(start))
        val stopped = ImeSessionModel.onTapControl(
            ready = true, reason = null, hasView = true,
            pkg = PKG, fieldId = FIELD, inputType = INPUT,
        )
        assertEquals(ImeSessionModel.ImeState.PAUSED, stopped.state)
        assertTrue(stopped.requestStop)
        assertNull(stopped.lease)

        ImeSessionModel.onHidden()
        val reopened = ImeSessionModel.onVisible(true, null, PKG, FIELD, INPUT)
        assertEquals(ImeSessionModel.ImeState.PAUSED, reopened.state)
        assertNull(reopened.lease) // NO auto start

        val resumed = ImeSessionModel.onTapControl(
            ready = true, reason = null, hasView = true,
            pkg = PKG, fieldId = FIELD, inputType = INPUT,
        )
        assertEquals(ImeSessionModel.ImeState.PREPARING, resumed.state)
        assertTrue(ImeSessionModel.isLeaseCurrent(resumed.lease))
    }

    @Test
    fun preparing_canceled_before_native_open_finishes() {
        val start = ImeSessionModel.onVisible(true, null, PKG, FIELD, INPUT)
        val lease1 = leaseOf(start)
        val cancel = ImeSessionModel.stop("user stop while preparing")
        assertFalse(ImeSessionModel.isLeaseCurrent(lease1))
        assertTrue(cancel.requestStop && cancel.closeNative)
    }

    @Test
    fun old_lease_delayed_events_rejected_after_new_editor() {
        val first = ImeSessionModel.onVisible(true, null, PKG, FIELD, INPUT)
        val lease1 = leaseOf(first)
        ImeSessionModel.onEditorRebinding(hasView = true, pkg = PKG, fieldId = FIELD, inputType = INPUT)
        assertFalse(ImeSessionModel.isLeaseCurrent(lease1))

        val second = ImeSessionModel.onVisible(true, null, PKG, FIELD, INPUT)
        val lease2 = leaseOf(second)
        assertTrue(ImeSessionModel.isLeaseCurrent(lease2))
        assertFalse(ImeSessionModel.isLeaseCurrent(lease1))
        assertTrue(lease1.epoch != lease2.epoch && lease1.nonce != lease2.nonce)
    }

    @Test
    fun field_id_reuse_keeps_leases_distinct() {
        val leaseA = leaseOf(ImeSessionModel.onVisible(true, null, PKG, FIELD, INPUT))
        ImeSessionModel.stop("switch")
        val b = ImeSessionModel.onVisible(true, null, PKG, FIELD, INPUT)
        val leaseB = leaseOf(b)
        assertEquals(leaseA.fieldId, leaseB.fieldId) // SAME reused fieldId
        assertTrue(leaseA.nonce != leaseB.nonce && leaseA.epoch != leaseB.epoch)
        assertTrue(ImeSessionModel.isLeaseCurrent(leaseB))
        assertFalse(ImeSessionModel.isLeaseCurrent(leaseA))
    }

    @Test
    fun ten_start_stop_cycles_never_reuse_a_stale_generation() {
        val issued = HashSet<ImeSessionModel.EditorLease>()
        var previousEpoch = ImeSessionModel.currentEpoch()
        repeat(10) {
            val lease = leaseOf(ImeSessionModel.onVisible(true, null, PKG, FIELD, INPUT))
            assertTrue(issued.none { it.epoch == lease.epoch && it.nonce == lease.nonce })
            issued.add(lease)
            assertTrue(ImeSessionModel.currentEpoch() > previousEpoch)
            previousEpoch = ImeSessionModel.currentEpoch()
            val stop = ImeSessionModel.stop("cycle end")
            assertFalse(ImeSessionModel.isLeaseCurrent(lease))
            assertTrue(stop.requestStop && stop.closeNative)
        }
    }

    @Test
    fun editor_switch_counts_as_stop_even_without_finish_input_view() {
        val lease = leaseOf(ImeSessionModel.onVisible(true, null, PKG, FIELD, INPUT))
        val rebind = ImeSessionModel.onEditorRebinding(
            hasView = true, pkg = PKG, fieldId = FIELD, inputType = INPUT,
        )
        assertTrue(rebind.requestStop && rebind.closeNative)
        assertFalse(ImeSessionModel.isLeaseCurrent(lease))
    }

    @Test
    fun error_marks_blocked_when_visible_hidden_when_not() {
        val lease = leaseOf(ImeSessionModel.onVisible(true, null, PKG, FIELD, INPUT))
        val failed = ImeSessionModel.generationFailed(lease, "microphone read failed")
        assertEquals(ImeSessionModel.ImeState.BLOCKED, failed.state)
        assertTrue(failed.requestStop && failed.closeNative)
        assertFalse(ImeSessionModel.isLeaseCurrent(lease))

        val lease2 = leaseOf(ImeSessionModel.onVisible(true, null, PKG, FIELD, INPUT))
        ImeSessionModel.onHidden()
        val failed2 = ImeSessionModel.generationFailed(lease2, "native feed failed")
        assertEquals(ImeSessionModel.ImeState.HIDDEN, failed2.state)
    }

    @Test
    fun hidden_invalidates_even_when_paused() {
        ImeSessionModel.onVisible(true, null, PKG, FIELD, INPUT)
        val pause = ImeSessionModel.onTapControl(
            ready = true, reason = null, hasView = true,
            pkg = PKG, fieldId = FIELD, inputType = INPUT,
        )
        assertEquals(ImeSessionModel.ImeState.PAUSED, pause.state)
        val hidden = ImeSessionModel.onHidden()
        assertEquals(ImeSessionModel.ImeState.HIDDEN, hidden.state)
        assertTrue(hidden.requestStop) // mic released regardless of PAUSED
    }
}
