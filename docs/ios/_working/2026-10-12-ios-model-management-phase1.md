# 2026-10-12 — iOS Phase 1 canonical model management (delivery report)

Base SHA: `258ea901adcb78422a0e33991baacd559690b493` · Final SHA: `0d11deb74862c4e92ee46eb879c56b4fcf8882d0`
Fast-forward push `258ea90..0d11deb` → `origin/master`; worktree clean; no competing writer; CI green on the exact SHA.

## Files changed (2 scoped conventional commits)

Commit 1 `5804bb3` — feat(rust)
- `src/models/ios_surface.rs` (new): host-testable iOS projection surface — catalog snapshot JSON (canonical embedded registry verbatim), download-spec validation (unknown id / bundled / non-HTTPS / missing sha256 rejected), fail-closed persisted select (`select_model_persisted` rolls back the previous active id when the config save fails), registry-validated install (`install_model_archive_validated` — Rust is the only SHA256 authority). 10 unit tests embedded (catalog 4 entries/order, verified vs experimental compatibility, spec fields for all 4 models, unknown-id rejection incl. `path/../traversal`, SHA-mismatch leaves prior install intact + staging cleaned, valid-plain-tar fixture full verify→extract→commit→manager-discovery lifecycle, select persistence + manager restart, save-failure rollback, experimental rows read-only).
- `src/models/mod.rs` / `manager.rs`: `manager` cfg now includes `target_os = "ios"`; its `install_registry_model` (URL transport API) stays desktop-gated; `ios_surface` wired desktop+iOS. No desktop/Android behavior change.
- `ios/ios-native/src/probe/model.rs` (new) + `lib.rs` mount: iOS-only C ABI.

Commit 2 `0d11deb` — feat(ios)
- App: `EcholetModelStore.swift` (new), `EcholetModelDownloadCoordinator.swift` (new); `EcholetAsrService.swift` (active-dir mirror, switch barrier + mirrored switch truth, create-then-swap `swapRecognizer`, honest `no_model_installed` / `model_switch_in_progress` errors; bundle fallback DEBUG-only); `WarmIPCService` (+`hasPendingActivation` readout), `AudioCaptureController` (+recording readout), `EcholetHomeViewController` (grouped catalog card), `KeyboardSessionController` (targeted No-Model keyboard projection), `AudioCapture...` `Info.plist` mic description states real dictation; `project.yml` drops the `../.local-runtime/models/bilingual-zh-en` folder resource.
- Protocol (pure Foundation, CLI-tested): `EcholetModelGate.swift`, `EcholetModelSwitch.swift`, `EcholetModelDownloadPolicy.swift`, `EcholetModelError.swift`; presenter catalog rows + No-Model home state; tests `EcholetModelFlowTests.swift` + updated `EcholetHomePresenterTests.swift`.

## Rust FFI surface (ios/ios-native, iPhoneOS arm64 only)

All `extern "C"`, `catch_unwind` guarded (no panic crosses), NULL-safe:
- `echolet_ios_model_manager_new(user_models_dir, config_path) -> *mut EcholetIosModelManager`
- `echolet_ios_model_manager_free`, `echolet_ios_model_string_free` (all `*mut c_char` results are caller-owned, malloc'd NUL-terminated UTF-8, freed exactly once by `_string_free`; only pointers returned by this ABI)
- `echolet_ios_model_snapshot_json(handle) -> *mut c_char` — `{schema_version, default_model_id, models:[{id, display_name, version, languages, verification_status, download_size_bytes, installed_size_bytes, runtime_compatible, installed, installed_dir, active}]}`
- `echolet_ios_model_download_spec_json(handle, model_id) -> *mut c_char` — `{model_id,url,sha256,download_size_bytes,installed_size_bytes}`; HTTPS+sha256 enforced in Rust
- `echolet_ios_model_install_archive(handle, model_id, archive_path, progress_cb(user_data, phase:i32), user_data) -> 0/-1` — phase codes 0 Starting/1 Verifying/2 Extracting/3 Installing/4 Completed; registry id validated before ANY disk work; sha256 → detect-format → safe_unpack (rejects abs/`..`/links) → manifest validation → atomic commit with backup/restore; single in-flight per model; staging auto-cleanup
- `echolet_ios_model_select(handle, model_id) -> 0/-1` — fail-closed persisted (save failure ⇒ previous id restored)
- `echolet_ios_model_active_dir(handle) -> *mut c_char` / `echolet_ios_model_error_json(handle) -> *mut c_char` / `echolet_ios_model_refresh(handle) -> 0/-1`

## App path ownership / metadata truth
- Install root: `<AppSupport>/Echolet/models/<canonical-id>`; config `<AppSupport>/Echolet/config.json`; staging `<AppSupport>/Echolet/downloads`. All created at bootstrap, `isExcludedFromBackup = true`, App-private sandbox, never the .app bundle, never the keyboard container. Downloaded staging archives are deleted after install success AND failure (Rust installer also cleans `.echolet-staging-*`).
- No startup auto-download; Download only from the explicit CTA. Existing model dirs survive upgrades via discover-on-bootstrap (`ModelManager::new_with_paths` → `discover_installed` → `resolve_active_model`).
- Relaunch persistence: selected id in config.json; misconfigured/broken model dirs fall back without phantom states.

## State machine truth (tested)
`Not installed → Downloading % → Verifying → Installing → Installed → Selected/Loaded`; `Failed + Retry`. Download success never claims Installed before Rust verify+commit. Duplicate taps coalesce (policy + single-flight install + fenced switch epoch). Stale completions fenced by generation token.

## Compatibility claim
Only X‑ASR (`Echolet Verified`) is claimed iOS-runtime compatible. Kroko/Nemotron/Parakeet are visible, read-only, marked Experimental — no selection, no forced download. Keyboard keeps Start/Stop + backspace + Globe; START with no model yields the targeted blocked message "No model installed. Open Echolet and download the verified model first."

## Verification results
- `cargo test` (host): all green EXCEPT pre-existing local-env failure `test_state::test_optional_recognizer_lifecycle_unload_and_reload` — verified failing identically at baseline `258ea90` (user's real `~/.echolet/models` data auto-load assumption); unrelated to this change.
- Linux CI (both x86_64 + ARM64) SUCCESS at `0d11deb`; Android/macOS/Windows CI skipped by path filter.
- Swift CLI suites ALL PASS (macOS host, `swiftc` build+run): EcholetModelFlowTests (gate denials w/ honest guidance, switch happy path, persistence-failure and swap-failure rollback, duplicate fencing), updated EcholetHomePresenterTests (No-Model home, catalog rows, transfer states, experimental read-only), EcholetKeyboardSessionTests, EcholetLaunchTests, AdmissionTests, AdversarialTests, CaptureLifecycleGate tests, WarmCaptureFlow tests.
- Clean device build (Xcode 16.2, iPhoneOS arm64, from fresh checkout): `xcodegen generate && xcodebuild -project Echolet.xcodeproj -scheme EcholetApp -destination 'generic/platform=iOS' -derivedDataPath DerivedData build CODE_SIGNING_ALLOWED=NO` → BUILD SUCCEEDED **without `.local-runtime/models/bilingual-zh-en` involvement**; built `.app` contains zero `.onnx`/model bytes; all 5 new model FFI symbols exported in the app image; Keyboard extension has ZERO `echolet_ios*` symbols (never links the bridge/static libs).
- Real iPad download/install/mic gate: **HUMAN_GATE_UNVERIFIED** — steps:
  1. Install built app on iPad; expect No Model + one Download CTA (Prepare disabled).
  2. Tap Download on X-ASR (~552 MB) → observe Downloading %, Verifying, Installing, Installed, Selected.
  3. Relaunch — Selected persists; airplane mode ON; Prepare Warm → recognizer loads.
  4. Safari/Chrome text field → Globe → Echolet keyboard → Start: speak zh + en → live partials; Stop → single insertion; mic hardware stays on after Stop until Disarm.
  5. Model switch while listening must be denied with "Stop Listening … Disarm Warm Mic" guidance and must NOT stop dictation.

## Invariants honored
No navigation/IPC/keyboard-flow refactors, Android/desktop untouched (`models` UI unchanged), no new permissions (mic description text clarified only), no cloud ASR, no signing/tag/release work, no model bytes committed, no force-push, no [skip ci]. Phase 2 onboarding wizard explicitly NOT started.
