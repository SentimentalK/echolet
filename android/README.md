# Echolet Android

Phase -1 bootstrap for local Android development. This module is a native Android shell that proves the pinned toolchain can produce a debug APK. It does not capture audio, expose an input method, or link the shared Rust core.

The product split stays the same: Android will own IME lifecycle, permissions, and editor projection. Shared Rust will own model, ASR, session, and product semantics.

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

Kotlin is compiled by AGP's built-in Kotlin support. Do not apply `org.jetbrains.kotlin.android`.

## Prepare this machine

JDK 17 and the Android command-line tools (`sdkmanager`) must already be installed. `scripts/bootstrap-dev.sh` does not install Android Studio and does not replace an existing SDK. Set `ANDROID_HOME` to the SDK you want to use, or let the scripts discover the Homebrew command-line tools SDK, an Android Studio SDK, or `sdk.dir` in the gitignored `local.properties`.

```sh
android/scripts/check-env.sh
android/scripts/bootstrap-dev.sh
android/scripts/check-env.sh
```

`check-env.sh` only prints the detected JDK, SDK, Build Tools, NDK, adb, Rust target, and cargo-ndk. It exits non-zero with one actionable line per missing capability.

`bootstrap-dev.sh` is idempotent. It installs only these missing SDK packages:

- `platforms;android-37` when that package id exists, otherwise `platforms;android-37.0` (the id current command-line tools publish for API 37)
- `build-tools;36.0.0`
- `ndk;30.0.16248370`

It also adds the `aarch64-linux-android` rustup target and installs `cargo-ndk` with `cargo install --locked` when those are missing. `adb` comes from Android platform-tools (`sdkmanager --install "platform-tools"`), which is a host prerequisite rather than one of the three packages above.

Export the JDK 17 home printed by `check-env.sh` before Gradle if the shell default is a different JDK:

```sh
export JAVA_HOME="<jdk 17 home from check-env.sh>"
export ANDROID_HOME="<sdk path from check-env.sh>"
cd android
./gradlew clean :app:assembleDebug
```

The debug APK is `android/app/build/outputs/apk/debug/app-debug.apk`. `local.properties` is generated locally and is not committed.

## What this app is

`MainActivity` shows the text `Echolet Android bootstrap`. The manifest does not request `INTERNET`, `RECORD_AUDIO`, or any other permission, and it does not declare an input method or foreground service.
