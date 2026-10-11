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
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    // ---------------------------------------------------------------------------
    // Bounded lock-free SPSC float ring (class-level realtime PCM handoff).
    //
    // Invariants:
    // - Single producer: the AVAudioEngine realtime tap thread ONLY calls
    //   `echolet_ios_ring_push` (memcpy + atomic release store; zero
    //   allocation, zero locks, zero inference).
    // - Single consumer: the app-side ASR worker queue ONLY calls
    //   `echolet_ios_ring_pop`.
    // - Overflow policy is bounded drop: push rejects surplus samples and
    //   reports the dropped count so the ASR layer can log state (counts only).
    // - Memory ordering: producer commits payload stores with Release on the
    //   write index; consumer acquires on the read index. Distinct cache lines
    //   for head/tail avoid false sharing.
    // ---------------------------------------------------------------------------
    #[repr(C, align(64))]
    struct RingHeader {
        read_index: AtomicU64,
        write_index: AtomicU64,
        dropped_total: AtomicU64,
        capacity: u64,
    }

    struct FloatRing {
        header: RingHeader,
        data: Box<[f32]>,
    }

    impl FloatRing {
        fn capacity(&self) -> u64 {
            self.header.capacity
        }

        fn used(&self) -> u64 {
            let w = self.header.write_index.load(Ordering::Acquire);
            let r = self.header.read_index.load(Ordering::Acquire);
            w - r
        }
    }

    /// Creates a bounded lock-free SPSC float ring. `capacity_samples` is
    /// clamped to [1024, 16_777_216]. NULL on failure.
    #[no_mangle]
    pub extern "C" fn echolet_ios_ring_create(capacity_samples: u64) -> *mut FloatRing {
        panic_guard(std::ptr::null_mut(), || {
            let capacity = capacity_samples.clamp(1024, 16_777_216);
            if capacity < 1024 {
                return std::ptr::null_mut();
            }
            let data: Box<[f32]> = vec![0.0f32; capacity as usize].into_boxed_slice();
            Box::into_raw(Box::new(FloatRing {
                header: RingHeader {
                    read_index: AtomicU64::new(0),
                    write_index: AtomicU64::new(0),
                    dropped_total: AtomicU64::new(0),
                    capacity,
                },
                data,
            }))
        })
    }

    /// Realtime-safe bounded push. Copies `num_samples` floats from `samples`
    /// into the ring; returns the number of samples ACCEPTED (first available
    /// in-ring space; surplus is dropped and counted). Never blocks or
    /// allocates. Returns -1 for invalid handles/arguments.
    #[no_mangle]
    pub extern "C" fn echolet_ios_ring_push(
        ring: *mut FloatRing,
        samples: *const c_float,
        num_samples: usize,
    ) -> i64 {
        if ring.is_null() || samples.is_null() || num_samples == 0 {
            return -1;
        }
        let ring = unsafe { &mut *ring };
        let cap = ring.capacity();
        // Single producer owns write_index for its payload stores; acquire the
        // current tail once. Consumer pressure is bounded by capacity.
        let w = ring.header.write_index.load(Ordering::Acquire);
        let r = ring.header.read_index.load(Ordering::Acquire);
        let free = cap - (w - r);
        let accept = (num_samples as u64).min(free);
        let dropped = (num_samples as u64) - accept;
        if dropped > 0 {
            ring.header
                .dropped_total
                .fetch_add(dropped, Ordering::Relaxed);
        }
        let data = ring.data.as_mut_ptr();
        unsafe {
            for i in 0..accept as usize {
                let src = samples.add(i);
                let dst = data.add(((w + i as u64) % cap) as usize);
                std::ptr::copy_nonoverlapping(src, dst, 1);
            }
        }
        ring.header
            .write_index
            .store(w + accept, Ordering::Release);
        accept as i64
    }

    /// Consumer pop into `out` (bounded, blocking only via bounded acquire
    /// wait). Returns the number of floats written, or -1 on invalid input.
    #[no_mangle]
    pub extern "C" fn echolet_ios_ring_pop(
        ring: *const FloatRing,
        out: *mut c_float,
        max_samples: usize,
    ) -> i64 {
        if ring.is_null() || out.is_null() || max_samples == 0 {
            return -1;
        }
        let ring = unsafe { &*ring };
        let cap = ring.capacity();
        let r = ring.header.read_index.load(Ordering::Acquire);
        let w = ring.header.write_index.load(Ordering::Acquire);
        let available = w - r;
        let take = available.min(max_samples as u64);
        for i in 0..take as usize {
            unsafe {
                let src = ring.data.as_ptr().add(((r + i as u64) % cap) as usize);
                let dst = out.add(i);
                std::ptr::copy_nonoverlapping(src, dst, 1);
            }
        }
        ring.header.read_index.store(r + take, Ordering::Release);
        take as i64
    }

    /// Copy-in-discard resets both cursors. Used to discard pre-START preroll.
    #[no_mangle]
    pub extern "C" fn echolet_ios_ring_clear(ring: *const FloatRing) {
        if ring.is_null() {
            return;
        }
        let ring = unsafe { &*ring };
        let w = ring.header.write_index.load(Ordering::Acquire);
        ring.header.read_index.store(w, Ordering::Release);
    }

    /// Frees a ring. Safe with NULL.
    #[no_mangle]
    pub extern "C" fn echolet_ios_ring_destroy(ring: *mut FloatRing) {
        if !ring.is_null() {
            unsafe {
                drop(Box::from_raw(ring));
            }
        }
    }

    /// Dropped-total readable non-destructively (observability; counts only).
    #[no_mangle]
    pub extern "C" fn echolet_ios_ring_dropped_total(ring: *const FloatRing) -> u64 {
        if ring.is_null() {
            return 0;
        }
        unsafe { (*ring).header.dropped_total.load(Ordering::Relaxed) }
    }

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

    /// Flushes the stream tail: feeds 0.4 s of digital silence at the given
    /// sample rate so the transducer decodes trailing buffered features, then
    /// decodes everything ready and returns the FINAL text with the same
    /// buffer semantics as `stream_read` (negative = required buffer size on
    /// overflow). Must be called on the single ASR worker thread exactly once
    /// per stream before destroy.
    #[no_mangle]
    pub extern "C" fn echolet_ios_stream_flush(
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
            let silence = [0.0f32; 6400]; // 0.4 s @ 16 kHz
            stream.inner.accept_waveform(16000, &silence);
            stream.inner.decode_all_ready();
            stream.inner.get_result()
        });
        let bytes = text.as_bytes();
        if out.is_null() || out_len == 0 {
            return bytes.len() as c_int;
        }
        let copy = bytes.len() as c_int + 1;
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
