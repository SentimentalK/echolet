#!/usr/bin/env bash
#
# build-ios-native.sh — stage the iPhoneOS arm64 static Sherpa-onnx C-API and
# onnxruntime libraries for the Echolet iOS native probe from the OFFICIAL
# pinned prebuilt xcframework distributions.
#
# Pinned artifacts (checksums verified against sherpa-onnx v1.13.6
# Package.swift / onnxruntime-libs v1.27.1 Package.swift):
#   sherpa-onnx xcframework  ("xcframework" release tag)
#     sherpa-onnx-v1.13.6-ios-static.xcframework.zip
#     sha256 0b8c880357e653af18c5f9c6e8b3c045e85b98403c00ff25692a1261d31aa332
#   onnxruntime-libs v1.27.1
#     onnxruntime-ios-static-xcframework-1.27.1.xcframework.zip
#     sha256 985deaff345c7bcfbe4979b2daeec09d7a745b1e9cb73f37f4077364eb578e62
#
# The device slice (ios-arm64) is required: Echolet ASR probes/ASR must run on
# the physical iPad, not a simulator. Outputs (git-ignored local cache):
#   .local-runtime/ios-native/lib/libsherpa-onnx-c-api.a
#   .local-runtime/ios-native/lib/libonnxruntime.a
#
# Safety: runs entirely in-user, never as root, touches nothing outside the
# repo's ignored cache, sets no global toolchain state.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
NATIVE_ROOT="${REPO_ROOT}/.local-runtime/ios-native"
SHERPA_VERSION="v1.13.6"
ORT_VERSION="1.27.1"
SHERPA_ZIP_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/xcframework/sherpa-onnx-${SHERPA_VERSION}-ios-static.xcframework.zip"
SHERPA_SHA256="0b8c880357e653af18c5f9c6e8b3c045e85b98403c00ff25692a1261d31aa332"
ORT_ZIP_URL="https://github.com/csukuangfj/onnxruntime-libs/releases/download/v${ORT_VERSION}/onnxruntime-ios-static-xcframework-${ORT_VERSION}.xcframework.zip"
ORT_SHA256="985deaff345c7bcfbe4979b2daeec09d7a745b1e9cb73f37f4077364eb578e62"

log() { echo "[build-ios-native] $*"; }

command -v unzip >/dev/null 2>&1 || { log "ERROR: unzip missing"; exit 1; }
command -v nm >/dev/null 2>&1 || { log "ERROR: nm missing"; exit 1; }

mkdir -p "${NATIVE_ROOT}"

fetch_and_verify() {
  local url="$1" sha256="$2" dest="$3"
  if [ -f "${dest}" ] && echo "${sha256}  ${dest}" | shasum -a 256 -c - >/dev/null 2>&1; then
    log "cached archive ok: ${dest}"
    return 0
  fi
  log "downloading ${url}"
  curl -L --fail --retry 3 -sS -o "${dest}" "${url}"
  echo "${sha256}  ${dest}" | shasum -a 256 -c -
}

SHERPA_ZIP="${NATIVE_ROOT}/ios-sherpa-static.xcframework.zip"
ORT_ZIP="${NATIVE_ROOT}/ios-ort-static.xcframework.zip"
SHERPA_FW="${NATIVE_ROOT}/sherpa-onnx.xcframework"
ORT_FW="${NATIVE_ROOT}/onnxruntime.xcframework"

# ---- 1. official Sherpa-onnx iOS static xcframework -------------------------
if [ ! -f "${SHERPA_FW}/ios-arm64/SherpaOnnxC.framework/SherpaOnnxC" ]; then
  fetch_and_verify "${SHERPA_ZIP_URL}" "${SHERPA_SHA256}" "${SHERPA_ZIP}"
  unzip -q -o "${SHERPA_ZIP}" -d "${NATIVE_ROOT}"
  rm -f "${SHERPA_ZIP}"
fi

# ---- 2. official ORT iOS static xcframework ---------------------------------
if [ ! -f "${ORT_FW}/ios-arm64/onnxruntime.framework/onnxruntime" ]; then
  fetch_and_verify "${ORT_ZIP_URL}" "${ORT_SHA256}" "${ORT_ZIP}"
  unzip -q -o "${ORT_ZIP}" -d "${NATIVE_ROOT}"
  rm -f "${ORT_ZIP}"
fi

# ---- 3. stage + verify device arm64 static archives ------------------------
LIB_DIR="${NATIVE_ROOT}/lib"
mkdir -p "${LIB_DIR}"
SHERPA_ARCHIVE="${SHERPA_FW}/ios-arm64/SherpaOnnxC.framework/SherpaOnnxC"
ORT_ARCHIVE="${ORT_FW}/ios-arm64/onnxruntime.framework/onnxruntime"

cp -a "${SHERPA_ARCHIVE}" "${LIB_DIR}/libsherpa-onnx-c-api.a"
cp -a "${ORT_ARCHIVE}" "${LIB_DIR}/libonnxruntime.a"

log "=== Artifact verification (device arm64) ==="
for f in "${LIB_DIR}/libsherpa-onnx-c-api.a" "${LIB_DIR}/libonnxruntime.a"; do
  file "${f}" | grep -q "current ar archive" || { log "ERROR: ${f} is not a static archive"; exit 1; }
done
lipo -info "${LIB_DIR}/libonnxruntime.a" | grep -q "arm64" || {
  log "ERROR: ORT archive lacks arm64 slice"; exit 1; }

# The sherpa archive is an ar of Mach-O arm64 members; verify one member.
AR_MEMBER="$(ar t "${LIB_DIR}/libsherpa-onnx-c-api.a" 2>/dev/null | grep '\.o$' | head -1)"
[ -n "${AR_MEMBER}" ] || { log "ERROR: no object members in sherpa archive"; exit 1; }
WORK_MEMBER="$(mktemp -d "${TMPDIR:-/tmp}/echolet-ar.XXXXXX")"
ar p "${LIB_DIR}/libsherpa-onnx-c-api.a" "${AR_MEMBER}" > "${WORK_MEMBER}/m.o"
lipo -info "${WORK_MEMBER}/m.o" | grep -q "arm64" || {
  log "ERROR: sherpa archive member is not arm64 Mach-O"; rm -rf "${WORK_MEMBER}"; exit 1; }
rm -rf "${WORK_MEMBER}"
log "ok: both archives are iPhoneOS arm64 static ar archives"

for sym in _SherpaOnnxCreateOnlineRecognizer _SherpaOnnxOnlineStreamAcceptWaveform \
           _SherpaOnnxGetOnlineStreamResult _SherpaOnnxIsOnlineStreamReady; do
  if nm -gU "${LIB_DIR}/libsherpa-onnx-c-api.a" | grep " ${sym}\$" >/dev/null; then
    log "ok: ${sym}"
  else
    log "ERROR: missing ${sym}"; exit 1
  fi
done
if nm -gU "${LIB_DIR}/libonnxruntime.a" | grep "_OrtGetApiBase" >/dev/null; then
  log "ok: _OrtGetApiBase"
else
  log "ERROR: missing _OrtGetApiBase in ORT archive"; exit 1
fi

log "Staged: ${LIB_DIR}/libsherpa-onnx-c-api.a, ${LIB_DIR}/libonnxruntime.a"
log "DONE"
