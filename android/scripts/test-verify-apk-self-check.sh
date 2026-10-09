#!/usr/bin/env bash
# Focused self-check for verify-apk-arm64.sh: exercises its FAIL-CLOSED
# negative cases against SYNTHETIC fixtures (header-only ELF64 files and a
# synthetic zip), so neither the real staged .so set nor the developer's real
# APK is ever touched, and nothing is left behind (all fixtures live in a
# unique temp dir removed on exit).
#
# Negative cases asserted (each must exit NONZERO and name its blocker):
#   1. missing APK
#   2. staged set incomplete (a required .so missing from staging)
#   3. packaged bytes differ from the staged set (stale/wrong library)
#   4. wrong architecture staged/packaged
#   5. required export missing (synthetic set with no JNI symbols, and the
#      DT_NEEDED gate right behind it, must fail — never "pass")
#
# The positive path is NOT faked here: synthetic headers have no dynamic
# sections or symbols on purpose, so a synthetic "PASS" would prove nothing.
# The positive path is proven by the real command of record in
# android/README.md (real staged set + Gradle-built APK).
#
# Usage: android/scripts/test-verify-apk-self-check.sh [path/to/llvm-readelf]
# With an argument the NDK readelf/nm pair used by the probes is echoed for
# the record; without one the verify script resolves it itself.

set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
verify="$script_dir/verify-apk-arm64.sh"

work="$(mktemp -d "${TMPDIR:-/tmp}/echolet-verify-selfcheck.XXXXXX")"
cleanup() { rm -rf "$work"; }
trap cleanup EXIT

fail() {
  printf 'selfcheck Blocker: %s\n' "$1" >&2
  exit 1
}

note() {
  printf 'selfcheck: %s\n' "$1"
}

if [ -n "${1:-}" ]; then
  [ -x "$1" ] || fail "$1 is not executable."
  printf 'probe readelf: %s\n' "$1"
  printf 'probe nm: %s\n' "$(dirname "$1")/llvm-nm"
fi

# Minimal 64-byte ELF64 header; e_machine is the only probed field
# (AArch64 = 0xB7, x86-64 = 0x3E).
emit_fake_elf() {
  local out="$1" machine_hex="$2"
  printf '\x7fELF\x02\x01\x01\x00' > "$out"
  head -c 8 /dev/zero >> "$out"
  printf '\x03\x00' >> "$out"
  printf "\x$machine_hex\x00" >> "$out"
  printf '\x01\x00\x00\x00' >> "$out"
  head -c 28 /dev/zero >> "$out"
  printf '\x40\x00\x38\x00\x00\x00\x40\x00\x00\x00\x00\x00' >> "$out"
  [ "$(wc -c < "$out" | tr -d ' ')" = "64" ] || fail "fake ELF header size is wrong for $out"
}

build_synthetic_pair() {
  # $1 = fixture dir; $2 = machine hex ("b7" or "3e"); $3 = "" or "mutate";
  # $4 = extra staged-lib name to omit ("" = keep all three);
  # $5 = "nozip" to skip building the APK (missing-APK case).
  local dir="$1" machine_hex="$2" mutate="$3" omit="$4" nozip="${5:-}" lib
  mkdir -p "$dir/staging/arm64-v8a" "$dir/apk/lib/arm64-v8a"
  for lib in libecholet_android.so libsherpa-onnx-c-api.so libonnxruntime.so; do
    if [ "$lib" != "$omit" ]; then
      emit_fake_elf "$dir/staging/arm64-v8a/$lib" "$machine_hex"
    else
      # The omitted lib is NOT staged here but IS packaged in the APK below.
      emit_fake_elf "$dir/apk/lib/arm64-v8a/$lib" "$machine_hex"
      continue
    fi
    if [ -n "$mutate" ] && [ "$lib" = "libecholet_android.so" ]; then
      {
        head -c 24 "$dir/staging/arm64-v8a/$lib"
        printf '\x02'
        tail -c 39 "$dir/staging/arm64-v8a/$lib"
      } > "$dir/apk/lib/arm64-v8a/$lib"
    else
      cp "$dir/staging/arm64-v8a/$lib" "$dir/apk/lib/arm64-v8a/$lib"
    fi
  done
  if [ "$nozip" != "nozip" ]; then
    # -D keeps zip from adding directory entries; Gradle APKs only carry the
    # lib/arm64-v8a/*.so file entries.
    ( cd "$dir/apk" && zip -q -X -D -r "$dir/app-debug.apk" lib )
  fi
}

run_case() {
  # $1 = expected blocker fragment; $2 = fixture dir
  local expect="$1" fixture="$2" log rc
  log="$work/last.log"
  set +e
  ( VERIFY_APK_STAGED_ROOT="$fixture/staging" "$verify" "$fixture/app-debug.apk" ) \
    > "$log" 2>&1
  rc=$?
  set -e
  if [ "$rc" -eq 0 ]; then
    cat "$log" >&2
    fail "case >>$expect<< unexpectedly PASSED"
  fi
  if ! grep -qF "$expect" "$log"; then
    cat "$log" >&2
    fail "blocker fragment >>$expect<< not reported (rc=$rc)"
  fi
  note "fail-closed OK: $expect"
}

command -v zip >/dev/null 2>&1 || fail "zip is required by the self-check but not found."

# 1. missing APK ---------------------------------------------------------------
c1="$work/case1"
build_synthetic_pair "$c1" "b7" "" "" "nozip"
run_case "APK not found" "$c1"

# 2. staged set incomplete (libonnxruntime.so omitted from staging) -------------
c2="$work/case2"
build_synthetic_pair "$c2" "b7" "" "libonnxruntime.so"
run_case "staged set incomplete: missing libonnxruntime.so" "$c2"

# 3. packaged bytes differ from the staged set ----------------------------------
c3="$work/case3"
build_synthetic_pair "$c3" "b7" "mutate" ""
run_case "STALE or WRONG" "$c3"

# 4. wrong architecture (x86-64 ELF staged and packaged) ------------------------
c4="$work/case4"
build_synthetic_pair "$c4" "3e" "" ""
run_case "is not AArch64" "$c4"

# 5. required export missing (symbol-less synthetic set) ------------------------
c5="$work/case5"
build_synthetic_pair "$c5" "b7" "" ""
run_case "does not export Java_com_mainstayx_echolet_NativeBridge_nativeOpen" "$c5"

echo
echo "selfcheck PASS: 5/5 fail-closed negative cases reject synthetic fixtures."
echo "fixtures were created under $work and were removed on exit."
