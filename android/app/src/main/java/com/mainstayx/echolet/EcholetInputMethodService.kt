package com.mainstayx.echolet

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.inputmethodservice.InputMethodService
import android.os.Handler
import android.os.Looper
import android.view.Gravity
import android.view.View
import android.view.inputmethod.EditorInfo
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import java.io.File

/**
 * Phase 0-B system IME: a visible keyboard with ONE status label and ONE wide
 * Start/Stop control. When permission + staged X-ASR model + native libs are
 * ready, tapping Start (or next visibility, unless user-paused) starts
 * microphone capture through the EXISTING JNI Rust session core and projects
 * partial/endpoint events onto the CURRENT InputConnection (composing region
 * only). No QWERTY, no model download, no network.
 */
class EcholetInputMethodService : InputMethodService() {

    private lateinit var controller: ImeSessionController
    private var status: TextView? = null
    private var control: Button? = null

    /** Parses lazily into a memoized flag; re-checked cheaply. */
    private var nativeLibsReady: Boolean? = null
    private val ui = Handler(Looper.getMainLooper())

    override fun onCreate() {
        super.onCreate()
        controller = ImeSessionController(
            applicationContext,
            onState = { state, text -> render(state, text) },
            icProvider = { currentInputConnection },
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

    // ------------------------------------------------------------- input view

    override fun onCreateInputView(): View {
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER
        }
        status = TextView(this).apply {
            textSize = 16f
            gravity = Gravity.CENTER
            setPadding(24, 24, 24, 24)
            setOnClickListener {
                if (controller.model.currentState == ImeSessionModel.ImeState.BLOCKED) {
                    // Explicit user action only: opens the setup Activity,
                    // never writes setup/error text into the editor.
                    val intent = Intent(this@EcholetInputMethodService, MainActivity::class.java)
                    intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    startActivity(intent)
                }
            }
        }
        control = Button(this).apply {
            textSize = 20f
            setOnClickListener {
                val ready = blockedReason() == null
                controller.onControlTap(
                    ready = ready,
                    blockedReason = blockedReason(),
                    ic = currentInputConnection,
                )
            }
        }
        root.addView(
            status,
            LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT)
        )
        root.addView(
            control,
            LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT)
        )
        render(controller.model.currentState, "Not listening")
        return root
    }

    private fun render(state: ImeSessionModel.ImeState, text: String) {
        runOnUi {
            status?.text = text
            control?.text = when (state) {
                ImeSessionModel.ImeState.LISTENING, ImeSessionModel.ImeState.PREPARING -> "Stop"
                else -> "Start"
            }
        }
    }

    private fun runOnUi(body: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) body() else ui.post(body)
    }

    // ------------------------------------------------------- lifecycle routing

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
        controller.onWindowShown()
    }

    override fun onWindowHidden() {
        controller.onWindowHidden()
        super.onWindowHidden()
    }

    override fun onDestroy() {
        controller.onDestroyed()
        super.onDestroy()
    }
}
