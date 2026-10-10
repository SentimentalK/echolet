package com.mainstayx.echolet

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Plain-JVM JUnit tests of the pure keyboard-shell presenter
 * [ImeKeyboardPresenter]: the Compact/Expanded layout state machine,
 * truthful model snapshot guidance, and the BLOCKED-only setup action.
 *
 * The presenter has NO controller reference and no request channel
 * other than the injected setup sink, so a session action from a layout
 * toggle is impossible by construction; these tests pin that contract
 * by counting every invocation of the injected sink.
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

    private fun sampleBilingualModel(installed: Boolean) = ModelItemUi(
        id = "echolet-xasr-zh-en-480ms",
        label = "Bilingual (ZH/EN) Fast",
        releaseDate = "2026-03",
        verificationLabel = "Verified",
        isVerified = true,
        selected = true,
        installed = installed,
        downloadPhase = "NotDownloading",
        downloadLabel = null,
        progressPercent = null,
        primaryAction = if (installed) "None" else "Download",
        enabled = true,
    )

    private fun sampleEnglishModel(installed: Boolean) = ModelItemUi(
        id = "sherpa-onnx-streaming-zipformer-en-20M-2023-02-17",
        label = "English Small (20M)",
        releaseDate = "2023-02",
        verificationLabel = "Verified",
        isVerified = true,
        selected = true,
        installed = installed,
        downloadPhase = "NotDownloading",
        downloadLabel = null,
        progressPercent = null,
        primaryAction = if (installed) "None" else "Download",
        enabled = true,
    )

    private fun sampleSnapshot(model: ModelItemUi, runtimeState: String = "Loaded") = ModelSnapshotUi(
        schemaVersion = 2,
        selectedModelId = model.id,
        selectedModelDir = if (model.installed) "/data/models/${model.id}" else null,
        runtimeState = runtimeState,
        groups = listOf(
            ModelGroupUi(
                id = "group-1",
                label = "Models",
                models = listOf(model),
            )
        ),
    )

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
        // When only model is missing, setup action button is not shown (handled in keyboard)
        assertFalse(panel.setupVisible)
    }

    @Test
    fun `initial layout is EXPANDED when permission is missing and explanation stays truthful`() {
        val (presenter, _) = countingPresenter()
        val plan = presenter.onFreshVisibility(noPermission)
        assertEquals(ImeLayoutMode.EXPANDED, plan.layout)
        val panel = plan.expandedPanel
        assertTrue(panel != null)
        panel!!
        assertTrue(panel.prerequisiteLines.any { !it.ok && it.title.contains("Microphone") })
        assertTrue(panel.prerequisiteLines.any { it.ok && it.title.contains("model") })
    }

    @Test
    fun `initial layout is EXPANDED when the native runtime failed to load`() {
        val (presenter, _) = countingPresenter()
        assertEquals(ImeLayoutMode.EXPANDED, presenter.onFreshVisibility(noNative).layout)
    }

    // ------------------------------------ truthful model snapshot presentation

    @Test
    fun `bilingual default snapshot renders accurate model label and ready state`() {
        val (presenter, _) = countingPresenter()
        presenter.onFreshVisibility(ready)
        val bilingual = sampleBilingualModel(installed = true)
        val plan = presenter.updateModelSnapshot(sampleSnapshot(bilingual))
        presenter.onToggleExpand()
        val expandedPlan = presenter.render(ImeSessionModel.ImeState.PAUSED, "Paused")
        val panel = expandedPlan.expandedPanel
        assertTrue(panel != null)
        val modelLine = panel!!.prerequisiteLines.first { it.title.contains("Voice model") }
        assertTrue("Model line must be ok", modelLine.ok)
        assertTrue("Title reflects bilingual model", modelLine.title.contains("Bilingual (ZH/EN) Fast"))
        assertTrue("Body indicates installed and ready", modelLine.body.contains("Installed and ready"))
    }

    @Test
    fun `user-selected English model renders English title and never claims fixed bilingual model`() {
        val (presenter, _) = countingPresenter()
        presenter.onFreshVisibility(ready)
        presenter.onToggleExpand()
        val english = sampleEnglishModel(installed = true)
        val plan = presenter.updateModelSnapshot(sampleSnapshot(english))
        val panel = plan.expandedPanel
        assertTrue(panel != null)
        val modelLine = panel!!.prerequisiteLines.first { it.title.contains("Voice model") }
        assertTrue("Model line must be ok", modelLine.ok)
        assertTrue("Title reflects English model", modelLine.title.contains("English Small (20M)"))
        assertFalse("Must never claim fixed bilingual model", modelLine.body.contains("bilingual"))
    }

    @Test
    fun `no installed model prompts keyboard download and does not route to setup activity`() {
        val (presenter, _) = countingPresenter()
        presenter.onFreshVisibility(noModel)
        val english = sampleEnglishModel(installed = false)
        val plan = presenter.updateModelSnapshot(sampleSnapshot(english, runtimeState = "NoModel"))
        val panel = plan.expandedPanel
        assertTrue(panel != null)
        val modelLine = panel!!.prerequisiteLines.first { it.title.contains("Voice model") }
        assertFalse("Model line is not ok", modelLine.ok)
        assertTrue("Directs user to download in keyboard", modelLine.body.contains("Download below"))
        assertFalse("Setup activity button is not visible", panel.setupVisible)
    }

    @Test
    fun `no selected model provides actionable guidance`() {
        val (presenter, _) = countingPresenter()
        presenter.onFreshVisibility(noModel)
        val snapshotNoSelection = ModelSnapshotUi(
            schemaVersion = 2,
            selectedModelId = "",
            selectedModelDir = null,
            runtimeState = "NoModel",
            groups = emptyList(),
        )
        val plan = presenter.updateModelSnapshot(snapshotNoSelection)
        val panel = plan.expandedPanel
        assertTrue(panel != null)
        val modelLine = panel!!.prerequisiteLines.first { it.title.contains("model") }
        assertFalse(modelLine.ok)
        assertEquals("No voice model selected", modelLine.title)
        assertTrue(modelLine.body.contains("Select and download"))
    }

    @Test
    fun `unknown snapshot falls back gracefully without stale copy`() {
        val (presenter, _) = countingPresenter()
        presenter.onFreshVisibility(noModel)
        val plan = presenter.updateModelSnapshot(null)
        val panel = plan.expandedPanel
        assertTrue(panel != null)
        val modelLine = panel!!.prerequisiteLines.first { it.title.contains("Voice model") }
        assertFalse(modelLine.ok)
        assertEquals("Voice model not installed", modelLine.title)
        assertFalse(modelLine.body.contains("Echolet Setup app below to place it"))
        assertFalse(modelLine.body.contains("nothing is downloaded from this keyboard"))
    }

    @Test
    fun `mic denied produces setup action and truthful guidance`() {
        val (presenter, _) = countingPresenter()
        presenter.onFreshVisibility(noPermission)
        presenter.render(ImeSessionModel.ImeState.BLOCKED, "Mic permission missing")
        val plan = presenter.render(ImeSessionModel.ImeState.BLOCKED, "Mic permission missing")
        val panel = plan.expandedPanel
        assertTrue(panel != null)
        val micLine = panel!!.prerequisiteLines.first { it.title.contains("Microphone") }
        assertFalse(micLine.ok)
        assertTrue("Setup button is visible when mic is denied", panel.setupVisible)
        assertTrue("Setup click forwards to sink", presenter.onSetupClicked())
    }

    @Test
    fun `native runtime blocked produces setup action and diagnostic message`() {
        val (presenter, _) = countingPresenter()
        presenter.onFreshVisibility(noNative)
        val plan = presenter.render(ImeSessionModel.ImeState.BLOCKED, "Native libraries failed to load")
        val panel = plan.expandedPanel
        assertTrue(panel != null)
        val nativeLine = panel!!.prerequisiteLines.first { it.title.contains("runtime") }
        assertFalse(nativeLine.ok)
        assertTrue("Setup button is visible when native is unavailable", panel.setupVisible)
        assertTrue(presenter.onSetupClicked())
    }

    @Test
    fun `stale setup copy is completely purged across all prerequisite lines`() {
        for (prereq in listOf(ready, noModel, noPermission, noNative)) {
            val (presenter, _) = countingPresenter()
            val plan = presenter.onFreshVisibility(prereq)
            val expandedPlan = if (plan.layout == ImeLayoutMode.EXPANDED) plan else presenter.onToggleExpand()
            val panel = expandedPlan.expandedPanel
            assertTrue("Panel must be present when expanded", panel != null)
            for (line in panel!!.prerequisiteLines) {
                assertFalse("Must not claim nothing downloaded", line.body.contains("nothing is downloaded"))
                assertFalse("Must not tell user to stage in setup app", line.body.contains("Echolet Setup app below to place it"))
                assertFalse("Must not claim fixed bilingual model", line.body.contains("fixed on-device bilingual"))
            }
            assertFalse(panel.offlineNote.contains("place it"))
            assertFalse(panel.offlineNote.contains("nothing is downloaded"))
        }
    }

    // ------------------------------------------- manual override persistence

    @Test
    fun `repeated onState render callbacks keep a manual layout override`() {
        val (presenter, _) = countingPresenter()
        presenter.onFreshVisibility(ready)
        assertEquals(ImeLayoutMode.EXPANDED, presenter.onToggleExpand().layout)
        repeat(5) { index ->
            val plan = presenter.render(
                if (index % 2 == 0) ImeSessionModel.ImeState.LISTENING
                else ImeSessionModel.ImeState.PAUSED,
                "probe $index",
            )
            assertEquals(ImeLayoutMode.EXPANDED, plan.layout)
        }
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
        presenter.render(ImeSessionModel.ImeState.LISTENING, "Listening…")
        val listeningPlan = presenter.onToggleExpand()
        assertEquals(ImeLayoutMode.EXPANDED, listeningPlan.layout)
        assertEquals("Listening…", listeningPlan.subtitle)
        presenter.render(ImeSessionModel.ImeState.PAUSED, "Paused — tap Start")
        val pausedPlan = presenter.onToggleExpand()
        assertEquals(ImeLayoutMode.COMPACT, pausedPlan.layout)
        assertEquals("Paused — tap Start", pausedPlan.subtitle)
        assertEquals("Start Listening", pausedPlan.primaryLabel)
        assertEquals(0, events.size)
    }

    // ---------------------------------------------- hide and fresh re-derive

    @Test
    fun `hide then a new visibility resets the layout to the readiness default`() {
        val (presenter, _) = countingPresenter()
        presenter.onFreshVisibility(ready)
        assertEquals(ImeLayoutMode.EXPANDED, presenter.onToggleExpand().layout)
        presenter.onHidden()
        assertEquals(ImeLayoutMode.COMPACT, presenter.onFreshVisibility(ready).layout)
    }

    @Test
    fun `duplicate fresh-visibility callbacks while visible do NOT reset the override`() {
        val (presenter, _) = countingPresenter()
        presenter.onFreshVisibility(ready)
        assertEquals(ImeLayoutMode.EXPANDED, presenter.onToggleExpand().layout)
        repeat(3) {
            assertEquals(ImeLayoutMode.EXPANDED, presenter.onFreshVisibility(ready).layout)
        }
    }

    // ------------------------------------------------- explicit setup action

    @Test
    fun `the setup action fires ONLY from BLOCKED state and when setup is useful`() {
        val (presenter, events) = countingPresenter()
        presenter.onFreshVisibility(noPermission)
        presenter.render(ImeSessionModel.ImeState.BLOCKED, "Mic permission missing")
        assertTrue(presenter.onSetupClicked())
        assertEquals(1, events.size)

        // Any non-blocked state must not forward setup intent
        presenter.render(ImeSessionModel.ImeState.PAUSED, "Paused — tap Start")
        assertFalse(presenter.onSetupClicked())
        presenter.render(ImeSessionModel.ImeState.LISTENING, "Listening…")
        assertFalse(presenter.onSetupClicked())
        assertEquals("setup stayed BLOCKED-only", 1, events.size)

        // When only model is missing, setup action does NOT forward (handled in keyboard)
        val (modelPresenter, modelEvents) = countingPresenter()
        modelPresenter.onFreshVisibility(noModel)
        modelPresenter.render(ImeSessionModel.ImeState.BLOCKED, "Model not installed")
        assertFalse(modelPresenter.onSetupClicked())
        assertEquals(0, modelEvents.size)
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
            repeat(2) {
                val plan = presenter.render(ImeSessionModel.ImeState.LISTENING, "Listening…")
                assertEquals(toggled.layout, plan.layout)
            }
            if (presenter.onSetupClicked()) setupClicks++
            presenter.onHidden()
        }
        assertEquals("setup action was never forwarded while not BLOCKED", 0, setupClicks)
        assertEquals("no side-channel events leaked across cycles", 0, events.size)
    }
}
