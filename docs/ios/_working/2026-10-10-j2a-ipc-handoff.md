# Echolet iOS J2A-R2 IPC Protocol Admission & Replay/Takeover Safety Handoff Notes

> **Status**: TEMPORARY HANDOFF DOCUMENT (J2A-R2 Deliverable)
> **Date**: 2026-10-10
> **Scope**: Cross-process IPC admission upgrade to wire v2, per-process host epoch validation, globally monotonic intent sequencing, lost-request/tombstone contract, and adversarial safety tests in Rust and Swift Foundation CLT.
> **Verification Environment**: Apple Swift version 5.10 (CLT), rustc/cargo 1.99.0 on macOS Darwin. Xcode.app, iOS Simulator, and app signing NOT required.
> **Predecessor**: `docs/ios/_working/2026-10-10-j2a-ipc-handoff.md` (J2A-R deliverable).

---

## 1. Safety Defect Analysis & Architectural Repairs (v2 Upgrade)

Following audit of the J2A-R deliverable at baseline `633cfb1`, several fundamental multi-process safety defects were identified and resolved in J2A-R2:

### Defect 1: Cold-Boot Authorization Replay
- **Problem**: `AppAdmissionGate::cold_boot` rejected requests while armed, but `authorize_boot()` simply cleared the boolean flag. The exact same cached START from durable App Group `UserDefaults` that was rejected before authorization would be admitted after authorization upon foregrounding.
- **Repair**: Introduced wire protocol **v2** and mandatory host process epoch fencing. The containing App host mints a cryptographically unpredictable UUID (opaque token) on **each** launch and publishes it to `echolet.app.epoch.v2`. Both requests and responses require non-blank `app_epoch`. `AppAdmissionGate` validates `app_epoch` on every incoming command. An old cached request from a previous process launch is permanently rejected with `StaleAppEpoch`, even after subsequent foreground authorization.

### Defect 2: Unfenced Session Preemption by Delayed START
- **Problem**: Any START with a different `session_id` and per-session `sequence == 1` was previously admitted as `ReplacedPriorSession`, without establishing that it was newer than the currently active editor session. A delayed or replayed old START could preempt a live newer session.
- **Repair**: Request envelopes gain mandatory `intent_sequence` (`u64 >= 1`), which monotonically increases **across** editor session boundaries. The Keyboard Extension reserves this sequence across extension restarts in App Group storage. `AppAdmissionGate` maintains the watermark `last_intent_sequence`. An incoming START with `intent_sequence <= last_intent_sequence` is rejected fail-closed without mutating active session state.

### Defect 3: Lossy Single-Slot Race & Tombstone Semantics
- **Problem**: Single-slot App Group `UserDefaults` (`echolet.keyboard.request.v2`) and edge-triggered Darwin notifications are inherently lossy. If a STOP overwrote an unseen START before the App woke, the gate had no defined semantics for a standalone STOP without an active session.
- **Repair**: Admitted as `AppAdmissionOutcome::TombstoneIgnored`. The App gate advances the `last_intent_sequence` watermark and records the request ID without starting audio, entering an error state, or holding the keyboard busy.

### Defect 4: Unbounded Memory & Preserved Command Correlation
- **Problem**: `KeyboardAdmissionGate::issued_commands` grew without bound over a long session, while multi-revision streaming partials required correlation with the initial START.
- **Repair**: Memory is bounded to `MAX_ISSUED_COMMANDS_HISTORY` (32). When capacity is exceeded, history is compacted by draining intermediate entries while preserving the root command (seq 1) for ongoing partial transcript correlation.

---

## 2. Wire Protocol Constants & Keys (v2)

Rust (`src/ios_ipc.rs`) and Swift (`ios/protocol/EcholetIPC.swift`) are synchronized:

| Property | Value | Description |
|---|---|---|
| `PROTOCOL_VERSION` | `2` | Wire protocol version (v1 payloads rejected fail-closed) |
| `APP_EPOCH_KEY` | `echolet.app.epoch.v2` | App Group key published by App host on each process launch |
| `KEYBOARD_REQUEST_KEY` | `echolet.keyboard.request.v2` | App Group key written only by Keyboard Extension |
| `APP_RESPONSE_KEY` | `echolet.app.response.v2` | App Group key written only by Containing App |
| `DARWIN_NOTIFICATION_REQUEST` | `com.echolet.ipc.request.v2` | Darwin wake hint posted on request write |
| `DARWIN_NOTIFICATION_RESPONSE` | `com.echolet.ipc.response.v2` | Darwin wake hint posted on response write |

---

## 3. Verification Commands & Real Test Output

All tests passed locally on macOS Darwin using Apple Swift 5.10 CLT and Cargo:

### A. Rust Unit & Adversarial Tests
```bash
cargo test --lib ios_ipc
```
Output:
```text
running 20 tests
test ios_ipc::tests::test_adversarial_defect_2_zero_sequence_and_zero_revision_rejection ... ok
test ios_ipc::tests::test_adverse_1_cold_boot_old_cached_start_and_epoch_mismatch_after_authorization ... ok
test ios_ipc::tests::test_adverse_2_delayed_old_start_lower_intent_rejected_without_mutating_session ... ok
test ios_ipc::tests::test_adversarial_defect_3_lost_stop_then_new_start_and_stale_stop ... ok
test ios_ipc::tests::test_adverse_3_old_stop_after_replacement_rejected_no_side_effects ... ok
test ios_ipc::tests::test_adversarial_defect_1_mismatched_ack_with_same_session_and_higher_revision ... ok
test ios_ipc::tests::test_adverse_10_valid_stop_cancel_immediately_fences_late_text ... ok
test ios_ipc::tests::test_adverse_7_malformed_blank_version_zero_values_overflow ... ok
test ios_ipc::tests::test_adverse_6_ack_matching_and_old_app_epoch_response_rejected ... ok
test ios_ipc::tests::test_adverse_4_stop_tombstone_no_auto_mic ... ok
test ios_ipc::tests::test_adverse_5_duplicate_replayed_request_same_or_lower_intent_rejected ... ok
test ios_ipc::tests::test_adverse_8_newly_minted_app_epoch_rebind_resumes ... ok
test ios_ipc::test_golden_fixtures_decode_and_validate ... ok
test ios_ipc::tests::test_app_admission_gate_lifecycle_and_stale_rejection ... ok
test ios_ipc::tests::test_keyboard_admission_gate_lifecycle_and_stale_rejection ... ok
test ios_ipc::tests::test_request_validation_failures ... ok
test ios_ipc::tests::test_adverse_9_multiple_partial_response_revisions_and_bounded_memory ... ok
test ios_ipc::tests::test_response_validation_failures ... ok
test ios_ipc::tests::test_valid_request_serialization_round_trip ... ok
test ios_ipc::tests::test_valid_response_serialization_round_trip ... ok

test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 56 filtered out; finished in 0.00s
```

Full crate library tests:
```bash
cargo test --lib
# Result: ok. 76 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

### B. Swift Foundation CLI Smoke & Invalidation Suite
```bash
mkdir -p /tmp/echolet_swift_smoke_v2
swiftc ios/protocol/EcholetIPC.swift ios/protocol/IPCCodecSmoke.swift -o /tmp/echolet_swift_smoke_v2/smoke
/tmp/echolet_swift_smoke_v2/smoke ios/protocol/fixtures
```
Output:
```text
[IPCCodecSmoke] Starting Swift Foundation IPC Codec Smoke Suite (v2)...
  ✓ golden_request_start.json decoded and round-tripped successfully
  ✓ golden_request_stop.json decoded and round-tripped successfully
  ✓ golden_response_partial.json decoded and round-tripped successfully
  ✓ golden_response_final.json decoded and round-tripped successfully
  ✓ Invalidation & malformed payload rejection checks passed
[IPCCodecSmoke] ALL TESTS PASSED.
```

### C. Formatting and Git Hygiene
- `rustfmt --check src/ios_ipc.rs`: Passed cleanly (exit code 0).
- `git diff --check`: Passed cleanly (exit code 0).

---

## 4. J2B Adapter Implementation Contracts & Host Duties

When Xcode is available and native iOS targets are constructed, the J2B host adapter must implement the following contracts:

1. **Host Process Epoch Minting**:
   - The Containing App entry point (`AppDelegate` / `@main`) must mint a unique UUID string on every process launch and persist it to `echolet.app.epoch.v2` in the App Group before initializing `AppAdmissionGate::cold_boot(epoch)`.
   - The epoch is a process-liveness and replay fence, not an authorization secret against hostile processes.
2. **Atomic Monotonic Intent Sequence Reservation**:
   - The Keyboard Extension must maintain a single-writer monotonic counter in App Group storage or memory mapped file. Before writing any request, reserve `intent_sequence` atomically. On integer overflow, fail closed.
   - If multiple extension instances race, serialization or atomic CAS must be used; collisions fail closed.
3. **Session Replacement Transition**:
   - When `AppAdmissionGate::admit_request` returns `AppAdmissionOutcome::ReplacedPriorSession { retired_session_id }`, the Containing App adapter **MUST** cancel the prior recognizer and stop audio capture before initializing recognition for the new session.
   - If audio cannot be stopped cleanly, fail closed rather than running concurrent capture pipelines.
4. **Transport Status & Unverified iOS Runtime**:
   - Pure Foundation and Rust logic are fully verified. OS-level `UserDefaults(suiteName:)` cross-sandbox persistence and Darwin notification delivery on real iOS runtime remain **unverified until Xcode setup and native compilation in J2B**.
