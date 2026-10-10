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
| **B. SWIFT IPC ADMISSION TESTS** | **PASS** | Pure Swift Foundation admission suite (`AdmissionTests.swift`, `AdversarialTests.swift`) verified: cold boot fence, monotonic intent, stale STOP rejection, clean replacement, tombstone handling, asynchronous permission and start/stop completion fences. `IPCCodecSmoke.swift` golden fixtures passed. |
| **C. RUST IPC SUITE** | **PASS** | `cargo test --lib ios_ipc` passed all 18 tests; full `cargo test --lib` passed all 76 tests. |
| **D. APP DEPLOYED TO IPAD** | **PASS** | Installed via `xcrun devicectl device install app` to `iPad (3)` (`com.mainstayx.echolet.app`, databaseSequenceNumber 6484) and launched via `devicectl device process launch`. |
| **E. REAL AUDIO COMPONENT** | **PASS** | `AudioCaptureController.swift` implemented with `AVAudioEngine.inputNode.installTap`, allocation-free RMS/peak metering (~4Hz dispatch), `.playAndRecord`, session deactivation on stop, and generation fencing on tap callbacks and start/stop completions. |
| **F. PRIVACY & BACKGROUND MODES** | **PASS** | `NSMicrophoneUsageDescription` and `UIBackgroundModes` [`audio`] configured in `ios/App/Info.plist`. Validated with `plutil -p`. |
| **G. KEYBOARD ISOLATION (APPLE SEC)**| **PASS** | Keyboard extension contains ZERO audio/recording code. Apple security constraint satisfied: microphone owned exclusively by containing app. Keyboard surfaces preparing, listening, and blocked states honestly. |
| **H. PHYSICAL FOREGROUND MIC PROBE** | **PASS (USER-CONFIRMED)** | User tested on iPad: enabled microphone, spoke into mic, observed real-time RMS meter bouncing and frame counter advancing. Keyboard START warm activation and STOP release confirmed working. |
| **I. BACKGROUND AUDIO CONTINUITY** | **READY (APP-OWNED)** | Code supports `UIBackgroundModes audio` during active recording. App-owned lifetime and route integrity established. Background intake is active only while iOS schedules the app process; cold wakes and background stealth recording are rejected. |
| **J. MOCK TEXT REGRESSION** | **PASS** | In-app editor and `DEBUG: Send Mock Transcription` flow preserved in `AppStatusViewController.swift`. Final mock text insertion in keyboard preserved. |

---

## 2. Key Code Deliverables

1. **`ios/App/AudioCaptureController.swift`**:
   - Centralized, containing-app-owned singleton managing real microphone recording via `AVAudioEngine`.
   - Uses `inputNode.installTap(onBus: 0, bufferSize: 1024)` receiving raw PCM buffers.
   - Computes allocation-free RMS and peak power levels directly on audio thread; rate-limits UI updates to ~4Hz (250ms).
   - Enforces generation fences across permission preflight, session activation, engine start, and stop.
   - Provides explicit completion callbacks for start (ensuring `listening` is only emitted after hardware start) and stop (ensuring tap removal and engine halt complete before ACK).
   - Handles route changes (e.g. headset disconnect) and system interruptions cleanly without auto-restart.

2. **`ios/App/WarmIPCService.swift`**:
   - App-process-owned lifecycle coordinated via `AppDelegate` and `UISceneDelegate` (independent of VC presentation).
   - Manages foreground polling of `echolet.keyboard.request.v2` and Darwin notification observer hints.
   - Emits `preparing` state while asynchronous start is underway, and `listening` only after verified native engine start.
   - Requires explicit user arming in the containing app before initiating audio capture from incoming `START` requests.
   - On `STOP`/`CANCEL`, halts engine cleanly and writes `completed` response only after hardware stop confirmation.
   - Preserves honest background command intake during active recording without promising cold wake or background scheduling when idle.

3. **`ios/App/AppStatusViewController.swift`**:
   - Interactive "Enable / Arm Microphone Test" switch and "Start/Stop Audio Test" buttons.
   - Live RMS level progress meter, dB level display, and frame counter.
   - In-app test editor and mock transcription button.
   - Removed VC-tied stopPolling to ensure process-level Warm IPC continuity.

4. **`ios/protocol/EcholetAdmission.swift`, `ios/protocol/AdmissionTests.swift`, `ios/protocol/AdversarialTests.swift`**:
   - Pure Foundation Swift port of Rust `src/ios_ipc.rs` admission gate.
   - Unit tests verifying wire admission, monotonic intent sequencing, stale STOP protection, duplicate rejection, tombstone advancement, and asynchronous cancellation/fencing.

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
