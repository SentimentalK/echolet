#!/usr/bin/env bash
set -euo pipefail

# This script downloads official macOS platform runtime libraries and acquires the frozen Echolet Base Model.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
STAGING_DIR="${REPO_ROOT}/.local-runtime"

compute_sha256() {
    local file_path="$1"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "${file_path}" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "${file_path}" | awk '{print $1}'
    else
        echo "[Error] Neither sha256sum nor shasum found!" >&2
        exit 1
    fi
}

# 1. Normalize architecture
RAW_ARCH="${1:-${ECHOLET_ARCH:-$(uname -m)}}"
case "${RAW_ARCH}" in
    x86_64|x64|amd64)
        ARCH="x64"
        SHERPA_ASSET="sherpa-onnx-v1.13.6-osx-x64-shared-lib.tar.bz2"
        SHERPA_SHA256="dbe7e7aa269f742efec7366d5c4d8020cc32fc833023b9b87e5d6282d70a62b8"
        SHERPA_DIR_NAME="sherpa-onnx-v1.13.6-osx-x64-shared-lib"
        ;;
    aarch64|arm64)
        ARCH="arm64"
        SHERPA_ASSET="sherpa-onnx-v1.13.6-osx-arm64-shared-lib.tar.bz2"
        SHERPA_SHA256="d628e43aed6b719be163549876f41c909b75df26b8f439a5af69de03896bc6f5"
        SHERPA_DIR_NAME="sherpa-onnx-v1.13.6-osx-arm64-shared-lib"
        ;;
    *)
        echo "[Error] Unsupported macOS architecture: ${RAW_ARCH}" >&2
        exit 1
        ;;
esac

SHERPA_VERSION="v1.13.6"
SHERPA_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/${SHERPA_VERSION}/${SHERPA_ASSET}"

TEST_WAV_URL="https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20/resolve/main/test_wavs/0.wav"
TEST_WAV_SHA256="7d93384ca14702cc584a7a33fe2fed92e89e708549161cb12ea38c916882103b"

echo "=== Downloading & Verifying Official Echolet macOS Assets (${ARCH}) ==="
echo "Repo root:    ${REPO_ROOT}"
echo "Staging dir:  ${STAGING_DIR}"
echo "Architecture: ${ARCH} (raw: ${RAW_ARCH})"

TMP_WORK_DIR=$(mktemp -d)
trap 'rm -rf "${TMP_WORK_DIR}"' EXIT

mkdir -p "${STAGING_DIR}/runtime/lib"
mkdir -p "${STAGING_DIR}/models/bilingual-zh-en/test_wavs"

SHERPA_ARCHIVE="${TMP_WORK_DIR}/${SHERPA_ASSET}"

# 1. Download and verify official sherpa-onnx macOS runtime
echo "--> Downloading official sherpa-onnx runtime (${SHERPA_VERSION} for macOS ${ARCH})..."
curl -L --fail --retry 3 --retry-delay 2 -s -o "${SHERPA_ARCHIVE}" "${SHERPA_URL}"
echo "--> Verifying SHA256 of sherpa runtime archive..."
CALC_SHA=$(compute_sha256 "${SHERPA_ARCHIVE}")
if [[ "${CALC_SHA}" != "${SHERPA_SHA256}" ]]; then
    echo "[Error] SHA256 mismatch for ${SHERPA_ASSET}!" >&2
    echo "        Expected: ${SHERPA_SHA256}" >&2
    echo "        Got:      ${CALC_SHA}" >&2
    exit 1
fi
echo "--> SHA256 verified: OK"

echo "--> Staging native libraries into .local-runtime/runtime/lib..."
tar -xjf "${SHERPA_ARCHIVE}" -C "${TMP_WORK_DIR}"
cp -a "${TMP_WORK_DIR}/${SHERPA_DIR_NAME}/lib"/* "${STAGING_DIR}/runtime/lib/"

# 2. Acquire Echolet Base Model via frozen model lock
echo "--> Acquiring Echolet Base Model (r1)..."
"${REPO_ROOT}/scripts/acquire-base-model.sh" "${STAGING_DIR}/models/bilingual-zh-en"

# 3. Download test wav for stream testing
echo "--> Downloading test wav..."
curl -L --fail --retry 3 --retry-delay 2 -s -o "${TMP_WORK_DIR}/0.wav" "${TEST_WAV_URL}"
WAV_SHA=$(compute_sha256 "${TMP_WORK_DIR}/0.wav")
if [[ "${WAV_SHA}" != "${TEST_WAV_SHA256}" ]]; then
    echo "[Error] SHA256 mismatch for test wav!" >&2
    exit 1
fi
cp "${TMP_WORK_DIR}/0.wav" "${STAGING_DIR}/models/bilingual-zh-en/test_wavs/"

# 4. Copy manifest & registry
cp "${REPO_ROOT}/model.json" "${STAGING_DIR}/models/bilingual-zh-en/model.json"
cp "${REPO_ROOT}/model.json" "${STAGING_DIR}/model.json"
mkdir -p "${STAGING_DIR}/models"
cp "${REPO_ROOT}/models/registry.json" "${STAGING_DIR}/models/registry.json"

echo "=== Official macOS assets staged successfully into .local-runtime/ (${ARCH}) ==="
ls -lh "${STAGING_DIR}/runtime/lib"
ls -lh "${STAGING_DIR}/models/bilingual-zh-en"
