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
| **B. SWIFT IPC ADMISSION TESTS** | **PASS** | Pure Swift Foundation admission suite (`AdmissionTests.swift`) verified: cold boot fence, monotonic intent, stale STOP rejection, clean replacement, tombstone handling. `IPCCodecSmoke.swift` golden fixtures passed. |
| **C. RUST IPC SUITE** | **PASS** | `cargo test --lib ios_ipc` passed all 18 tests; full `cargo test --lib` passed all 76 tests. |
| **D. APP DEPLOYED TO IPAD** | **PASS** | Installed via `xcrun devicectl device install app` to `iPad (3)` (`com.mainstayx.echolet.app`, databaseSequenceNumber 6484) and launched via `devicectl device process launch`. |
| **E. REAL AUDIO COMPONENT** | **PASS** | `AudioCaptureController.swift` implemented with `AVAudioEngine.inputNode.installTap`, allocation-free RMS/peak metering (~4Hz dispatch), `.playAndRecord`, and session deactivation on stop. |
| **F. PRIVACY & BACKGROUND MODES** | **PASS** | `NSMicrophoneUsageDescription` and `UIBackgroundModes` [`audio`] configured in `ios/App/Info.plist`. Validated with `plutil -p`. |
| **G. KEYBOARD ISOLATION (APPLE SEC)**| **PASS** | Keyboard extension contains ZERO audio/recording code. Apple security constraint satisfied: microphone owned exclusively by containing app. Keyboard surfaces listening/blocked state honestly. |
| **H. PHYSICAL FOREGROUND MIC PROBE** | **LIMITED / PENDING INTERACTION** | App is installed and running on iPad. Physical microphone access requires explicit interactive OS permission prompt approval and tap on "Enable / Arm Microphone Test" or "Start Audio Test" on iPad screen. Meter and frame counter are ready. |
| **I. BACKGROUND AUDIO CONTINUITY** | **PHYSICAL UNVERIFIED** | Code supports `UIBackgroundModes audio` during active recording. Physical verification requires user to start audio test in app, switch to Notes / another editor, and observe meter upon return. |
| **J. MOCK TEXT REGRESSION** | **PASS** | In-app editor and `DEBUG: Send Mock Transcription` flow preserved in `AppStatusViewController.swift`. Final mock text insertion in keyboard preserved. |

---

## 2. Key Code Deliverables

1. **`ios/App/AudioCaptureController.swift`**:
   - Centralized, containing-app-owned singleton managing real microphone recording via `AVAudioEngine`.
   - Uses `inputNode.installTap(onBus: 0, bufferSize: 1024)` receiving raw PCM buffers.
   - Computes allocation-free RMS and peak power levels directly on audio thread; rate-limits UI updates to ~4Hz (250ms).
   - Handles route changes (e.g. headset disconnect) and system interruptions cleanly.
   - Deactivates `AVAudioSession` and cleans up engine tap on stop/cancel.

2. **`ios/App/WarmIPCService.swift`**:
   - Manages foreground polling of `echolet.keyboard.request.v2`.
   - Adopts `EcholetAdmission.Gate` with the containing app process epoch (`AppDelegate.sharedEpoch`).
   - Requires explicit user arming before initiating audio capture from incoming `START` requests.
   - Emits snapshot responses (`EcholetIPC.AppResponse`) with strictly monotonic revisions and Darwin notification hints.

3. **`ios/App/AppStatusViewController.swift`**:
   - Added interactive "Enable / Arm Microphone Test" switch and "Start/Stop Audio Test" buttons.
   - Added live RMS level progress meter, dB level display, and frame counter.
   - Preserved contained in-app test editor and mock transcription button.

4. **`ios/protocol/EcholetAdmission.swift` & `ios/protocol/AdmissionTests.swift`**:
   - Pure Foundation Swift port of Rust `src/ios_ipc.rs` admission gate.
   - 9 comprehensive unit tests verifying wire admission, monotonic intent sequencing, stale STOP protection, duplicate rejection, and tombstone advancement.

5. **`ios/App/Info.plist`**:
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
