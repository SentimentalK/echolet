#!/usr/bin/env bash
set -euo pipefail

# This script prepares native runtime libraries and pre-trained models in .local-runtime/
#
# Logic:
# 1. If .local-runtime/ is already fully staged, nothing to do.
# 2. If a local sherpa-onnx build directory exists (via $SHERPA_SOURCE_DIR or local path),
#    stage from local build artifacts.
# 3. Otherwise, automatically download and verify official pinned releases
#    via scripts/download-official-assets.sh.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGING_DIR="${REPO_ROOT}/.local-runtime"
SHERPA_SOURCE_DIR="${SHERPA_SOURCE_DIR:-/home/sentimentalk/sherpa-onnx}"

# Check if already complete
if [[ -f "${STAGING_DIR}/runtime/lib/libsherpa-onnx-c-api.so" && \
      -f "${STAGING_DIR}/models/bilingual-zh-en/tokens.txt" && \
      -f "${STAGING_DIR}/models/bilingual-zh-en/encoder-epoch-99-avg-1.int8.onnx" ]]; then
    echo "[Assets] .local-runtime/ is already populated and valid."
    exit 0
fi

if [[ -d "${SHERPA_SOURCE_DIR}/build-shared/lib" ]]; then
    echo "=== Staging from local sherpa-onnx build: ${SHERPA_SOURCE_DIR} ==="
    MODEL_SOURCE_DIR="${SHERPA_SOURCE_DIR}/sherpa-onnx-streaming-zipformer-small-bilingual-zh-en-2023-02-16"
    LIB_SOURCE_DIR="${SHERPA_SOURCE_DIR}/build-shared/lib"
    ORT_SOURCE_DIR="${SHERPA_SOURCE_DIR}/build-shared/_deps/onnxruntime-src/lib"

    mkdir -p "${STAGING_DIR}/runtime/lib"
    mkdir -p "${STAGING_DIR}/models/bilingual-zh-en/test_wavs"

    cp -a "${LIB_SOURCE_DIR}"/*.so* "${STAGING_DIR}/runtime/lib/"
    if [[ -d "${ORT_SOURCE_DIR}" ]]; then
        cp -a "${ORT_SOURCE_DIR}"/*.so* "${STAGING_DIR}/runtime/lib/"
    fi

    if [[ -d "${MODEL_SOURCE_DIR}" ]]; then
        cp "${MODEL_SOURCE_DIR}/encoder-epoch-99-avg-1.int8.onnx" "${STAGING_DIR}/models/bilingual-zh-en/"
        cp "${MODEL_SOURCE_DIR}/decoder-epoch-99-avg-1.onnx" "${STAGING_DIR}/models/bilingual-zh-en/"
        cp "${MODEL_SOURCE_DIR}/joiner-epoch-99-avg-1.int8.onnx" "${STAGING_DIR}/models/bilingual-zh-en/"
        cp "${MODEL_SOURCE_DIR}/tokens.txt" "${STAGING_DIR}/models/bilingual-zh-en/"
        if [[ -f "${MODEL_SOURCE_DIR}/test_wavs/0.wav" ]]; then
            cp "${MODEL_SOURCE_DIR}/test_wavs/0.wav" "${STAGING_DIR}/models/bilingual-zh-en/test_wavs/"
        fi
    fi
    cp "${REPO_ROOT}/model.json" "${STAGING_DIR}/model.json"
    echo "=== Local assets staged successfully in .local-runtime/ ==="
else
    echo "=== Local sherpa build not found; downloading official pinned assets ==="
    "${REPO_ROOT}/scripts/download-official-assets.sh"
fi
