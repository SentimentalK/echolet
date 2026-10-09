package com.mainstayx.echolet

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.provider.Settings
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.Gravity
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import java.io.File
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import org.json.JSONArray
import org.json.JSONObject

/**
 * Phase 0-A diagnostic activity: proves the shared Rust ASR core (via
 * libecholet_android.so / sherpa-onnx C API) recognizes the real pinned X-ASR
 * fixture on this device. This is NOT the IME, NOT streaming microphone
 * capture, and NOT an InputConnection — no deltas are written to any OS editor
 * here, and history stays off. Phase 0-B adds ONLY two setup affordances:
 * 'Allow microphone' and 'Enable/Select Echolet keyboard'.
 */
class MainActivity : Activity() {
    private var runButton: TextView? = null
    private var micButton: TextView? = null
    private var imeButton: TextView? = null
    private var output: TextView? = null
    private var executor: ExecutorService? = null
    private val main = Handler(Looper.getMainLooper())

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER_HORIZONTAL
        }

        runButton = TextView(this).apply {
            text = "Run offline ASR fixture"
            textSize = 20f
            gravity = Gravity.CENTER
            setPadding(32, 24, 32, 24)
            setOnClickListener { runFixtureOnce() }
        }
        root.addView(
            runButton,
            LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT,
            ),
        )

        // Phase 0-B setup affordances (explicit user gesture only).
        micButton = TextView(this).apply {
            text = "Allow microphone"
            textSize = 18f
            gravity = Gravity.CENTER
            setPadding(32, 24, 32, 24)
            setOnClickListener {
                val granted =
                    checkSelfPermission(Manifest.permission.RECORD_AUDIO) ==
                        PackageManager.PERMISSION_GRANTED
                if (granted) {
                    status("Microphone permission already granted.")
                } else {
                    requestPermissions(
                        arrayOf(Manifest.permission.RECORD_AUDIO),
                        REQUEST_MIC,
                    )
                }
            }
        }
        root.addView(
            micButton,
            LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT,
            ),
        )

        imeButton = TextView(this).apply {
            text = "Enable/Select Echolet keyboard"
            textSize = 18f
            gravity = Gravity.CENTER
            setPadding(32, 24, 32, 24)
            setOnClickListener {
                // System gesture path: open IME settings; the picker needs an
                // active token we do not have from an idle Activity.
                startActivity(Intent(Settings.ACTION_INPUT_METHOD_SETTINGS))
            }
        }
        root.addView(
            imeButton,
            LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT,
            ),
        )

        output = TextView(this).apply {
            text = "Press the button to run the offline fixture."
            textSize = 16f
            setPadding(24, 24, 24, 24)
        }
        val scroll = ScrollView(this).apply {
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.MATCH_PARENT,
            )
            addView(output)
        }
        root.addView(scroll)

        setContentView(root)
        executor = Executors.newSingleThreadExecutor()
    }

    private fun status(text: String) {
        main.post { output?.text = text }
    }

    /** Fixture staging path: only the pinned bilingual-zh-en debug fixture. */
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
                main.post { button.isEnabled = true }
            }
        }
    }

    /**
     * One complete open→feed→close cycle per run so no stale events leak
     * between runs; the handle is closed in `finally` even when a feed throws.
     */
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
        // One nativeOpen per run: "reuse resident model, fresh session" is the
        // bridge's behavior under the hood.
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
            // 3200-frame chunks (200 ms at 16 kHz). Every nativeFeed returns
            // the already-ADMITTED event array; applying the char-unit diff
            // mirrors src/diff.rs exactly (never UTF-8 byte lengths).
            // FloatArray has no stdlib chunked(): convert once to 3200-frame
            // chunks (200 ms at 16 kHz).
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

    override fun onDestroy() {
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
            status(
                if (grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED) {
                    "Microphone permission granted."
                } else {
                    "Microphone permission denied; the IME stays BLOCKED until granted."
                }
            )
        }
    }

    companion object {
        private const val REQUEST_MIC = 7001
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
                // "endpoint" events carry the completed admitted text of the
                // utterance, which stays visible via the partial stream; the
                // diagnostic sink simply records them (no-op here).
                "endpoint" -> {}
                else -> throw IllegalArgumentException(
                    "malformed wire event: unknown event kind ${event.optString("kind")}"
                )
            }
        }
        return visible.toString()
    }
}
