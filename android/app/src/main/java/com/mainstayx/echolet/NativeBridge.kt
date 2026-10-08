package com.mainstayx.echolet

/**
 * JNI bindings for the shared Rust ASR core (`libecholet_android.so`).
 *
 * The three externals are the entire Phase 0-A native surface. They must only
 * be called from the app's single background executor, never the main/UI
 * thread. All transcript/session/model semantics live in the Rust core;
 * Kotlin never re-implements them.
 *
 * Wire contract of `nativeFeed` (stable for the Phase 0-B IME slice):
 * UTF-8 JSON ARRAY. Empty response is `[]`. Each event is one of
 *   {"kind":"partial","session":<u64>,"revision":<u64>,
 *    "backspaces":<usize>,"suffix":"<string>","text":"<full recognized text>"}
 *   {"kind":"endpoint","session":<u64>,"text":"<last admitted completed text>"}
 * (endpoint omitted when this feed produced no completed text).
 *
 * Diff application reconstructs visible text in CHARACTER units — apply
 * `backspaces` pops, then append `suffix` — never by UTF-8 byte length.
 * This diagnostic Activity is not an InputConnection; editor binding must be
 * re-verified in Phase 0-B before applying deltas to the OS editor.
 */
object NativeBridge {
    init {
        // Load order matters: dependencies first. The linker also resolves
        // DT_NEEDED entries against the app's nativeLibraryDir, but explicit
        // ordering gives an actionable error message when a required .so is
        // missing from the APK.
        System.loadLibrary("onnxruntime")
        System.loadLibrary("sherpa-onnx-c-api")
        System.loadLibrary("echolet_android")
    }

    /**
     * Opens one ASR session over the staged model directory (must contain
     * model.json + encoder/decoder/joiner/tokens). Returns a fresh nonzero
     * handle. Throws IllegalStateException when a session is already active
     * or the model cannot be opened; IllegalArgumentException for a missing
     * model directory. The zero handle is the failure fallback.
     */
    external fun nativeOpen(modelDir: String): Long

    /**
     * Feeds one chunk of mono 16 kHz PCM. Returns the JSON event array
     * described above ([] = no progress). Throws IllegalArgumentException for
     * malformed input (rate != 16000, NaN samples, oversized chunk) and
     * IllegalStateException for stale/unknown handles or native failures.
     */
    external fun nativeFeed(handle: Long, samples: FloatArray, sampleRate: Int): String

    /**
     * Cancels the session (drops buffered audio, rejects stale generations)
     * and frees the stream. Duplicate/stale handles are harmless no-ops. No
     * final text is appended and history stays off in this slice.
     */
    external fun nativeClose(handle: Long)
}
