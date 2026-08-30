#!/usr/bin/env bash
set -euo pipefail

# This script downloads pinned official sherpa-onnx runtime and model assets,
# strictly verifies their SHA256 checksums, and stages them into .local-runtime/

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CACHE_DIR="${REPO_ROOT}/.cache"
STAGING_DIR="${REPO_ROOT}/.local-runtime"

SHERPA_VERSION="v1.13.6"
SHERPA_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/${SHERPA_VERSION}/sherpa-onnx-${SHERPA_VERSION}-linux-x64-shared-lib.tar.bz2"
SHERPA_SHA256="bbeb203da0f69e37235b50e168d61d1f64ad2de256490cc64ed5535957415a97"
SHERPA_ARCHIVE="${CACHE_DIR}/sherpa-onnx-${SHERPA_VERSION}-linux-x64-shared-lib.tar.bz2"

MODEL_TAG="asr-models"
MODEL_NAME="sherpa-onnx-streaming-zipformer-small-bilingual-zh-en-2023-02-16"
MODEL_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/${MODEL_TAG}/${MODEL_NAME}.tar.bz2"
MODEL_SHA256="0b57b2335b28de2c4e55cc705bda18a723b1fcb15844fc409365a0724d316817"
MODEL_ARCHIVE="${CACHE_DIR}/${MODEL_NAME}.tar.bz2"

echo "=== Downloading & Verifying Official Echolet Assets ==="
echo "Repo root:    ${REPO_ROOT}"
echo "Cache dir:    ${CACHE_DIR}"
echo "Staging dir:  ${STAGING_DIR}"

mkdir -p "${CACHE_DIR}"
mkdir -p "${STAGING_DIR}/runtime/lib"
mkdir -p "${STAGING_DIR}/models/bilingual-zh-en/test_wavs"

# Helper to download with resume and retry
download_file() {
    local url="$1"
    local target="$2"
    echo "--> Downloading from: ${url}"
    curl -L --fail --retry 5 --retry-delay 3 --retry-all-errors -C - -o "${target}" "${url}"
}

# 1. Download and verify official sherpa-onnx runtime
if [[ ! -f "${SHERPA_ARCHIVE}" ]] || ! echo "${SHERPA_SHA256}  ${SHERPA_ARCHIVE}" | sha256sum -c --status 2>/dev/null; then
    download_file "${SHERPA_URL}" "${SHERPA_ARCHIVE}"
fi

echo "--> Verifying SHA256 of sherpa runtime archive..."
if ! echo "${SHERPA_SHA256}  ${SHERPA_ARCHIVE}" | sha256sum -c -; then
    echo "--> Checksum mismatch detected on runtime archive. Re-downloading from scratch..."
    rm -f "${SHERPA_ARCHIVE}"
    download_file "${SHERPA_URL}" "${SHERPA_ARCHIVE}"
    echo "${SHERPA_SHA256}  ${SHERPA_ARCHIVE}" | sha256sum -c -
fi

# 2. Download and verify official ASR model
if [[ ! -f "${MODEL_ARCHIVE}" ]] || ! echo "${MODEL_SHA256}  ${MODEL_ARCHIVE}" | sha256sum -c --status 2>/dev/null; then
    download_file "${MODEL_URL}" "${MODEL_ARCHIVE}"
fi

echo "--> Verifying SHA256 of model archive..."
if ! echo "${MODEL_SHA256}  ${MODEL_ARCHIVE}" | sha256sum -c -; then
    echo "--> Checksum mismatch detected on model archive. Re-downloading from scratch..."
    rm -f "${MODEL_ARCHIVE}"
    download_file "${MODEL_URL}" "${MODEL_ARCHIVE}"
    echo "${MODEL_SHA256}  ${MODEL_ARCHIVE}" | sha256sum -c -
fi

# 3. Extract native runtime libraries into .local-runtime/
echo "--> Staging native libraries into .local-runtime/runtime/lib..."
TMP_EXTRACT_DIR=$(mktemp -d)
trap 'rm -rf "${TMP_EXTRACT_DIR}"' EXIT

tar -xjf "${SHERPA_ARCHIVE}" -C "${TMP_EXTRACT_DIR}"
cp -a "${TMP_EXTRACT_DIR}/sherpa-onnx-${SHERPA_VERSION}-linux-x64-shared-lib/lib"/* "${STAGING_DIR}/runtime/lib/"

# 4. Extract model files into .local-runtime/
echo "--> Staging model files into .local-runtime/models/bilingual-zh-en..."
tar -xjf "${MODEL_ARCHIVE}" -C "${TMP_EXTRACT_DIR}"
EXTRACTED_MODEL="${TMP_EXTRACT_DIR}/${MODEL_NAME}"

cp "${EXTRACTED_MODEL}/encoder-epoch-99-avg-1.int8.onnx" "${STAGING_DIR}/models/bilingual-zh-en/"
cp "${EXTRACTED_MODEL}/decoder-epoch-99-avg-1.onnx" "${STAGING_DIR}/models/bilingual-zh-en/"
cp "${EXTRACTED_MODEL}/joiner-epoch-99-avg-1.int8.onnx" "${STAGING_DIR}/models/bilingual-zh-en/"
cp "${EXTRACTED_MODEL}/tokens.txt" "${STAGING_DIR}/models/bilingual-zh-en/"

if [[ -f "${EXTRACTED_MODEL}/test_wavs/0.wav" ]]; then
    cp "${EXTRACTED_MODEL}/test_wavs/0.wav" "${STAGING_DIR}/models/bilingual-zh-en/test_wavs/"
fi

# 5. Copy manifest
cp "${REPO_ROOT}/model.json" "${STAGING_DIR}/model.json"

echo "=== Official assets staged successfully into .local-runtime/ ==="
ls -lh "${STAGING_DIR}/runtime/lib"
ls -lh "${STAGING_DIR}/models/bilingual-zh-en"
