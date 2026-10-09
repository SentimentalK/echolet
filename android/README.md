# Echolet Android

Phase 0-A native feasibility slice: load the EXISTING shared Rust ASR core
(`OnlineRecognizer`, `SessionEngine`) over the pinned sherpa-onnx **C API** on
an arm64 Android device from a small diagnostic Activity. This is NOT the IME
yet — no `InputMethodService`, no `AudioRecord`, no editor injection, no cloud,
no user-facing model download. The next slice (Phase 0-B) connects
InputMethodService/AudioRecord/the current InputConnection.

The product split stays the same: Android owns IME lifecycle, permissions and
editor projection; shared Rust owns model, ASR, session and product semantics.
No Rust source is copied into `android/` — `android/native` is a tiny cdylib
(re-export bridge) depending on the root `echolet` crate by path.

## Pinned toolchain

| Tool | Version |
| --- | --- |
| JDK | 17 |
| Android Gradle Plugin | 9.4.0, built-in Kotlin |
| Gradle | 9.6.0 |
| compileSdk | 37 |
| targetSdk | 36 |
| minSdk | 26 |
| Build Tools | 36.0.0 |
| NDK | 30.0.16248370 (r30 LTS) |
| Rust Android target | `aarch64-linux-android` |
| Rust/NDK bridge | `cargo-ndk` |

Application id: `com.mainstayx.echolet`.

Kotlin is compiled by AGP's built-in Kotlin support. Do not apply
`org.jetbrains.kotlin.android`.

## Prepare this machine

JDK 17 and the Android command-line tools (`sdkmanager`) must already be
installed. `scripts/bootstrap-dev.sh` does not install Android Studio and does
not replace an existing SDK.

```sh
android/scripts/check-env.sh
android/scripts/bootstrap-dev.sh
android/scripts/check-env.sh
```

`check-env.sh` prints the detected JDK, SDK, Build Tools, NDK, adb, Rust target
and cargo-ndk, and exits non-zero with one actionable line per missing
capability.

### CMake for the native build

The pin needs CMake >= 3.24. If `cmake` is not on PATH, either run
`brew install cmake` (not handled by bootstrap-dev.sh) or place a portable
Kitware CMake under the ignored build cache:

```sh
mkdir -p .local-runtime/android-native
curl -L --fail -o .local-runtime/android-native/cmake.tar.gz \
  https://github.com/Kitware/CMake/releases/download/v3.31.6/cmake-3.31.6-macos-universal.tar.gz
tar xzf .local-runtime/android-native/cmake.tar.gz -C .local-runtime/android-native
rm .local-runtime/android-native/cmake.tar.gz
```

## Build + verify the APK (host, no device needed)

The canonical sequence of record MUST be run exactly like this — the staged
native set lives OUTSIDE `app/build` precisely so the earlier `clean` cannot
delete it, and the verification step is the gate that decides PASS:

```sh
android/scripts/build-native-arm64.sh                        # staged set + ELF audits
export JAVA_HOME="<jdk 17 home from check-env.sh>"
export ANDROID_HOME="<sdk path>"
(cd android && ./gradlew clean :app:assembleDebug)           # clean MUST NOT remove the staged set
android/scripts/verify-apk-arm64.sh                          # APK packaging gate (fail-closed)
```

Run the Gradle + verify pair TWICE to prove idempotence: `./gradlew clean`
deletes `app/build` but `app/.native-jniLibs` (gitignored, never committed)
survives, and the APK would fail to assemble if the source set were lost.

`verify-apk-arm64.sh` resolves its ELF tools through the SAME read-only SDK
discovery as `check-env.sh` — `ANDROID_HOME`/`ANDROID_SDK_ROOT`,
`android/local.properties`, `sdkmanager`, then the default Darwin/Linux roots
— and then, within the resolved SDK, the pinned
`ndk/30.0.16248370/toolchains/llvm/prebuilt/<host-tag>/bin`. When several
prebuilt host tags exist, the prebuilt matching the host architecture is
preferred. A `llvm-readelf`/`llvm-nm` pair on PATH is accepted only as an
explicitly reported fallback. If no verified SDK/NDK pair exists, the script
fails with the exact resolved/searched SDK root and NDK pin; it never installs
anything. Run it once without arguments to see the resolved paths:

```sh
android/scripts/test-verify-apk-self-check.sh   # fail-closed negative cases (synthetic fixtures)
```

This self-check exercises the gate's negative cases (missing APK, missing
staged `.so`, mismatched packaged bytes, wrong architecture, missing JNI
export) against header-only synthetic ELFs in a temp dir; it never touches the
real staged set or the developer's APK.

`verify-apk-arm64.sh` confirms, with hard nonzero failures:

* the APK packages `lib/arm64-v8a/libecholet_android.so`,
  `libsherpa-onnx-c-api.so` and `libonnxruntime.so` (exact names, each entry
  EXACTLY once, plus `libc++_shared.so` only when the ELF dependency check
  requires it);
* every packaged `.so` is ELF64 AArch64;
* the packaged bytes are byte-identical (cmp/sha256) to the staged set — a
  stale or wrong `.so` cannot pass, because the packaged bytes are re-validated
  against the staging root;
* the staged JNI lib exports the three
  `Java_com_…_NativeBridge_native{Open,Feed,Close}` symbols;
* the FULL `DT_NEEDED` dependency map of all three packaged libraries is
  audited: Android platform libraries (`libc.so`, `libm.so`, `libdl.so`,
  `liblog.so`, `libandroid.so`, `libstdc++.so`, `libz.so`) are device-provided
  and never packaged; every OTHER `DT_NEEDED` name must be a packaged
  arm64-v8a library (missing, duplicate, unplanned or wrong-architecture
  dependencies fail). `libc++_shared.so` is currently NOT required (`readelf
  -d` of the pinned set shows no dependency on it); if a future build needs
  it, the gate fails until `build-native-arm64.sh` stages the matching NDK
  file.

`./gradlew :app:assembleDebug` alone is NEVER reported as proof of packaging.

## Device run (real inference evidence)

```sh
$ANDROID_HOME/platform-tools/adb install -r android/app/build/outputs/apk/debug/app-debug.apk
android/scripts/stage-fixture.sh                             # gates ON device presence/ABI/app installed
$ANDROID_HOME/platform-tools/adb shell am start -n com.mainstayx.echolet/.MainActivity
```

The staging ORDER matters and is enforced by the scripts:

1. `build-native-arm64.sh` (staged set),
2. `./gradlew clean :app:assembleDebug`,
3. `verify-apk-arm64.sh` (APK verified),
4. **install** the APK with `adb install -r` — the app-specific external files
   directory (`…/Android/data/com.mainstayx.echolet/files`) only becomes
   usable after the app has been installed once,
5. `stage-fixture.sh` — it verifies, with raw `adb devices -l` evidence, that
   an authorized arm64-v8a device is attached AND `pm path com.mainstayx.echolet`
   finds the installed app, then pushes the pinned fixture,
6. launch MainActivity and press **Run offline ASR fixture**; the output must
   show a NONEMPTY recognized transcript — an empty transcript FAILS loudly
   (`FAILED: real decoding produced NO transcript…`), it is never reported OK,
7. press the button again: the second run must repeat the transcript from a
   fresh handle (no stale partials).

No device? `stage-fixture.sh` prints the exact `adb devices -l` output and
fails; the honest result is "APK verified, DEVICE INFERENCE UNVERIFIED" —
never Phase 0-A complete without a real phone.

## Native host tests (no fake ASR success)

```sh
cargo test --manifest-path android/native/Cargo.toml
# -> the real-model tests report as "ignored; … 8 ignored", never silently PASS
DYLD_LIBRARY_PATH="$PWD/.local-runtime/runtime/lib" \
  cargo test --release --manifest-path android/native/Cargo.toml -- --ignored --nocapture
# -> with the fixture staged via scripts/acquire-base-model.sh these run the
#    REAL pinned X-ASR model end to end and print the recognized transcript
```

A missing fixture is a validation blocker (`require_fixture()` panics with
the exact expected path and staging instructions), not a success.

What `build-native-arm64.sh` does, exactly:

1. Runs `check-env.sh` (read-only; no packages installed).
2. Clones `k2-fsa/sherpa-onnx` tag `v1.13.6` and FAILS unless the checkout is
   exactly commit `1cb484af5e69d3c7803c1eb0b3b5ab8041e0e911` (the desktop
   native runtime source of record).
3. Fetches the ONNX Runtime Android shared build paired with that tag
   (`onnxruntime-android-1.27.1`, via csukuangfj/onnxruntime-libs — the exact
   archive the upstream `build-android-arm64-v8a.sh` fetches).
4. Runs the UPSTREAM `build-android-arm64-v8a.sh` with
   `SHERPA_ONNX_ENABLE_C_API=ON SHERPA_ONNX_ENABLE_JNI=OFF
    SHERPA_ONNX_ENABLE_TTS=OFF SHERPA_ONNX_ENABLE_BINARY=OFF
    SHERPA_ONNX_ENABLE_SPEAKER_DIARIZATION=OFF SHERPA_ONNX_ENABLE_PORTAUDIO=OFF
    BUILD_SHARED_LIBS=ON SHERPA_ONNX_ANDROID_PLATFORM=android-26
    ANDROID_ABI=arm64-v8a`.
5. Audits the produced ELF files with the NDK tools: every staged `.so` must
   be ELF64 AArch64; `libsherpa-onnx-c-api.so` must export
   `SherpaOnnxCreateOnlineRecognizer`, `...OnlineStreamAcceptWaveform`,
   `...GetOnlineStreamResult` and related symbols; unexpected DT_NEEDED
   dependencies fail the stage.
6. Builds the shared Rust core + the `echolet_android` cdylib with
   `cargo ndk -t arm64-v8a -o android/app/.native-jniLibs build --release --manifest-path android/native/Cargo.toml`
   and `ECHOLET_NATIVE_LIB_DIR` pointing at the C API build output.
7. Audits `libecholet_android.so` for the three exported
   `Java_com_mainstayx_echolet_NativeBridge_*` symbols and re-verifies the
   whole staged set (minimum set: `libecholet_android.so`,
   `libsherpa-onnx-c-api.so`, `libonnxruntime.so`; `libc++_shared.so` is staged
   only if dependency inspection requires it).

The script reports actionable failures (wrong ELF arch, missing symbol,
missing transitive library) — never a silent success after a file copy. It is
also idempotent: pinned source and binaries live under the gitignored
`.local-runtime/android-native` and are reused across runs.

## JNI wire contract (stable input for Phase 0-B)

`NativeBridge.nativeFeed(handle, samples, sampleRate)` returns a UTF-8 JSON
ARRAY. `[]` means no progress. Events:

```json
{"kind":"partial","session":123,"revision":1,"backspaces":0,"suffix":"昨天是","text":"昨天是"}
```

```json
{"kind":"endpoint","session":123,"text":"<last admitted completed text>"}
```

- `partial` deltas are already ADMITTED by the shared `SessionEngine`
  (`accept_delivery` before return); unadmitted deltas are never returned.
- Apply `backspaces` pops, then `suffix`, **in Python-style CHARACTER units**
  (`src/diff.rs` semantics), never by UTF-8 byte length.
- `endpoint` is omitted when a feed produced no completed text.
- Handles are opaque increasing integers owned by the Rust runtime; `nativeClose`
  is idempotent and a late `nativeFeed` after close throws.
- The diagnostic Activity is NOT an InputConnection: Phase 0-B must re-verify
  editor binding (generation + contiguous-revision watermark) before applying
  deltas to the OS editor.

## Debug fixture

Model artifacts are validated on the HOST via the locked, hash-verified
acquisition path (`models/base-model.lock.json` +
`scripts/acquire-base-model.sh`); nothing is downloaded by or inside the app,
and the model is never packaged into the APK. `stage-fixture.sh` pushes the
verified fixture to the app-specific external files dir
(`…/Android/data/com.mainstayx.echolet/files/models/bilingual-zh-en`). No
device? The script prints the exact `adb devices -l` evidence and fails rather
than faking verification.

## Permissions

The manifest does not request `INTERNET`, `RECORD_AUDIO`, or any other
permission; it does not declare an input method or a foreground service.
