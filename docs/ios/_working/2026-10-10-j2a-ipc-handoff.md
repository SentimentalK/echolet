# Echolet iOS J2A IPC Protocol & Stale-Session Safety Handoff Notes

> **Status**: TEMPORARY HANDOFF DOCUMENT (J2A Deliverable)
> **Date**: 2026-10-10
> **Scope**: Pure data contract & stale-session safety logic in Rust and Swift Foundation CLT. No Xcode/Simulator required.
> **Predecessor**: `docs/ios/_working/2026-10-10-mac-environment-audit.md` (J1B environment audit).

---

## 1. Schema & Ownership Design

The iOS Keyboard Extension runs in a separate process sandbox from the Containing Application. They communicate via shared App Groups storage and Darwin Notification wake hints.

### Keys & Notifications
- **`echolet.keyboard.request.v1`**: Dedicated single-writer key in AppGroup `UserDefaults`, written **only** by the Keyboard Extension, read by the App.
- **`echolet.app.response.v1`**: Dedicated single-writer key in AppGroup `UserDefaults`, written **only** by the Containing App, read by the Keyboard Extension.
- **`com.echolet.ipc.request.v1`**: Darwin notification posted by Keyboard Extension when a new request is written.
- **`com.echolet.ipc.response.v1`**: Darwin notification posted by Containing App when a new response snapshot is written.

### Wire Envelopes (Version 1)
- **`KeyboardRequest`**:
  ```json
  {
    "protocol_version": 1,
    "session_id": "string",
    "sequence": 1,
    "request_id": "string",
    "command": "start" | "stop" | "cancel",
    "client_timestamp_ms": 1728570000000
  }
  ```
- **`AppResponse`**:
  ```json
  {
    "protocol_version": 1,
    "session_id": "string",
    "acknowledged_request_id": "string",
    "acknowledged_sequence": 1,
    "revision": 3,
    "state": "idle" | "requested" | "preparing" | "listening" | "processing" | "completed" | "blocked",
    "recognized_text": "string",
    "is_final": false,
    "error_code": null,
    "server_timestamp_ms": 1728570001000
  }
  ```

### Ownership & Stale-Session Admission
1. **Keyboard Extension owns `session_id` & `sequence`**:
   - Each editor session lifecycle mints a unique `session_id`.
   - When the editor focus changes or the keyboard dismisses, the Keyboard invalidates its active session locally **first**.
   - `KeyboardAdmissionGate` strictly enforces that response snapshots match the active `session_id` and have strictly increasing monotonic revisions. Any response received for an older session, duplicates, or responses received after the session is closed/finalized are rejected fail-closed (preventing text resurrection).
2. **App owns `revision`, `state`, and `recognized_text`**:
   - `AppAdmissionGate` strictly enforces valid `START -> STOP / CANCEL` sequence order.
   - Crucially, late `STOP` or `CANCEL` commands belonging to an older session are rejected and **never** terminate or interfere with a newer active session.
3. **Stop vs Cancel Semantics**:
   - `Stop`: Graceful stop; speech recognition completes buffered audio, and recognized text snapshot is preserved.
   - `Cancel`: Immediate abort; pending audio is discarded; already committed text is retained in the editor, and the session is immediately fenced.

---

## 2. Verification Commands and Test Output

All tests run locally using the existing Rust toolchain and Swift 5.10 Command Line Tools without requiring Xcode.app or iOS SDKs.

### A. Rust Unit & Golden Tests
```bash
cargo test --lib ios_ipc
```
Output:
```text
running 7 tests
test ios_ipc::tests::test_keyboard_admission_gate_lifecycle_and_stale_rejection ... ok
test ios_ipc::tests::test_app_admission_gate_lifecycle_and_stale_rejection ... ok
test ios_ipc::tests::test_request_validation_failures ... ok
test ios_ipc::tests::test_response_validation_failures ... ok
test ios_ipc::test_golden_fixtures_decode_and_validate ... ok
test ios_ipc::tests::test_valid_request_serialization_round_trip ... ok
test ios_ipc::tests::test_valid_response_serialization_round_trip ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 56 filtered out; finished in 0.00s
```

Full library test suite:
```bash
cargo test --lib
# Result: ok. 63 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

### B. Swift Foundation CLI Smoke Test
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
- `cargo fmt --all -- --check`: Pre-existing formatting diffs exist in unrelated desktop/windows crates (preserved untouched).

---

## 3. Known Limitations & Blocked Areas

- **Full Xcode & iOS SDK Absence**:
  - Confirmed in J1B audit: Xcode 16.2 is not yet installed; only Swift 5.10 CLT is available.
  - Actual iOS APIs (`UserDefaults(suiteName: "group.com.echolet")`, `notify_post`, `notify_register_dispatch`, `UIInputViewController`, `textDocumentProxy`) cannot be built or tested until Xcode is installed.
  - All J2A code is pure platform-neutral Foundation / Rust std logic.
- **Audio Capture in Keyboard**:
  - iOS Keyboard Extension process sandbox cannot record microphone audio; audio engine must run in Containing App.
- **Delivery Guarantees**:
  - Darwin Notifications are edge-trigger wake hints and may coalesce or drop during process suspension. The reader must always re-read the latest versioned snapshot upon wake and UI appearance.

---

## 4. Next J2B Gates & Human Handoff Steps

1. **Human Action Required**:
   - Sign in to Apple Developer Downloads and install **Xcode 16.2** on the Mac (`/Applications/Xcode.app`).
   - Run `sudo xcode-select -s /Applications/Xcode.app/Contents/Developer` and `sudo xcodebuild -license accept`.
   - Connect iPad or prepare iPhone/iPad Simulator. No paid developer membership is required to run Simulator tests.
2. **J2B Scope**:
   - Create Xcode project / SPM targets for Containing App and Keyboard Extension.
   - Configure App Groups entitlement (`group.com.echolet`).
   - Implement `UserDefaults` + `notify_post` IPC adapter using `EcholetIPC` contract.
   - Implement native UIInputViewController integrating `textDocumentProxy`.
