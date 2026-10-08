#!/usr/bin/env bash
# Builds the pinned sherpa-onnx C API + onnxruntime for arm64-v8a and the
# Echolet Android Rust cdylib (libecholet_android.so), staging the minimum
# .so set for the Gradle debug build.
#
# Idempotent: pinned source and the ONNX Runtime archive are cached under
# `.local-runtime/android-native` (gitignored). This script NEVER installs
# SDK/JDK packages (use scripts/bootstrap-dev.sh for that) and never commits
# native binaries.
#
# Command sequence of record:
#   1. android/scripts/check-env.sh                      (read-only env gate)
#   2. git clone k2-fsa/sherpa-onnx @ v1.13.6, pin 1cb484af5e69d3c7803c1eb0b3b5ab8041e0e911
#   3. fetch paired onnxruntime-android 1.27.1 zip (csukuangfj/onnxruntime-libs)
#   4. SHERPA_ONNX_ENABLE_C_API=ON SHERPA_ONNX_ENABLE_JNI=OFF ... upstream build
#      script build-android-arm64-v8a.sh
#   5. ELF audit: llvm-readelf/llvm-nm on libsherpa-onnx-c-api.so
#   6. cargo ndk -t arm64-v8a -o <staging> build --release
#      --manifest-path android/native/Cargo.toml (ECHOLET_NATIVE_LIB_DIR = C API dir)
#   7. ELF audit of libecholet_android.so + staged set validation

set -euo pipefail

SHERPA_TAG="v1.13.6"
SHERPA_SHA="1cb484af5e69d3c7803c1eb0b3b5ab8041e0e911"
SHERPA_URL="https://github.com/k2-fsa/sherpa-onnx"
# The ONNX Runtime Android shared build paired with this sherpa tag; the
# upstream build script downloads exactly this version when not provided.
ORT_VERSION="1.27.1"
ORT_ZIP_URL="https://github.com/csukuangfj/onnxruntime-libs/releases/download/v${ORT_VERSION}/onnxruntime-android-${ORT_VERSION}.zip"

script_dir="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"
android_root="$repo_root/android"
cache_root="$repo_root/.local-runtime/android-native"
staging="$android_root/app/build/generated/jniLibs"

fail() {
  printf 'Blocker: %s\n' "$*" >&2
  exit 1
}

# --- 1. Host environment (read-only check; no package installs here) --------
if ! "$script_dir/check-env.sh"; then
  fail "host toolchain incomplete; fix the lines printed above (bootstrap-dev.sh installs only SDK packages/rust target/cargo-ndk), then re-run this script."
fi

# Reproduce the same discovery check-env.sh performed.
# shellcheck source=check-env.sh
source "$script_dir/check-env.sh"
sdk_root=$(find_sdk_root || true)
[ -n "$sdk_root" ] || fail "Android SDK root not found (check-env.sh printed no SDK)."
ndk="$sdk_root/ndk/30.0.16248370"
[ -d "$ndk/toolchains/llvm" ] || fail "NDK 30.0.16248370 missing at $ndk."
export ANDROID_NDK="$ndk"
# cargo-ndk locates the toolchain via ANDROID_NDK_HOME/ANDROID_HOME too.
export ANDROID_NDK_HOME="$ndk"
export ANDROID_HOME="${ANDROID_HOME:-$sdk_root}"

adb_bin="$sdk_root/platform-tools/adb"

# Locates the NDK toolchain binary directory:
#   NDK >= r19: <ndk>/toolchains/llvm/prebuilt/<host-tag>/bin
find_ndk_bin_dir() {
  local ndk="$1" dir
  for dir in "$ndk"/toolchains/llvm/prebuilt/*/bin; do
    [ -x "$dir/llvm-readelf" ] && { printf '%s\n' "$dir"; return 0; }
  done
  printf '%s\n' ""
}

# CMake is required by the sherpa-onnx build. If cmake is not on PATH, use the
# portable Kitware build cached under .local-runtime/android-native (see
# android/README.md for the fetch command; no system install is performed).
if ! command -v cmake >/dev/null 2>&1; then
  portable_cmake="$cache_root/cmake-3.31.6-macos-universal/CMake.app/Contents/bin"
  [ -d "$portable_cmake" ] || fail "cmake is required but not found. Install cmake with the host package manager (macOS: brew install cmake) or place the portable CMake.app under $cache_root."
  export PATH="$portable_cmake:$PATH"
fi

# --- 2. Pinned sherpa-onnx source -------------------------------------------
sherpa_src="$cache_root/sherpa-onnx"
if [ ! -d "$sherpa_src/.git" ]; then
  mkdir -p "$cache_root"
  git clone --depth 1 --branch "$SHERPA_TAG" "$SHERPA_URL" "$sherpa_src" \
    || fail "cannot clone sherpa-onnx $SHERPA_TAG (network/cache issue)."
else
  # Keep an existing cache pinned even if it drifted.
  if [ "$(git -C "$sherpa_src" rev-parse HEAD)" != "$SHERPA_SHA" ]; then
    fail "$(git -C "$sherpa_src" rev-parse HEAD) in cache ($sherpa_src) is not the pinned $SHERPA_TAG source $SHERPA_SHA; remove $sherpa_src and re-run."
  fi
fi
if [ "$(git -C "$sherpa_src" rev-parse HEAD)" != "$SHERPA_SHA" ]; then
  fail "checked-out sherpa-onnx is $(git -C "$sherpa_src" rev-parse HEAD), expected $SHERPA_SHA."
fi

# --- 3. Paired ONNX Runtime Android shared build -----------------------------
ort_dist="$cache_root/onnxruntime-android-${ORT_VERSION}"
ort_zip="$cache_root/onnxruntime-android-${ORT_VERSION}.zip"
if [ ! -d "$ort_dist/jni/arm64-v8a" ]; then
  mkdir -p "$cache_root"
  if [ ! -f "$ort_zip" ]; then
    echo "--> fetching paired onnxruntime-android ${ORT_VERSION}"
    curl -L --fail --retry 3 --retry-delay 2 -o "$ort_zip.part" "$ORT_ZIP_URL" \
      || fail "cannot download $ORT_ZIP_URL"
    mv -f "$ort_zip.part" "$ort_zip"
  fi
  unzip -q -o "$ort_zip" -d "$ort_dist" || fail "cannot unpack $ort_zip"
fi
[ -f "$ort_dist/jni/arm64-v8a/libonnxruntime.so" ] || fail "fetched onnxruntime zip lacks jni/arm64-v8a/libonnxruntime.so."
export SHERPA_ONNX_ONNXRUNTIME_ROOT="$ort_dist"

# --- 4. Official sherpa-onnx arm64 C API build --------------------------------
install_lib="$cache_root/sherpa-onnx/build-android-arm64-v8a/install/lib"
if [ ! -f "$install_lib/libsherpa-onnx-c-api.so" ]; then
  echo "--> building sherpa-onnx $SHERPA_TAG C API for arm64-v8a (this takes a while)"
  (
    # The upstream script builds into $PWD/build-android-arm64-v8a and reads
    # CMakeLists.txt from the current directory: it must run from the source
    # root.
    cd "$sherpa_src"

    export SHERPA_ONNX_ENABLE_C_API=ON
    export SHERPA_ONNX_ENABLE_JNI=OFF
    export SHERPA_ONNX_ENABLE_TTS=OFF
    export SHERPA_ONNX_ENABLE_BINARY=OFF
    export SHERPA_ONNX_ENABLE_SPEAKER_DIARIZATION=OFF
    export SHERPA_ONNX_ANDROID_PLATFORM=android-26
    export BUILD_SHARED_LIBS=ON
    # The upstream script already keeps python/tests/check/pytest off for the
    # Android path and compiles with the NDK toolchain below.
    export SHERPA_ONNX_ENABLE_PORTAUDIO=OFF
    command bash build-android-arm64-v8a.sh
  ) || fail "sherpa-onnx arm64 build failed; see the CMake output above."
fi
[ -f "$install_lib/libsherpa-onnx-c-api.so" ] || fail "upstream script produced no libsherpa-onnx-c-api.so."
[ -f "$install_lib/libonnxruntime.so" ] || fail "upstream build did not copy libonnxruntime.so into $install_lib."

# --- 5. ELF/ABI audit of the produced C API ----------------------------------
ndk_bin="$(find_ndk_bin_dir "$ndk")"
[ -n "$ndk_bin" ] || fail "NDK toolchain bin dir (llvm-readelf/llvm-nm) not found under $ndk/toolchains/llvm/prebuilt/*."
readelf_bin="$ndk_bin/llvm-readelf"
nm_bin="$ndk_bin/llvm-nm"
[ -x "$readelf_bin" ] || fail "NDK llvm-readelf missing at $readelf_bin."
[ -x "$nm_bin" ] || fail "NDK llvm-nm missing at $nm_bin."

audit_elf_arch() {
  local so="$1" name header
  name=$(basename "$so")
  [ -f "$so" ] || fail "$name missing."
  # Capture BEFORE grepping: `grep -q` short-circuits and SIGPIPEs the
  # producer under `set -o pipefail`, flipping the pipeline status. Output
  # is small; capture is safe.
  header=$("$readelf_bin" -h "$so" 2>/dev/null || true)
  printf '%s' "$header" | grep -qi "Machine:.*AArch64" || fail "$name is not an AArch64 ELF (llvm-readelf -h output: $(printf '%s' "$header" | head -3)); the build produced a wrong-architecture binary."
  printf '%s' "$header" | grep -q "ELF64" || fail "$name is not ELF64."
}

require_exported_symbol() {
  local so="$1" symbol="$2" name exported
  name=$(basename "$so")
  [ -f "$so" ] || fail "$name missing."
  exported=$("$nm_bin" -D --defined-only "$so" 2>/dev/null || true)
  if ! printf '%s' "$exported" | grep -qE "[[:space:]]${symbol}$"; then
    fail "$name does not export $symbol; the pinned C API is ABI-incompatible with src/ffi.rs."
  fi
}

echo "--> auditing sherpa-onnx C API ELF"
for lib in libsherpa-onnx-c-api.so libonnxruntime.so; do
  audit_elf_arch "$install_lib/$lib"
done
for symbol in \
  SherpaOnnxCreateOnlineRecognizer \
  SherpaOnnxDestroyOnlineRecognizer \
  SherpaOnnxCreateOnlineStream \
  SherpaOnnxDestroyOnlineStream \
  SherpaOnnxOnlineStreamAcceptWaveform \
  SherpaOnnxIsOnlineStreamReady \
  SherpaOnnxDecodeOnlineStream \
  SherpaOnnxGetOnlineStreamResult \
  SherpaOnnxDestroyOnlineRecognizerResult \
  SherpaOnnxOnlineStreamIsEndpoint \
  SherpaOnnxOnlineStreamReset \
  SherpaOnnxOnlineStreamInputFinished \
  SherpaOnnxOnlineStreamSetOption; do
  require_exported_symbol "$install_lib/libsherpa-onnx-c-api.so" "$symbol"
done

# Dependency map: libsherpa-onnx-c-api.so must not need anything beyond what
# we stage (record every DT_NEEDED and stage libc++_shared.so only if asked).
c_api_needed=$("$readelf_bin" -d "$install_lib/libsherpa-onnx-c-api.so" \
  | grep -o 'lib[^]\[]*\.so[o]*' | sort -u || true)
for dep in $c_api_needed; do
  case "$dep" in
    libonnxruntime.so|libsherpa-onnx-c-api.so|libdl.so|libc.so|libm.so|liblog.so|libandroid.so|libc++_shared.so)
      # system libraries (always present on the device) or our planned set
      ;;
    *)
      fail "libsherpa-onnx-c-api.so unexpectedly depends on $dep; stage it too or report this dependency."
      ;;
  esac
done

# --- 6. Rust Android cdylib ---------------------------------------------------
echo "--> building echolet_android for aarch64-linux-android"
(
  cd "$repo_root"
  export ECHOLET_NATIVE_LIB_DIR="$install_lib"
  cargo ndk -t arm64-v8a -o "$staging" build --release --manifest-path android/native/Cargo.toml \
    || fail "cargo ndk build of echolet_android failed."
)

cdylib="$staging/arm64-v8a/libecholet_android.so"
[ -f "$cdylib" ] || fail "cargo ndk did not stage $cdylib."

# --- 7. Minimal staged set audit ----------------------------------------------
staged_lib="$android_root/app/build/generated/jniLibs/arm64-v8a"
mkdir -p "$staged_lib"
cp -f "$install_lib/libsherpa-onnx-c-api.so" "$install_lib/libonnxruntime.so" "$staged_lib/"

audit_elf_arch "$cdylib"
for symbol in \
  Java_com_mainstayx_echolet_NativeBridge_nativeOpen \
  Java_com_mainstayx_echolet_NativeBridge_nativeFeed \
  Java_com_mainstayx_echolet_NativeBridge_nativeClose; do
  require_exported_symbol "$cdylib" "$symbol"
done
for lib in "$staged_lib"/*.so; do
  audit_elf_arch "$lib"
done

# echolet_android must need exactly the staged native libs at run time.
own_needed=$("$readelf_bin" -d "$cdylib" | grep -o 'lib[^]\[]*\.so[o]*' | sort -u || true)
for dep in $own_needed; do
  case "$dep" in
    # System libraries present on every Android device: never staged.
    libdl.so|libc.so|libm.so|liblog.so|libandroid.so|libstdc++.so|libz.so)
      ;;
    # Third-party deps that MUST be inside the APK.
    libsherpa-onnx-c-api.so|libonnxruntime.so|libc++_shared.so)
      [ -f "$staged_lib/$dep" ] || fail "$dep is required by libecholet_android.so but was not staged."
      ;;
    *)
      fail "libecholet_android.so unexpectedly depends on $dep; stage it too or report this dependency."
      ;;
  esac
done

echo
echo "Staged arm64-v8a native set:"
ls -lh "$staged_lib"
echo "Native build complete. Next: cd android && ./gradlew :app:assembleDebug"
