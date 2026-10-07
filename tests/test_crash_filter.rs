//! Verifies the Windows native exception filter catches a real native crash
//! (null-pointer dereference) and writes a FATAL line to the log before the
//! process dies.
//!
//! Run with:  cargo test --test test_crash_filter -- --ignored --nocapture
//! The test process is *expected* to crash; success = a FATAL line appears in
//! ~/.echolet/echolet.log. Because it crashes, cargo reports it as failed —
//! that's expected; inspect the log instead.

#![cfg(target_os = "windows")]
#![allow(clippy::infinite_loop)]

#[test]
#[ignore]
fn trigger_native_crash() {
    echolet::log::init();
    echolet::log::log("INFO", "about to trigger a deliberate native crash (null deref)");
    // Force a real access violation so the unhandled-exception filter runs.
    unsafe {
        let p: *mut u8 = std::ptr::null_mut();
        std::ptr::write_volatile(p, 1);
    }
}
