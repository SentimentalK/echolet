package com.mainstayx.echolet

import android.Manifest
import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.view.WindowInsets
import android.view.inputmethod.InputMethodManager
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import java.io.File
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import org.json.JSONArray
import org.json.JSONObject

/**
 * Echolet one-time setup and diagnostic Activity.
 *
 * Provides a truthful landing surface for OS prerequisites:
 * 1. Microphone permission (RECORD_AUDIO).
 * 2. Echolet InputMethodService enabled in Android settings.
 * 3. Echolet currently selected as active input method (when observable).
 *
 * Model catalog browsing, download, and selection take place inside the
 * Echolet keyboard itself; this Activity does not manage models.
 *
 * Also provides a clearly separated developer diagnostics affordance for the
 * offline X-ASR fixture run.
 */
class MainActivity : Activity() {

    // Status views
    private var micStatusView: TextView? = null
    private var imeEnabledStatusView: TextView? = null
    private var guidanceView: TextView? = null

    // Setup action buttons
    private var micButton: TextView? = null
    private var imeButton: TextView? = null

    // Diagnostics views
    private var runButton: TextView? = null
    private var output: TextView? = null

    private var executor: ExecutorService? = null
    private val main = Handler(Looper.getMainLooper())
    @Volatile private var isDestroyed = false
    private var micPermissionDeniedOnce = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER_HORIZONTAL
            fitsSystemWindows = true
            setBackgroundColor(COLOR_BG)
        }

        root.setOnApplyWindowInsetsListener { v, insets ->
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                val bars = insets.getInsets(WindowInsets.Type.systemBars())
                v.setPadding(bars.left, bars.top, bars.right, bars.bottom)
            } else {
                @Suppress("DEPRECATION")
                v.setPadding(
                    insets.systemWindowInsetLeft,
                    insets.systemWindowInsetTop,
                    insets.systemWindowInsetRight,
                    insets.systemWindowInsetBottom,
                )
            }
            insets
        }

        val scroll = ScrollView(this).apply {
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.MATCH_PARENT,
            )
            isVerticalScrollBarEnabled = true
        }

        val content = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(20), dp(16), dp(20), dp(24))
        }

        // --- Header ---
        val headerTitle = TextView(this).apply {
            text = "Echolet Voice Keyboard"
            textSize = 22f
            typeface = Typeface.DEFAULT_BOLD
            setTextColor(COLOR_TEXT_PRIMARY)
        }
        val headerSubtitle = TextView(this).apply {
            text = "One-time setup for offline, on-device voice typing"
            textSize = 14f
            setTextColor(COLOR_TEXT_MUTED)
            setPadding(0, dp(4), 0, dp(16))
        }
        content.addView(headerTitle)
        content.addView(headerSubtitle)

        // --- Card 1: Live OS Status ---
        val statusCard = roundedCard().apply {
            val cardTitle = TextView(this@MainActivity).apply {
                text = "System Setup Status"
                textSize = 16f
                typeface = Typeface.DEFAULT_BOLD
                setTextColor(COLOR_TEXT_PRIMARY)
                setPadding(0, 0, 0, dp(12))
            }
            addView(cardTitle)

            micStatusView = createStatusRow(this@MainActivity, "1. Microphone Access")
            addView(micStatusView)

            imeEnabledStatusView = createStatusRow(this@MainActivity, "2. Keyboard in System Settings")
            addView(imeEnabledStatusView)

            val modelInfoNote = TextView(this@MainActivity).apply {
                text = "Voice models: Downloaded and selected inside Echolet keyboard."
                textSize = 13f
                setTextColor(COLOR_TEXT_MUTED)
                setPadding(0, dp(8), 0, dp(8))
            }
            addView(modelInfoNote)

            guidanceView = TextView(this@MainActivity).apply {
                textSize = 14f
                typeface = Typeface.DEFAULT_BOLD
                setTextColor(COLOR_TEXT_PRIMARY)
                background = roundedFilled(0x1A2563EB, dp(6))
                setPadding(dp(12), dp(10), dp(12), dp(10))
            }
            addView(guidanceView)
        }
        content.addView(
            statusCard,
            LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT,
            ).apply { bottomMargin = dp(16) }
        )

        // --- Card 2: Setup Actions ---
        val actionsCard = roundedCard().apply {
            val actionsTitle = TextView(this@MainActivity).apply {
                text = "Setup Actions"
                textSize = 16f
                typeface = Typeface.DEFAULT_BOLD
                setTextColor(COLOR_TEXT_PRIMARY)
                setPadding(0, 0, 0, dp(12))
            }
            addView(actionsTitle)

            micButton = TextView(this@MainActivity).apply {
                textSize = 15f
                typeface = Typeface.DEFAULT_BOLD
                setTextColor(Color.WHITE)
                gravity = Gravity.CENTER
                minHeight = dp(48)
                setPadding(dp(16), dp(12), dp(16), dp(12))
                isClickable = true
                isFocusable = true
            }
            addView(
                micButton,
                LinearLayout.LayoutParams(
                    LinearLayout.LayoutParams.MATCH_PARENT,
                    LinearLayout.LayoutParams.WRAP_CONTENT,
                ).apply { bottomMargin = dp(10) }
            )

            imeButton = TextView(this@MainActivity).apply {
                text = "Enable / Select Echolet keyboard"
                textSize = 15f
                typeface = Typeface.DEFAULT_BOLD
                setTextColor(Color.WHITE)
                gravity = Gravity.CENTER
                minHeight = dp(48)
                background = roundedFilled(COLOR_PRIMARY_BUTTON)
                setPadding(dp(16), dp(12), dp(16), dp(12))
                isClickable = true
                isFocusable = true
                setOnClickListener {
                    val opened = try {
                        startActivity(Intent(Settings.ACTION_INPUT_METHOD_SETTINGS))
                        true
                    } catch (_: ActivityNotFoundException) {
                        false
                    } catch (_: SecurityException) {
                        false
                    }
                    if (!opened) {
                        status("Could not open keyboard settings automatically. Open Android Settings > System > Languages & input to enable Echolet.")
                    }
                }
            }
            addView(
                imeButton,
                LinearLayout.LayoutParams(
                    LinearLayout.LayoutParams.MATCH_PARENT,
                    LinearLayout.LayoutParams.WRAP_CONTENT,
                )
            )
        }
        content.addView(
            actionsCard,
            LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT,
            ).apply { bottomMargin = dp(20) }
        )

        // --- Card 3: Developer Diagnostics (Phase 0-A Fixture) ---
        val diagCard = roundedCard().apply {
            val diagTitle = TextView(this@MainActivity).apply {
                text = "Diagnostics"
                textSize = 15f
                typeface = Typeface.DEFAULT_BOLD
                setTextColor(COLOR_TEXT_MUTED)
                setPadding(0, 0, 0, dp(8))
            }
            addView(diagTitle)

            val diagSubtitle = TextView(this@MainActivity).apply {
                text = "Verify offline speech recognition using the pinned test fixture."
                textSize = 13f
                setTextColor(COLOR_TEXT_MUTED)
                setPadding(0, 0, 0, dp(12))
            }
            addView(diagSubtitle)

            runButton = TextView(this@MainActivity).apply {
                text = "Run offline ASR fixture"
                textSize = 15f
                typeface = Typeface.DEFAULT_BOLD
                setTextColor(COLOR_TEXT_PRIMARY)
                gravity = Gravity.CENTER
                minHeight = dp(48)
                background = roundedOutline()
                setPadding(dp(16), dp(12), dp(16), dp(12))
                isClickable = true
                isFocusable = true
                setOnClickListener { runFixtureOnce() }
            }
            addView(
                runButton,
                LinearLayout.LayoutParams(
                    LinearLayout.LayoutParams.MATCH_PARENT,
                    LinearLayout.LayoutParams.WRAP_CONTENT,
                ).apply { bottomMargin = dp(12) }
            )

            output = TextView(this@MainActivity).apply {
                text = "Fixture output will appear here."
                textSize = 13f
                setTextColor(COLOR_TEXT_MUTED)
                background = roundedFilled(0xFFE5E7EB.toInt(), dp(4))
                setPadding(dp(12), dp(10), dp(12), dp(10))
            }
            addView(output)
        }
        content.addView(
            diagCard,
            LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT,
            )
        )

        scroll.addView(content)
        root.addView(scroll)
        setContentView(root)

        executor = Executors.newSingleThreadExecutor()
    }

    override fun onResume() {
        super.onResume()
        refreshStatusUi()
    }

    override fun onDestroy() {
        isDestroyed = true
        executor?.shutdown()
        executor = null
        super.onDestroy()
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == REQUEST_MIC) {
            val granted = grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED
            if (!granted) {
                micPermissionDeniedOnce = true
            }
            refreshStatusUi()
        }
    }

    // ----------------------------------------------------------- status logic

    fun deriveOsStatus(): SetupOsStatus {
        val micGranted = checkSelfPermission(Manifest.permission.RECORD_AUDIO) ==
            PackageManager.PERMISSION_GRANTED

        val targetComponent = ComponentName(this, EcholetInputMethodService::class.java)
        val targetPkg = targetComponent.packageName
        val targetCls = targetComponent.className
        val targetShort = targetComponent.flattenToShortString()
        val targetFull = targetComponent.flattenToString()

        val imm = getSystemService(Context.INPUT_METHOD_SERVICE) as? InputMethodManager
        val enabledMethods = try {
            imm?.enabledInputMethodList
        } catch (_: Throwable) {
            null
        }
        val imeEnabled = enabledMethods?.any {
            it.packageName == targetPkg && it.serviceName == targetCls
        } ?: false

        val currentIme = try {
            Settings.Secure.getString(contentResolver, Settings.Secure.DEFAULT_INPUT_METHOD)
        } catch (_: Throwable) {
            null
        }

        val selectedState = if (!imeEnabled) {
            KeyboardSelectedState.NOT_SELECTED
        } else {
            SetupStatusResolver.resolveSelectedState(
                currentImeSetting = currentIme,
                targetIdShort = targetShort,
                targetIdFull = targetFull,
            )
        }

        return SetupOsStatus(
            micGranted = micGranted,
            imeEnabled = imeEnabled,
            imeSelected = selectedState,
        )
    }

    private fun refreshStatusUi() {
        if (isDestroyed || isFinishing) return
        val status = deriveOsStatus()

        // Update status rows
        micStatusView?.text = "1. Microphone Access: " +
            SetupStatusResolver.micStatusSummary(status.micGranted)
        micStatusView?.setTextColor(if (status.micGranted) COLOR_SUCCESS else COLOR_DANGER)

        imeEnabledStatusView?.text = "2. Keyboard in System Settings: " +
            SetupStatusResolver.imeEnabledSummary(status.imeEnabled)
        imeEnabledStatusView?.setTextColor(if (status.imeEnabled) COLOR_SUCCESS else COLOR_DANGER)

        guidanceView?.text = SetupStatusResolver.deriveGuidance(status)

        // Update mic button
        if (status.micGranted) {
            micButton?.text = "Microphone permission granted ✓"
            micButton?.background = roundedFilled(COLOR_SUCCESS)
            micButton?.setOnClickListener(null)
            micButton?.isEnabled = false
        } else if (micPermissionDeniedOnce) {
            micButton?.text = "Open App Settings for Microphone"
            micButton?.background = roundedFilled(COLOR_PRIMARY_BUTTON)
            micButton?.isEnabled = true
            micButton?.setOnClickListener {
                val opened = try {
                    val intent = Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS).apply {
                        data = Uri.fromParts("package", packageName, null)
                        addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    }
                    startActivity(intent)
                    true
                } catch (_: ActivityNotFoundException) {
                    false
                } catch (_: SecurityException) {
                    false
                }
                if (!opened) {
                    status("Could not open app settings automatically. Please grant microphone access in Android Settings > Apps > Echolet.")
                }
            }
        } else {
            micButton?.text = "Allow microphone"
            micButton?.background = roundedFilled(COLOR_PRIMARY_BUTTON)
            micButton?.isEnabled = true
            micButton?.setOnClickListener {
                requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), REQUEST_MIC)
            }
        }

        // Update keyboard button
        if (status.imeEnabled) {
            imeButton?.text = "Echolet keyboard enabled ✓"
            imeButton?.background = roundedFilled(COLOR_SUCCESS)
            imeButton?.setOnClickListener(null)
            imeButton?.isEnabled = false
        } else {
            imeButton?.text = "Enable Echolet keyboard"
            imeButton?.background = roundedFilled(COLOR_PRIMARY_BUTTON)
            imeButton?.isEnabled = true
            imeButton?.setOnClickListener {
                val opened = try {
                    startActivity(Intent(Settings.ACTION_INPUT_METHOD_SETTINGS))
                    true
                } catch (_: ActivityNotFoundException) {
                    false
                } catch (_: SecurityException) {
                    false
                }
                if (!opened) {
                    status("Could not open keyboard settings automatically. Open Android Settings > System > Languages & input to enable Echolet.")
                }
            }
        }
    }

    private fun status(text: String) {
        main.post {
            if (!isDestroyed && !isFinishing) {
                output?.text = text
            }
        }
    }

    // ----------------------------------------------------- fixture diagnostic

    private fun fixturePath(): File {
        val base =
            getExternalFilesDir("models") ?: throw IllegalStateException(
                "app-specific external files dir unavailable"
            )
        return File(base, "bilingual-zh-en")
    }

    private fun runFixtureOnce() {
        val pool = executor ?: return
        val button = runButton ?: return
        button.isEnabled = false
        status("Running offline ASR fixture…")
        pool.execute {
            try {
                val result = runFixtureBlocking()
                status(result)
            } catch (t: Throwable) {
                status("FAILED: " + (t.message ?: t.javaClass.simpleName))
            } finally {
                main.post {
                    if (!isDestroyed && !isFinishing) {
                        button.isEnabled = true
                    }
                }
            }
        }
    }

    private fun runFixtureBlocking(): String {
        val dir = fixturePath()
        val required =
            listOf(
                "model.json",
                "encoder-480ms.onnx",
                "decoder-480ms.onnx",
                "joiner-480ms.onnx",
                "tokens.txt",
                "test_wavs/0.wav",
            )
        for (name in required) {
            val f = File(dir, name)
            if (!f.exists() || f.length() == 0L) {
                throw IllegalStateException(
                    "model missing: fixture file $name absent/empty under " +
                        dir.absolutePath +
                        ". Stage it with android/scripts/stage-fixture.sh (adb)."
                )
            }
        }

        val samples = WavFixture.readMono16k(File(dir, "test_wavs/0.wav"))
        if (samples.isEmpty()) {
            throw IllegalArgumentException(
                "${File(dir, "test_wavs/0.wav").path} contains zero PCM frames " +
                    "(zero-length/invalid WAV); refusing to treat it as a valid fixture"
            )
        }

        val visible = StringBuilder()
        val handle =
            try {
                NativeBridge.nativeOpen(dir.absolutePath)
            } catch (e: UnsatisfiedLinkError) {
                throw IllegalStateException(
                    "JNI library load failure: " + (e.message ?: e.javaClass.simpleName) +
                        ". Run android/scripts/verify-apk-arm64.sh — the APK likely " +
                        "misses libecholet_android.so or its dependencies."
                )
            }
        try {
            val chunks: List<FloatArray> = samples.toList().chunked(3200) { it.toFloatArray() }
            for ((index, chunk) in chunks.withIndex()) {
                val response = NativeBridge.nativeFeed(handle, chunk, 16000)
                val applied = WireEvents.applyTo(visible, response)
                status(
                    "chunk " + (index + 1) + " fed\n  admitted event applied\n  visible: " +
                        applied +
                        "\n"
                )
            }
            val transcript = visible.toString()
            if (transcript.isBlank()) {
                throw IllegalStateException(
                    "real decoding produced NO transcript: the visible buffer is " +
                        "empty/blank after " + chunks.size + " chunks (never report OK " +
                        "for an empty fixture run; check model dir + tokens.txt)"
                )
            }
            return "OK. Recognized transcript:\n$transcript"
        } finally {
            NativeBridge.nativeClose(handle)
        }
    }

    // ---------------------------------------------------------- view helpers

    private fun createStatusRow(context: Context, label: String): TextView =
        TextView(context).apply {
            text = "$label: Checking…"
            textSize = 14f
            typeface = Typeface.DEFAULT_BOLD
            setTextColor(COLOR_TEXT_MUTED)
            setPadding(0, dp(4), 0, dp(6))
        }

    private fun roundedCard(): LinearLayout = LinearLayout(this).apply {
        orientation = LinearLayout.VERTICAL
        background = roundedFilled(COLOR_CARD, dp(10))
        setPadding(dp(16), dp(14), dp(16), dp(14))
    }

    private fun roundedFilled(color: Int, radiusDp: Int = 8): GradientDrawable =
        GradientDrawable().apply {
            cornerRadius = dp(radiusDp).toFloat()
            setColor(color)
        }

    private fun roundedOutline(): GradientDrawable = GradientDrawable().apply {
        cornerRadius = dp(8).toFloat()
        setColor(Color.WHITE)
        setStroke(dp(1), 0xFFD1D5DB.toInt())
    }

    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()

    companion object {
        private const val REQUEST_MIC = 7001

        private const val COLOR_BG = 0xFFF9FAFB.toInt()
        private const val COLOR_CARD = 0xFFFFFFFF.toInt()
        private const val COLOR_TEXT_PRIMARY = 0xFF111827.toInt()
        private const val COLOR_TEXT_MUTED = 0xFF6B7280.toInt()
        private const val COLOR_PRIMARY_BUTTON = 0xFF1F2937.toInt()
        private const val COLOR_SUCCESS = 0xFF059669.toInt()
        private const val COLOR_WARNING = 0xFFD97706.toInt()
        private const val COLOR_DANGER = 0xFFDC2626.toInt()
    }
}

/** Minimal validated reader for the debug fixture: mono 16-bit 16 kHz PCM WAV. */
object WavFixture {
    fun readMono16k(file: File): FloatArray {
        val bytes = file.readBytes()
        if (bytes.size < 12 || bytes.sliceArray(0 until 4).decodeToString() != "RIFF") {
            throw IllegalArgumentException("${file.name} is not a RIFF/WAVE file")
        }
        var cursor = 12
        var format = -1
        var channels = 0
        var sampleRate = 0
        var bitsPerSample = 0
        var data: ByteArray? = null
        while (cursor + 8 <= bytes.size) {
            val id = bytes.sliceArray(cursor until cursor + 4).decodeToString()
            val len = readU32(bytes, cursor + 4)
            val end = cursor + 8 + len
            val payload =
                if (end <= bytes.size) bytes.sliceArray(cursor + 8 until end) else ByteArray(0)
            when (id) {
                "fmt " -> {
                    format = readU16(payload, 0)
                    channels = readU16(payload, 2)
                    sampleRate = readU32(payload, 4)
                    bitsPerSample = readU16(payload, 14)
                }
                "data" -> data = payload
            }
            cursor = end
            if (len % 2 == 1) cursor += 1 // chunks are word-aligned
        }
        // Only the exact fixture shape is accepted; empty PCM data is invalid.
        if (format != 1 ||
                channels != 1 ||
                sampleRate != 16000 ||
                bitsPerSample != 16 ||
                data == null ||
                data.isEmpty()
        ) {
            throw IllegalArgumentException(
                "${file.name} is not mono 16-bit 16 kHz PCM WAV " +
                    "(format=$format, channels=$channels, rate=$sampleRate, bits=$bitsPerSample)"
            )
        }
        return FloatArray(data.size / 2) { i ->
            val lo = data[2 * i].toInt() and 0xFF
            val hi = data[2 * i + 1].toInt()
            ((hi shl 8) or lo).toShort().toFloat() / 32768f
        }
    }

    private fun readU32(bytes: ByteArray, off: Int): Int =
        (bytes[off].toInt() and 0xFF) or
            ((bytes[off + 1].toInt() and 0xFF) shl 8) or
            ((bytes[off + 2].toInt() and 0xFF) shl 16) or
            ((bytes[off + 3].toInt() and 0xFF) shl 24)

    private fun readU16(bytes: ByteArray, off: Int): Int =
        (bytes[off].toInt() and 0xFF) or ((bytes[off + 1].toInt() and 0xFF) shl 8)
}

/** Applies admitted wire events to a character-unit visible buffer. */
object WireEvents {
    fun applyTo(visible: StringBuilder, jsonText: String): String {
        val events = JSONArray(jsonText)
        for (i in 0 until events.length()) {
            val event = events.getJSONObject(i)
            when (event.optString("kind")) {
                "partial" -> {
                    if (!event.has("backspaces")) {
                        throw IllegalArgumentException(
                            "malformed wire event: partial without backspaces: $event"
                        )
                    }
                    CodepointDiffBuffer.apply(
                        visible,
                        event.optInt("backspaces"),
                        event.optString("suffix", ""),
                    )
                }
                "endpoint" -> {}
                else -> throw IllegalArgumentException(
                    "malformed wire event: unknown event kind ${event.optString("kind")}"
                )
            }
        }
        return visible.toString()
    }
}
