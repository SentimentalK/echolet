package com.mainstayx.echolet

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Plain-JVM tests for the pure OS setup status resolver seam
 * [SetupStatusResolver] and [SetupOsStatus].
 *
 * Tests:
 * - Enabled but not selected
 * - Selected but permission denied
 * - All ready (usable)
 * - Unknown read (null / unreadable OS setting, never false-positive ready)
 * - State transitions across onResume refreshes
 * - Truthful guidance strings
 */
class SetupStatusResolverTest {

    private val targetShort = "com.mainstayx.echolet/.EcholetInputMethodService"
    private val targetFull = "com.mainstayx.echolet/com.mainstayx.echolet.EcholetInputMethodService"
    private val otherIme = "com.google.android.inputmethod.latin/com.android.inputmethod.latin.LatinIME"

    @Test
    fun `resolveSelectedState accurately detects selected Echolet short and full IDs`() {
        assertEquals(
            KeyboardSelectedState.SELECTED,
            SetupStatusResolver.resolveSelectedState(targetShort, targetShort, targetFull)
        )
        assertEquals(
            KeyboardSelectedState.SELECTED,
            SetupStatusResolver.resolveSelectedState(targetFull, targetShort, targetFull)
        )
        // Subtype suffix handling
        assertEquals(
            KeyboardSelectedState.SELECTED,
            SetupStatusResolver.resolveSelectedState("$targetShort;12345", targetShort, targetFull)
        )
    }

    @Test
    fun `resolveSelectedState detects unselected when another IME is default`() {
        assertEquals(
            KeyboardSelectedState.NOT_SELECTED,
            SetupStatusResolver.resolveSelectedState(otherIme, targetShort, targetFull)
        )
    }

    @Test
    fun `resolveSelectedState yields UNKNOWN when setting is null or unreadable`() {
        assertEquals(
            KeyboardSelectedState.UNKNOWN,
            SetupStatusResolver.resolveSelectedState(null, targetShort, targetFull)
        )
        assertEquals(
            KeyboardSelectedState.UNKNOWN,
            SetupStatusResolver.resolveSelectedState("", targetShort, targetFull)
        )
        assertEquals(
            KeyboardSelectedState.UNKNOWN,
            SetupStatusResolver.resolveSelectedState("   ", targetShort, targetFull)
        )
    }

    @Test
    fun `allPrerequisitesReady is true ONLY when mic granted, IME enabled, and IME selected`() {
        val allReady = SetupOsStatus(
            micGranted = true,
            imeEnabled = true,
            imeSelected = KeyboardSelectedState.SELECTED,
        )
        assertTrue(allReady.allPrerequisitesReady)
        assertTrue(SetupStatusResolver.deriveGuidance(allReady).contains("All OS prerequisites satisfied"))
    }

    @Test
    fun `enabled but not selected never reports all ready`() {
        val enabledNotSelected = SetupOsStatus(
            micGranted = true,
            imeEnabled = true,
            imeSelected = KeyboardSelectedState.NOT_SELECTED,
        )
        assertFalse(enabledNotSelected.allPrerequisitesReady)
        assertTrue(SetupStatusResolver.deriveGuidance(enabledNotSelected).contains("Select Echolet as your active keyboard"))
    }

    @Test
    fun `selected but permission denied never reports all ready`() {
        val selectedNoMic = SetupOsStatus(
            micGranted = false,
            imeEnabled = true,
            imeSelected = KeyboardSelectedState.SELECTED,
        )
        assertFalse(selectedNoMic.allPrerequisitesReady)
        assertTrue(SetupStatusResolver.deriveGuidance(selectedNoMic).contains("Grant microphone permission"))
    }

    @Test
    fun `unknown selected state never false-positives ready`() {
        val unknownSelection = SetupOsStatus(
            micGranted = true,
            imeEnabled = true,
            imeSelected = KeyboardSelectedState.UNKNOWN,
        )
        assertFalse("Must never false-positive ready when selection is unknown", unknownSelection.allPrerequisitesReady)
        assertTrue(SetupStatusResolver.deriveGuidance(unknownSelection).contains("Check in keyboard settings"))
    }

    @Test
    fun `isImeEnabled correctly checks enabled list`() {
        val enabledList = listOf(otherIme, targetShort)
        assertTrue(SetupStatusResolver.isImeEnabled(enabledList, targetShort, targetFull))

        val disabledList = listOf(otherIme)
        assertFalse(SetupStatusResolver.isImeEnabled(disabledList, targetShort, targetFull))
    }

    @Test
    fun `simulated onResume refresh transitions update status cleanly`() {
        // Initial launch: mic not granted, IME not enabled, not selected
        var state = SetupOsStatus(
            micGranted = false,
            imeEnabled = false,
            imeSelected = KeyboardSelectedState.NOT_SELECTED,
        )
        assertFalse(state.allPrerequisitesReady)

        // Step 1: User grants microphone
        state = state.copy(micGranted = true)
        assertFalse(state.allPrerequisitesReady)
        assertEquals("Android keyboard settings below.", SetupStatusResolver.deriveGuidance(state).substringAfter("Enable Echolet in "))

        // Step 2: User enables IME in settings and returns via onResume
        state = state.copy(imeEnabled = true)
        assertFalse(state.allPrerequisitesReady)
        assertTrue(SetupStatusResolver.deriveGuidance(state).contains("Select Echolet as your active keyboard"))

        // Step 3: User selects IME and returns via onResume
        state = state.copy(imeSelected = KeyboardSelectedState.SELECTED)
        assertTrue(state.allPrerequisitesReady)
    }
}
