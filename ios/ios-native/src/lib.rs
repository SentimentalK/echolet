//! Thin iPhoneOS FFI bridge over the shared Echolet Rust core.
//!
//! Compiled ONLY for `aarch64-apple-ios` (iPhoneOS device): the crate contents
//! are inside `#[cfg(target_os = "ios")]`, so no other platform ever links
//! this module. It re-exports the platform-neutral core (`echolet::asr`,
//! `echolet::ffi`, `echolet::session`) behind a minimal C ABI for the Swift
//! app-side DEBUG probe. It must NEVER be linked into the Keyboard extension.
//!
//! Ownership rules:
//! - every `_create` returns a boxed handle that must be freed by the matching
//!   `_destroy`;
//! - results are returned as malloc'd C strings or written into bounded
//!   caller-provided buffers;
//! - no panics cross the C ABI (everything is `catch_unwind` guarded);
//! - no raw PCM is persisted and no network/cloud calls exist downstream.

use std::os::raw::{c_char, c_float, c_int};

#[cfg(target_os = "ios")]
mod probe {
    use super::{c_char, c_float, c_int};
    use echolet::asr::{OnlineRecognizer, OnlineStream};
    use std::ffi::CStr;
    use std::panic::catch_unwind;
    use std::sync::Arc;

    /// ABI marker of the Echolet iOS bridge crate.
    pub const ECHOLET_IOS_PROBE_VERSION: &[u8] = b"echolet-ios-probe 0.1.0\x00";

    fn panic_guard<T>(
        default: T,
        f: impl FnOnce() -> T,
    ) -> T {
        catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or(default)
    }

    /// Opaque shared Echolet recognizer handle (real root-core wrapper over
    /// the pinned Sherpa-onnx C API).
    #[repr(C)]
    pub struct EcholetIosRecognizer {
        inner: Arc<OnlineRecognizer>,
    }

    #[repr(C)]
    pub struct EcholetIosStream {
        inner: OnlineStream,
    }

    #[no_mangle]
    pub extern "C" fn echolet_ios_probe_version() -> *const c_char {
        ECHOLET_IOS_PROBE_VERSION.as_ptr().cast()
    }

    /// Creates a real offline recognizer rooted in the shared core against a
    /// model directory containing encoder/decoder/joiner .onnx files,
    /// tokens.txt and (optional) model.json manifest.
    ///
    /// Returns a handle the caller must destroy with
    /// `echolet_ios_recognizer_destroy`; NULL on failure.
    #[no_mangle]
    pub extern "C" fn echolet_ios_recognizer_create(model_dir: *const c_char) -> *mut EcholetIosRecognizer {
        if model_dir.is_null() {
            return std::ptr::null_mut();
        }
        let path = unsafe { CStr::from_ptr(model_dir) };
        let path = match path.to_str() {
            Ok(p) => p,
            Err(_) => return std::ptr::null_mut(),
        };
        let result = panic_guard(
            Err("panic during recognizer_create".to_string()),
            || OnlineRecognizer::new(path),
        );
        match result {
            Ok(rec) => Box::into_raw(Box::new(EcholetIosRecognizer {
                inner: Arc::new(rec),
            })),
            Err(_) => std::ptr::null_mut(),
        }
    }

    /// Opens a streaming session handle; NULL on failure. The handle must be
    /// destroyed with `echolet_ios_stream_destroy` before the recognizer.
    #[no_mangle]
    pub extern "C" fn echolet_ios_stream_create(rec: *const EcholetIosRecognizer) -> *mut EcholetIosStream {
        if rec.is_null() {
            return std::ptr::null_mut();
        }
        let rec = unsafe { &*rec };
        let result = panic_guard(Err("panic during stream_create".to_string()), || {
            rec.inner.create_stream()
        });
        match result {
            Ok(stream) => Box::into_raw(Box::new(EcholetIosStream { inner: stream })),
            Err(_) => std::ptr::null_mut(),
        }
    }

    /// Feeds a bounded chunk of mono 16 kHz / manifest-rate PCM floats. The
    /// buffer is copied synchronously and never retained; `num_samples` must
    /// be non-negative and <= `i32::MAX`.
    #[no_mangle]
    pub extern "C" fn echolet_ios_stream_feed(
        stream: *const EcholetIosStream,
        sample_rate: c_int,
        samples: *const c_float,
        num_samples: c_int,
    ) -> c_int {
        if stream.is_null() || samples.is_null() {
            return -1;
        }
        if num_samples < 0 {
            return -1;
        }
        let ptr = unsafe { std::slice::from_raw_parts(samples, num_samples as usize) };
        let stream = unsafe { &*stream };
        panic_guard(
            -1,
            || {
                stream.inner.accept_waveform(sample_rate, ptr);
                0
            },
        )
    }

    /// Reads the current partial/final text into a caller-owned buffer and
    /// returns its string length in bytes. Passing a null/zero buffer queries
    /// the required size. Returns -1 on error.
    #[no_mangle]
    pub extern "C" fn echolet_ios_stream_read(
        stream: *const EcholetIosStream,
        out: *mut c_char,
        out_len: c_int,
    ) -> c_int {
        if stream.is_null() {
            return -1;
        }
        if out_len > 0 && out.is_null() {
            return -1;
        }
        let stream = unsafe { &*stream };
        let text = panic_guard(String::new(), || {
            stream.inner.decode_all_ready();
            stream.inner.get_result()
        });
        let bytes = text.as_bytes();
        if out.is_null() || out_len == 0 {
            return bytes.len() as c_int;
        }
        let copy = bytes.len() as c_int + 1; // include NUL terminator
        if copy > out_len {
            return -(copy);
        }
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), out as *mut u8, bytes.len());
            *out.add(bytes.len()) = 0;
        };
        bytes.len() as c_int
    }

    /// Destroys a recognizer handle created by `echolet_ios_recognizer_create`.
    /// Safe to call with NULL.
    #[no_mangle]
    pub extern "C" fn echolet_ios_recognizer_destroy(rec: *mut EcholetIosRecognizer) {
        if !rec.is_null() {
            unsafe {
                drop(Box::from_raw(rec));
            }
        }
    }

    /// Destroys a stream handle. Safe to call with NULL.
    #[no_mangle]
    pub extern "C" fn echolet_ios_stream_destroy(stream: *mut EcholetIosStream) {
        if !stream.is_null() {
            unsafe {
                drop(Box::from_raw(stream));
            }
        }
    }
}
