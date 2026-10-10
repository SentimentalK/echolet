package com.mainstayx.echolet

/**
 * Phase 1-A PURE keyboard-shell presenter (JUnit-testable, no Android
 * runtime). Owns ONLY the Compact/Expanded layout mode of the native voice
 * keyboard view. It performs no session or model decisions: every plan it
 * produces is display data plus at most one non-session
 * [ImeUiRequest.OpenSetup]. Starting or stopping dictation stays exclusively
 * with the existing controller tap path that [EcholetInputMethodService]
 * wires to the primary wide control. No ASR/model business state, no JNI, no
 * filesystem access.
 *
 * Layout rules (Design C):
 *  - A fresh keyboard visibility resets any manual expand/collapse override
 *    to the readiness default: COMPACT when every prerequisite is usable,
 *    EXPANDED otherwise (setup-first).
 *  - A manual override persists across renders and view redraws of the same
 *    visible activation.
 *  - Duplicate fresh-visibility callbacks while still visible are no-ops, so
 *    a repeated onWindowShown never resets the user's layout choice.
 *  - Hiding marks the activation stale; the NEXT visibility re-derives the
 *    readiness default.
 */

/** Readiness snapshot pushed in by the service at visibility boundaries. */
data class ImePrerequisites(
    val permissionGranted: Boolean,
    val modelStaged: Boolean,
    val nativeReady: Boolean,
) {
    val usable: Boolean
        get() = permissionGranted && modelStaged && nativeReady
}

enum class ImeLayoutMode { COMPACT, EXPANDED }

/** One truthful prerequisite entry shown in the Expanded body. */
data class ImePrerequisiteLine(
    val title: String,
    val body: String,
    val ok: Boolean,
)

/** Verbatim display content of the Expanded setup body. */
data class ImeExpandedPanel(
    val statusTitle: String,
    val statusBody: String,
    val prerequisiteLines: List<ImePrerequisiteLine>,
    val offlineNote: String,
    /** Open Echolet Setup action is offered ONLY while BLOCKED. */
    val setupVisible: Boolean,
    /** Phase 1-B: catalog models and download/selection presentation. */
    val modelSnapshot: ModelSnapshotUi? = null,
)

/** Everything the view binder needs to paint one frame of the shell. */
data class ImeKeyboardPlan(
    val layout: ImeLayoutMode,
    val primaryLabel: String,
    /** True when the primary control is the active red Stop style. */
    val primaryStopStyle: Boolean,
    val subtitle: String,
    /** Null in Compact mode. */
    val expandedPanel: ImeExpandedPanel?,
)

/**
 * Pure presenter state owner. [onSetupRequested] is the injected setup sink
 * (the service opens the setup Activity); the presenter performs no
 * Activity/controller work itself and holds no controller reference.
 */
class ImeKeyboardPresenter(private val onSetupRequested: () -> Unit) {

    private var mode: ImeLayoutMode? = null
    private var activationStale = true

    private var lastPrereq = ImePrerequisites(
        permissionGranted = false, modelStaged = false, nativeReady = false,
    )
    private var lastState = ImeSessionModel.ImeState.HIDDEN
    private var lastStatus = ""
    private var modelSnapshot: ModelSnapshotUi? = null

    // -------------------------------------------------------------- lifecycle

    /**
     * Updates model snapshot for the Expanded browser.
     */
    fun updateModelSnapshot(snapshot: ModelSnapshotUi?): ImeKeyboardPlan {
        modelSnapshot = snapshot
        return plan()
    }

    /**
     * A fresh visibility (new keyboard activation): re-derive the readiness
     * default. Duplicate calls while the same activation is still live are
     * no-ops, so repeated onWindowShown callbacks never reset the layout.
     */
    fun onFreshVisibility(prereq: ImePrerequisites): ImeKeyboardPlan {
        lastPrereq = prereq
        if (mode != null && !activationStale) return plan()
        activationStale = false
        mode = defaultMode(prereq)
        return plan()
    }

    /** The keyboard actually hid: the next visibility re-derives default. */
    fun onHidden() {
        activationStale = true
    }

    /** A render callback: refreshes content, NEVER the layout decision. */
    fun render(state: ImeSessionModel.ImeState, status: String): ImeKeyboardPlan {
        lastState = state
        lastStatus = status
        return plan()
    }

    // ----------------------------------------------------------- user actions

    /**
     * Expand/Collapse tap: flips the manual layout override. Pure view state —
     * cannot start/stop a session because the plan carries no session request
     * and the presenter holds no controller reference.
     */
    fun onToggleExpand(): ImeKeyboardPlan {
        mode = when (mode ?: defaultMode(lastPrereq)) {
            ImeLayoutMode.COMPACT -> ImeLayoutMode.EXPANDED
            ImeLayoutMode.EXPANDED -> ImeLayoutMode.COMPACT
        }
        return plan()
    }

    /**
     * The explicit setup action: invoked onClick; calls the injected sink
     * ONLY while BLOCKED, so it can never be smuggled into a session start.
     * Returns true when the request was forwarded (test/audit seam).
     */
    fun onSetupClicked(): Boolean {
        val blocked = lastState == ImeSessionModel.ImeState.BLOCKED
        if (blocked) onSetupRequested()
        return blocked
    }

    // ------------------------------------------------------------------ plan

    private fun defaultMode(prereq: ImePrerequisites): ImeLayoutMode =
        if (prereq.usable) ImeLayoutMode.COMPACT else ImeLayoutMode.EXPANDED

    private fun plan(): ImeKeyboardPlan {
        val current = mode ?: defaultMode(lastPrereq)
        val stop = lastState == ImeSessionModel.ImeState.LISTENING ||
            lastState == ImeSessionModel.ImeState.PREPARING
        return ImeKeyboardPlan(
            layout = current,
            primaryLabel = if (stop) "Stop Listening" else "Start Listening",
            primaryStopStyle = stop,
            subtitle = lastStatus,
            expandedPanel = if (current == ImeLayoutMode.EXPANDED) panel() else null,
        )
    }

    private fun panel(): ImeExpandedPanel {
        val p = lastPrereq
        val permissionLine = if (p.permissionGranted) {
            ImePrerequisiteLine(
                "Microphone access",
                "Granted. Microphone audio is captured only while Listen is active.",
                ok = true,
            )
        } else {
            ImePrerequisiteLine(
                "Microphone permission missing",
                "Echolet records audio only while you are dictating, on-device. " +
                    "Open Echolet Setup below and grant microphone access to continue.",
                ok = false,
            )
        }
        val modelLine = if (p.modelStaged) {
            ImePrerequisiteLine(
                "Offline voice model",
                "The fixed on-device bilingual voice model is installed and ready.",
                ok = true,
            )
        } else {
            ImePrerequisiteLine(
                "Voice model not installed",
                "The offline bilingual voice model is not staged on this device yet. " +
                    "Use the Echolet Setup app below to place it; nothing is downloaded " +
                    "from this keyboard.",
                ok = false,
            )
        }
        val nativeLine = if (p.nativeReady) {
            ImePrerequisiteLine(
                "Recognition runtime",
                "On-device recognition libraries are loaded.",
                ok = true,
            )
        } else {
            ImePrerequisiteLine(
                "Recognition runtime unavailable",
                "Echolet's offline recognition libraries failed to load on this device. " +
                    "Restart the device or reinstall the Echolet app, then retry.",
                ok = false,
            )
        }
        return ImeExpandedPanel(
            statusTitle = when (lastState) {
                ImeSessionModel.ImeState.HIDDEN -> "Echolet voice keyboard"
                ImeSessionModel.ImeState.PREPARING -> "Preparing"
                ImeSessionModel.ImeState.LISTENING -> "Listening"
                ImeSessionModel.ImeState.PAUSED -> "Paused"
                ImeSessionModel.ImeState.BLOCKED -> "Blocked"
            },
            statusBody = lastStatus,
            prerequisiteLines = listOf(permissionLine, modelLine, nativeLine),
            offlineNote = "Voice typing stays on-device: audio is processed locally. " +
                "Model downloads use Internet solely when requested by you.",
            setupVisible = lastState == ImeSessionModel.ImeState.BLOCKED,
            modelSnapshot = modelSnapshot,
        )
    }
}
