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
devices=$("$adb_bin" devices | tail -n +2 | grep -c "device" || true)
if [ "$devices" -eq 0 ]; then
  "$adb_bin" devices -l
  fail "no physical arm64 device attached (see 'adb devices -l' above). REAL device inference stays UNVERIFIED; report the gate, do not claim PASS."
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

"$adb_bin" shell "mkdir -p '$device_fixture/test_wavs'" || fail "adb mkdir failed (USB/disconnected device)."

for f in "${required[@]}"; do
  echo "--> staging $f"
  "$adb_bin" push "$host_fixture/$f" "$device_fixture/$f" >/dev/null \
    || fail "adb push $f failed (USB denied/unauthorized/no space; check device screen for debug-USB prompt)."
done

# 3. Verify every named file landed nonempty.
for f in "${required[@]}"; do
  remote="$device_fixture/$f"
  size=$("$adb_bin" shell "stat -c %s '$remote' 2>/dev/null" | tr -d '\r' || echo "")
  [ -n "$size" ] && [ "$size" != "0" ] || fail "staged $f missing/empty on device at $remote."
done

echo "Fixture staged at $device_fixture on $("$adb_bin" get-serialno | tr -d '\r')."
