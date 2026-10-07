#!/usr/bin/env bash
# Idempotent setup for the pinned Android SDK packages, Rust target, and cargo-ndk.
# JDK 17 and Android command-line tools must already be installed.
# This script does not install or rewrite Android Studio.

set -euo pipefail

script_dir=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=check-env.sh
source "$script_dir/check-env.sh"

android_root="$(android_dir)"

jdk_home=$(find_jdk17 || true)
if [ -z "$jdk_home" ]; then
  cat >&2 <<'EOF'
Blocker: JDK 17 is not installed.
bootstrap-dev.sh does not install JDKs. Install JDK 17 with the host package manager, then re-run android/scripts/bootstrap-dev.sh.
  macOS: brew install --cask corretto@17
  Debian/Ubuntu: sudo apt-get install openjdk-17-jdk
EOF
  exit 1
fi

export JAVA_HOME="$jdk_home"
export PATH="$JAVA_HOME/bin:$PATH"

sdk_root=$(find_sdk_root || true)
sdkmanager_bin=$(find_sdkmanager "${sdk_root:-}" || true)
if [ -z "$sdkmanager_bin" ]; then
  cat >&2 <<'EOF'
Blocker: Android command-line tools (sdkmanager) are not installed.
bootstrap-dev.sh does not install Android Studio or the command-line tools package.
  macOS: brew install --cask android-commandlinetools
Point ANDROID_HOME at the existing Android Studio SDK if one is already installed.
EOF
  exit 1
fi
if [ -z "${sdk_root:-}" ]; then
  echo "Blocker: sdkmanager is present but the Android SDK root could not be determined. Set ANDROID_HOME to the directory that contains cmdline-tools." >&2
  exit 1
fi

export ANDROID_HOME="$sdk_root"
unset ANDROID_SDK_ROOT || true

printf 'JAVA_HOME=%s\n' "$JAVA_HOME"
printf 'ANDROID_HOME=%s\n' "$ANDROID_HOME"
printf 'sdkmanager=%s\n' "$sdkmanager_bin"

accept_sdk_licenses() {
  local i
  set +e
  set +o pipefail
  {
    for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
      printf 'y\n'
    done
  } | "$sdkmanager_bin" --sdk_root="$sdk_root" --licenses >/dev/null 2>&1
  set -o pipefail
  set -e
}

ensure_package() {
  local pkg="$1" marker="$2"
  if [ -e "$marker" ]; then
    printf 'present: %s\n' "$pkg"
    return 0
  fi
  printf 'installing: %s\n' "$pkg"
  "$sdkmanager_bin" --sdk_root="$sdk_root" --install "$pkg"
  if [ ! -e "$marker" ]; then
    printf 'Blocker: sdkmanager finished but %s is still missing at %s\n' "$pkg" "$marker" >&2
    exit 1
  fi
}

ensure_platform() {
  local marker
  marker=$(platform_jar_path "$sdk_root" || true)
  if [ -n "$marker" ]; then
    printf 'present: Android SDK platform API 37 (%s)\n' "$marker"
    return 0
  fi
  printf 'installing: platforms;android-37\n'
  if ! "$sdkmanager_bin" --sdk_root="$sdk_root" --install "platforms;android-37"; then
    printf 'platforms;android-37 is not in this SDK repository; installing platforms;android-37.0\n'
    "$sdkmanager_bin" --sdk_root="$sdk_root" --install "platforms;android-37.0"
  fi
  marker=$(platform_jar_path "$sdk_root" || true)
  if [ -z "$marker" ]; then
    echo "Blocker: Android SDK platform API 37 is still missing after sdkmanager install." >&2
    exit 1
  fi
}

if [ -z "$(platform_jar_path "$sdk_root" || true)" ] \
  || [ ! -d "$sdk_root/build-tools/36.0.0" ] \
  || [ ! -d "$sdk_root/ndk/30.0.16248370/toolchains/llvm" ]; then
  accept_sdk_licenses
fi

ensure_platform
ensure_package "build-tools;36.0.0" "$sdk_root/build-tools/36.0.0"
ensure_package "ndk;30.0.16248370" "$sdk_root/ndk/30.0.16248370/toolchains/llvm"

if ! command -v rustup >/dev/null 2>&1; then
  echo "Blocker: rustup is not installed, so aarch64-linux-android cannot be added. Install rustup, then re-run android/scripts/bootstrap-dev.sh." >&2
  exit 1
fi
if rustup target list --installed | grep -qx 'aarch64-linux-android'; then
  echo "present: rust target aarch64-linux-android"
else
  rustup target add aarch64-linux-android
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "Blocker: cargo is not installed, so cargo-ndk cannot be installed. Install Rust via rustup, then re-run android/scripts/bootstrap-dev.sh." >&2
  exit 1
fi
if cargo ndk --version >/dev/null 2>&1; then
  printf 'present: %s\n' "$(cargo ndk --version)"
else
  cargo install cargo-ndk --locked
fi

props="$android_root/local.properties"
if [ -f "$props" ] && grep -q '^sdk\.dir=' "$props"; then
  printf 'present: %s sdk.dir\n' "$props"
else
  escaped=$(printf '%s\n' "$sdk_root" | sed 's/\\/\\\\/g; s/:/\\:/g')
  printf 'sdk.dir=%s\n' "$escaped" >> "$props"
  printf 'wrote: %s\n' "$props"
fi

echo "Bootstrap complete."
