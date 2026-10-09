#!/usr/bin/env bash
# APK verification gate for the Phase 0-A native slice (arm64-v8a).
#
# NEVER trust `assembleDebug` alone: this script fails nonzero unless the
# finished APK actually packages the required native set, and that the
# packaged bytes are IDENTICAL to the canonical staged set produced by
# build-native-arm64.sh (so a stale or wrong .so cannot pass silently).
#
# Checks, in order (every failure names the exact file):
#   0. APK exists; staging root exists and survived ANY gradlew clean.
#   1. Exact filename check inside the APK (zipinfo -1); no unplanned native
#      payload; every APK-local dependency name occurs EXACTLY once.
#   2. Every required .so is ELF64 AArch64 after extraction.
#   3. Packaged bytes compare byte-for-byte (cmp) with the staged files.
#   4. Staged libecholet_android.so exports the three JNI symbols.
#   5. Full ELF DT_NEEDED audit of ALL staged libraries (the byte-identical
#      packaged set: libecholet_android.so, libsherpa-onnx-c-api.so AND
#      libonnxruntime.so). Android API system libraries (libc.so, libm.so,
#      libdl.so, liblog.so, libandroid.so, libstdc++.so, libz.so) are device-
#      provided and never packaged; every OTHER DT_NEEDED name must be a
#      packaged, ELF64 AArch64 arm64-v8a library (missing / duplicate /
#      unexpected / wrong-architecture => FAIL, fail-closed).
#
# Toolchain discovery reuses the single read-only SDK resolver in
# check-env.sh (ANDROID_HOME, ANDROID_SDK_ROOT, android/local.properties,
# sdkmanager, Darwin/Linux defaults) — it does NOT duplicate or reimplement
# it — and the pinned NDK 30.0.16248370 toolchain under
# <sdk>/ndk/30.0.16248370/toolchains/llvm/prebuilt/<host-tag>/bin is what the
# ELF checks use. When several prebuilt host tags exist, the one matching the
# host architecture is preferred; the executables probed are the HOST
# llvm-readelf/llvm-nm (never an Android-target wrapper name). A PATH
# llvm-readelf/llvm-nm pair is accepted only as an explicitly reported
# fallback when no verified SDK/NDK pair is found, and nothing here mutates
# developer configuration or installs toolchains.
#
# Usage:
#   android/scripts/verify-apk-arm64.sh [apk-path]
#     (default: android/app/build/outputs/apk/debug/app-debug.apk)
# Test hook (self-check only; the default behaviour never changes):
#   VERIFY_APK_STAGED_ROOT <dir>   point the staging-root check elsewhere
#                                  (used by test-verify-apk-self-check.sh)

set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"
android_root="$repo_root/android"

APK="${1:-$android_root/app/build/outputs/apk/debug/app-debug.apk}"
staged_root="${VERIFY_APK_STAGED_ROOT:-$android_root/app/.native-jniLibs}"
staged_lib="$staged_root/arm64-v8a"
ndk_pin="30.0.16248370"
required=(
  "libecholet_android.so"
  "libsherpa-onnx-c-api.so"
  "libonnxruntime.so"
)
# Android platform-provided libraries: always present on the device, never
# staged or packaged.
system_needed="libdl.so libm.so libc.so liblog.so libandroid.so libstdc++.so libz.so"
jni_symbols=(
  "Java_com_mainstayx_echolet_NativeBridge_nativeOpen"
  "Java_com_mainstayx_echolet_NativeBridge_nativeFeed"
  "Java_com_mainstayx_echolet_NativeBridge_nativeClose"
)

fail() {
  printf 'Blocker: %s\n' "$*" >&2
  exit 1
}

note() {
  printf 'verify-apk: %s\n' "$*"
}

command -v unzip >/dev/null 2>&1 || fail "unzip is required but not found."
command -v zipinfo >/dev/null 2>&1 || fail "zipinfo is required but not found."

# --- 0. Inputs ----------------------------------------------------------------
[ -f "$APK" ] || fail "APK not found at $APK. Run '(cd android && ./gradlew clean :app:assembleDebug)' first."
[ -f "$staged_lib/libecholet_android.so" ] \
  || fail "canonical staging root $staged_lib lacks libecholet_android.so. Run android/scripts/build-native-arm64.sh first."
[ -f "$staged_lib/libsherpa-onnx-c-api.so" ] || fail "staged set incomplete: missing libsherpa-onnx-c-api.so under $staged_lib."
[ -f "$staged_lib/libonnxruntime.so" ] || fail "staged set incomplete: missing libonnxruntime.so under $staged_lib."

note "APK $APK"
ls -l "$APK" | sed 's/^/  /'
note "staged native root: $staged_root (outside app/build; survives gradlew clean)"

# --- 1. SDK/NDK + toolchain resolver (single authoritative discovery path) ----
# Source the read-only resolvers of check-env.sh; this runs none of its main()
# logic and no installer.
# shellcheck source=check-env.sh
. "$script_dir/check-env.sh"

sdk_root=$(find_sdk_root || true)
ndk=""
if [ -n "$sdk_root" ] && [ -d "$sdk_root/ndk/$ndk_pin/toolchains/llvm" ]; then
  ndk="$sdk_root/ndk/$ndk_pin"
fi

# Select <ndk>/toolchains/llvm/prebuilt/<host-tag>/bin: only bins that
# actually contain the host executables are candidates, and the prebuilt host
# tag matching the host architecture wins over anything else (never a random
# pick, never an Android-target wrapper binary).
select_toolchain_bin() {
  local ndk_root="$1" bin_dir host_arch primary="" fallback=""
  case "$(uname -s):$(uname -m)" in
    *[aA]rm64|*aarch64*) host_arch="aarch64" ;;
    *) host_arch="x86_64" ;;
  esac
  for bin_dir in "$ndk_root"/toolchains/llvm/prebuilt/*/bin; do
    [ -x "$bin_dir/llvm-readelf" ] || continue
    case "$bin_dir" in
      *"-$host_arch/bin") primary="$bin_dir" ;;
      *) [ -z "$fallback" ] && fallback="$bin_dir" ;;
    esac
  done
  printf '%s\n' "${primary:-$fallback}"
}

ndk_bin=""
if [ -n "$ndk" ]; then
  ndk_bin=$(select_toolchain_bin "$ndk" || true)
fi

if [ -n "$ndk_bin" ] && [ -x "$ndk_bin/llvm-readelf" ] && [ -x "$ndk_bin/llvm-nm" ]; then
  readelf_bin="$ndk_bin/llvm-readelf"
  nm_bin="$ndk_bin/llvm-nm"
  note "NDK $ndk_pin verified at $ndk; toolchain bin dir: $ndk_bin"
else
  # Explicit, documented fallback: PATH tools, only when the pinned NDK
  # toolchain pair could not be verified. Never the sole intended path.
  readelf_bin=$(command -v llvm-readelf 2>/dev/null || true)
  [ -n "$readelf_bin" ] && [ -x "$readelf_bin" ] \
    || fail "no verified NDK toolchain (SDK root candidates: ANDROID_HOME '${ANDROID_HOME:-unset}', ANDROID_SDK_ROOT '${ANDROID_SDK_ROOT:-unset}', android/local.properties, sdkmanager, default Darwin/Linux roots; resolved: '${sdk_root:-<none>}'; NDK pin searched: <SDK>/ndk/$ndk_pin) and no llvm-readelf on PATH. Install with: sdkmanager --sdk_root=<sdk> --install \"ndk;$ndk_pin\"."
  nm_bin="$(dirname "$readelf_bin")/llvm-nm"
  [ -x "$nm_bin" ] || nm_bin=$(command -v llvm-nm 2>/dev/null || true)
  [ -n "$nm_bin" ] && [ -x "$nm_bin" ] \
    || fail "llvm-readelf found on PATH at $readelf_bin but no llvm-nm next to it; install the matching LLVM tools or NDK $ndk_pin."
  note "FALLBACK in effect: using PATH LLVM tools ($readelf_bin) instead of the pinned NDK $ndk_pin toolchain (resolved SDK root: '${sdk_root:-<none>}')."
fi

# --- Exact filename check inside the APK --------------------------------------
listing=$(zipinfo -1 "$APK")
lib_entries=$(printf '%s\n' "$listing" | grep '^lib/arm64-v8a/' || true)
[ -n "$lib_entries" ] || fail "APK contains no lib/arm64-v8a/ entries; the native jniLibs source set was not packaged."

for lib in "${required[@]}"; do
  if ! printf '%s\n' "$listing" | grep -qx "lib/arm64-v8a/$lib"; then
    fail "APK is missing lib/arm64-v8a/$lib (unexpected/partial packaging or stale source set)."
  fi
  count=$(printf '%s\n' "$listing" | grep -cx "lib/arm64-v8a/$lib" || true)
  [ "$count" = "1" ] || fail "APK packages lib/arm64-v8a/$lib $count times (duplicate native entry is invalid)."
done

# Nothing unexpected beyond the staged set + the toolchain runtime we allow.
for entry in $lib_entries; do
  case "$entry" in
    lib/arm64-v8a/libecholet_android.so|lib/arm64-v8a/libsherpa-onnx-c-api.so|lib/arm64-v8a/libonnxruntime.so|lib/arm64-v8a/libc++_shared.so) ;;
    *) fail "APK packages unexpected native entry $entry (unplanned native payload).";;
  esac
done

# --- 2/3. Extract in a unique temp dir; ELF + byte-identity checks ------------
work=$(mktemp -d "${TMPDIR:-/tmp}/echolet-apk-verify.XXXXXX")
cleanup() { rm -rf "$work"; }
trap cleanup EXIT

unzip -q "$APK" 'lib/arm64-v8a/*' -d "$work" || fail "cannot unzip native entries from $APK."

elf_arch_ok() {
  local so="$1" name header
  name=$(basename "$so")
  [ -f "$so" ] || fail "$name missing at $so."
  header=$("$readelf_bin" -h "$so" 2>/dev/null || true)
  printf '%s' "$header" | grep -q "ELF64" || fail "$name ($name path: $so) is not ELF64."
  printf '%s' "$header" | grep -qi "Machine:.*AArch64" \
    || fail "$name ($so) is not AArch64."
}

note "ELF + byte-identity checks (packaged vs staged)"
for lib in "${required[@]}"; do
  [ -f "$work/lib/arm64-v8a/$lib" ] || fail "APK entry lib/arm64-v8a/$lib did not unzip (corrupt APK entry)."
  elf_arch_ok "$work/lib/arm64-v8a/$lib"
  elf_arch_ok "$staged_lib/$lib"
  if ! cmp -s "$work/lib/arm64-v8a/$lib" "$staged_lib/$lib"; then
    sha_pack=$(shasum -a 256 "$work/lib/arm64-v8a/$lib" | cut -d' ' -f1)
    sha_stag=$(shasum -a 256 "$staged_lib/$lib" | cut -d' ' -f1)
    fail "packaged lib/arm64-v8a/$lib differs from staged $staged_lib/$lib (packaged sha256 $sha_pack, staged $sha_stag); the APK contains a STALE or WRONG library."
  fi
  note "  byte-identical: lib/arm64-v8a/$lib"
done

# Any NON-required packaged entry (library runtime like libc++_shared.so) is
# still ELF-verified and, when staged, byte-compared.
if printf '%s\n' "$lib_entries" | grep -qx "lib/arm64-v8a/libc++_shared.so"; then
  elf_arch_ok "$work/lib/arm64-v8a/libc++_shared.so"
  if [ -f "$staged_lib/libc++_shared.so" ]; then
    cmp -s "$work/lib/arm64-v8a/libc++_shared.so" "$staged_lib/libc++_shared.so" \
      || fail "packaged lib/arm64-v8a/libc++_shared.so differs from staged $staged_lib/libc++_shared.so; the APK runtime is STALE or WRONG."
  else
    note "  libc++_shared.so packaged by the toolchain (not staged by build script) — ELF verified above"
  fi
fi

# --- 4. Core symbols ------------------------------------------------------------
exported=$("$nm_bin" -D --defined-only "$staged_lib/libecholet_android.so" 2>/dev/null || true)
for symbol in "${jni_symbols[@]}"; do
  printf '%s' "$exported" | grep -qE "[[:space:]]${symbol}$" \
    || fail "packaged libecholet_android.so does not export $symbol."
done

# --- 5. FULL DT_NEEDED audit of ALL staged libraries ---------------------------
# The staged set was just proven byte-identical to the APK payload, so the
# dependency map of the staged set IS the dependency map of the APK.
note "DT_NEEDED audit of the packaged native set (all ${#required[@]} libraries)"
for lib in "${required[@]}"; do
  needed=$("$readelf_bin" -d "$staged_lib/$lib" | grep -o 'lib[^]\[]*\.so[o]*' | sort -u || true)
  [ -n "$needed" ] || fail "no DT_NEEDED entries parsed from $lib (readelf output unusable)."
  note "  DT_NEEDED of $lib: $(echo $needed | tr '\n' ' ')"
  for dep in $needed; do
    case " $system_needed " in
      *" $dep "*) continue ;;
    esac
    # APK-local dependency: must be packaged in the APK exactly once and be
    # ELF64 AArch64. libecholet_android.so/libsherpa-onnx-c-api.so/
    # libonnxruntime.so are the planned cross-deps; anything else is unplanned.
    case "$dep" in
      libecholet_android.so|libsherpa-onnx-c-api.so|libonnxruntime.so|libc++_shared.so) ;;
      *) fail "$lib unexpectedly DT_NEEDs $dep (unplanned APK-local dependency; the staged set does not include it)." ;;
    esac
    dep_count=$(printf '%s\n' "$lib_entries" | grep -cx "lib/arm64-v8a/$dep" || true)
    [ "$dep_count" = "1" ] || fail "$lib DT_NEEDs lib/arm64-v8a/$dep but the APK packages it $dep_count times (the dependency must be packaged exactly once)."
    if [ "$dep" != "$lib" ]; then
      elf_arch_ok "$work/lib/arm64-v8a/$dep"
    fi
    if [ "$dep" = "libc++_shared.so" ]; then
      if [ -f "$staged_lib/$dep" ]; then
        cmp -s "$work/lib/arm64-v8a/$dep" "$staged_lib/$dep" \
          || fail "packaged lib/arm64-v8a/$dep differs from staged $staged_lib/$dep; pack the byte-identical file from the SAME pinned NDK."
      else
        fail "$lib DT_NEEDs libc++_shared.so but the build script did not stage it; copy the matching arm64-v8a libc++_shared.so from the SAME pinned NDK 30.0.16248370 into $staged_lib."
      fi
    fi
  done
done

echo
note "PASS: APK packages the required arm64-v8a native set, byte-identical to the staged dir."
note "IDs the exact packaging path if you need to inspect: unzip -l $APK | grep arm64-v8a"
