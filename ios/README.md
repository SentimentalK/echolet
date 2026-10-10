# Echolet iOS & iPadOS Development Guide

This directory contains the native iOS / iPadOS Containing Application and Custom Keyboard Extension scaffold for Echolet.

## Architecture

- **`App/`**: Minimal UIKit containing application (`EcholetApp`).
  - Initializes and displays the per-process launch epoch (`echolet.app.epoch.v2`).
  - Contains an in-app editable `UITextView` test harness so the custom keyboard can be tested directly inside Echolet without app-switching or losing keyboard focus.
  - Houses `AudioCaptureController.swift` managing real `AVAudioEngine` microphone tap, truthful permission flow, allocation-free RMS/peak level metering, interruption and route changes.
  - Houses `WarmIPCService.swift` coordinating foreground keyboard requests, admission gate checks, and response snapshots.
  - Houses a DEBUG-only test responder button to validate IPC message intake and mock regression testing.
- **`Keyboard/`**: Custom `UIInputViewController` Keyboard Extension (`EcholetKeyboard`).
  - Note: Per Apple custom keyboard security requirements, custom keyboards have **no microphone access**. Containing `EcholetApp` exclusively owns audio capture.
  - Provides required Apple keyboard switching (`advanceToNextInputMode()`).
  - Dispatches validated v2 `KeyboardRequest` envelopes into App Group `UserDefaults` (`group.com.mainstayx.echolet.dev`).
  - Gated response consumer: safely correlates session ID, app epoch, sequence, and strictly monotonic response revision before invoking `textDocumentProxy.insertText`.
  - Honestly surfaces listening and blocked microphone states without inserting fabricated transcripts.
  - Automatically fences and invalidates active session on input focus change (`textWillChange`).
- **`protocol/`**: Pure Foundation Swift wire protocol codec (`EcholetIPC.swift`) and admission gate port (`EcholetAdmission.swift`), mirroring canonical Rust `src/ios_ipc.rs`. Includes pure Swift CLI tests (`AdmissionTests.swift`, `IPCCodecSmoke.swift`).
- **`project.yml`**: Declarative XcodeGen project specification generating `Echolet.xcodeproj`.
- **`scripts/`**: Safe device discovery and automation scripts (`verify-device.sh`).

---

## Status: BUILT & TESTED (Xcode 16.2 / Sonoma)

All pure Foundation protocol tests and admission gate tests run via CLI. Native `EcholetApp` and embedded `EcholetKeyboard` targets compile cleanly on Xcode 16.2 (`** BUILD SUCCEEDED **`) for both physical iPhoneOS (arm64) and Simulator. Signed debug build deployed to connected iPad mini (iPadOS 18.7.7).

---

## Project Generation & Build Commands (Once Xcode 16.2 is Installed)

### 1. Generate Xcode Project
Install XcodeGen if needed, then generate the project file:
```bash
brew install xcodegen
cd ios
xcodegen generate
```

### 2. Configure Active Developer Directory
Point to Xcode.app:
```bash
sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
```

### 3. Run Device Discovery
```bash
./ios/scripts/verify-device.sh
```

### 4. Build via CLI (Simulator or Physical Device)
Build for iOS Simulator:
```bash
xcodebuild -project ios/Echolet.xcodeproj -scheme EcholetApp -destination 'generic/platform=iOS Simulator' build
```

Build for connected physical device (requires signing team configured):
```bash
xcodebuild -project ios/Echolet.xcodeproj -scheme EcholetApp -destination 'generic/platform=iOS' build
```

---

## Physical iPad (iPadOS 18.7.7) Preparation & Signing Notes

1. **Hardware Connection**: Connect iPad via USB-C to Mac.
2. **Trust & Developer Mode**:
   - Tap "Trust This Computer" on iPad when prompted.
   - On iPadOS 18.7.7: Navigate to **Settings -> Privacy & Security -> Developer Mode** and switch on (requires restart).
3. **Personal Team Provisioning & App Groups Limitation**:
   - In Xcode under Target `EcholetApp` and `EcholetKeyboard` -> **Signing & Capabilities**, select your Personal Team.
   - **Crucial Warning**: Free Apple Developer Personal Teams often do **not** support the `App Groups` entitlement. If signing fails with an App Groups provisioning profile error, you will need an Apple Developer Program membership to test cross-process App Group communication on a physical device, or run within the iOS Simulator where App Groups can run locally.
4. **Deploying to Physical Device**:
   ```bash
   xcrun devicectl device install app --device <DEVICE_UUID> <PATH_TO_APP_BUNDLE>
   ```
