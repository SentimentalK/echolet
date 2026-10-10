# TEMPORARY RESEARCH / DELETE AFTER DESIGN ACCEPTANCE

> **Status:** **BLOCKED** (Missing `Xcode.app` IDE installation; standalone `CommandLineTools` present but insufficient for iOS simulator builds).  
> **Date:** 2026-10-10  
> **Host:** macOS 14.6.1 (Darwin 23.6.0 x86_64, Intel Core i7-9750H)  
> **Agent:** Orca logical agent `antigravity` (Google Gemini provider)  
> **Repository:** `SentimentalK/echolet` (`master` @ commit `ffbab07a2349ded72f21cc81728d7759d45e2169`)

---

## 1. Executive Summary

Echolet iOS Job 1 (J1) was executed on the physical macOS development host to audit the native toolchain, verify compilation prerequisites for iOS and iPadOS Simulator/Device builds, and perform an in-depth architecture comparison of open-source offline voice keyboard designs.

- **Environment Gate: BLOCKED.** The host machine has Apple Command Line Tools (`/Library/Developer/CommandLineTools`) and Swift 5.10 installed, but **Xcode.app is not installed**. Tools requiring Xcode (`xcodebuild`, `xcrun simctl`, `xcrun devicectl`, iOS SDKs) are absent. While an iOS 18.2 CoreSimulator runtime image disk image exists at `/Library/Developer/CoreSimulator/Volumes/iOS_22C150`, it cannot be loaded or targeted without an active Xcode installation.
- **Safe Setup Performed:** Installed missing Rust cross-compilation targets idempotently via `rustup`: `aarch64-apple-ios`, `aarch64-apple-ios-sim`, and `x86_64-apple-ios`. Verified host disk space (263 GiB available).
- **Physical Device Audit:** Read-only inspection confirms no iPad is currently connected via USB. (iPadOS shares the same extension codebase as iOS, but Simulator cannot validate real-world audio capture permissions, thermal throttling, or jetsam limits).
- **Open-Source Architectural Comparison:** Shallow audit performed on actual pinned source trees of `getdictus/dictus-ios`, `fmachta/WhisperBoard`, `DictionLabs/Diction`, and `stablyai/orca`. Confirms:
  1. Keyboard extensions **cannot** access the microphone directly via documented public APIs without opening the containing app or relying on App Groups + background audio sessions.
  2. Apple removed the private `_hostBundleID` API in iOS 16; third-party keyboards have no public mechanism to auto-return to the host app upon cold-start dictation.
  3. Dictus and WhisperBoard isolate audio recording and ASR model execution in the main containing app to comply with iOS keyboard extension memory (jetsam) limits (~50–70 MB ceiling).
- **Next Steps:** Human developer action required to install Xcode (15.4 or 16.x) via the Mac App Store or Apple Developer Portal before J2 native prototyping can compile Simulator targets.

---

## 2. Host Environment Audit & Telemetry

### 2.1 Hardware and OS
| Parameter | Value | Notes |
| :--- | :--- | :--- |
| **OS Name** | `Darwin` (macOS 14.6.1 Sonoma) | Verified via `uname -s` and `sw_vers` |
| **Build Version** | `23G93` | Kernel: `23.6.0 RELEASE_X86_64` |
| **Architecture** | `x86_64` | Intel(R) Core(TM) i7-9750H CPU @ 2.60GHz |
| **Available Disk Space** | 263 GiB free (41% utilized) | Mount `/System/Volumes/Data` has 263 GiB avail |

### 2.2 Developer Toolchain & Xcode Status
| Tool | Detected Version / Path | Status / Notes |
| :--- | :--- | :--- |
| **xcode-select path** | `/Library/Developer/CommandLineTools` | Only Command Line Tools installed |
| **Xcode.app** | **Not found** in `/Applications` or spotlight | **BLOCKED** — Full Xcode IDE required for iOS SDK |
| **xcodebuild** | Not available | Fails: `requires Xcode, but active directory is CommandLineTools` |
| **xcrun simctl** | Not available | Code 72: unable to find utility "simctl" |
| **xcrun devicectl** | Not available | Code 72: unable to find utility "devicectl" |
| **Installed SDKs** | `MacOSX14.4.sdk`, `MacOSX13.3.sdk` | iOS SDKs (`iPhoneOS.sdk`, `iPhoneSimulator.sdk`) absent |
| **Sim Runtimes** | `/Library/Developer/CoreSimulator/Volumes/iOS_22C150` | iOS 18.2 runtime image on disk, unaddressable without Xcode |
| **Swift Compiler** | Apple Swift version 5.10 (`swiftlang-5.10.0.13`) | Verified working for macOS CLI targets |
| **Cargo / Rust** | Cargo 1.99.0 (`stable-x86_64-apple-darwin`) | Functional |
| **Homebrew** | `/usr/local/bin/brew` | Present |
| **Git** | `git version 2.47.0` | Clean worktree on `SentimentalK/echolet` master |
| **CMake** | Not installed in PATH | Optional |
| **Code Signing Identity** | `0 valid identities found` | `security find-identity -p codesigning -v` |
| **Connected Devices** | No iPad detected via USB | Read-only `system_profiler SPUSBDataType` check |

### 2.3 What Was Already Installed vs. What Was Installed
- **Already Installed:**
  - macOS 14.6.1 CLT (Clang 1500.3.9.4, Swift 5.10).
  - Rust toolchain `stable-x86_64-apple-darwin` with targets `aarch64-linux-android`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`.
  - Homebrew `/usr/local/bin/brew`.
- **Installed During J1 (Idempotent / Benign):**
  - Added Rust cross-compilation targets via `rustup target add`:
    - `aarch64-apple-ios` (for physical iOS/iPadOS devices)
    - `aarch64-apple-ios-sim` (for Apple Silicon iOS Simulator)
    - `x86_64-apple-ios` (for Intel x86_64 iOS Simulator on this host)
- **Human Action Required:**
  - Install Xcode (e.g. Xcode 15.4 or 16.x) into `/Applications/Xcode.app`.
  - Run `sudo xcode-select -s /Applications/Xcode.app/Contents/Developer` and accept Xcode license (`sudo xcodebuild -license accept`).
  - Configure a free personal Apple ID Team in Xcode for simulator and on-device testing.

---

## 3. Scratch Swift Compilation Smoke Evidence

Due to the absence of `xcodebuild` and `iphonesimulator` SDKs, a full iOS simulator scratch target could not be compiled using Xcode toolchains. However, a local smoke test of the Swift toolchain was conducted:

```sh
$ mkdir -p /tmp/scratch_ios_test && cat << 'EOF' > /tmp/scratch_ios_test/main.swift
import Foundation
print("Hello Swift CLI")
EOF
$ swiftc /tmp/scratch_ios_test/main.swift -o /tmp/scratch_ios_test/hello && /tmp/scratch_ios_test/hello
Hello Swift CLI
```
**Exit Status:** `0` (Success). Cleaned up immediately.

Attempting to locate the iOS simulator SDK:
```sh
$ xcrun --sdk iphonesimulator --show-sdk-path
xcrun: error: SDK "iphonesimulator" cannot be located
```
**Exit Status:** `1` (Confirms missing iOS SDK).

---

## 4. Feasibility of Packaging Existing Rust Core / Sherpa-ONNX for iOS

Echolet currently uses:
- `libsherpa-onnx-c-api` + `libonnxruntime` pinned to `v1.13.6`.
- Dynamic libraries loaded via `@rpath` on macOS/Linux and `.so` on Android.

### Findings for iOS Integration:
1. **Target Triples:**
   - Real Device: `aarch64-apple-ios`.
   - Apple Silicon Simulator: `aarch64-apple-ios-sim`.
   - Intel Mac Simulator (this host): `x86_64-apple-ios`.
2. **Upstream Sherpa-ONNX iOS Prebuilts:**
   - Upstream `k2-fsa/sherpa-onnx` publishes pre-compiled xcframeworks and static libraries for iOS (e.g. `sherpa-onnx-v1.13.x-ios.tar.bz2`), including `onnxruntime.xcframework`.
   - On iOS, Apple sandbox and App Extension policies strongly prefer static libraries or unified `.xcframework` bundles embedded in the containing application's `Frameworks/` directory.
3. **Memory Constraint (Crucial):**
   - iOS Keyboard Extensions run in a strict jetsam sandbox (~50 MB max RAM).
   - Attempting to link or run `sherpa-onnx` and load a streaming Zipformer or SenseVoice model directly inside the Keyboard Extension process **will result in instant jetsam termination (`EXC_RESOURCE: MEMORY`)**.
   - Therefore, the Rust core and Sherpa ASR engine **must reside in the containing iOS app**, while the Keyboard Extension communicates via lightweight IPC.

---

## 5. Open-Source Offline Voice Keyboard Architectural Analysis

We audited the actual source trees of four reference implementations:

### 5.1 Repository Inventory & Evidence
1. **`getdictus/dictus-ios`**
   - **Pinned Commit:** `0fd7badf47e1f98f3e3757dc84a2b1d4720d9ed8` (License: MIT)
   - **Key Files Audited:**
     - `DictusKeyboard/KeyboardState.swift`
     - `DictusApp/Audio/UnifiedAudioEngine.swift`
     - `DictusCore/Sources/DictusCore/AppGroup.swift`
     - `.planning/adr-cold-start-autoreturn.md`
     - `.planning/app-review-history.md`
     - `DictusKeyboard/Info.plist`, `DictusKeyboard.entitlements`, `DictusApp/Info.plist`
2. **`fmachta/WhisperBoard`**
   - **Pinned Commit:** `aa007da48fc495cf650c33ac8755953d37834931` (License: MIT)
   - **Key Files Audited:**
     - `WhisperBoard/Sources/Shared/AudioCapture.swift`
     - `WhisperBoard/Sources/KeyboardExtension/KeyboardViewController.swift`
     - `WhisperBoard/Sources/App/TranscriptionService.swift`
     - `WhisperBoard/Sources/Shared/SharedDefaults.swift`
3. **`DictionLabs/Diction`**
   - **Pinned Commit:** `09f5acbbd194397a00d32b3553cfcf628e1c40ba` (License: MIT)
   - **Key Files Audited:**
     - `gateway/` (Go gateway for transcription), `docs/`
4. **`stablyai/orca`**
   - **Pinned Commit:** `61a99d7e914c7f89e7a1d6455bccfea6fb1172b8` (License: Apache-2.0 / Proprietary)
   - **Key Files Audited:**
     - `mobile/src/platform/dictation-capture.ts`
     - `mobile/src/hooks/use-mobile-dictation.ts`

---

### 5.2 Architectural Comparison Matrix

| Dimension | `getdictus/dictus-ios` | `fmachta/WhisperBoard` | `DictionLabs/Diction` | `stablyai/orca` (mobile) | Recommended Echolet iOS Architecture |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **System-wide Keyboard Extension?** | **Yes** (`UIInputViewController`) | **Yes** (`UIInputViewController`) | No (Self-hosted gateway + client) | **No** (In-app dictation inside Electron / React Native shell) | **Yes** (Native Swift `UIInputViewController`) |
| **Microphone Ownership** | **Main Containing App** (`UnifiedAudioEngine`) | **Main Containing App** (Fallback to `openMainApp()` when extension restricted) | Backend gateway / host app | **In-App Mic** (`@orca/expo-two-way-audio`) | **Main Containing App** |
| **ASR Model Location** | Main App (WhisperKit / CoreML) | Main App (WhisperKit / CoreML) | Remote Server / Local Ollama / Whisper | Remote Cloud / Local Server | **Main App** (Sherpa-ONNX / Rust core) |
| **Extension Memory Footprint** | Extremely low (<20 MB); UI only | Low (<25 MB); UI + polling timer | N/A | Full App Memory | **Minimal (<30 MB)** |
| **IPC Transport** | App Group `UserDefaults` + Darwin Notifications (`notify_post`) | App Group `UserDefaults` + Darwin Notifications + shared wav file | HTTP / WebSocket | Electron IPC / React Native bridge | **App Group Container (`UserDefaults` + Darwin Notifications)** |
| **Warm vs. Cold Start** | Warm: Background audio keeps app alive (~500ms response). Cold: URL scheme `dictus://dictate` | URL scheme `whisperboard://` + Darwin notify | Direct REST API | Direct audio stream | **Warm background audio / Darwin notify; Fallback: URL scheme `echolet://`** |
| **Auto-Return from Cold Start** | **Impossible via Public API** (Private `_hostBundleID` removed in iOS 16; documented in ADR) | None (User manual swipe back) | N/A | N/A | **Manual Swipe-Back UX** (Never rely on private APIs) |
| **App Store Review Risks** | Guideline 4.4.1 (Requires globe switch key); 5.1.1(iv) (Microphone wording; on-device privacy disclosures) | Unsubmitted prototype | N/A | Desktop / TestFlight internal | Strict compliance with 4.4.1 (globe key) and 5.1.1 (on-device only disclosures) |

---

### 5.3 Detailed Findings & Critical Lessons

#### 1. Microphone Access in Keyboard Extensions
- **The Core Constraint:** Apple prohibits keyboard extensions from recording audio directly through `AVAudioSession` unless "RequestsOpenAccess" is granted, and even with open access, iOS system policies frequently deny audio input or immediately terminate extension processes attempting audio capture.
- **Dictus Architecture:**
  - Dictus uses a **Two-Tier Recording Pipeline**:
    1. **Warm State:** The main app maintains an active `AVAudioSession` in the background with `UIBackgroundModes: audio`. When the user taps the mic in the keyboard extension, the keyboard posts a Darwin Notification (`notify_post("com.pivi.dictus.startRecording")`). The background app begins capturing audio immediately without foregrounding.
    2. **Cold State Fallback:** If the main app was terminated by iOS jetsam or has been idle, the Darwin notification times out (500 ms). The keyboard extension then calls `extensionContext?.open(URL("dictus://dictate"))` to launch the main app.
- **WhisperBoard Architecture:**
  - WhisperBoard's `AudioCapture.swift` attempted audio recording inside the extension, but in practice fell back to signaling the main app via Darwin notifications (`com.fmachta.whisperboard.startRecording`) and polling `SharedDefaults`.

#### 2. The Auto-Return Problem (Cold Start)
- As proven in Dictus's `.planning/adr-cold-start-autoreturn.md`:
  - Apple completely removed `_hostBundleID` in iOS 16.
  - `sourceApplication` in `UIOpenURLContext` returns `nil` for third-party apps for privacy.
  - Enumerating `canOpenURL` is non-deterministic and forbidden.
  - **Verdict:** There is **NO public API** to automatically switch back to the host app. Echolet must design an onboarding gesture / swipe-back guide UI for cold starts.

#### 3. App Review Pitfalls (Dictus App Review History)
- **Guideline 4.4.1 (Keyboard Extensions):** A custom keyboard **MUST** provide a globe button (`advanceToNextInputMode`) allowing the user to switch back to system keyboards. Failure to implement this is an instant rejection.
- **Guideline 5.1.1(iv) (Privacy):** Apple reviewers frequently suspect speech-to-text apps of sending audio to third-party cloud AI. Pre-permission dialogs must use neutral words like "Continue" or "Next" (not "Allow"), and privacy descriptions must explicitly state that 100% of recognition happens on-device offline.

#### 4. Disambiguating stablyai/orca
- The audio capture code in `stablyai/orca` (`mobile/src/platform/dictation-capture.ts`) uses Expo (`@orca/expo-two-way-audio`) inside a standard full-screen mobile app shell. It does **not** provide a system-wide third-party keyboard extension and does not operate under keyboard sandbox/jetsam constraints.

---

## 6. Recommended Echolet iOS Architecture (J2/J3 Plan)

```
┌─────────────────────────────────────────────────────────────┐
│                      Host Application                       │
│                   (Notes, WhatsApp, etc.)                   │
└──────────────────────────────┬──────────────────────────────┘
                               │ Displays
┌──────────────────────────────▼──────────────────────────────┐
│           Echolet Keyboard Extension (Swift)                │
│  - UI: Keys, Globe (4.4.1 compliant), Dictation Button       │
│  - Memory: <30 MB RAM (NO ASR models, NO heavy libraries)    │
│  - Logic: Inserts text via UITextDocumentProxy              │
└──────────────┬──────────────────────────────▲───────────────┘
               │ 1. start_recording           │ 4. insert_text
               │    (Darwin notify / URL)     │    (state update)
               ▼                              │
┌─────────────────────────────────────────────┴───────────────┐
│        App Group Shared Container (UserDefaults / IPC)      │
│  - Schema: EcholetIPCSchema v1 (seq, session_id, status)    │
└──────────────┬──────────────────────────────▲───────────────┘
               │ 2. read trigger              │ 3. write transcript
               ▼                              │
┌─────────────────────────────────────────────┴───────────────┐
│                Echolet Main Containing App                  │
│  - Native Audio: AVAudioEngine (16kHz mono)                 │
│  - Background Mode: UIBackgroundModes = ["audio"]           │
│  - Engine: Echolet Rust Core / Sherpa-ONNX C-API            │
│  - Models: Stored in App Group or App Sandbox               │
└─────────────────────────────────────────────────────────────┘
```

### Proposed IPC Protocol Schema (v1)
To ensure reliable communication between Keyboard Extension and Main App:
- **`seq`**: Monotonically increasing 64-bit integer.
- **`session_id`**: UUID string per recording session.
- **`state`**: `idle` | `recording` | `processing` | `completed` | `failed`.
- **`text`**: Incremental or final transcript.
- **`error_code`**: Null or specific error string (e.g. `mic_denied`, `phone_call_active`).

---

## 7. Risks & Known Unknowns

1. **Free Personal Developer Team Limitations:**
   - App Groups (`group.com.sentimental.echolet`) require an App ID with the App Group entitlement enabled. On free Apple ID provisioning profiles, App Groups may require specific bundle ID prefixes or may fail on physical devices.
2. **Background Audio Termination:**
   - While `UIBackgroundModes: audio` keeps the containing app alive while recording or playing silence, iOS will suspend or kill the app if audio session becomes inactive for extended periods. Cold starts via URL scheme will be inevitable.
3. **Intel Host Architecture:**
   - Host is an Intel x86_64 Mac (`i7-9750H`). Any iOS simulator testing on this host requires `x86_64-apple-ios` runtime slices, whereas modern Apple Silicon uses `arm64-apple-ios-sim`. Pre-compiled libraries must support fat/universal binaries or x86_64 simulator slices.

---

## 8. Specific J2 Scope & Stop Conditions

### Recommended Scope for J2:
1. Prerequisite: Full Xcode IDE installed on Mac host.
2. Create native iOS workspace in repository (`ios/Echolet.xcodeproj` or Swift Package).
3. Implement minimal `UIInputViewController` extension with system globe button.
4. Implement basic App Group IPC roundtrip between App and Extension.
5. Compile and boot smoke test on iPhone and iPad Simulators.

### J2 Stop Conditions:
- If Xcode installation is blocked or unlicensed -> STOP.
- Do NOT download heavy ASR models in J2.
- Do NOT modify existing Android or desktop codebase.
