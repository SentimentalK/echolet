package com.mainstayx.echolet

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Plain-JVM JUnit tests of the pure keyboard-shell presenter
 * [ImeKeyboardPresenter]: the Compact/Expanded layout state machine and the
 * BLOCKED-only setup action. These are behavior tests of the real presenter
 * (not source-grep shape checks). The presenter has NO controller reference
 * and no request channel other than the injected setup sink, so a session
 * action from a layout toggle is impossible by construction; these tests pin
 * that contract by counting every invocation of the injected sink.
 */
class ImeKeyboardPresenterTest {

    private val ready = ImePrerequisites(
        permissionGranted = true, modelStaged = true, nativeReady = true,
    )

    private val noPermission = ImePrerequisites(
        permissionGranted = false, modelStaged = true, nativeReady = true,
    )

    private val noModel = ImePrerequisites(
        permissionGranted = true, modelStaged = false, nativeReady = true,
    )

    private val noNative = ImePrerequisites(
        permissionGranted = true, modelStaged = true, nativeReady = false,
    )

    /** Presenter whose ONLY side channel is a counted test sink. */
    private fun countingPresenter(): Pair<ImeKeyboardPresenter, MutableList<String>> {
        val events = mutableListOf<String>()
        val presenter = ImeKeyboardPresenter(onSetupRequested = { events.add("setup") })
        return presenter to events
    }

    // --------------------------------------------------- initial layout mode

    @Test
    fun `initial layout is COMPACT when ready staged model and runtime are usable`() {
        val (presenter, events) = countingPresenter()
        val plan = presenter.onFreshVisibility(ready)
        assertEquals(ImeLayoutMode.COMPACT, plan.layout)
        assertEquals("Start Listening", plan.primaryLabel)
        assertEquals("no expanded panel in COMPACT", null, plan.expandedPanel)
        assertEquals("no action from a fresh visibility", 0, events.size)
    }

    @Test
    fun `initial layout is EXPANDED when the staged model is missing`() {
        val (presenter, _) = countingPresenter()
        val plan = presenter.onFreshVisibility(noModel)
        assertEquals(ImeLayoutMode.EXPANDED, plan.layout)
        val panel = plan.expandedPanel
        assertTrue(panel != null)
        panel!!
        assertTrue(panel.prerequisiteLines.any { !it.ok && it.title.contains("Voice model") })
        assertTrue(panel.prerequisiteLines.none { it.title.contains("Select") })
        assertTrue(panel.prerequisiteLines.none { it.title.contains("Download") })
    }

    @Test
    fun `initial layout is EXPANDED when permission is missing and explanation stays truthful`() {
        val (presenter, _) = countingPresenter()
        val plan = presenter.onFreshVisibility(noPermission)
        assertEquals(ImeLayoutMode.EXPANDED, plan.layout)
        val panel = plan.expandedPanel
        assertTrue(panel != null)
        panel!!
        assertTrue(panel.prerequisiteLines.any { !it.ok && it.title.contains("permission") })
        assertTrue(panel.prerequisiteLines.any { it.ok && it.title.contains("Offline voice model") })
    }

    @Test
    fun `initial layout is EXPANDED when the native runtime failed to load`() {
        val (presenter, _) = countingPresenter()
        assertEquals(ImeLayoutMode.EXPANDED, presenter.onFreshVisibility(noNative).layout)
    }

    // ------------------------------------------- manual override persistence

    @Test
    fun `repeated onState render callbacks keep a manual layout override`() {
        val (presenter, _) = countingPresenter()
        presenter.onFreshVisibility(ready)
        assertEquals(ImeLayoutMode.EXPANDED, presenter.onToggleExpand().layout)
        // Simulate several state renders: none may reset the manual choice.
        repeat(5) { index ->
            val plan = presenter.render(
                if (index % 2 == 0) ImeSessionModel.ImeState.LISTENING
                else ImeSessionModel.ImeState.PAUSED,
                "probe $index",
            )
            assertEquals(ImeLayoutMode.EXPANDED, plan.layout)
        }
        // And a collapse override also persists across renders.
        assertEquals(ImeLayoutMode.COMPACT, presenter.onToggleExpand().layout)
        val afterRender = presenter.render(
            ImeSessionModel.ImeState.LISTENING,
            "Listening…",
        )
        assertEquals(ImeLayoutMode.COMPACT, afterRender.layout)
    }

    // --------------------------------------- toggle cannot touch the session

    @Test
    fun `toggling IS_LISTENING or PAUSED emits no session action of any kind`() {
        val (presenter, events) = countingPresenter()
        presenter.onFreshVisibility(ready)
        // LISTENING: the expansion toggle arrives mid-dictation.
        presenter.render(ImeSessionModel.ImeState.LISTENING, "Listening…")
        val listeningPlan = presenter.onToggleExpand()
        assertEquals(ImeLayoutMode.EXPANDED, listeningPlan.layout)
        assertEquals("Listening…", listeningPlan.subtitle)
        // PAUSED: the same toggle must stay pure view state.
        presenter.render(ImeSessionModel.ImeState.PAUSED, "Paused — tap Start")
        val pausedPlan = presenter.onToggleExpand()
        assertEquals(ImeLayoutMode.COMPACT, pausedPlan.layout)
        assertEquals("Paused — tap Start", pausedPlan.subtitle)
        // Primary label reflects the real controller state, untouched.
        assertEquals("Start Listening", pausedPlan.primaryLabel)
        assertEquals(
            "toggles emitted no side-channel request whatsoever (a session start/stop " +
                "could only appear as presenter events, and the only injected sink is setup)",
            0,
            events.size,
        )
    }

    // ---------------------------------------------- hide and fresh re-derive

    @Test
    fun `hide then a new visibility resets the layout to the readiness default`() {
        val (presenter, _) = countingPresenter()
        presenter.onFreshVisibility(ready)
        assertEquals(ImeLayoutMode.EXPANDED, presenter.onToggleExpand().layout)
        // Real hide: manual override does not survive it.
        presenter.onHidden()
        assertEquals(ImeLayoutMode.COMPACT, presenter.onFreshVisibility(ready).layout)
    }

    @Test
    fun `duplicate fresh-visibility callbacks while visible do NOT reset the override`() {
        val (presenter, _) = countingPresenter()
        presenter.onFreshVisibility(ready)
        assertEquals(ImeLayoutMode.EXPANDED, presenter.onToggleExpand().layout)
        // Repeated onWindowShown while still visible: presenter must no-op.
        repeat(3) {
            assertEquals(ImeLayoutMode.EXPANDED, presenter.onFreshVisibility(ready).layout)
        }
    }

    // ------------------------------------------------- explicit setup action

    @Test
    fun `the setup action fires ONLY from BLOCKED state`() {
        val (presenter, events) = countingPresenter()
        presenter.onFreshVisibility(noModel)
        presenter.render(ImeSessionModel.ImeState.BLOCKED, "Model not staged (setup in Echolet app)")
        assertTrue(presenter.onSetupClicked())
        assertEquals(1, events.size)
        // Any non-blocked state must not smuggle the setup intent.
        presenter.render(ImeSessionModel.ImeState.PAUSED, "Paused — tap Start")
        assertTrue(!presenter.onSetupClicked())
        presenter.render(ImeSessionModel.ImeState.LISTENING, "Listening…")
        assertTrue(!presenter.onSetupClicked())
        assertEquals("setup stayed BLOCKED-only", 1, events.size)
    }

    // ------------------------------------------------- ten visibility cycles

    @Test
    fun `ten redraw hide and return cycles emit no duplicate or session events`() {
        val (presenter, events) = countingPresenter()
        var setupClicks = 0
        repeat(10) { cycle ->
            val readyCycle = cycle % 2 == 0
            val initial = presenter.onFreshVisibility(if (readyCycle) ready else noModel)
            val expectedDefault =
                if (readyCycle) ImeLayoutMode.COMPACT else ImeLayoutMode.EXPANDED
            assertEquals(expectedDefault, initial.layout)
            val toggled = presenter.onToggleExpand()
            assertNotEquals(expectedDefault, toggled.layout)
            // Renders inside the activation keep the manual layout.
            repeat(2) {
                val plan = presenter.render(ImeSessionModel.ImeState.LISTENING, "Listening…")
                assertEquals(toggled.layout, plan.layout)
            }
            // A stray click while NOT blocked must not forward the request.
            if (presenter.onSetupClicked()) setupClicks++
            presenter.onHidden()
        }
        assertEquals("setup action was never forwarded while not BLOCKED", 0, setupClicks)
        assertEquals("no side-channel events leaked across cycles", 0, events.size)
    }
}
