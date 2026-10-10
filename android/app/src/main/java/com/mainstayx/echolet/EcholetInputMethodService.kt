package com.mainstayx.echolet

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.ColorStateList
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.ClipDrawable
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.LayerDrawable
import android.graphics.drawable.RippleDrawable
import android.inputmethodservice.InputMethodService
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.text.SpannableStringBuilder
import android.text.Spanned
import android.text.TextUtils
import android.text.style.ForegroundColorSpan
import android.text.style.RelativeSizeSpan
import android.view.Gravity
import android.view.HapticFeedbackConstants
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.inputmethod.EditorInfo
import android.widget.ImageView
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
    private val deleteHandler = Handler(Looper.getMainLooper())
    private val modelExecutor = java.util.concurrent.Executors.newSingleThreadExecutor { r ->
        Thread(r, "echolet-ime-model-io").apply { isDaemon = true }
    }
    @Volatile private var modelSnapshotGen = 0L
    @Volatile private var isDestroyed = false

    // View references; cleared safely on recreation/destroy so stale click
    // callbacks can never touch a dead view tree.
    private var root: ViewGroup? = null
    private var primary: TextView? = null
    private var expandToggle: ImageView? = null
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
        } ?: return false

        val dir = File(selectedDir)
        if (!dir.isDirectory) return false
        val hasModelJson = File(dir, "model.json").let { it.exists() && it.length() > 0L }
        val hasTokens = File(dir, "tokens.txt").let { it.exists() && it.length() > 0L }
        return hasModelJson && hasTokens
    }

    private fun blockedReason(): String? = when {
        (checkSelfPermission(Manifest.permission.RECORD_AUDIO)
            != PackageManager.PERMISSION_GRANTED) -> "Mic permission missing"
        !nativeLibrariesLoadable() -> "Native libraries failed to load"
        !modelStaged() -> "Selected model not installed (download in keyboard)"
        else -> null
    }

    /** Refresh the readiness snapshot; called only at visibility boundaries. */
    private fun refreshReadiness() {
        if (isDestroyed) return
        readiness = ImePrerequisites(
            permissionGranted = checkSelfPermission(Manifest.permission.RECORD_AUDIO)
                == PackageManager.PERMISSION_GRANTED,
            modelStaged = modelStaged(),
            nativeReady = nativeLibrariesLoadable(),
        )
    }

    private fun refreshModelSnapshot() {
        if (isDestroyed) return
        val gen = synchronized(this) {
            if (isDestroyed) return
            ++modelSnapshotGen
        }
        try {
            modelExecutor.execute {
                if (isDestroyed) return@execute
                try {
                    val json = NativeBridge.nativeModelSnapshot()
                    val parsed = ModelSnapshotUi.parseJson(json)
                    runOnUi {
                        synchronized(this) {
                            if (isDestroyed || gen != modelSnapshotGen || root == null) return@runOnUi
                        }
                        applyPlan(presenter.updateModelSnapshot(parsed))
                    }
                } catch (t: Throwable) {
                    android.util.Log.w("EcholetIme", "Failed to refresh model snapshot", t)
                }
            }
        } catch (_: java.util.concurrent.RejectedExecutionException) {
            // Fail closed if modelExecutor is shut down or rejects
        }
    }

    // ------------------------------------------------------------- input view

    private fun handleDelete() {
        val ic = currentInputConnection ?: return
        val selected = ic.getSelectedText(0)
        if (!selected.isNullOrEmpty()) {
            ic.commitText("", 1)
        } else {
            ic.sendKeyEvent(KeyEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DEL))
            ic.sendKeyEvent(KeyEvent(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DEL))
        }
    }

    override fun onCreateInputView(): View {
        deleteHandler.removeCallbacksAndMessages(null)
        clearViewRefs()
        refreshReadiness()
        applyNavBarAppearance()

        // Desktop-panel look: white strip on top, soft gray body below.
        val container = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(COLOR_CARD)
            setPadding(0, 0, 0, dp(16))
        }

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            container.setOnApplyWindowInsetsListener { v, insets ->
                // Keep every control above the IME nav bar (back + globe
                // switcher) that the system draws over the bottom of the IME.
                val nav = insets.getInsets(WindowInsets.Type.navigationBars())
                val bars = insets.getInsets(WindowInsets.Type.systemBars())
                val bottom = maxOf(nav.bottom, bars.bottom)
                v.setPadding(0, 0, 0, if (bottom > 0) bottom else dp(48))
                insets
            }
            container.requestApplyInsets()
        }

        container.addView(
            View(this).apply { setBackgroundColor(COLOR_BORDER) },
            LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(1)),
        )

        val header = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(12), dp(10), dp(12), dp(10))
        }

        val toggle = ImageView(this).apply {
            setImageDrawable(icon(KeyIconDrawable.Kind.CHEVRON_UP))
            scaleType = ImageView.ScaleType.CENTER
            background = keyBackground()
            isClickable = true
            isFocusable = true
            contentDescription = "Expand"
            setOnClickListener {
                haptic(it, strong = false)
                applyPlan(presenter.onToggleExpand())
            }
        }

        val primaryButton = TextView(this).apply {
            textSize = 15f
            typeface = Typeface.DEFAULT_BOLD
            setTextColor(Color.WHITE)
            gravity = Gravity.CENTER
            isSingleLine = true
            isClickable = true
            isFocusable = true
            setOnClickListener {
                haptic(it, strong = true)
                val reason = blockedReason()
                controller.onControlTap(
                    ready = reason == null,
                    blockedReason = reason,
                    ic = currentInputConnection,
                )
            }
        }

        val backspaceButton = ImageView(this).apply {
            setImageDrawable(icon(KeyIconDrawable.Kind.BACKSPACE))
            scaleType = ImageView.ScaleType.CENTER
            background = keyBackground()
            isClickable = true
            isFocusable = true
            contentDescription = "Backspace"
            setOnTouchListener { v, event ->
                when (event.actionMasked) {
                    MotionEvent.ACTION_DOWN -> {
                        v.isPressed = true
                        haptic(v, strong = false)
                        handleDelete()
                        deleteHandler.removeCallbacksAndMessages(null)
                        deleteHandler.postDelayed(object : Runnable {
                            override fun run() {
                                if (isDestroyed) return
                                haptic(v, strong = false)
                                handleDelete()
                                deleteHandler.postDelayed(this, 60)
                            }
                        }, 400)
                        true
                    }
                    MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                        v.isPressed = false
                        deleteHandler.removeCallbacksAndMessages(null)
                        true
                    }
                    else -> false
                }
            }
        }

        header.addView(
            toggle,
            LinearLayout.LayoutParams(dp(52), dp(52)).apply { marginEnd = dp(8) },
        )
        header.addView(primaryButton, LinearLayout.LayoutParams(0, dp(52), 1f))
        header.addView(
            backspaceButton,
            LinearLayout.LayoutParams(dp(52), dp(52)).apply { marginStart = dp(8) },
        )

        val scroll = ScrollView(this).apply {
            isVerticalScrollBarEnabled = false
            overScrollMode = View.OVER_SCROLL_NEVER
            setBackgroundColor(COLOR_BG)
            visibility = View.GONE
        }
        val body = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(16), dp(14), dp(16), dp(10))
        }
        scroll.addView(body)

        container.addView(header)
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
        subtitle = null
        expandedScroll = scroll
        expandedBody = body
        lastPanel = null

        // Initial layout decision for this visible activation, then content.
        applyPlan(presenter.onFreshVisibility(readiness))
        applyPlan(presenter.render(controller.model.currentState, controller.lastStatusText))
        return container
    }

    /**
     * Our IME surface is light, so ask for dark nav-bar glyphs. Without this
     * the system draws the IME back/globe switcher in white, which is
     * invisible on the white keyboard. Re-applied when the window is shown
     * because the framework resets appearance while attaching the IME window.
     */
    private fun applyNavBarAppearance() {
        val w = window?.window ?: return
        @Suppress("DEPRECATION")
        w.navigationBarColor = COLOR_CARD
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            w.isNavigationBarContrastEnforced = false
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            @Suppress("DEPRECATION")
            w.setDecorFitsSystemWindows(false)
            w.insetsController?.setSystemBarsAppearance(
                WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS,
                WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS,
            )
        }
        @Suppress("DEPRECATION")
        w.decorView.systemUiVisibility =
            w.decorView.systemUiVisibility or View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR
    }

    private fun haptic(v: View, strong: Boolean) {
        val type = if (strong && Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            HapticFeedbackConstants.CONFIRM
        } else {
            HapticFeedbackConstants.KEYBOARD_TAP
        }
        v.isHapticFeedbackEnabled = true
        v.performHapticFeedback(type, HapticFeedbackConstants.FLAG_IGNORE_VIEW_SETTING)
    }

    private fun icon(kind: KeyIconDrawable.Kind): KeyIconDrawable =
        KeyIconDrawable(kind, dp(24), 2f * resources.displayMetrics.density, COLOR_TEXT_PRIMARY)

    /** White outlined key with a subtle press ripple. */
    private fun keyBackground(): RippleDrawable =
        RippleDrawable(ColorStateList.valueOf(0x1A000000), roundedOutline(12), null)

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

        val stop = plan.primaryStopStyle
        primaryView.text = plan.primaryLabel
        primaryView.background = RippleDrawable(
            ColorStateList.valueOf(0x33FFFFFF),
            roundedFilled(if (stop) COLOR_STOP_RED else COLOR_PRIMARY_BUTTON, 12),
            null,
        )
        // Compact: arrow points up (the panel opens upward). Expanded: down.
        val expanded = plan.layout == ImeLayoutMode.EXPANDED
        toggleView.setImageDrawable(
            icon(if (expanded) KeyIconDrawable.Kind.CHEVRON_DOWN else KeyIconDrawable.Kind.CHEVRON_UP)
        )
        toggleView.contentDescription = if (expanded) "Collapse" else "Expand"

        val scroll = expandedScroll ?: return
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

    /**
     * Builds the scrollable Expanded body, styled after the desktop panel:
     * an attention card ONLY when something needs fixing (the Start/Stop
     * button already shows the session state), a MODELS section of compact
     * single-line rows, and a short footer.
     */
    private fun buildExpandedBody(body: ViewGroup, panel: ImeExpandedPanel) {
        body.removeAllViews()

        // 1. Attention card — only when blocked or setup is required.
        // The Start/Stop button already shows session state, and the model
        // list covers voice-model status, so the mic/runtime/model checklist
        // is not shown when everything is usable.
        val blocked = panel.statusTitle == "Blocked"
        val setupLines = panel.prerequisiteLines.filter { line ->
            !line.ok && (
                line.title.contains("Microphone", ignoreCase = true) ||
                    line.title.contains("runtime", ignoreCase = true)
                )
        }
        val showNotice = setupLines.isNotEmpty() || panel.setupVisible ||
            (blocked && panel.statusBody.isNotBlank())
        if (showNotice) {
            val notice = LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                background = GradientDrawable().apply {
                    cornerRadius = dp(10).toFloat()
                    setColor(COLOR_NOTICE_BG)
                    setStroke(dp(1), COLOR_NOTICE_BORDER)
                }
                setPadding(dp(12), dp(10), dp(12), dp(10))
            }
            if (setupLines.isNotEmpty()) {
                setupLines.forEach { line ->
                    notice.addView(textView(line.title, 13f, COLOR_NOTICE_TEXT, bold = true))
                    notice.addView(textView(line.body, 12f, COLOR_TEXT_SECONDARY))
                }
            } else if (blocked && panel.statusBody.isNotBlank()) {
                notice.addView(textView(panel.statusBody, 13f, COLOR_NOTICE_TEXT, bold = true))
            }
            if (panel.setupVisible) {
                notice.addView(
                    TextView(this).apply {
                        text = "Open Echolet Setup"
                        textSize = 13f
                        typeface = Typeface.DEFAULT_BOLD
                        setTextColor(Color.WHITE)
                        gravity = Gravity.CENTER
                        background = RippleDrawable(
                            ColorStateList.valueOf(0x33FFFFFF),
                            roundedFilled(COLOR_PRIMARY_BUTTON, 8),
                            null,
                        )
                        isClickable = true
                        isFocusable = true
                        // Explicit user action only: opens the setup Activity,
                        // never writes setup/error text into the editor and never
                        // routed to the controller.
                        setOnClickListener { presenter.onSetupClicked() }
                    },
                    LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(40))
                        .apply { topMargin = dp(8) },
                )
            }
            body.addView(notice, matchWrap().apply { bottomMargin = dp(16) })
        }

        // 2. Models.
        val snapshot = panel.modelSnapshot
        if (snapshot != null && snapshot.groups.isNotEmpty()) {
            body.addView(sectionLabel("MODELS"), matchWrap().apply { bottomMargin = dp(8) })
            snapshot.groups.forEachIndexed { gi, group ->
                body.addView(
                    singleLine(group.label, 11f, COLOR_TEXT_MUTED, bold = true),
                    matchWrap().apply {
                        if (gi > 0) topMargin = dp(10)
                        bottomMargin = dp(6)
                    },
                )
                group.models.forEach { model ->
                    body.addView(modelRow(model), matchWrap().apply { bottomMargin = dp(6) })
                }
            }
        }

        // 3. Footer.
        body.addView(
            singleLine("Local dictation. Audio never leaves this device.", 10f, COLOR_TEXT_SUBTLE).apply {
                typeface = Typeface.MONOSPACE
                gravity = Gravity.CENTER
            },
            matchWrap().apply { topMargin = dp(10) },
        )
    }

    /** One desktop-style model row: name + date on the left, action on the right. */
    private fun modelRow(model: ModelItemUi): View {
        val row = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(12), dp(6), dp(8), dp(6))
            background = GradientDrawable().apply {
                cornerRadius = dp(10).toFloat()
                setColor(if (model.selected) COLOR_ROW_SELECTED else COLOR_CARD)
                setStroke(dp(1), if (model.selected) COLOR_BORDER_STRONG else COLOR_BORDER)
            }
        }

        val info = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }

        // Desktop shows the short name and puts the version/date on the next
        // line, so a long "Name — version" title is not cut off with an ellipsis.
        val parts = model.label.split(" — ", limit = 2)
        val shortName = parts[0].trim().ifEmpty { model.label.trim() }
        val name = SpannableStringBuilder(shortName)
        if (!model.isVerified && model.verificationLabel.isNotBlank()) {
            val start = name.length
            name.append("  ").append(model.verificationLabel)
            name.setSpan(ForegroundColorSpan(COLOR_STOP_RED), start, name.length, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
            name.setSpan(RelativeSizeSpan(0.8f), start, name.length, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
        }
        info.addView(infoText(name, 13f, COLOR_TEXT_PRIMARY, bold = true))

        val inProgress = model.downloadPhase in IN_PROGRESS_PHASES
        val subtitle = model.releaseDate.trim().ifEmpty { parts.getOrNull(1)?.trim().orEmpty() }
        if (subtitle.isNotEmpty()) {
            info.addView(
                infoText(subtitle, 11f, COLOR_TEXT_SUBTLE),
                matchWrap().apply { topMargin = dp(1) },
            )
        }
        row.addView(info, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f))

        val action = TextView(this).apply {
            textSize = 12f
            typeface = Typeface.DEFAULT_BOLD
            gravity = Gravity.CENTER
            isSingleLine = true
            includeFontPadding = false
            setPadding(dp(4), 0, dp(4), 0)
        }
        when {
            model.selected -> {
                action.text = "✓ Selected"
                action.setTextColor(Color.WHITE)
                action.background = roundedFilled(COLOR_PRIMARY_BUTTON, 8)
            }
            inProgress -> {
                val pct = model.progressPercent
                action.text = when (model.downloadPhase) {
                    "Downloading" -> pct?.let { "$it%" } ?: "Downloading"
                    "Starting" -> "Starting"
                    "Verifying" -> "Verifying"
                    "Extracting" -> "Extracting"
                    "Installing" -> "Installing"
                    else -> pct?.let { "$it%" } ?: model.downloadPhase
                }
                action.setTextColor(COLOR_PRIMARY_BUTTON)
                val fill = ClipDrawable(roundedFilled(COLOR_BORDER_STRONG, 8), Gravity.START, ClipDrawable.HORIZONTAL)
                fill.level = ((pct ?: 0).coerceIn(0, 100)) * 100
                action.background = LayerDrawable(arrayOf(smallOutline(), fill))
            }
            model.primaryAction == "Select" || model.primaryAction == "Download" ||
                model.primaryAction == "RetryDownload" -> {
                action.text = when (model.primaryAction) {
                    "Select" -> "Select"
                    "RetryDownload" -> "Retry"
                    else -> "Download"
                }
                action.isEnabled = model.enabled
                action.setTextColor(if (model.enabled) COLOR_PRIMARY_BUTTON else COLOR_TEXT_SUBTLE)
                action.background = if (model.enabled) {
                    RippleDrawable(ColorStateList.valueOf(0x1A000000), smallOutline(), null)
                } else {
                    roundedFilled(COLOR_ROW_SELECTED, 8)
                }
                action.isClickable = true
                action.isFocusable = true
                action.setOnClickListener { v ->
                    haptic(v, strong = false)
                    if (model.primaryAction == "Select") selectModel(model.id) else {
                        downloadManager.startDownload(model.id)
                        refreshModelSnapshot()
                    }
                }
            }
            else -> action.visibility = View.GONE
        }
        row.addView(
            action,
            LinearLayout.LayoutParams(dp(96), dp(34)).apply { marginStart = dp(8) },
        )
        return row
    }

    private fun selectModel(modelId: String) {
        if (isDestroyed) return
        if (controller.isSessionActive()) {
            android.util.Log.w("EcholetIme", "Model selection rejected: session active")
            return
        }
        try {
            modelExecutor.execute {
                if (isDestroyed) return@execute
                if (controller.isSessionActive()) {
                    android.util.Log.w("EcholetIme", "Model selection rejected: session became active")
                    return@execute
                }
                val ok = try {
                    NativeBridge.nativeSelectModel(modelId)
                } catch (t: Throwable) {
                    android.util.Log.e("EcholetIme", "Model selection failed for $modelId", t)
                    false
                }
                if (ok) {
                    runOnUi {
                        if (isDestroyed) return@runOnUi
                        refreshReadiness()
                        refreshModelSnapshot()
                    }
                } else {
                    android.util.Log.w("EcholetIme", "Model selection rejected by core for $modelId")
                }
            }
        } catch (_: java.util.concurrent.RejectedExecutionException) {
            // Fail closed
        }
    }

    private fun matchWrap() = LinearLayout.LayoutParams(
        ViewGroup.LayoutParams.MATCH_PARENT,
        ViewGroup.LayoutParams.WRAP_CONTENT,
    )

    /** Monospace uppercase eyebrow heading, as on the desktop panel. */
    private fun sectionLabel(text: String): TextView =
        singleLine(text, 10f, COLOR_TEXT_MUTED, bold = true).apply {
            typeface = Typeface.create(Typeface.MONOSPACE, Typeface.BOLD)
            letterSpacing = 0.12f
        }

    private fun singleLine(text: String, size: Float, color: Int, bold: Boolean = false): TextView =
        TextView(this).apply {
            this.text = text
            textSize = size
            setTextColor(color)
            if (bold) typeface = Typeface.DEFAULT_BOLD
            isSingleLine = true
            ellipsize = TextUtils.TruncateAt.END
            includeFontPadding = false
        }

    /** Model-row copy: wraps inside the left column and never draws an ellipsis. */
    private fun infoText(text: CharSequence, size: Float, color: Int, bold: Boolean = false): TextView =
        TextView(this).apply {
            this.text = text
            textSize = size
            setTextColor(color)
            if (bold) typeface = Typeface.DEFAULT_BOLD
            includeFontPadding = false
            setLineSpacing(0f, 1f)
            layoutParams = matchWrap()
        }

    private fun textView(text: String, size: Float, color: Int, bold: Boolean = false): TextView =
        TextView(this).apply {
            this.text = text
            textSize = size
            setTextColor(color)
            if (bold) typeface = Typeface.DEFAULT_BOLD
            setPadding(0, dp(2), 0, dp(2))
        }

    private fun roundedFilled(color: Int, radiusDp: Int = 12): GradientDrawable = GradientDrawable().apply {
        cornerRadius = dp(radiusDp).toFloat()
        setColor(color)
    }

    private fun roundedOutline(radiusDp: Int = 12): GradientDrawable = GradientDrawable().apply {
        cornerRadius = dp(radiusDp).toFloat()
        setColor(COLOR_CARD)
        setStroke(dp(1), COLOR_BORDER)
    }

    private fun smallOutline(): GradientDrawable = roundedOutline(8)

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
        applyNavBarAppearance()
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
        applyNavBarAppearance()
        root?.requestApplyInsets()
        ui.post {
            if (!isDestroyed) applyNavBarAppearance()
        }
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
        deleteHandler.removeCallbacksAndMessages(null)
        synchronized(this) {
            isDestroyed = true
            modelSnapshotGen++
        }
        if (::downloadManager.isInitialized) {
            downloadManager.detachObserver()
        }
        controller.onDestroyed()
        clearViewRefs()
        modelExecutor.shutdownNow()
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
        if (isDestroyed) return
        val action = Runnable {
            if (isDestroyed) return@Runnable
            body()
        }
        if (Looper.myLooper() == Looper.getMainLooper()) action.run() else ui.post(action)
    }

    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()

    private companion object {
        const val COLOR_BG = 0xFFF9FAFB.toInt()
        const val COLOR_CARD = 0xFFFFFFFF.toInt()
        const val COLOR_BORDER = 0xFFE5E7EB.toInt()
        const val COLOR_TEXT_PRIMARY = 0xFF111827.toInt()
        const val COLOR_TEXT_MUTED = 0xFF6B7280.toInt()
        const val COLOR_PRIMARY_BUTTON = 0xFF1F2937.toInt()
        const val COLOR_STOP_RED = 0xFFDC2626.toInt()
        const val COLOR_DANGER = 0xFFDC2626.toInt()
        const val COLOR_SUCCESS = 0xFF059669.toInt()
        const val COLOR_WARNING = 0xFFD97706.toInt()
        const val COLOR_NOTICE_BG = 0xFFFEF3C7.toInt()
        const val COLOR_NOTICE_BORDER = 0xFFFDE68A.toInt()
        const val COLOR_NOTICE_TEXT = 0xFF92400E.toInt()
        const val COLOR_TEXT_SECONDARY = 0xFF4B5563.toInt()
        const val COLOR_TEXT_SUBTLE = 0xFF9CA3AF.toInt()
        const val COLOR_ROW_SELECTED = 0xFFF3F4F6.toInt()
        const val COLOR_BORDER_STRONG = 0xFFD1D5DB.toInt()

        val IN_PROGRESS_PHASES = setOf(
            "Starting",
            "Preparing",
            "Downloading",
            "Verifying",
            "Extracting",
            "Installing",
        )

        // Backward compatibility aliases
        const val COLOR_CHARCOAL = COLOR_PRIMARY_BUTTON
        const val COLOR_SURFACE = COLOR_BG
        const val COLOR_TEXT = COLOR_TEXT_PRIMARY
        const val COLOR_MUTED = COLOR_TEXT_MUTED
    }
}
