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
#   1. Exact filename check inside the APK (zipinfo -1).
#   2. Every required .so is ELF64 AArch64 after extraction.
#   3. Packaged bytes compare byte-for-byte (cmp) with the staged files.
#   4. Staged libecholet_android.so exports the three JNI symbols and
#      neither staged nor packaged .so carries unexpected DT_NEEDED.
#
# Usage:
#   android/scripts/verify-apk-arm64.sh [apk-path]
#     (default: android/app/build/outputs/apk/debug/app-debug.apk)

set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"
android_root="$repo_root/android"

APK="${1:-$android_root/app/build/outputs/apk/debug/app-debug.apk}"
staged_root="$android_root/app/.native-jniLibs"
staged_lib="$staged_root/arm64-v8a"
required=(
  "libecholet_android.so"
  "libsherpa-onnx-c-api.so"
  "libonnxruntime.so"
)
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

# --- 1. Exact filename check inside the APK ----------------------------------
listing=$(zipinfo -1 "$APK")
lib_entries=$(printf '%s\n' "$listing" | grep '^lib/arm64-v8a/' || true)
[ -n "$lib_entries" ] || fail "APK contains no lib/arm64-v8a/ entries; the native jniLibs source set was not packaged."

for lib in "${required[@]}"; do
  if ! printf '%s\n' "$listing" | grep -qx "lib/arm64-v8a/$lib"; then
    fail "APK is missing lib/arm64-v8a/$lib (unexpected/partial packaging or stale source set)."
  fi
done

# Nothing unexpected beyond the staged set + optional packaged extras we allow.
for entry in $lib_entries; do
  case "$entry" in
    lib/arm64-v8a/libecholet_android.so|lib/arm64-v8a/libsherpa-onnx-c-api.so|lib/arm64-v8a/libonnxruntime.so|lib/arm64-v8a/libc++_shared.so) ;;
    *) fail "APK packages unexpected native entry $entry (unplanned native payload).";;
  esac
done

# --- 2/3. Extract in a unique temp dir; ELF + byte-identical checks -----------
work=$(mktemp -d "${TMPDIR:-/tmp}/echolet-apk-verify.XXXXXX")
cleanup() { rm -rf "$work"; }
trap cleanup EXIT

unzip -q "$APK" 'lib/arm64-v8a/*' -d "$work" || fail "cannot unzip native entries from $APK."

# NDK readelf when available (prefer PATH llvm-readelf, then the installed
# NDK's toolchain). Falling through to `file(1)` alone is NOT acceptable —
# ELF validation needs a real ELF parser when verifying arch/machine bits.
readelf_bin=""
for cand in \
  "$(command -v llvm-readelf 2>/dev/null || true)" \
  "$repo_root"/.local-runtime/android-native/*/toolchains/llvm/prebuilt/*/bin/llvm-readelf; do
  [ -n "$cand" ] && [ -x "$cand" ] && { readelf_bin="$cand"; break; }
done
if [ -z "$readelf_bin" ]; then
  for dir in /usr/local/share/android-commandlinetools/ndk/*/toolchains/llvm/prebuilt/*/bin; do
    [ -x "$dir/llvm-readelf" ] && { readelf_bin="$dir/llvm-readelf"; break; }
  done
fi

[ -x "$readelf_bin" ] || fail "no llvm-readelf available (install Android NDK 30.0.16248370)."
nm_bin="$(dirname "$readelf_bin")/llvm-nm"
[ -x "$nm_bin" ] || fail "no llvm-nm next to $readelf_bin."

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

if [ -f "$work/lib/arm64-v8a/libc++_shared.so" ]; then
  elf_arch_ok "$work/lib/arm64-v8a/libc++_shared.so"
  [ -f "$staged_lib/libc++_shared.so" ] \
    && cmp -s "$work/lib/arm64-v8a/libc++_shared.so" "$staged_lib/libc++_shared.so" \
    || { note "  libc++_shared.so packaged by the toolchain (not staged by build script) — ELF verified above"; }
else
  note "  libc++_shared.so not packaged (only allowed when nothing needs it):"
  case "$("$readelf_bin" -d "$staged_lib/libecholet_android.so" "$staged_lib/libsherpa-onnx-c-api.so" 2>/dev/null | grep -c 'libc++_shared.so' || true)" in
    0) note "    confirmed: no staged .so needs it." ;;
    *) fail "a staged .so DT_NEEDs libc++_shared.so but the APK lacks it." ;;
  esac
fi

# --- 4. Core symbols + DT_NEEDED sanity on the staged (== packaged) binaries --
exported=$("$nm_bin" -D --defined-only "$staged_lib/libecholet_android.so" 2>/dev/null || true)
for symbol in "${jni_symbols[@]}"; do
  printf '%s' "$exported" | grep -qE "[[:space:]]${symbol}$" \
    || fail "packaged libecholet_android.so does not export $symbol."
done

for so in "$staged_lib"/libecholet_android.so "$staged_lib"/libsherpa-onnx-c-api.so; do
  [ -f "$so" ] || fail "$(basename "$so") missing."
  needed=$("$readelf_bin" -d "$so" | grep -o 'lib[^]\[]*\.so[o]*' | sort -u || true)
  note "  DT_NEEDED of $(basename "$so"): $(echo $needed | tr '\n' ' ')"
  for dep in $needed; do
    case "$dep" in
      libdl.so|libc.so|libm.so|liblog.so|libandroid.so|libstdc++.so|libz.so) ;;
      libsherpa-onnx-c-api.so|libonnxruntime.so|libc++_shared.so) ;;
      *) fail "$(basename "$so") unexpectedly DT_NEEDs $dep (unplanned dependency)." ;;
    esac
  done
done

echo
note "PASS: APK packages the required arm64-v8a native set, byte-identical to the staged dir."
note "IDs the exact packaging path if you need to inspect: unzip -l $APK | grep arm64-v8a"
