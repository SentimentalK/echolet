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

## Build + run the native slice

```sh
android/scripts/build-native-arm64.sh   # pinned source -> ELF-audited .so set + cargo ndk build
export JAVA_HOME="<jdk 17 home from check-env.sh>"
export ANDROID_HOME="<sdk path>"
cd android && ./gradlew clean :app:assembleDebug && cd ..
android/scripts/stage-fixture.sh        # stages model fixture to the device (needs adb device)
$ANDROID_HOME/platform-tools/adb install -r android/app/build/outputs/apk/debug/app-debug.apk
$ANDROID_HOME/platform-tools/adb shell am start -n com.mainstayx.echolet/.MainActivity
# press "Run offline ASR fixture", then
$ANDROID_HOME/platform-tools/adb logcat -d
```

Then press **Run offline ASR fixture**. Recognized partial/final transcript is
shown in the output area.

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
   `cargo ndk -t arm64-v8a -o android/app/build/generated/jniLibs build --release --manifest-path android/native/Cargo.toml`
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
