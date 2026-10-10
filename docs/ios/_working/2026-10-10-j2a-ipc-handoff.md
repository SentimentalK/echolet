# Echolet iOS J2A-R IPC Protocol Admission & Lost-Request Contract Handoff Notes

> **Status**: TEMPORARY HANDOFF DOCUMENT (J2A-R Deliverable)
> **Date**: 2026-10-10
> **Scope**: Cross-process IPC admission repair, request correlation gate, cold-boot fence, and lost-request contract in Rust and Swift Foundation CLT. No Xcode/Simulator required.
> **Predecessor**: `docs/ios/_working/2026-10-10-j2a-ipc-handoff.md` (original J2A deliverable) & `docs/ios/_working/2026-10-10-mac-environment-audit.md` (J1B environment audit).

---

## 1. Safety Defect Analysis & Architectural Repairs

Following independent code review of the initial J2A deliverable, five architectural safety defects were identified and resolved in J2A-R:

### Defect 1: Loose Request Correlation & Stale Text Injection
- **Problem**: `KeyboardAdmissionGate::admit_response` previously checked only `session_id` and increasing `revision`. It never verified `acknowledged_request_id` or `acknowledged_sequence`. An App response acknowledging a phantom or wrong request ID could advance watermarks and inject text into the active session.
- **Repair**: `KeyboardAdmissionGate` now tracks outgoing commands via `register_issued_command`. When evaluating an incoming `AppResponse`, the gate enforces that `acknowledged_request_id` and `acknowledged_sequence` match a sent command in the active session. Uncorrelated responses, responses acknowledging unissued future sequence numbers, or responses with empty IDs are rejected with `RequestCorrelationMismatch` fail-closed without advancing the watermark.

### Defect 2: Loose Zero Sequence & Zero Revision Validation
- **Problem**: Wire validation permitted `acknowledged_sequence == 0` and `revision == 0` in `AppResponse`, violating the strict 1-based monotonic numbering invariants enforced on requests.
- **Repair**: Both Rust `AppResponse::validate()` and Swift `EcholetIPC.AppResponse.validate()` / initializers now strictly reject `acknowledged_sequence == 0` and `revision == 0`. Unnecessary Swift `bool_alias` was also removed.

### Defect 3: Missed STOP Leading to Deadlock / Conflicted Session
- **Problem**: In a single-slot wire where Darwin notifications are edge-trigger and lossy, an unobserved STOP followed by a new editor session's START would previously result in `ActiveSessionConflict`, locking the App gate permanently unless manually reset.
- **Repair**: `AppAdmissionGate::admit_request` provides an explicit, safe handoff: when a fresh START (`sequence == 1`) arrives for a new session, the prior stranded session is atomically retired (`AppAdmissionOutcome::ReplacedPriorSession`), and the new session is armed. Stale late STOP or CANCEL commands from the retired session are rejected and can **never** kill the new session.

### Defect 4: Cold-Boot Replay of Cached UserDefaults Requests
- **Problem**: App Group `UserDefaults` holds durable keys. If the Containing App process restarts or is killed, reading the cached `KEYBOARD_REQUEST_KEY` on boot could autonomously restart the microphone without fresh keyboard user intent.
- **Repair**: `AppAdmissionGate` introduces a cold-boot fencing contract (`AppAdmissionGate::cold_boot(boot_epoch)`). While cold-boot armed, any cached request in `UserDefaults` is rejected fail-closed (`ColdBootFenceReject`). The gate must be explicitly authorized (`authorize_boot()`) by the host when real foreground/IPC activation evidence is verified. Duplicate identical request replay is also rejected via `last_applied_request_id`.

### Defect 5: Correct STOP vs CANCEL Semantics (Echolet Keyboard Contract)
- **Problem**: The previous handoff note claimed `STOP` would "drain buffered audio then append final", which contradicted `src/session.rs` and the core Echolet keyboard lifecycle contract.
- **Repair**: Both `STOP` and `CANCEL` immediately fence the session upon hide, focus loss, or user action. Only already visibly inserted text snapshot is preserved. No hidden-audio completion or late asynchronous draft text injection is authorized. Once the keyboard closes locally (`close_session()`), every subsequent response is rejected as `ResurrectionAttemptAfterClose`, regardless of whether `is_final` is true or false.

---

## 2. Single-Slot Wire & Delivery Contract

### Wire Reality
- Single latest-request slot (`echolet.keyboard.request.v1`) and single latest-response slot (`echolet.app.response.v1`) cannot guarantee guaranteed delivery of every transient command in an RPC-style queue.
- A rapid sequence where `STOP` overrides `START` before the App wakes results in a safe tombstone (the App reads the latest STOP and stays idle).
- A new `START` after a missed `STOP` safely retires the prior session.
- Darwin Notifications are edge-trigger wake hints and can coalesce or be dropped while processes are suspended.

### Transport Verification Status
- **Label**: **NOT YET VERIFIED ON HARDWARE/SIMULATOR** (pure Foundation/Rust logic verified; OS-level `UserDefaults(suiteName:)` and Darwin notification wake behavior require J2B on real iOS runtime).

---

## 3. Verification Commands & Test Results

All verification ran locally on macOS using Rust toolchain (`cargo`) and Swift 5.10 CLT (`swiftc`), without Xcode.app.

### A. Rust Unit & Adversarial Tests
```bash
cargo test --lib ios_ipc
```
Output:
```text
running 12 tests
test ios_ipc::tests::test_adversarial_defect_4_old_cached_start_after_process_restart ... ok
test ios_ipc::tests::test_adversarial_defect_2_zero_sequence_and_zero_revision_rejection ... ok
test ios_ipc::tests::test_adversarial_defect_3_lost_stop_then_new_start_and_stale_stop ... ok
test ios_ipc::tests::test_adversarial_defect_1_mismatched_ack_with_same_session_and_higher_revision ... ok
test ios_ipc::tests::test_adversarial_defect_5_local_close_ignoring_late_valid_higher_revision ... ok
test ios_ipc::tests::test_app_admission_gate_lifecycle_and_stale_rejection ... ok
test ios_ipc::tests::test_keyboard_admission_gate_lifecycle_and_stale_rejection ... ok
test ios_ipc::tests::test_request_validation_failures ... ok
test ios_ipc::tests::test_response_validation_failures ... ok
test ios_ipc::tests::test_valid_request_serialization_round_trip ... ok
test ios_ipc::tests::test_valid_response_serialization_round_trip ... ok
test ios_ipc::test_golden_fixtures_decode_and_validate ... ok

test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 56 filtered out; finished in 0.00s
```

Full library test suite:
```bash
cargo test --lib
# Result: ok. 68 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

### B. Swift Foundation CLI Smoke & Invalidation Tests
```bash
swiftc ios/protocol/EcholetIPC.swift ios/protocol/IPCCodecSmoke.swift -o /tmp/echolet-ipc-smoke-j2a
/tmp/echolet-ipc-smoke-j2a ios/protocol/fixtures
```
Output:
```text
[IPCCodecSmoke] Starting Swift Foundation IPC Codec Smoke Suite...
  ✓ golden_request_start.json decoded and round-tripped successfully
  ✓ golden_request_stop.json decoded and round-tripped successfully
  ✓ golden_response_partial.json decoded and round-tripped successfully
  ✓ golden_response_final.json decoded and round-tripped successfully
  ✓ Invalidation & malformed payload rejection checks passed
[IPCCodecSmoke] ALL TESTS PASSED.
```

### C. Formatting Check
- `rustfmt --check src/ios_ipc.rs`: Passed cleanly.
- `cargo fmt --check`: Fails only on pre-existing unrelated files (`src/platform/windows/*`, `src/ui/*`, `tests/*`), which are preserved untouched per scope rules.

---

## 4. J2B Adapter Implementation Requirements

When Xcode 16.2 is installed and the native targets are built, the J2B adapter layer must fulfill the following contracts:

1. **Re-read on Wake/Appearance**:
   - Because Darwin notifications are edge-trigger and lossy, the keyboard must re-read `APP_RESPONSE_KEY` whenever the keyboard view appears or receives input focus.
   - The Containing App must re-read `KEYBOARD_REQUEST_KEY` upon cold start, warm foregrounding (`sceneDidBecomeActive`), and Darwin wake notification.
2. **Foreground Evidence Requirement**:
   - The Containing App must NOT begin audio recording from a cached request during background wake.
   - The App must supply real foreground/user authorization to `AppAdmissionGate::authorize_boot()` before admitting START commands.
3. **Correlation Registration**:
   - The Keyboard Extension must call `KeyboardAdmissionGate::register_issued_command` before or immediately upon writing any command to `UserDefaults`.
4. **Immediate Local Fencing**:
   - When the user dismisses the keyboard or changes input fields, call `KeyboardAdmissionGate::close_session()` immediately to ensure no subsequent text injection can occur.
