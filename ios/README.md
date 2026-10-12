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
    - Drives pure Foundation `WarmCaptureFlowCoordinator`: bridges wire admission outcomes from `EcholetAdmission.Gate` with native audio capture lifecycle.
    - Ownership separation: distinguishes keyboard-owned capture from manual in-app test capture; if manual test is active, incoming keyboard START commands are rejected as busy without disrupting manual recording; keyboard STOP never stops manual audio.
    - Async cancellation fencing: disarming mic invalidates pending start before permission or native start callbacks complete; delayed permission callbacks cannot hot-mic after ARM OFF.
    - Session replacement: when B replaces A, hardware stop of A is awaited before starting B; late Stop(A) completion cannot overwrite B's response snapshot.
    - Monotonic checked revisions and single-writer response synchronization for App Group responses.
    - Manages background command intake as long as iOS genuinely schedules the app under `UIBackgroundModes audio`; avoids busy loops and makes no false claims about cold wake or background scheduling guarantees when suspended.
- **`Keyboard/`**: Custom `UIInputViewController` Keyboard Extension (`EcholetKeyboard`).
  - Note: Per Apple custom keyboard security requirements, custom keyboards have **no microphone access**. Containing `EcholetApp` exclusively owns audio capture.
  - Provides required Apple keyboard switching (`advanceToNextInputMode()`).
  - Dispatches validated v2 `KeyboardRequest` envelopes into App Group `UserDefaults` (`group.com.mainstayx.echolet.dev`).
  - Gated response consumer: safely correlates session ID, app epoch, sequence, and strictly monotonic response revision before invoking `textDocumentProxy.insertText`.
  - Honestly surfaces preparing, listening, and blocked microphone states without inserting fabricated transcripts.
  - Automatically fences and invalidates active session on input focus change (`textWillChange`).
- **`protocol/`**: Pure Foundation Swift wire protocol codec (`EcholetIPC.swift`), admission gate port (`EcholetAdmission.swift`), capture lifecycle gate (`CaptureLifecycleGate.swift`), and warm capture flow coordinator (`WarmCaptureFlow.swift`), mirroring canonical Rust `src/ios_ipc.rs`. Includes pure Swift CLI tests (`AdmissionTests.swift`, `AdversarialTests.swift`, `CaptureLifecycleTests.swift`, `IPCCodecSmoke.swift`, `WarmCaptureFlowTests.swift`).
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

---

## 2026-10-11 — J5: single-button keyboard -> auto app prepare -> one-tap stop + auto final insert (STATUS)

### What changed
- **Keyboard UI** (`Keyboard/KeyboardViewController.swift`): the four test
  buttons are gone. ONE prominent button now drives the whole flow — dark
  "Start Listening" while idle, red "Stop Listening" only AFTER a real
  `.listening` ACK for the exact session, neutral "Starting…/Finalizing…"
  while pending, plus the required Apple globe key. FINAL transcripts are
  inserted into the editable host EXACTLY ONCE automatically via
  `textDocumentProxy` (no manual Finish/Consume in primary UX).
- **Launch handshake** (`protocol/EcholetLaunch.swift`, pure Foundation):
  `echolet://dictation/prepare?nonce=<UUID>&ts=<ms>` deep link; App Group
  records — keyboard-written one-shot `DemandRecord` (nonce+TTL),
  app-written `PhaseRecord` (uriReceived/modelLoaded/micNativeReady/
  warmReady/failed, nonce- and process-epoch-fenced, TTL'd), and the
  app-written `WarmLiveRecord` (native hardware recording truth; the
  keyboard's authority for "positively fresh active warm mic").
- **Keyboard core** (`protocol/EcholetKeyboardSessionController.swift`,
  pure Foundation, CLI-testable): state machine ready -> starting ->
  listening -> finalizing -> ready/blocked; exact-session v2 START/STOP/
  CANCEL publication with fresh requestIds and sequence+1 stops; strict
  response correlation (epoch/session/ACK identity/monotonic revision);
  bounded polling/timedouts; auto-attempt at most ONCE per fresh keyboard
  appearance; user-tap attempts stay available; reopen-loop fence after a
  recent warmReady phase; editor-identity (document fingerprint) fail-closed
  insert gating; own insertion churn does NOT cancel the session.
- **App side** (`SceneDelegate.swift`, `WarmIPCService.swift`,
  `App/Info.plist`): cold-launch + `openURLContexts` entry points; the deep
  link is honored only for a fresh nonce-matched demand (fan-out/replay
  fails closed with a truthful failed phase); GUARANTEED ordering on the
  shared `ipcQueue`: arm mic -> offline model load -> warm standby prepare;
  micNativeReady/warmReady phases and the warm-live record are published
  ONLY from the confirmed native hardware start completion; bounded 20s
  activation timeout publishes an honest failed phase.
- **Auto return app -> keyboard**: iOS exposes NO public API to
  programmatically return to the host app's keyboard; the app logs
  `RETURN_ATTEMPT(auto_return_unsupported)` and the system "Back to previous
  app" / App Switcher UI is the explicit route. The keyboard auto-STARTs
  itself once the warm-live record is fresh, so no extra app tap is needed
  after the cold or warm prepare.

### Honest platform limits (no private APIs used)
- Keyboard extensions cannot legally open URLs with a guaranteed public
  API. The implementation uses only `NSExtensionContext.open(_:completionHandler:)`
  as a measured best effort and, if the OS refuses it (likely on current
  iOS), reports `AUTO_LAUNCH_UNSUPPORTED_*` and falls back to: user tap
  attempt + explicit manual-open instruction ("open Echolet from Home, then
  switch back; the keyboard finishes activation itself").
- No `LSApplicationWorkspace`, private selectors, hidden entitlements, or
  responder hacks; the keyboard never touches the microphone itself.

### Pure Foundation CLI tests (same pattern as prior jobs)
```bash
P=ios/protocol
swiftc $P/EcholetIPC.swift $P/EcholetAdmission.swift $P/EcholetLaunch.swift \
       $P/EcholetKeyboardSessionController.swift $P/EcholetLaunchTests.swift \
       -o /tmp/echolet_launch_tests && /tmp/echolet_launch_tests
swiftc $P/EcholetIPC.swift $P/EcholetAdmission.swift $P/EcholetLaunch.swift \
       $P/EcholetKeyboardSessionController.swift $P/EcholetKeyboardSessionTests.swift \
       -o /tmp/echolet_session_tests && /tmp/echolet_session_tests
```
Covered: auto-attempt-once narration, fresh warm skips open, tapped
fallback, recent-warmReady reopen fence, cold dead app stale epoch, warmReady
evidence auto-START once, permission blocked responses, exact STOP ack ->
insert exactly once (Unicode zh/en), stale/other/out-of-order/duplicate
response suppression, editor-mismatch fail-closed insert, focus-change
cancel, insert churn not cancelling, hide-cancel, rapid-tap dedupe, open
timeout -> AUTO_LAUNCH_UNSUPPORTED, start ack timeout.

### 2026-10-10 — J4A: iOS arm64 native ASR bridge feasibility probe (STATUS)

Native Rust↔Sherpa-onnx toolchain linkage for iPhoneOS (aarch64) is now wired
and verified at link/bundle level; product keyboard ASR remains future work.

### What exists now
- `Cargo.toml` / `src/lib.rs` / `src/{ui,paths,models}`: desktop-only modules
  (`actions`, `app`, `audio`, cpal/Slint/`dirs` …) are now gated to
  `any(linux|macos|windows)` instead of `not(android)`, so the iOS Rust build
  no longer imports desktop layers. `paths.rs` home-dir fallbacks also cover
  `ios`. Android/desktop semantics unchanged.
- `build.rs`: new `ios` branch links **static** archives
  (`libsherpa-onnx-c-api.a`) from `.local-runtime/ios-native/lib` with
  `-lc++`, Foundation/CoreFoundation/Accelerate frameworks. The macOS dylib,
  Android `.so`, Linux and Windows paths are untouched.
- `ios/ios-native` (new crate, `staticlib` for iPhoneOS only): thin C bridge
  `echolet_ios_probe_version`, `echolet_ios_recognizer_create`, `…stream_create`,
  `…stream_feed`, `…stream_read`, and the paired `…_destroy` functions — all
  backed by the real shared-core `echolet::asr::OnlineRecognizer` (real sherpa
  C API, panic-guarded, explicit ownership, NULL/length validation, no PCM
  persistence). Built via
  `cargo build --release --manifest-path ios/ios-native/Cargo.toml --target aarch64-apple-ios`.
- `ios/scripts/build-ios-native.sh`: stages and checksum-verifies the pinned
  official prebuilt device (ios-arm64) static xcframeworks —
  sherpa-onnx **v1.13.6** (`sherpa-onnx-v1.13.6-ios-static.xcframework.zip`,
  sha256 `0b8c880357e653af18c5f9c6e8b3c045e85b98403c00ff25692a1261d31aa332`,
  from the `xcframework` release tag referenced by the pinned repo's own
  Package.swift) and ORT **1.27.1** (`onnxruntime-ios-static-xcframework-1.27.1.xcframework.zip`,
  sha256 `985deaff345c7bcfbe4979b2daeec09d7a745b1e9cb73f37f4077364eb578e62`).
  Artifacts land in git-ignored `.local-runtime/ios-native/lib/`.
- `ios/probe/` (retired): initial standalone `EcholetNativeProbe` debug app used
  for J4A validation of static Rust/sherpa linking and offline model inference.
  Retired and removed once full ASR and warm mic support were integrated into `EcholetApp`.

### Status gates (as probed 2026-10-10, physical iPad mini 6, iPadOS 18.7.7, devicectl)
| Gate | Status |
|---|---|
| RUST_iOS_COMPILE (root core + bridge, aarch64-apple-ios) | PASS |
| SHERPA_IOS_NATIVE_LINK (real pinned sherpa/ORT static link, Mach-O arm64) | PASS |
| SWIFT_APP_FFI_LOAD (probe installs + launches via devicectl, FFI resolves) | PASS |
| REAL_OFFLINE_MODEL_INFERENCE (on-device, bundled model) | PASS (160850/160850 samples fed to the real recognizer via the bridge; true transcript of the official pinned test wav returned by the real C API, no fixture mocking) |
| iPAD_DEVICE_ASR (mic → transcript dictation) | BLOCKED (future work: audio capture → recognizer) |
| FULL_DICTATION_E2E (keyboard insertion) | BLOCKED (future work) |

On-device probe output (real recognizer, official sherpa test wav `0.wav`,
10.05 s of 16 kHz speech (160850 samples); no personal audio):
`ECHOLET_IOS_PROBE_BEGIN / version=echolet-ios-probe 0.1.0 /
recognizer_create_invalid_path=NULL_OK / real_model=LOADED /
feed_samples_approved=160850/160850 / read_rc=68 /
transcript=昨天是 monday， today is 礼拜二， the day after tomorrow 是 /
ECHOLET_IOS_PROBE_END`.

### Reproduce
```bash
./ios/scripts/build-ios-native.sh
cargo build --release --manifest-path ios/ios-native/Cargo.toml --target aarch64-apple-ios
cd ios/probe && xcodegen generate && xcodebuild -project EcholetNativeProbe.xcodeproj \
  -scheme EcholetNativeProbe -sdk iphoneos -destination 'generic/platform=iOS' build
xcrun devicectl device install app --device <UUID> <DerivedData>/…/EcholetNativeProbe.app
```
Note: no CI pipeline exists for iOS artifacts; nothing runs automatically.

### 2026-10-10 J4A 决策附录（decision appendix, J4A evidence pack）

实测环境（decision-relevant toolchain facts, all measured）:
- Host: Intel Mac, macOS 14.6.1, Xcode 16.2 (16C5032a), iPhoneOS SDK 18.2.
- Rust: rustc 1.99.0 (b940084d 2026-09-28), cargo 1.99.0,
  `aarch64-apple-ios` target installed. Note: rustc's LLVM 23 emits objects
  newer than the Xcode 16.2 toolchain's `nm` understands (harmless
  "Unknown attribute kind" banners both during probe and desktop builds).
- Device: iPad mini 6 (iPad14,1, iPadOS 18.7.7), reachable via
  `xcrun devicectl`, signed with team 668LTD8W55 (Apple Dev
  kevxu.cad@gmail.com, RJ3C3F7JFM, is the valid codesign identity; the
  free-team iOS Team Provisioning Profiles for `com.mainstayx.echolet.app{,.test,…}`
  expire 2026-10-17 — 7-day renewal churn applies to all sideload builds).

Pinned native distribution (channel = official prebuilt artifacts, no build
from source needed):
- sherpa-onnx source pin `1cb484af5e69d3c7803c1eb0b3b5ab8041e0e911`
  (= root repo's Android pin, v1.13.6), binary via the upstream
  iOS **static xcframework** (`sherpa-onnx-v1.13.6-ios-static.xcframework.zip`,
  sha256 `0b8c880357e653af18c5f9c6e8b3c045e85b98403c00ff25692a1261d31aa332`,
  published under the `xcframework` release tag and wired into the pinned
  source's own Package.swift — this is upstream's supported iOS channel).
  Device slice `ios-arm64` = static `ar` archive, 20 MB, all required
  `SherpaOnnx*` C-API symbols verified present; simulator slice also ships.
- onnxruntime-libs **v1.27.1** iOS static xcframework
  (sha256 `985deaff345c7bcfbe4979b2daeec09d7a745b1e9cb73f37f4077364eb578e62`),
  42 MB ar archive, `MinimumOSVersion 15.1` — **note: while our deployment
  target is iOS 15.0, ORT requires ≥15.1**; raise the app target when
  productizing. Also includes `_OrtSessionOptionsAppendExecutionProvider_CoreML`
  (CoreML EP available for later speed/latency experiments; current manifest
  pins `provider=cpu`).
- Model: `echolet-xasr-zh-en-480ms-689ff18c…` (X-ASR zh-en upstream r1,
  Apache-2.0, sha-verified; encoder 593 MB, dir 586 MB); licenses already in
  `licenses/` (sherpa-onnx MIT, onnxruntime, openmdw). Apache-2.0 etc. are
  attribution-compatible with the existing repo conventions.

Sizes/implications measured on device build:
- Final probe .app including model: 627 MB; framework statics total ≈103 MB
  (linker dead-strips unused, incl. desktop-ish code inside the 41 MB
  bridge archive — release `opt-level`/`panic=abort` tuning is available).
- iPad mini 6 has 4 GB RAM; a 593 MB float32 encoder for ASR + core
  viability (startup latency/thermal) is **unmeasured** — quantized/FP16
  variants of the X-ASR model were NOT attempted (would be a new frozen
  acquisition, outside J4A's no-new-model-download scope).

Open decisions the coordinator/user still needs to make (next gate):
1. **Model delivery channel for product**: bundle vs first-run on-demand
   install (repo already has a frozen-lock local installer pattern,
   `scripts/acquire-base-model.sh` + manifest sha validation) — probe proved
   recognition works from a bundle-folder reference; delivery UX is open.
2. **CoreML EP experiment** (`provider=cpu` → `coreml`) for latency on
   iPad: API is present, zero code exists; measure before promising speed.
3. **J4B mic wiring**: AVAudioEngine tap → `echolet_ios_stream_feed/read`
   streaming loop → WarmIPC partial/final → keyboard insertion; iOS forbids
   mic in the keyboard extension, so the App stays the audio owner. The
   shipped bridge API (feed/read/create/destroy, NULL+bounds checked,
   panic-guarded) was designed for exactly this call pattern.
4. **Signing/persistence**: free team profile expiry (2026-10-17) affects
   any long-lived sideload install; paid Developer Program or profile
   refresh cadence is a user/product choice, unchanged by this probe.

---

## 2026-10-11 — J4-E2E: on-device warm dictation vertical slice

### What was added (source only; no binaries/models committed)
- `ios/ios-native/src/lib.rs`: added lock-free bounded SPSC float ring FFI
  (`echolet_ios_ring_create/push/pop/clear/destroy/dropped_total`) and
  `echolet_ios_stream_flush` (trailing-silence tail flush → final text).
  Realtime contract: the AVAudioEngine tap calls ONLY `ring_push` (memset
  log2 memcpy + atomic release store; no allocation/locks/sync-queues/FFI
  inference).
- `ios/App/EcholetAsrService.swift`: app-side serial ASR worker.
  - `asrQueue` owns recognizer + every stream FFI call (create/feed/read/
    flush/destroy). Sessions are fenced against the exact capture
    generation; stale rings fail closed. The recognizer loads once and is
    destroyed only at stream teardown boundaries.
  - PCM path: tap → ring (lock-free) → asrQueue drain → `AVAudioConverter`
    native-rate → 16 kHz mono → stream_feed. Bounded overflow drops are
    logged. No PCM persistence, no network, transcripts only via delegate.
- `ios/App/AudioCaptureController.swift`: opt-in `startCapture(includePCM:)`
  wraps an allocation-free mono downmix + bounded ring push per tap. Meter
  semantics (J3A) untouched. `EcholetCapturePCMContext` lifetime matches the
  tap (scratch + ring allocated at start, freed at stop).
- `ios/App/WarmIPCService.swift` + `ios/protocol/WarmCaptureFlow.swift`:
  - New explicit `.warmStandby` capture owner: Prepare (foreground, user
    tap) → mic permission → ONE warm recorder runs; keyboard START adopts
    the same hardware via `adoptWarmStandbyAsr` (NO restart, no second tap);
    pre-START ring backlog is discarded at the ring boundary.
  - Keyboard STOP (Finish) acks the exact stop requestId/sequence and
    publishes the REAL final recognizer text via a single-writer provider;
    cancel/disarm/replacement paths dispose ASR without fabricating text.
  - Session replacement while warm hardware is live adopts the replacement
    from the same recorder (warm continuity).
- `ios/Keyboard/KeyboardViewController.swift`: Finish/Commit button (START
  seq+1, fresh requestId, session held for final transcript Consume).
- `ios/project.yml`: deployment target 15.1 (ORT requirement); EcholetApp
  links pinned `libecholet_ios.a` + `libonnxruntime.a` device static
  archives and bundles the frozen local `bilingual-zh-en` model folder as a
  non-repo folder resource. Keyboard stays native-lib-free.

### Rebuild + audit recipe (fresh checkout, no hidden steps)
1. `cargo build --release --manifest-path ios/ios-native/Cargo.toml --target aarch64-apple-ios`
2. `./ios/scripts/build-ios-native.sh` (stages + sha-verifies pinned sherpa
   v1.13.6 / ORT 1.27.1 device statics into `.local-runtime/ios-native/lib`)
3. Place the frozen `bilingual-zh-en` model dir at
   `.local-runtime/models/bilingual-zh-en` (manifest id field must match).
4. `cd ios && xcodegen generate`
5. `xcodebuild -project Echolet.xcodeproj -scheme EcholetApp -destination 'generic/platform=iOS' -derivedDataPath DerivedData build`
6. Pure Swift CLI protocol tests:
   `swiftc` each of `AdmissionTests`, `AdversarialTests`,
   `CaptureLifecycleTests`, `IPCCodecSmoke` (with the `fixtures` dir),
   `WarmCaptureFlowTests`, `EcholetLaunchTests`,
   `EcholetKeyboardSessionTests` with their matching protocol sources
   (same pattern as prior jobs) — all must exit 0.
7. Install on your own sideloaded iPad:
   `xcrun devicectl device install app --device <ID> .../EcholetApp.app`

### Manual acceptance (physical; human-owned) — J5 single-button flow
1. Echolet app: Prepare Warm Dictation (or let the keyboard open the app) →
   `WARM: warm dictation recording ACTIVE`.
2. In any editable host (Notes/Chrome), summon the Echolet keyboard via Globe.
3. AUTO path: keyboard reappearing over a fresh warm mic auto-sends START
   without opening the app; shows red "Stop Listening" only after the real
   mic/ASR ACK. Speak (zh/en), tap Stop once → final transcript inserts ONCE.
4. TAPPED path: if the keyboard could not open Echolet automatically, a tap
   attempts the public open; if the OS refuses, follow the on-instruction to
   open Echolet manually, return, and the keyboard auto-STARTs.
5. Verify: no reopen loop, no duplicate insertion, mic stops after Stop, and
   hide/focus change closes the session truthfully.

### Manual acceptance (physical; human-owned)
1. Open Echolet > "Load Offline Model" (or first Prepare) — state:
   `ASR: OFFLINE MODEL LOADED (on-device only)`.
2. Test A (app foreground): Start Live Dictation → speak (zh/en) → Stop &
   Finalize Live → REAL phrase must render in the ASR box and the editor.
3. Test B (warm keyboard): Prepare Warm Dictation → ACTIVE indicator →
   switch to Notes → Echolet keyboard → Start → speak → Finish → Consume
   Response → recognized transcript inserted ONCE.
4. Test C (cancel safety): cancel or rearm afterwards → mic stops (status
   `STOPPED`), no stale transcript reuse, no leftover recording session.
