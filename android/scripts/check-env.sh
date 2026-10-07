#!/usr/bin/env bash
# Read-only check for the Echolet Android host toolchain.
# This script does not install packages or write files.

set -u

errors=""

add_error() {
  errors="${errors}- $1"$'\n'
}

android_dir() {
  (
    cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd
  )
}

resolve_link() {
  local path="$1" target dir
  while [ -L "$path" ]; do
    target=$(readlink "$path") || break
    case "$target" in
      /*) path="$target" ;;
      *)
        dir=$(cd "$(dirname "$path")" && pwd)
        path="$dir/$target"
        ;;
    esac
  done
  if [ -d "$path" ]; then
    (cd "$path" && pwd)
  else
    dir=$(cd "$(dirname "$path")" && pwd)
    printf '%s/%s\n' "$dir" "$(basename "$path")"
  fi
}

java_major_from_bin() {
  local bin="$1" line major
  line=$("$bin" -version 2>&1 | head -n 1 || true)
  major=$(printf '%s\n' "$line" | sed -n 's/.*version "\([0-9][0-9]*\).*/\1/p')
  if [ -z "$major" ]; then
    major=$(printf '%s\n' "$line" | sed -n 's/.*version "1\.\([0-9][0-9]*\).*/\1/p')
  fi
  printf '%s\n' "$major"
}

consider_java_home() {
  local home="$1" bin
  if [ -z "$home" ] || [ ! -d "$home" ]; then
    return 1
  fi
  if [ -x "$home/bin/java" ]; then
    bin="$home/bin/java"
  elif [ -x "$home/libexec/openjdk.jdk/Contents/Home/bin/java" ]; then
    home="$home/libexec/openjdk.jdk/Contents/Home"
    bin="$home/bin/java"
  else
    return 1
  fi
  if [ "$(java_major_from_bin "$bin")" = "17" ]; then
    printf '%s\n' "$home"
    return 0
  fi
  return 1
}

find_jdk17() {
  local home candidate prefix dir
  if [ -n "${JAVA_HOME:-}" ]; then
    home=$(consider_java_home "$JAVA_HOME" || true)
    if [ -n "$home" ]; then
      printf '%s\n' "$home"
      return 0
    fi
  fi
  if [ "$(uname -s)" = "Darwin" ] && [ -x /usr/libexec/java_home ]; then
    candidate=$(/usr/libexec/java_home -v 17 2>/dev/null || true)
    home=$(consider_java_home "$candidate" || true)
    if [ -n "$home" ]; then
      printf '%s\n' "$home"
      return 0
    fi
  fi
  if command -v brew >/dev/null 2>&1; then
    prefix=$(brew --prefix openjdk@17 2>/dev/null || true)
    home=$(consider_java_home "$prefix" || true)
    if [ -n "$home" ]; then
      printf '%s\n' "$home"
      return 0
    fi
  fi
  if command -v java >/dev/null 2>&1; then
    candidate=$(resolve_link "$(command -v java)")
    candidate=$(dirname "$(dirname "$candidate")")
    home=$(consider_java_home "$candidate" || true)
    if [ -n "$home" ]; then
      printf '%s\n' "$home"
      return 0
    fi
  fi
  if [ -d /usr/lib/jvm ]; then
    for dir in /usr/lib/jvm/*; do
      case "$(basename "$dir")" in
        *17*)
          home=$(consider_java_home "$dir" || true)
          if [ -n "$home" ]; then
            printf '%s\n' "$home"
            return 0
          fi
          ;;
      esac
    done
  fi
  if [ "$(uname -s)" = "Darwin" ]; then
    for dir in /Library/Java/JavaVirtualMachines/*.jdk "$HOME/Library/Java/JavaVirtualMachines"/*.jdk; do
      if [ -d "$dir/Contents/Home" ]; then
        home=$(consider_java_home "$dir/Contents/Home" || true)
        if [ -n "$home" ]; then
          printf '%s\n' "$home"
          return 0
        fi
      fi
    done
  fi
  return 1
}

read_local_sdk() {
  local file line value
  file="$(android_dir)/local.properties"
  [ -f "$file" ] || return 1
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      sdk.dir=*)
        value=${line#sdk.dir=}
        value=$(printf '%s\n' "$value" | sed 's/\\:/:/g; s/\\\\/\\/g')
        if [ -d "$value" ]; then
          printf '%s\n' "$value"
          return 0
        fi
        ;;
    esac
  done < "$file"
  return 1
}

is_sdk_root() {
  local root="$1"
  [ -n "$root" ] && [ -d "$root" ] || return 1
  [ -d "$root/cmdline-tools" ] || [ -d "$root/platforms" ] || [ -d "$root/platform-tools" ] || [ -d "$root/ndk" ]
}

sdk_root_from_manager_bin() {
  local dir
  dir=$(dirname "$1")
  dir=$(dirname "$dir")
  if [ "$(basename "$dir")" = "latest" ]; then
    dir=$(dirname "$dir")
  fi
  if [ "$(basename "$dir")" = "cmdline-tools" ]; then
    dirname "$dir"
    return 0
  fi
  return 1
}

find_sdk_root() {
  local candidate prefix c
  local extra=()
  if is_sdk_root "${ANDROID_HOME:-}"; then
    printf '%s\n' "$ANDROID_HOME"
    return 0
  fi
  if is_sdk_root "${ANDROID_SDK_ROOT:-}"; then
    printf '%s\n' "$ANDROID_SDK_ROOT"
    return 0
  fi
  candidate=$(read_local_sdk || true)
  if is_sdk_root "$candidate"; then
    printf '%s\n' "$candidate"
    return 0
  fi
  if command -v sdkmanager >/dev/null 2>&1; then
    candidate=$(sdk_root_from_manager_bin "$(resolve_link "$(command -v sdkmanager)")" || true)
    if is_sdk_root "$candidate"; then
      printf '%s\n' "$candidate"
      return 0
    fi
  fi
  if [ "$(uname -s)" = "Darwin" ]; then
    extra+=("$HOME/Library/Android/sdk")
  else
    extra+=(
      "$HOME/Android/Sdk"
      "/opt/android-sdk"
      "/usr/lib/android-sdk"
      "/usr/local/lib/android/sdk"
    )
  fi
  if command -v brew >/dev/null 2>&1; then
    prefix=$(brew --prefix 2>/dev/null || true)
    if [ -n "$prefix" ]; then
      extra+=("$prefix/share/android-commandlinetools")
    fi
  fi
  extra+=("/usr/local/share/android-commandlinetools")
  for c in "${extra[@]}"; do
    if is_sdk_root "$c"; then
      printf '%s\n' "$c"
      return 0
    fi
  done
  return 1
}

find_sdkmanager() {
  local root="${1:-}" candidate
  if [ -n "$root" ]; then
    for candidate in \
      "$root/cmdline-tools/latest/bin/sdkmanager" \
      "$root/cmdline-tools/bin/sdkmanager"
    do
      if [ -x "$candidate" ]; then
        printf '%s\n' "$candidate"
        return 0
      fi
    done
  fi
  if command -v sdkmanager >/dev/null 2>&1; then
    command -v sdkmanager
    return 0
  fi
  return 1
}

pkg_revision() {
  local file="$1"
  [ -f "$file" ] || return 1
  sed -n 's/^Pkg.Revision *= *//p' "$file" | head -n 1
}

# Current cmdline-tools publish API 37 as platforms;android-37.0, installed at
# platforms/android-37.0. Older repositories used platforms;android-37.
platform_jar_path() {
  local root="$1"
  if [ -f "$root/platforms/android-37/android.jar" ]; then
    printf '%s\n' "$root/platforms/android-37/android.jar"
    return 0
  fi
  if [ -f "$root/platforms/android-37.0/android.jar" ]; then
    printf '%s\n' "$root/platforms/android-37.0/android.jar"
    return 0
  fi
  return 1
}

main() {
  local jdk_home java_bin sdk_root sdkmanager_bin revision adb_bin rustup_bin platform_jar

  printf 'Echolet Android environment\n\n'

  jdk_home=$(find_jdk17 || true)
  if [ -n "$jdk_home" ]; then
    java_bin="$jdk_home/bin/java"
    printf 'Java:\n'
    "$java_bin" -version 2>&1 | sed 's/^/  /'
    printf '  home: %s\n' "$jdk_home"
  else
    printf 'Java: missing\n'
    add_error "JDK 17 not found. Install it with the host package manager (macOS: brew install --cask corretto@17; Debian/Ubuntu: sudo apt-get install openjdk-17-jdk), then re-run android/scripts/check-env.sh."
  fi

  sdk_root=$(find_sdk_root || true)
  sdkmanager_bin=$(find_sdkmanager "$sdk_root" || true)
  if [ -n "$sdkmanager_bin" ]; then
    revision=""
    if [ -n "$sdk_root" ]; then
      revision=$(pkg_revision "$sdk_root/cmdline-tools/latest/source.properties" || true)
    fi
    printf 'sdkmanager: %s\n' "$sdkmanager_bin"
    if [ -n "$revision" ]; then
      printf '  cmdline-tools revision: %s\n' "$revision"
    fi
  else
    printf 'sdkmanager: missing\n'
    add_error "Android sdkmanager not found. Install Android command-line tools (macOS: brew install --cask android-commandlinetools). If Android Studio is already installed, set ANDROID_HOME to its SDK instead of installing a second copy."
  fi

  if [ -n "$sdk_root" ]; then
    printf 'Android SDK: %s\n' "$sdk_root"
    platform_jar=$(platform_jar_path "$sdk_root" || true)
    if [ -n "$platform_jar" ]; then
      printf 'Platform android-37: %s\n' "$platform_jar"
    else
      printf 'Platform android-37: missing\n'
      add_error "Android SDK platform android-37 is missing. Current command-line tools publish this API as platforms;android-37.0. Run android/scripts/bootstrap-dev.sh or: sdkmanager --sdk_root=\"$sdk_root\" --install \"platforms;android-37.0\"."
    fi
    if [ -d "$sdk_root/build-tools/36.0.0" ]; then
      revision=$(pkg_revision "$sdk_root/build-tools/36.0.0/source.properties" || true)
      printf 'Build-Tools 36.0.0: %s' "$sdk_root/build-tools/36.0.0"
      if [ -n "$revision" ]; then
        printf ' (Pkg.Revision %s)' "$revision"
      fi
      printf '\n'
    else
      printf 'Build-Tools 36.0.0: missing\n'
      add_error "Android SDK Build-Tools 36.0.0 are missing. Run android/scripts/bootstrap-dev.sh or: sdkmanager --sdk_root=\"$sdk_root\" --install \"build-tools;36.0.0\"."
    fi
    if [ -d "$sdk_root/ndk/30.0.16248370/toolchains/llvm" ]; then
      revision=$(pkg_revision "$sdk_root/ndk/30.0.16248370/source.properties" || true)
      printf 'NDK 30.0.16248370: %s' "$sdk_root/ndk/30.0.16248370"
      if [ -n "$revision" ]; then
        printf ' (Pkg.Revision %s)' "$revision"
      fi
      printf '\n'
    else
      printf 'NDK 30.0.16248370: missing\n'
      add_error "Android NDK 30.0.16248370 is missing. Run android/scripts/bootstrap-dev.sh or: sdkmanager --sdk_root=\"$sdk_root\" --install \"ndk;30.0.16248370\"."
    fi
  else
    printf 'Android SDK: missing\n'
    add_error "Android SDK root was not found. Set ANDROID_HOME to the directory that contains cmdline-tools (Homebrew default: \"\$(brew --prefix)/share/android-commandlinetools\", Android Studio default on macOS: \"\$HOME/Library/Android/sdk\", Linux: \"\$HOME/Android/Sdk\")."
  fi

  adb_bin=""
  if command -v adb >/dev/null 2>&1; then
    adb_bin=$(command -v adb)
  elif [ -n "$sdk_root" ] && [ -x "$sdk_root/platform-tools/adb" ]; then
    adb_bin="$sdk_root/platform-tools/adb"
  fi
  if [ -n "$adb_bin" ]; then
    printf 'adb: %s\n' "$adb_bin"
    "$adb_bin" version 2>&1 | sed 's/^/  /'
  else
    printf 'adb: missing\n'
    add_error "adb was not found on PATH or under the SDK platform-tools directory. Install it with: sdkmanager --install \"platform-tools\" and ensure platform-tools is on PATH."
  fi

  if ! command -v rustup >/dev/null 2>&1; then
    printf 'Rust target aarch64-linux-android: rustup missing\n'
    add_error "rustup is not installed, so aarch64-linux-android cannot be verified. Install rustup, then run: rustup target add aarch64-linux-android"
  elif rustup target list --installed 2>/dev/null | grep -qx 'aarch64-linux-android'; then
    printf 'Rust target aarch64-linux-android: installed\n'
    if command -v rustup >/dev/null 2>&1; then
      rustup_bin=$(command -v rustup)
      printf '  rustup: %s\n' "$rustup_bin"
    fi
  else
    printf 'Rust target aarch64-linux-android: missing\n'
    add_error "Rust target aarch64-linux-android is not installed. Run: rustup target add aarch64-linux-android"
  fi

  if ! command -v cargo >/dev/null 2>&1; then
    printf 'cargo-ndk: cargo missing\n'
    add_error "cargo is not installed, so cargo-ndk cannot be verified. Install Rust via rustup, then run: cargo install cargo-ndk --locked"
  elif cargo ndk --version >/dev/null 2>&1; then
    printf 'cargo-ndk: %s\n' "$(cargo ndk --version)"
  else
    printf 'cargo-ndk: missing\n'
    add_error "cargo-ndk is not installed. Run: cargo install cargo-ndk --locked"
  fi

  if [ -n "$errors" ]; then
    printf '\nMissing required capabilities:\n%s' "$errors"
    exit 1
  fi
  printf '\nAll required capabilities are present.\n'
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  main "$@"
fi
