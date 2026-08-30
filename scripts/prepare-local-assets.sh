#!/usr/bin/env bash
set -euo pipefail

# This script is a local development helper for staging native runtime libraries
# and pre-trained models from the local build environment into .local-runtime/
#
# IMPORTANT: This is the ONLY script in the entire codebase permitted to reference
# the host machine's sherpa build paths.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGING_DIR="${REPO_ROOT}/.local-runtime"
SHERPA_SOURCE_DIR="${SHERPA_SOURCE_DIR:-/home/sentimentalk/sherpa-onnx}"
MODEL_SOURCE_DIR="${SHERPA_SOURCE_DIR}/sherpa-onnx-streaming-zipformer-small-bilingual-zh-en-2023-02-16"
LIB_SOURCE_DIR="${SHERPA_SOURCE_DIR}/build-shared/lib"
ORT_SOURCE_DIR="${SHERPA_SOURCE_DIR}/build-shared/_deps/onnxruntime-src/lib"

echo "=== Staging local assets into .local-runtime ==="
echo "Repo root:     ${REPO_ROOT}"
echo "Staging target: ${STAGING_DIR}"

if [[ ! -d "${SHERPA_SOURCE_DIR}" ]]; then
    echo "[Error] sherpa-onnx source directory not found at: ${SHERPA_SOURCE_DIR}" >&2
    exit 1
fi

# 1. Create target staging directories
mkdir -p "${STAGING_DIR}/runtime/lib"
mkdir -p "${STAGING_DIR}/models/bilingual-zh-en/test_wavs"

# 2. Copy native shared libraries
echo "--> Copying native runtime libraries..."
cp -a "${LIB_SOURCE_DIR}"/*.so* "${STAGING_DIR}/runtime/lib/"
cp -a "${ORT_SOURCE_DIR}"/*.so* "${STAGING_DIR}/runtime/lib/"

# 3. Copy bilingual model files
echo "--> Copying model files..."
cp "${MODEL_SOURCE_DIR}/encoder-epoch-99-avg-1.int8.onnx" "${STAGING_DIR}/models/bilingual-zh-en/"
cp "${MODEL_SOURCE_DIR}/decoder-epoch-99-avg-1.onnx" "${STAGING_DIR}/models/bilingual-zh-en/"
cp "${MODEL_SOURCE_DIR}/joiner-epoch-99-avg-1.int8.onnx" "${STAGING_DIR}/models/bilingual-zh-en/"
cp "${MODEL_SOURCE_DIR}/tokens.txt" "${STAGING_DIR}/models/bilingual-zh-en/"

if [[ -f "${MODEL_SOURCE_DIR}/test_wavs/0.wav" ]]; then
    cp "${MODEL_SOURCE_DIR}/test_wavs/0.wav" "${STAGING_DIR}/models/bilingual-zh-en/test_wavs/"
fi

# 4. Copy model.json manifest
cp "${REPO_ROOT}/model.json" "${STAGING_DIR}/model.json"

echo "--> Validating staged assets..."
ls -la "${STAGING_DIR}/runtime/lib"
ls -la "${STAGING_DIR}/models/bilingual-zh-en"

echo "=== Local assets prepared successfully in .local-runtime ==="
