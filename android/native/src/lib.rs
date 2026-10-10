//! `libecholet_android.so` — the arm64 JNI bridge for Echolet Phase 0-A.
//!
//! Exposes exactly the three `com.mainstayx.echolet.NativeBridge` externals:
//!
//! * `nativeOpen(modelDir: String): Long` — opens one ASR session
//! * `nativeFeed(handle: Long, samples: FloatArray, sampleRate: Int): String`
//!   — feeds one mono 16 kHz chunk; returns the UTF-8 JSON event ARRAY
//! * `nativeClose(handle: Long): Unit` — cancels and frees the session
//!
//! All methods run on Kotlin's single background executor and lock one
//! process-global runtime (see `runtime.rs`). No ASR algorithm, session
//! generation, partial correction or model parsing lives here: everything
//! reuses the shared `echolet` core (`asr`, `session`, `capture`, `diff`).
//!
//!JNI wire contract (stable for the Phase 0-B IME slice; each event is one
//! object inside the returned JSON array, `[]` means no progress):
//!
//! * `{"kind":"partial","session":u64,"revision":u64,"backspaces":usize,
//!    "suffix":string,"text":string}`
//! * `{"kind":"endpoint","session":u64,"text":string}` — omitted when there
//!   is no completed text
//!
//! The consumer reconstructs visible text in CHARACTER units (backspaces,
//! then the suffix), never by UTF-8 byte length. The diagnostic UI is not an
//! InputConnection; the next slice must re-verify editor binding before
//! applying a delta. No model audio is ever logged.

#[cfg(target_os = "android")]
mod jni;

pub mod model_owner;
mod runtime;
#[cfg(test)]
mod tests;

pub use model_owner::{
    shared_model_owner, AndroidModelOwner, ModelSnapshot, LEGACY_MODEL_DIR_NAME,
};
pub use runtime::{
    shared_runtime, AndroidRuntime, BridgeError, MAX_CHUNK_SAMPLES, REQUIRED_SAMPLE_RATE,
};
