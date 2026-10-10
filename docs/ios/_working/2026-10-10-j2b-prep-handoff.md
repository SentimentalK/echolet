# Echolet iOS J2B-Prep Scaffold & Physical iPad Automation Handoff

> **Status**: TEMPORARY HANDOFF DOCUMENT (J2B-Prep Deliverable)  
> **Date**: 2026-10-10  
> **Repository Baseline**: `64f6a2460b715fd37085102d8f856fb49cbfa0f2` (origin/master)  
> **Target OS & Devices**: iPhone & iPad (iPadOS 18.7.7 target compatibility, minimum deployment iOS 15.0)  
> **Xcode Version Target**: Apple Xcode 16.2 (Installation in progress by workstation owner)  
> **Native UIKit Status**: **UNCOMPILED** (Native compilation deferred until host Xcode installation completes)

---

## 1. Summary of Deliverables

During concurrent user installation of Apple Xcode 16.2, the following build-ready native scaffolds, manifests, and verification scripts were established:

1. **XcodeGen Project Specification (`ios/project.yml`)**:
   - Multi-target manifest configuring `EcholetApp` (containing UIKit application) and `EcholetKeyboard` (custom `UIInputViewController` keyboard extension).
   - Platform: `iOS`, deployment target: `15.0`, targeted device families: `1,2` (iPhone and iPad).
   - Shared App Group entitlement: `group.com.mainstayx.echolet.dev`.
   - Single source of truth for protocol: imports existing Foundation `ios/protocol/EcholetIPC.swift` v2 into both targets without duplicate wire structs.

2. **Containing App Scaffold (`ios/App/`)**:
   - `AppDelegate.swift`, `SceneDelegate.swift`, `Info.plist`, `EcholetApp.entitlements`.
   - `AppStatusViewController.swift`:
     - Generates and writes unique launch process `app_epoch` UUID to App Group `echolet.app.epoch.v2`.
     - Displays keyboard setup guidance and App Group connectivity diagnostics.
     - DEBUG-only test responder: reads valid incoming keyboard requests, verifies active app epoch, and publishes an explicit mock response snapshot into `echolet.app.response.v2`.

3. **Keyboard Extension Scaffold (`ios/Keyboard/`)**:
   - `Info.plist` (configured with `com.apple.keyboard-service`, `RequestsOpenAccess = false`), `EcholetKeyboard.entitlements`.
   - `KeyboardViewController.swift`:
     - Apple HIG compliance: includes `advanceToNextInputMode()` globe key for switching keyboards.
     - Explicit user-triggered START test button (no automatic recording, clearly marked demo).
     - Single-writer intent sequence reservation using App Group persistence.
     - Safety gating on text consumption: correlation of session ID, app epoch, request ID, sequence, and strictly monotonic response revision before calling `textDocumentProxy.insertText`.
     - Fails closed on focus switches (`textWillChange`), keyboard dismissals (`viewWillDisappear`), or absent App Group entitlements.

4. **Discovery & Automation Script (`ios/scripts/verify-device.sh`)**:
   - Read-only environment discovery script checking active developer directory, `xcodebuild`, `xcodegen`, connected physical devices (`xcrun devicectl`), and available simulators (`xcrun simctl`).
   - Strict safety: strictly read-only by default, no `sudo`, no destructive device wipes, no secret or credential storage.

5. **Documentation (`ios/README.md`)**:
   - Comprehensive steps for generating the project via XcodeGen, switching developer directories, running device discovery, and deploying to iPadOS 18.7.7.

---

## 2. iPadOS 18.7.7 & Physical Device Provisioning Notes

- **Physical Device Pre-requisites**:
  1. Connect iPad to Mac using USB-C cable.
  2. Unlock iPad and approve "Trust This Computer".
  3. Turn on Developer Mode: **Settings -> Privacy & Security -> Developer Mode** (iPad will reboot and ask for confirmation).
- **Personal Team / Free Apple Account App Group Constraint**:
  - Apple's free Personal Team provisioning profiles typically **do not permit** the `com.apple.security.application-groups` entitlement on physical hardware.
  - If Xcode build or install fails with code signing errors regarding App Groups, the App and Extension can still be tested locally on the iOS Simulator without paid provisioning, or verified independently. Do not purchase developer accounts prematurely.

---

## 3. Real Test Verification Executed Today

| Test Suite | Command | Result |
|---|---|---|
| Rust iOS IPC Gate Tests | `cargo test --lib ios_ipc` | **PASS** (20 passed, 0 failed) |
| Rust Core Library Tests | `cargo test --lib` | **PASS** (76 passed, 0 failed) |
| Swift Foundation v2 Codec Smoke | `swiftc ios/protocol/EcholetIPC.swift ...` | **PASS** (Round-trip & invalidation golden tests pass) |
| Device Discovery Shell Syntax | `bash -n ios/scripts/verify-device.sh` | **PASS** (Syntax valid) |
| Device Discovery Execution | `./ios/scripts/verify-device.sh` | **PASS** (Reported current CommandLineTools state safely) |
| XcodeGen YAML Manifest Validation | `ruby -e "YAML.load_file('ios/project.yml')"` | **PASS** (Valid YAML structure) |
| Native UIKit Build | `xcodebuild` | **UNCOMPILED** (Host currently has CommandLineTools active while Xcode 16.2 installs) |

---

## 4. Verification & Generation Commands for Host Owner (Post-Xcode)

When Xcode 16.2 finishes installing:
```bash
# 1. Point developer directory to full Xcode
sudo xcode-select -s /Applications/Xcode.app/Contents/Developer

# 2. Run device discovery
./ios/scripts/verify-device.sh

# 3. Generate .xcodeproj
brew install xcodegen
cd ios && xcodegen generate

# 4. Open in Xcode or compile smoke
xcodebuild -project Echolet.xcodeproj -scheme EcholetApp -destination 'generic/platform=iOS Simulator' build
```
