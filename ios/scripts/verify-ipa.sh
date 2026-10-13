#!/usr/bin/env bash
#
# verify-ipa.sh — fail-closed verification gate for the UNSIGNED iOS device
# IPA produced by the ci/release pipeline (or a local xcodebuild run).
#
# Usage: ios/scripts/verify-ipa.sh <ipa-path> [expected-version]
#
# Verifies against the extracted Payload:
#   1. Payload/EcholetApp.app exists and contains
#      PlugIns/EcholetKeyboard.appex (the keyboard extension is EMBEDDED).
#   2. Correct bundle IDs: com.mainstayx.echolet.app /
#      com.mainstayx.echolet.app.keyboard.
#   3. CFBundleShortVersionString matches <expected-version> when provided;
#      CFBundleVersion is a nonempty numeric build code.
#   4. Both app and extension binaries are Mach-O arm64 for iPhoneOS
#      (UIRequiredDeviceCapabilities arm64 + Rust/static libs are arm64 only).
#   5. NO bundled ASR model: the ~593 MB X-ASR model must NOT be packaged.
#      Fails on any "bilingual-zh-en" payload, *.mlmodelc directory entry or
#      a suspiciously large unzipped payload.
#
# The IPA is an UNSIGNED re-signing input (SideStore/Sideloadly etc.), never
# a one-click installable package; this script checks the resigning-relevant
# structure only.

set -euo pipefail

fail() { printf 'Blocker: %s\n' "$*" >&2; exit 1; }
note() { printf 'verify-ipa: %s\n' "$*"; }

[ "$#" -ge 1 ] || fail "usage: verify-ipa.sh <ipa-path> [expected-version]"
IPA="$1"
EXPECTED_VERSION="${2:-}"
PLISTBUDDY=/usr/libexec/PlistBuddy
command -v unzip >/dev/null 2>&1 || fail "unzip is required but not found."
[ -x "$PLISTBUDDY" ] || fail "PlistBuddy not found at $PLISTBUDDY (macOS runner required)."
[ -f "$IPA" ] || fail "IPA not found at $IPA (build/packaging step failed silently)."

work=$(mktemp -d "${TMPDIR:-/tmp}/echolet-ipa-verify.XXXXXX")
trap 'rm -rf "$work"' EXIT

unzip -q "$IPA" -d "$work" || fail "cannot unzip $IPA (corrupt archive)."
APP="$work/Payload/EcholetApp.app"
APPEX="$APP/PlugIns/EcholetKeyboard.appex"
[ -f "$APP/EcholetApp" ] || fail "Payload/EcholetApp.app main executable missing — wrong top-level bundle structure."
[ -d "$APPEX" ] || fail "Payload/EcholetApp.app lacks PlugIns/EcholetKeyboard.appex — the keyboard extension is not embedded."

# --- 2. bundle IDs -------------------------------------------------------------
app_bundle_id=$("$PLISTBUDDY" -c 'Print :CFBundleIdentifier' "$APP/Info.plist" 2>/dev/null || true)
[ "$app_bundle_id" = "com.mainstayx.echolet.app" ] \
  || fail "app bundle id is '$app_bundle_id', expected com.mainstayx.echolet.app."
appex_bundle_id=$("$PLISTBUDDY" -c 'Print :CFBundleIdentifier' "$APPEX/Info.plist" 2>/dev/null || true)
[ "$appex_bundle_id" = "com.mainstayx.echolet.app.keyboard" ] \
  || fail "keyboard extension bundle id is '$appex_bundle_id', expected com.mainstayx.echolet.app.keyboard."

# --- 3. versions ---------------------------------------------------------------
app_version=$("$PLISTBUDDY" -c 'Print :CFBundleShortVersionString' "$APP/Info.plist" 2>/dev/null || true)
app_build=$("$PLISTBUDDY" -c 'Print :CFBundleVersion' "$APP/Info.plist" 2>/dev/null || true)
appex_version=$("$PLISTBUDDY" -c 'Print :CFBundleShortVersionString' "$APPEX/Info.plist" 2>/dev/null || true)
appex_build=$("$PLISTBUDDY" -c 'Print :CFBundleVersion' "$APPEX/Info.plist" 2>/dev/null || true)
[ -n "$app_version" ] || fail "app CFBundleShortVersionString is missing."
[ "$app_version" = "$appex_version" ] \
  || fail "app ($app_version) and keyboard ($appex_version) CFBundleShortVersionString disagree."
[ "$app_build" = "$appex_build" ] \
  || fail "app ($app_build) and keyboard ($appex_build) CFBundleVersion disagree."
printf '%s' "$app_build" | grep -qE '^[0-9]+$' \
  || fail "CFBundleVersion '$app_build' is not a numeric build code."
if [ -n "$EXPECTED_VERSION" ]; then
  [ "$app_version" = "$EXPECTED_VERSION" ] \
    || fail "CFBundleShortVersionString is $app_version, expected $EXPECTED_VERSION (tagged source version)."
fi

# --- 4. arm64 iPhoneOS binaries -------------------------------------------------
binary_arch() {
  local bin="$1" name
  name=$(basename "$1")
  [ -f "$bin" ] || fail "$name missing in bundle."
  file "$bin" | grep -qi "Mach-O 64-bit executable arm64\|Mach-O universal binary" \
    || fail "$name is not a Mach-O arm64 (universal) iPhoneOS binary for iPhoneOS: $(file "$bin")."
  if file "$bin" | grep -qi "universal"; then
    lipo -info "$bin" | grep -qi "arm64" || fail "$name universal binary lacks an arm64 slice."
  fi
}
binary_arch "$APP/EcholetApp"
binary_arch "$APPEX/EcholetKeyboard"

# Double-check the LC_BUILD_VERSION platform of the main executable: a
# simulator destination build must never land in the device IPA.
if otool -l "$APP/EcholetApp" 2>/dev/null | grep -q "IOSSIMULATOR"; then
  fail "app binary targets the iOS SIMULATOR; the device IPA must contain an iphoneos build."
fi

# --- 5. NO model payload --------------------------------------------------------
listing=$(unzip -l "$IPA" | awk '{print $4}')
if printf '%s\n' "$listing" | grep -q "bilingual-zh-en"; then
  fail "IPA packages the bilingual-zh-en model payload — the model must be a runtime download, never bundled."
fi
ml_count=$(printf '%s\n' "$listing" | grep -c 'mlmodelc' || true)
[ "$ml_count" -eq 0 ] || fail "IPA contains $ml_count mlmodelc entries; compiled model payload must not be packaged."

total_bytes=$(du -sk "$work" | awk '{print $1}')
model_threshold_kb=$((700 * 1024))
[ "$total_bytes" -lt "$model_threshold_kb" ] \
  || fail "unszipped payload is ${total_bytes} KB; above the model-free bound of ${model_threshold_kb} KB (a 550MB+ model appears to be bundled)."

# --- summary --------------------------------------------------------------------
note "app bundle id OK"
note "keyboard appex embedded + bundle id OK"
note "versions: $app_version build $app_build (expected: ${EXPECTED_VERSION:-<unchecked>})"
[ -n "$EXPECTED_VERSION" ] || note "expected-version argument absent; tagged-source cross-check happens in the release preflight job"
note "arm64 iPhoneOS binaries OK"
note "no model payload OK (unzipped $total_bytes KB)"
note "PASS: unsigned IPA is structurally a valid re-signing input"
