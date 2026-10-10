package com.mainstayx.echolet

enum class KeyboardSelectedState {
    SELECTED,
    NOT_SELECTED,
    UNKNOWN,
}

data class SetupOsStatus(
    val micGranted: Boolean,
    val imeEnabled: Boolean,
    val imeSelected: KeyboardSelectedState = KeyboardSelectedState.NOT_SELECTED,
) {
    val allPrerequisitesReady: Boolean
        get() = micGranted && imeEnabled
}

/**
 * Pure testable seam for resolving and explaining Android OS prerequisites
 * for the Echolet voice keyboard setup screen.
 */
object SetupStatusResolver {

    /**
     * Resolves whether Echolet is currently selected as active IME.
     * Never produces a false-positive ready status if the OS setting cannot be read.
     */
    fun resolveSelectedState(
        currentImeSetting: String?,
        targetIdShort: String,
        targetIdFull: String,
    ): KeyboardSelectedState {
        if (currentImeSetting.isNullOrBlank()) {
            return KeyboardSelectedState.UNKNOWN
        }
        val trimmed = currentImeSetting.trim()
        val base = trimmed.substringBefore(';')
        return if (base == targetIdShort || base == targetIdFull) {
            KeyboardSelectedState.SELECTED
        } else {
            KeyboardSelectedState.NOT_SELECTED
        }
    }

    fun isImeEnabled(
        enabledImeIds: List<String>,
        targetIdShort: String,
        targetIdFull: String,
    ): Boolean {
        return enabledImeIds.any { id ->
            val trimmed = id.trim().substringBefore(';')
            trimmed == targetIdShort || trimmed == targetIdFull
        }
    }

    fun deriveGuidance(status: SetupOsStatus): String = when {
        status.allPrerequisitesReady ->
            "All setup complete. Switch to Echolet voice keyboard when typing in any app."
        !status.micGranted && !status.imeEnabled ->
            "Grant microphone permission and enable Echolet in Android keyboard settings to get started."
        !status.micGranted ->
            "Grant microphone permission so Echolet can record audio while you dictate."
        !status.imeEnabled ->
            "Enable Echolet in Android keyboard settings below."
        else ->
            "Switch to Echolet voice keyboard when typing in any app."
    }

    fun micStatusSummary(micGranted: Boolean): String =
        if (micGranted) "Granted" else "Not granted"

    fun imeEnabledSummary(imeEnabled: Boolean): String =
        if (imeEnabled) "Enabled in Settings" else "Not enabled"

    fun imeSelectedSummary(selectedState: KeyboardSelectedState): String = when (selectedState) {
        KeyboardSelectedState.SELECTED -> "Selected as active"
        KeyboardSelectedState.NOT_SELECTED -> "Not selected"
        KeyboardSelectedState.UNKNOWN -> "Check in keyboard settings"
    }
}
