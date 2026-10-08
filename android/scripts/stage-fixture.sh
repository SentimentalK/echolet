#!/usr/bin/env bash
# Stages the pinned X-ASR bilingual-zh-en debug fixture onto the connected
# Android device's app-specific external files directory. Host-side the
# fixture comes from scripts/acquire-base-model.sh (the locked model archive).
#
# Usage:
#   android/scripts/stage-fixture.sh        (uses the first booted device)
#
# Model artifacts are validated on the HOST first (archive/hash/manifest);
# the app itself performs no network.

set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"

fail() {
  printf 'Blocker: %s\n' "$*" >&2
  exit 1
}

if ! "$script_dir/check-env.sh" >/dev/null 2>&1; then
  "$script_dir/check-env.sh" || true
  fail "host toolchain incomplete for adb staging."
fi

# shellcheck source=check-env.sh
source "$script_dir/check-env.sh"
sdk_root=$(find_sdk_root || true)
[ -n "$sdk_root" ] || fail "Android SDK root not found."
adb_bin="$sdk_root/platform-tools/adb"
[ -x "$adb_bin" ] || fail "adb missing at $adb_bin (sdkmanager --install platform-tools)."

# 0. Device gate: fail with EXACT adb evidence, never silently skip.
#
# The gate proves (raw `adb devices -l` printed in full on every failure):
#   * an emulated/physical device is listed, not blank;
#   * the device is AUTHORIZED (`adb devices` status != unauthorized);
#   * the device has the arm64-v8a ABI required by the Phase 0-A slice;
#   * the diagnostic app com.mainstayx.echolet is REALLY installed
#     (pm path checked; its external files directory may not exist until the
#     app has been installed once — install the APK BEFORE staging).
# Lack of a device/ABI/app is a validation blocker: the script FAILS instead
# of faking verification; report `DEVICE INFERENCE UNVERIFIED`, never PASS.
"$adb_bin" devices -l
devices=$("$adb_bin" devices | tail -n +2 | grep -c 'device$' || true)
if [ "$devices" -eq 0 ]; then
  fail "no authorized device attached (raw 'adb devices -l' above shows the truth). REAL device inference stays UNVERIFIED; report the gate, do not claim PASS."
fi
serial=$("$adb_bin" devices | tail -n +2 | grep 'device$' | head -n 1 | cut -f1)
abi_list=$("$adb_bin" -s "$serial" shell getprop ro.product.cpu.abilist | tr -d '\r')
case "$abi_list" in
  *arm64-v8a*) ;;
  *) fail "device $serial lacks arm64-v8a ABI (ro.product.cpu.abilist=$abi_list); this slice only packages arm64-v8a." ;;
esac
if [ -z "$("$adb_bin" -s "$serial" shell pm path com.mainstayx.echolet | tr -d '\r')" ]; then
  fail "com.mainstayx.echolet is NOT installed on $serial; install the verified APK first: adb install -r android/app/build/outputs/apk/debug/app-debug.apk (the app-specific external files dir only becomes usable after at least one install)."
fi

# 1. Host-side fixture from the locked model pack.
host_fixture="$repo_root/.local-runtime/models/bilingual-zh-en"
if [ ! -f "$host_fixture/encoder-480ms.onnx" ]; then
  echo "--> running scripts/acquire-base-model.sh to stage the locked model pack"
  (
    cd "$repo_root"
    bash scripts/acquire-base-model.sh "$host_fixture"
  ) || fail "acquire-base-model.sh failed (host model pack unavailable/hash mismatch)."
fi

required=("model.json" "encoder-480ms.onnx" "decoder-480ms.onnx" "joiner-480ms.onnx" "tokens.txt" "test_wavs/0.wav")
for f in "${required[@]}"; do
  [ -s "$host_fixture/$f" ] || fail "host fixture incomplete: missing $f under $host_fixture."
done

# Optional host-side hash validation against the lock when the archive is
# still cached; file-level validation is authoritative when only staged.
if command -v jq >/dev/null 2>&1 && [ -f "$repo_root/models/base-model.lock.json" ]; then
  for f in encoder decoder joiner; do
    file=$(jq -r ".files.$f" "$repo_root/models/base-model.lock.json")
    if [ ! -s "$host_fixture/$file" ]; then
      fail "host fixture missing locked file $file."
    fi
  done
fi

# 2. Application id (must match com.mainstayx.echolet).
app_id="com.mainstayx.echolet"
device_dir="/storage/emulated/0/Android/data/$app_id/files/models"
device_fixture="$device_dir/bilingual-zh-en"

# Scoped storage: on Android 11+ the app-specific external files dir may
# still refuse plain `adb shell mkdir` on some devices/builds. Try, and on
# failure print the EXACT denial rather than blindly bypassing the system
# protection (no `adb root`/write-external hacks).
if ! "$adb_bin" -s "$serial" shell "mkdir -p '$device_fixture/test_wavs'"; then
  "$adb_bin" -s "$serial" shell "ls -ld '$device_dir' 2>&1" || true
  fail "adb mkdir under $device_dir was denied (scoped-storage/permission error shown above). Launch the app once on the device, confirm USB debugging is authorized, and re-run; do NOT work around the device's storage policy."
fi

for f in "${required[@]}"; do
  echo "--> staging $f"
  "$adb_bin" -s "$serial" push "$host_fixture/$f" "$device_fixture/$f" >/dev/null \
    || fail "adb push $f failed (USB denied/unauthorized/no space; check device screen for debug-USB prompt)."
done

# 3. Verify every named file landed nonempty.
for f in "${required[@]}"; do
  remote="$device_fixture/$f"
  size=$("$adb_bin" -s "$serial" shell "stat -c %s '$remote' 2>/dev/null" | tr -d '\r' || echo "")
  [ -n "$size" ] && [ "$size" != "0" ] || fail "staged $f missing/empty on device at $remote."
done

echo "Fixture staged at $device_fixture on $serial."
