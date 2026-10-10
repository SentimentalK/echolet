# J3A Real iPad Microphone Capture, Warm-Path IPC & Lifecycle Probe Handoff

> **Date**: 2026-10-10
> **Target Device**: iPad (3) (`iPad14,1`, iPad mini 6th generation, iPadOS 18.7.7, build 22H340, arm64e)
> **Host Toolchain**: Xcode 16.2 (`16C5032a`) at `/Applications/Xcode.app/Contents/Developer` on macOS Sonoma 14.6.1 Intel
> **Provisioning Team**: `668LTD8W55` / `Apple Development: kevxu.cad@gmail.com (RJ3C3F7JFM)`
> **Git Baseline**: `3768ebc8783baf5e9be6f77a9f26e688cd3c5630` (Clean, fast-forwardable)

---

## 1. Acceptance Status Matrix

| Component / Subsystem | Status | Evidence & Measured Verification |
|---|---|---|
| **A. NATIVE BUILD** | **PASS** | Xcode 16.2 native `EcholetApp` + embedded `EcholetKeyboard.appex` compiled for `iphoneos` (arm64) and signed with `668LTD8W55` (`** BUILD SUCCEEDED **`). |
| **B. SWIFT IPC ADMISSION TESTS** | **PASS** | Pure Swift Foundation admission suite (`AdmissionTests.swift`, `AdversarialTests.swift`, `CaptureLifecycleTests.swift`) verified: cold boot fence, monotonic intent, stale STOP rejection, clean replacement, tombstone handling, delayed start rejection, overflow handling, and asynchronous permission and start/stop completion fences. `IPCCodecSmoke.swift` golden fixtures passed. |
| **C. RUST IPC SUITE** | **PASS** | `cargo test --lib ios_ipc` passed all 20 tests; full `cargo test --lib` passed. |
| **D. APP DEPLOYED TO IPAD** | **PASS (HISTORICAL)** | Installed via `xcrun devicectl device install app` to `iPad (3)` (`com.mainstayx.echolet.app`, databaseSequenceNumber 6484) and launched via `devicectl device process launch`. Post-repair physical mic/background marked unverified pending batch E2E. |
| **E. REAL AUDIO COMPONENT** | **PASS (REPAIRED J3A-R2a)** | `AudioCaptureController.swift` threading and lifecycle repaired: single serial `stateQueue` executor for all mutations, pure Foundation `CaptureLifecycleGate` production state machine. Dedicated `AudioCaptureTapContext` / `MeterContext` strongly retained by installed native tap closure (`[weak self, meterContext]`) ensuring context persists throughout tap lifetime without leaking controller, eliminating silent deallocation bug. Added behavioral closure retention and rate-limiting test (Scenario 8) in `CaptureLifecycleTests.swift`. |
| **E2. WARM IPC CANCELLATION & OWNERSHIP** | **PASS (REPAIRED J3A-R2b)** | `WarmIPCService.swift` and pure Foundation `WarmCaptureFlowCoordinator` (`ios/protocol/WarmCaptureFlow.swift`): eliminates hot-mic race on late permission callback after user disarm, ensures session replacement Stop(A) completes before B starts, fences stale Stop completions from overwriting newer response snapshots, and prevents keyboard STOP/START from interrupting manual in-app audio test. Verified by comprehensive CLI behavioral test suite (`WarmCaptureFlowTests.swift`). |

| **F. PRIVACY & BACKGROUND MODES** | **PASS** | `NSMicrophoneUsageDescription` and `UIBackgroundModes` [`audio`] configured in `ios/App/Info.plist`. Validated with `plutil -p`. |
| **G. KEYBOARD ISOLATION (APPLE SEC)**| **PASS** | Keyboard extension contains ZERO audio/recording code. Apple security constraint satisfied: microphone owned exclusively by containing app. Keyboard surfaces preparing, listening, and blocked states honestly. |
| **H. PHYSICAL FOREGROUND MIC PROBE** | **PASS (USER-CONFIRMED HISTORICAL)** | User previously confirmed foreground iPad mic: RMS bouncing and frame counter advancing. Post-repair physical verification marked UNVERIFIED until batch E2E. |
| **I. BACKGROUND AUDIO CONTINUITY** | **READY (APP-OWNED)** | Code supports `UIBackgroundModes audio` during active recording. App-owned lifetime and route integrity established. Background intake is active only while iOS schedules the app process; cold wakes and background stealth recording are rejected. |
| **J. MOCK TEXT REGRESSION** | **PASS** | In-app editor and `DEBUG: Send Mock Transcription` flow preserved in `AppStatusViewController.swift`. Final mock text insertion in keyboard preserved. |

---

## 2. Key Code Deliverables

1. **`ios/App/AudioCaptureController.swift`**:
   - Centralized, containing-app-owned singleton managing real microphone recording via `AVAudioEngine`.
   - Serialized executor model: all engine/session mutations, status, counters, and lifecycle transitions execute strictly on `stateQueue`.
   - Integrates production `CaptureLifecycleGate` to fence delayed hardware acknowledgments, duplicate/reentrant calls, and late tap callbacks.
   - Dedicated per-tap `MeterContext` holding immutable session tokens; tap closure modifies only context accumulators and reads zero mutable controller fields, eliminating cross-thread Swift data races.
   - Computes allocation-free RMS and peak power levels directly on audio thread; rate-limits (~4Hz / 250ms) before `DispatchQueue.async` dispatch to prevent ~47 tasks/sec queue flooding.
   - Hardware stop completion only after tap removed, engine stopped, and session deactivate attempted.
   - Handles route changes and system interruptions cleanly without auto-restart.

2. **`ios/protocol/CaptureLifecycleGate.swift` & `ios/protocol/CaptureLifecycleTests.swift`**:
   - Pure Foundation production state machine managing monotonic generation issuance, start ack/failure, cancellation, stop, and immutable meter snapshots.
   - CLI tests driving delayed hardware acknowledgments, post-stop stale queued meter dropping, duplicate start prevention, caller expectedGeneration fencing, and overflow fail-closed behavior.

3. **`ios/protocol/WarmCaptureFlow.swift` & `ios/protocol/WarmCaptureFlowTests.swift`**:
   - Pure Foundation flow coordinator state machine bridging wire admission (`EcholetAdmission.Gate`) and native capture lifecycle.
   - Explicit capture ownership: differentiates keyboard session capture from in-app manual testing capture. Rejects keyboard START as busy if manual capture active; prevents keyboard STOP from stopping manual capture.
   - Monotonic cancellation generations fence against late permission callbacks after disarm and late hardware start success callbacks.
   - Serialized session replacement: awaits native stop completion of session A before initiating permission and start for session B.
   - Prevents stale stop completions from overwriting newer session responses in App Group.
   - Strict monotonic response revision with fail-closed overflow handling.

4. **`ios/App/WarmIPCService.swift`**:
   - App-process-owned lifecycle coordinated via `AppDelegate` and `UISceneDelegate` (independent of VC presentation).
   - Manages foreground polling of `echolet.keyboard.request.v2` and Darwin notification observer hints.
   - Drives production `WarmCaptureFlowCoordinator` on dedicated `ipcQueue`.
   - Single-writer App Group response publisher with Darwin notification signaling.
   - Preserves honest background command intake during active recording without promising cold wake or background scheduling when idle.

5. **`ios/App/AppStatusViewController.swift`**:
   - Interactive "Enable / Arm Microphone Test" switch and "Start/Stop Audio Test" buttons.
   - Live RMS level progress meter, dB level display, and frame counter.
   - Local UI permission generation fencing and manual capture ownership registration with `WarmIPCService`.
   - In-app test editor and mock transcription button routed through `WarmIPCService.writeResponseSnapshot`.

6. **`ios/protocol/EcholetAdmission.swift`, `ios/protocol/AdmissionTests.swift`, `ios/protocol/AdversarialTests.swift`**:
   - Pure Foundation Swift port of Rust `src/ios_ipc.rs` admission gate.
   - Unit tests verifying wire admission, monotonic intent sequencing, stale STOP protection, duplicate rejection, tombstone advancement, and asynchronous cancellation/fencing. Adversarial tests updated to drive real `CaptureLifecycleGate`.

7. **`ios/App/Info.plist`**:
   - Configured `NSMicrophoneUsageDescription`: "Echolet requires microphone access to capture real audio for on-device speech-to-text recognition test."
   - Configured `UIBackgroundModes` array with `audio`.

---

## 3. Physical iPad Verification Script

To complete the physical microphone smoke test on the connected iPad:

1. **Unlock iPad (3)** (iPad mini 6th generation).
2. **Open Echolet App**:
   - Notice the green "App Group: Connected" status.
   - Scroll to "iPad Microphone Capture Probe (J3A)".
3. **Trigger OS Permission Prompt**:
   - Tap "Start Audio Test" or toggle "Enable / Arm Microphone Test".
   - An iOS system prompt will appear: *"Echolet Would Like to Access the Microphone"*.
   - Tap **OK / Allow**.
4. **Observe Real Metering**:
   - Speak near the iPad microphone.
   - Observe the progress bar bounce and "Frames: X | RMS: -Y dB | Peak: -Z dB | Time: T.Ts" advance continuously.
5. **Stop Capture**:
   - Tap "Stop Audio Test".
   - Confirm status changes to "STOPPED (Hardware Tap Released)" and frame count stops advancing.
6. **Test Background Continuity**:
   - Tap "Start Audio Test".
   - Swipe up to switch to Notes or Home Screen.
   - Wait 5-10 seconds.
   - Switch back to Echolet: confirm frame count grew by ~200,000-400,000 frames during background capture.
