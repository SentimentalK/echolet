# Echolet iOS & iPadOS Development Guide

This directory contains the native iOS / iPadOS Containing Application and Custom Keyboard Extension scaffold for Echolet.

## Architecture

- **`App/`**: Minimal UIKit containing application (`EcholetApp`).
  - Initializes and displays the per-process launch epoch (`echolet.app.epoch.v2`).
  - Contains an in-app editable `UITextView` test harness so the custom keyboard can be tested directly inside Echolet without app-switching or losing keyboard focus.
  - Houses `AudioCaptureController.swift`:
    - Manages real `AVAudioEngine` microphone tap, truthful permission flow, allocation-free RMS/peak level metering, interruption and route changes.
    - Serialized executor model: all engine/session mutations, status, counters, and lifecycle transitions execute strictly on `stateQueue`.
    - Pure Foundation `CaptureLifecycleGate` integration: fences delayed hardware acknowledgements, duplicate/reentrant calls, and late tap callbacks.
    - Real-time-safe metering: dedicated `MeterContext` with immutable session tokens prevents cross-thread Swift data races. Rate-limiting (~4Hz / 250ms) before `DispatchQueue.async` avoids callback queue flooding (~47 tasks/sec).
  - Houses `WarmIPCService.swift`:
    - Process-owned lifecycle wired directly to `AppDelegate` and `UISceneDelegate`, running independently of UIViewController appearance.
    - Reads latest App Group state at launch/reactivation, polls at a bounded cadence (0.3s) while foreground/active, and listens for Darwin notification hints (`com.echolet.ipc.request.v2`).
    - Distinguishes receipt of START from affirmative user arming; emits `preparing` while asynchronous audio engine start is in flight, and `listening` only after confirmed hardware start.
    - Synchronously invalidates active session tokens on STOP/CANCEL and writes `completed` only after native hardware stops.
    - Manages background command intake as long as iOS genuinely schedules the app under `UIBackgroundModes audio`; avoids busy loops and makes no false claims about cold wake or background scheduling guarantees when suspended.
- **`Keyboard/`**: Custom `UIInputViewController` Keyboard Extension (`EcholetKeyboard`).
  - Note: Per Apple custom keyboard security requirements, custom keyboards have **no microphone access**. Containing `EcholetApp` exclusively owns audio capture.
  - Provides required Apple keyboard switching (`advanceToNextInputMode()`).
  - Dispatches validated v2 `KeyboardRequest` envelopes into App Group `UserDefaults` (`group.com.mainstayx.echolet.dev`).
  - Gated response consumer: safely correlates session ID, app epoch, sequence, and strictly monotonic response revision before invoking `textDocumentProxy.insertText`.
  - Honestly surfaces preparing, listening, and blocked microphone states without inserting fabricated transcripts.
  - Automatically fences and invalidates active session on input focus change (`textWillChange`).
- **`protocol/`**: Pure Foundation Swift wire protocol codec (`EcholetIPC.swift`), admission gate port (`EcholetAdmission.swift`), and capture lifecycle gate (`CaptureLifecycleGate.swift`), mirroring canonical Rust `src/ios_ipc.rs`. Includes pure Swift CLI tests (`AdmissionTests.swift`, `AdversarialTests.swift`, `CaptureLifecycleTests.swift`, `IPCCodecSmoke.swift`).
- **`project.yml`**: Declarative XcodeGen project specification generating `Echolet.xcodeproj`.
- **`scripts/`**: Safe device discovery and automation scripts (`verify-device.sh`).

---

## Status: BUILT & TESTED (Xcode 16.2 / Sonoma)

All pure Foundation protocol tests, admission gate tests, capture lifecycle tests, and adversarial fence tests run via CLI. Native `EcholetApp` and embedded `EcholetKeyboard` targets compile cleanly on Xcode 16.2 (`** BUILD SUCCEEDED **`) for physical iPhoneOS (arm64) and Simulator. Signed debug build previously deployed to connected iPad mini (iPadOS 18.7.7).

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
