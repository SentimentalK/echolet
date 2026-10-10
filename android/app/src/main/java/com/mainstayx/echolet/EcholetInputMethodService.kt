package com.mainstayx.echolet

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.inputmethodservice.InputMethodService
import android.os.Handler
import android.os.Looper
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.view.inputmethod.EditorInfo
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import java.io.File

/**
 * Phase 1-A native Android voice keyboard shell: a Compact strip and a
 * bounded, scrollable Expanded setup body built from plain light Android
 * Views (no Compose/framework UI). The wide primary control and the
 * lifecycle/session/editor pipeline are UNCHANGED from Phase 0: every
 * Start/Stop still routes ONLY through
 * [ImeSessionController.onControlTap(ready, blockedReason, ic)] with the
 * current InputConnection, and Expand/Collapse toggles view state ONLY.
 */
class EcholetInputMethodService : InputMethodService() {

    private lateinit var controller: ImeSessionController
    private lateinit var downloadManager: ModelDownloadManager
    private val presenter = ImeKeyboardPresenter(onSetupRequested = ::openSetupActivity)

    /** Readiness snapshot, NEVER re-derived on render/toggle (Design D). */
    private var readiness = ImePrerequisites(false, false, false)

    /** Parses lazily into a memoized flag; re-checked cheaply. */
    private var nativeLibsReady: Boolean? = null
    private val ui = Handler(Looper.getMainLooper())

    // View references; cleared safely on recreation/destroy so stale click
    // callbacks can never touch a dead view tree.
    private var root: ViewGroup? = null
    private var primary: TextView? = null
    private var expandToggle: TextView? = null
    private var subtitle: TextView? = null
    private var expandedScroll: ScrollView? = null
    private var expandedBody: ViewGroup? = null
    private var lastPanel: ImeExpandedPanel? = null

    override fun onCreate() {
        super.onCreate()
        controller = ImeSessionController(
            applicationContext,
            onState = { state, text -> render(state, text) },
            icProvider = { currentInputConnection },
        )
        downloadManager = ModelDownloadManager(
            applicationContext,
            onProgressOrStatusChanged = {
                runOnUi {
                    refreshReadiness()
                    refreshModelSnapshot()
                }
            },
        )
        controller.onServiceCreated()
    }

    // ------------------------------------------------------------- readiness

    private fun nativeLibrariesLoadable(): Boolean {
        // Memoized only on success; failure stays retryable.
        nativeLibsReady?.let { return it }
        val ok = try {
            NativeBridge.hashCode() // triggers the explicit load order above
            true
        } catch (t: Throwable) {
            false
        }
        if (ok) nativeLibsReady = true
        return ok
    }

    private fun modelStaged(): Boolean {
        val selectedDir = try {
            NativeBridge.nativeGetSelectedModelDir()
        } catch (_: Throwable) {
            null
        }
        if (selectedDir != null) {
            val dir = File(selectedDir)
            if (dir.isDirectory) {
                val hasModelJson = File(dir, "model.json").let { it.exists() && it.length() > 0L }
                val hasTokens = File(dir, "tokens.txt").let { it.exists() && it.length() > 0L }
                if (hasModelJson && hasTokens) return true
            }
        }
        val base = getExternalFilesDir("models") ?: return false
        val dir = File(base, "bilingual-zh-en")
        val required =
            listOf("model.json", "encoder-480ms.onnx", "decoder-480ms.onnx", "joiner-480ms.onnx", "tokens.txt")
        return required.all { name -> File(dir, name).let { it.exists() && it.length() > 0L } }
    }

    private fun blockedReason(): String? = when {
        (checkSelfPermission(Manifest.permission.RECORD_AUDIO)
            != PackageManager.PERMISSION_GRANTED) -> "Mic permission missing"
        !modelStaged() -> "Model not staged (setup in Echolet app)"
        !nativeLibrariesLoadable() -> "Native libraries failed to load"
        else -> null
    }

    /** Refresh the readiness snapshot; called only at visibility boundaries. */
    private fun refreshReadiness() {
        readiness = ImePrerequisites(
            permissionGranted = checkSelfPermission(Manifest.permission.RECORD_AUDIO)
                == PackageManager.PERMISSION_GRANTED,
            modelStaged = modelStaged(),
            nativeReady = nativeLibrariesLoadable(),
        )
    }

    private fun refreshModelSnapshot() {
        try {
            val json = NativeBridge.nativeModelSnapshot()
            val parsed = ModelSnapshotUi.parseJson(json)
            applyPlan(presenter.updateModelSnapshot(parsed))
        } catch (t: Throwable) {
            android.util.Log.w("EcholetIme", "Failed to refresh model snapshot", t)
        }
    }

    // ------------------------------------------------------------- input view

    override fun onCreateInputView(): View {
        clearViewRefs()
        refreshReadiness()

        val container = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(COLOR_SURFACE)
            setPadding(dp(4), dp(4), dp(4), dp(6))
        }
        val header = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
        }

        val primaryButton = TextView(this).apply {
            textSize = 18f
            typeface = Typeface.DEFAULT_BOLD
            setTextColor(Color.WHITE)
            gravity = Gravity.CENTER
            minHeight = dp(48)
            minWidth = dp(48)
            setPadding(dp(16), dp(12), dp(16), dp(12))
            isSingleLine = true
            isClickable = true
            isFocusable = true
        }
        // THE only session control action: Phase 0 contract preserved.
        primaryButton.setOnClickListener {
            val reason = blockedReason()
            controller.onControlTap(
                ready = reason == null,
                blockedReason = reason,
                ic = currentInputConnection,
            )
        }

        val toggle = TextView(this).apply {
            textSize = 20f
            setTextColor(COLOR_TEXT)
            gravity = Gravity.CENTER
            minHeight = dp(48)
            minWidth = dp(48)
            background = roundedOutline()
            isClickable = true
            isFocusable = true
        }
        // Layout visibility/height ONLY: never a session action.
        toggle.setOnClickListener { applyPlan(presenter.onToggleExpand()) }

        header.addView(
            primaryButton,
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
                1f,
            )
        )
        header.addView(
            toggle,
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.MATCH_PARENT,
            ).apply { marginStart = dp(6) }
        )

        val subtitleView = TextView(this).apply {
            textSize = 12f
            setTextColor(COLOR_MUTED)
            maxLines = 1
            ellipsize = android.text.TextUtils.TruncateAt.END
            setPadding(dp(12), dp(4), dp(12), dp(2))
        }

        val scroll = ScrollView(this).apply {
            isVerticalScrollBarEnabled = false
            visibility = View.GONE
        }
        val body = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(8), dp(2), dp(8), dp(8))
        }
        scroll.addView(body)

        container.addView(header)
        container.addView(
            subtitleView,
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            )
        )
        container.addView(
            scroll,
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT,
            )
        )

        root = container
        primary = primaryButton
        expandToggle = toggle
        subtitle = subtitleView
        expandedScroll = scroll
        expandedBody = body
        lastPanel = null

        // Initial layout decision for this visible activation, then content.
        applyPlan(presenter.onFreshVisibility(readiness))
        applyPlan(presenter.render(controller.model.currentState, controller.lastStatusText))
        return container
    }

    /**
     * Not a fullscreen IME: the bounded Compact/Expanded shell overlays the
     * editor; fullscreen extraction mode stays off (Design B/D).
     */
    override fun onEvaluateFullscreenMode(): Boolean = false

    /**
     * The ONLY non-session shell route: opens the setup Activity (the old
     * BLOCKED status-tap behavior, now an explicit button). Never writes
     * setup/error text into the editor and never routed to the controller.
     */
    private fun openSetupActivity() {
        val intent = Intent(this, MainActivity::class.java)
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        startActivity(intent)
    }

    // ------------------------------------------------------------ plan binder

    private fun render(state: ImeSessionModel.ImeState, text: String) {
        runOnUi { applyPlan(presenter.render(state, text)) }
    }

    /** Applies one pure plan to the live views; view refs may be gone. */
    private fun applyPlan(plan: ImeKeyboardPlan) {
        val container = root ?: return
        val primaryView = primary ?: return
        val toggleView = expandToggle ?: return

        primaryView.text = plan.primaryLabel
        primaryView.background = roundedFilled(
            if (plan.primaryStopStyle) COLOR_STOP_RED else COLOR_CHARCOAL,
        )
        toggleView.text = if (plan.layout == ImeLayoutMode.EXPANDED) "▴" else "▾"
        toggleView.contentDescription =
            if (plan.layout == ImeLayoutMode.EXPANDED) "Collapse" else "Expand"

        subtitle?.text = plan.subtitle
        subtitle?.visibility = if (plan.subtitle.isNotBlank()) View.VISIBLE else View.GONE

        val scroll = expandedScroll ?: return
        val expanded = plan.layout == ImeLayoutMode.EXPANDED
        scroll.visibility = if (expanded) View.VISIBLE else View.GONE
        if (expanded) {
            val panel = plan.expandedPanel
            val body = expandedBody
            if (panel != null && body != null) {
                if (panel != lastPanel) {
                    lastPanel = panel
                    buildExpandedBody(body, panel)
                }
                scroll.post { clampExpandedHeight(scroll) }
            }
        } else {
            // Un-clamp so the next expand can re-derive a fresh bounded size.
            (scroll.layoutParams as? LinearLayout.LayoutParams)?.let {
                it.height = ViewGroup.LayoutParams.WRAP_CONTENT
                scroll.layoutParams = it
            }
        }
        container.requestLayout()
    }

    /** Builds the scrollable Expanded body: truthful, bounded content only. */
    private fun buildExpandedBody(body: ViewGroup, panel: ImeExpandedPanel) {
        body.removeAllViews()
        body.addView(card(panel.statusTitle, panel.statusBody, highlightTitle = true))
        val prereqCard = roundedCard()
        panel.prerequisiteLines.forEach { line ->
            prereqCard.addView(textView(line.title, 14f, if (line.ok) COLOR_TEXT else COLOR_STOP_RED, bold = true))
            prereqCard.addView(textView(line.body, 13f, COLOR_MUTED))
        }
        body.addView(prereqCard)

        // Phase 1-B: Model Catalog Browser
        val snapshot = panel.modelSnapshot
        if (snapshot != null && snapshot.groups.isNotEmpty()) {
            val modelsCard = roundedCard()
            modelsCard.addView(textView("Available Voice Models", 15f, COLOR_TEXT, bold = true))

            snapshot.groups.forEach { group ->
                modelsCard.addView(
                    textView(group.label, 13f, COLOR_MUTED, bold = true).apply {
                        setPadding(0, dp(6), 0, dp(2))
                    }
                )

                group.models.forEach { model ->
                    val row = LinearLayout(this).apply {
                        orientation = LinearLayout.HORIZONTAL
                        gravity = Gravity.CENTER_VERTICAL
                        setPadding(0, dp(6), 0, dp(6))
                    }

                    val infoLayout = LinearLayout(this).apply {
                        orientation = LinearLayout.VERTICAL
                        layoutParams = LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f)
                    }

                    val nameRow = LinearLayout(this).apply {
                        orientation = LinearLayout.HORIZONTAL
                        gravity = Gravity.CENTER_VERTICAL
                    }

                    val nameText = textView(
                        if (model.selected) "✓ ${model.label}" else model.label,
                        14f,
                        COLOR_TEXT,
                        bold = model.selected,
                    )
                    nameRow.addView(nameText)

                    val badge = TextView(this).apply {
                        text = " ${model.verificationLabel} "
                        textSize = 10f
                        setTextColor(if (model.isVerified) 0xFF059669.toInt() else 0xFFD97706.toInt())
                        background = GradientDrawable().apply {
                            cornerRadius = 4 * resources.displayMetrics.density
                            setColor(if (model.isVerified) 0x1A059669 else 0x1AD97706)
                        }
                    }
                    val badgeParams = LinearLayout.LayoutParams(
                        ViewGroup.LayoutParams.WRAP_CONTENT,
                        ViewGroup.LayoutParams.WRAP_CONTENT,
                    ).apply { marginStart = dp(6) }
                    nameRow.addView(badge, badgeParams)

                    infoLayout.addView(nameRow)

                    val statusSubtitle = when {
                        model.downloadPhase != "NotDownloading" && model.downloadPhase != "Completed" ->
                            model.downloadLabel ?: model.downloadPhase
                        model.installed -> "Installed · ${model.releaseDate}"
                        else -> "Available for download · ${model.releaseDate}"
                    }
                    infoLayout.addView(textView(statusSubtitle, 12f, COLOR_MUTED))

                    row.addView(infoLayout)

                    // Action button (>=48dp touch target)
                    val actionButton = TextView(this).apply {
                        textSize = 13f
                        typeface = Typeface.DEFAULT_BOLD
                        gravity = Gravity.CENTER
                        minHeight = dp(48)
                        minWidth = dp(48)
                        setPadding(dp(12), dp(8), dp(12), dp(8))
                        isClickable = true
                        isFocusable = true

                        when {
                            model.selected -> {
                                text = "Selected"
                                setTextColor(COLOR_MUTED)
                                background = roundedOutline()
                                isEnabled = false
                            }
                            model.downloadPhase == "Starting" ||
                            model.downloadPhase == "Downloading" ||
                            model.downloadPhase == "Verifying" ||
                            model.downloadPhase == "Extracting" ||
                            model.downloadPhase == "Installing" -> {
                                text = model.progressPercent?.let { "$it%" } ?: "..."
                                setTextColor(Color.WHITE)
                                background = roundedFilled(0xFF2563EB.toInt())
                                isEnabled = false
                            }
                            model.primaryAction == "Select" -> {
                                text = "Select"
                                setTextColor(Color.WHITE)
                                background = roundedFilled(COLOR_CHARCOAL)
                                isEnabled = model.enabled
                                setOnClickListener {
                                    val isListening = controller.model.currentState == ImeSessionModel.ImeState.LISTENING ||
                                                      controller.model.currentState == ImeSessionModel.ImeState.PREPARING
                                    if (isListening) {
                                        android.util.Log.w("EcholetIme", "Model selection rejected: session active")
                                        return@setOnClickListener
                                    }
                                    NativeBridge.nativeSelectModel(model.id)
                                    refreshReadiness()
                                    refreshModelSnapshot()
                                }
                            }
                            model.primaryAction == "Download" || model.primaryAction == "RetryDownload" -> {
                                text = if (model.primaryAction == "RetryDownload") "Retry" else "Download"
                                setTextColor(Color.WHITE)
                                background = roundedFilled(0xFF2563EB.toInt())
                                isEnabled = model.enabled
                                setOnClickListener {
                                    downloadManager.startDownload(model.id)
                                    refreshModelSnapshot()
                                }
                            }
                            else -> {
                                visibility = View.GONE
                            }
                        }
                    }

                    val btnParams = LinearLayout.LayoutParams(
                        ViewGroup.LayoutParams.WRAP_CONTENT,
                        ViewGroup.LayoutParams.WRAP_CONTENT,
                    ).apply { marginStart = dp(8) }
                    row.addView(actionButton, btnParams)

                    modelsCard.addView(row)
                }
            }

            body.addView(modelsCard)
        }

        body.addView(
            textView(panel.offlineNote, 13f, COLOR_MUTED).apply {
                setPadding(dp(8), dp(6), dp(8), dp(2))
            }
        )
        if (panel.setupVisible) {
            body.addView(
                TextView(this).apply {
                    text = "Open Echolet Setup"
                    textSize = 16f
                    setTextColor(Color.WHITE)
                    gravity = Gravity.CENTER
                    minHeight = dp(48)
                    minWidth = dp(48)
                    background = roundedFilled(COLOR_CHARCOAL)
                    setPadding(dp(16), dp(12), dp(16), dp(12))
                    isClickable = true
                    isFocusable = true
                    // Explicit user action only: opens the setup Activity,
                    // never writes setup/error text into the editor and never
                    // routed to the controller.
                    setOnClickListener { presenter.onSetupClicked() }
                },
                LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.WRAP_CONTENT,
                ).apply { topMargin = dp(6) }
            )
        }
    }

    private fun roundedCard(): LinearLayout = LinearLayout(this).apply {
        orientation = LinearLayout.VERTICAL
        background = roundedFilled(COLOR_CARD)
        setPadding(dp(14), dp(12), dp(14), dp(12))
    }

    private fun card(title: String, bodyText: String, highlightTitle: Boolean): LinearLayout =
        roundedCard().apply {
            addView(textView(title, 15f, COLOR_TEXT, bold = highlightTitle))
            if (bodyText.isNotBlank()) addView(textView(bodyText, 13f, COLOR_MUTED))
        }

    private fun textView(text: String, size: Float, color: Int, bold: Boolean = false): TextView =
        TextView(this).apply {
            this.text = text
            textSize = size
            setTextColor(color)
            if (bold) typeface = Typeface.DEFAULT_BOLD
            setPadding(0, dp(2), 0, dp(4))
        }

    private fun roundedFilled(color: Int): GradientDrawable = GradientDrawable().apply {
        cornerRadius = 14 * resources.displayMetrics.density
        setColor(color)
    }

    private fun roundedOutline(): GradientDrawable = GradientDrawable().apply {
        cornerRadius = 14 * resources.displayMetrics.density
        setColor(Color.WHITE)
        setStroke(dp(1), COLOR_MUTED)
    }

    /**
     * Expanded height clamp: the scroll region never exceeds a conservative
     * fraction of the screen height, and the ScrollView handles the
     * constrained difference. Dynamic IME host inset behavior is UNVERIFIED
     * until tested on device.
     */
    private fun clampExpandedHeight(scroll: ScrollView) {
        if (scroll !== expandedScroll) return
        val cap = (resources.displayMetrics.heightPixels * 0.4f).toInt().coerceAtLeast(dp(200))
        val content = scroll.getChildAt(0) ?: return
        if (scroll.width <= 0) {
            scroll.requestLayout()
            return
        }
        content.measure(
            View.MeasureSpec.makeMeasureSpec(scroll.width, View.MeasureSpec.EXACTLY),
            View.MeasureSpec.makeMeasureSpec(cap, View.MeasureSpec.AT_MOST),
        )
        (scroll.layoutParams as? LinearLayout.LayoutParams)?.let {
            it.height = content.measuredHeight.coerceAtMost(cap)
            scroll.layoutParams = it
            scroll.requestLayout()
        }
    }

    // -------------------------------------------------------------- lifecycle

    override fun onStartInput(attribute: EditorInfo?, restarting: Boolean) {
        super.onStartInput(attribute, restarting)
        controller.onCurrentInputConnection(currentInputConnection)
        controller.onInputStarted(attribute, restarting, viewVisible = false)
    }

    override fun onStartInputView(editorInfo: EditorInfo?, restarting: Boolean) {
        super.onStartInputView(editorInfo, restarting)
        if (editorInfo == null) {
            controller.onInputStarted(null, restarting, viewVisible = false)
            return
        }
        refreshReadiness()
        refreshModelSnapshot()
        applyPlan(presenter.onFreshVisibility(readiness))
        val reason = blockedReason()
        controller.onCurrentInputConnection(currentInputConnection)
        // Blocking read of readiness data; queued native work stays on the lane.
        controller.onInputViewStarted(
            editorInfo,
            currentInputConnection,
            ready = reason == null,
            blockedReason = reason,
        )
        // Model may need time to open (Preparing…); mic NOT started yet.
    }

    override fun onFinishInputView(finishingInput: Boolean) {
        controller.onFinishInputView(finishingInput)
        super.onFinishInputView(finishingInput)
    }

    override fun onFinishInput() {
        controller.onFinishInput()
        super.onFinishInput()
    }

    override fun onWindowShown() {
        super.onWindowShown()
        refreshReadiness()
        refreshModelSnapshot()
        applyPlan(presenter.onFreshVisibility(readiness))
        controller.onWindowShown()
    }

    override fun onWindowHidden() {
        controller.onWindowHidden()
        // The manual layout override does not survive a real hide; the next
        // visibility re-derives the readiness default.
        presenter.onHidden()
        super.onWindowHidden()
    }

    override fun onDestroy() {
        controller.onDestroyed()
        clearViewRefs()
        super.onDestroy()
    }

    /** Drops every view reference so no stale callback can outlive a view. */
    private fun clearViewRefs() {
        root = null
        primary = null
        expandToggle = null
        subtitle = null
        expandedScroll = null
        expandedBody = null
        lastPanel = null
    }

    private fun runOnUi(body: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) body() else ui.post(body)
    }

    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()

    private companion object {
        const val COLOR_CHARCOAL = 0xFF18181B.toInt()
        const val COLOR_STOP_RED = 0xFFEF4444.toInt()
        const val COLOR_SURFACE = 0xFFFAFAFA.toInt()
        const val COLOR_CARD = -0x1 // Color.WHITE
        const val COLOR_TEXT = 0xFF18181B.toInt()
        const val COLOR_MUTED = 0xFF3F3F46.toInt()
    }
}
