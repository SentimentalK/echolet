#!/usr/bin/env bash
set -euo pipefail

# This script downloads pinned official sherpa-onnx runtime and model assets,
# strictly verifies their SHA256 checksums, and stages them into .local-runtime/

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGING_DIR="${REPO_ROOT}/.local-runtime"

SHERPA_VERSION="v1.13.6"
SHERPA_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/${SHERPA_VERSION}/sherpa-onnx-${SHERPA_VERSION}-linux-x64-shared-lib.tar.bz2"
SHERPA_SHA256="bbeb203da0f69e37235b50e168d61d1f64ad2de256490cc64ed5535957415a97"

MODEL_TAG="asr-models"
MODEL_NAME="sherpa-onnx-streaming-zipformer-small-bilingual-zh-en-2023-02-16"
MODEL_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/${MODEL_TAG}/${MODEL_NAME}.tar.bz2"
MODEL_SHA256="2b7c63322b32e5e0f2526043a1103366119ca58dd615cd7105a37c01db9553d7"

echo "=== Downloading & Verifying Official Echolet Assets ==="
echo "Repo root:    ${REPO_ROOT}"
echo "Staging dir:  ${STAGING_DIR}"

TMP_WORK_DIR=$(mktemp -d)
trap 'rm -rf "${TMP_WORK_DIR}"' EXIT

mkdir -p "${STAGING_DIR}/runtime/lib"
mkdir -p "${STAGING_DIR}/models/bilingual-zh-en/test_wavs"

SHERPA_ARCHIVE="${TMP_WORK_DIR}/sherpa-onnx-${SHERPA_VERSION}-linux-x64-shared-lib.tar.bz2"
MODEL_ARCHIVE="${TMP_WORK_DIR}/${MODEL_NAME}.tar.bz2"

# 1. Download and verify official sherpa-onnx runtime
echo "--> Downloading official sherpa-onnx runtime (${SHERPA_VERSION})..."
curl -L --fail --retry 3 --retry-delay 2 -s -o "${SHERPA_ARCHIVE}" "${SHERPA_URL}"
echo "--> Verifying SHA256 of sherpa runtime archive..."
echo "${SHERPA_SHA256}  ${SHERPA_ARCHIVE}" | sha256sum -c -

# 2. Download and verify official ASR model
echo "--> Downloading official bilingual model (${MODEL_NAME})..."
curl -L --fail --retry 3 --retry-delay 2 -s -o "${MODEL_ARCHIVE}" "${MODEL_URL}"
echo "--> Verifying SHA256 of model archive..."
echo "${MODEL_SHA256}  ${MODEL_ARCHIVE}" | sha256sum -c -

# 3. Extract native runtime libraries into .local-runtime/
echo "--> Staging native libraries into .local-runtime/runtime/lib..."
tar -xjf "${SHERPA_ARCHIVE}" -C "${TMP_WORK_DIR}"
cp -a "${TMP_WORK_DIR}/sherpa-onnx-${SHERPA_VERSION}-linux-x64-shared-lib/lib"/* "${STAGING_DIR}/runtime/lib/"

# 4. Extract model files into .local-runtime/
echo "--> Staging model files into .local-runtime/models/bilingual-zh-en..."
tar -xjf "${MODEL_ARCHIVE}" -C "${TMP_WORK_DIR}"
EXTRACTED_MODEL="${TMP_WORK_DIR}/${MODEL_NAME}"

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
