#!/usr/bin/env bash
set -euo pipefail

# This script downloads pinned official sherpa-onnx runtime and X-ASR 480ms model assets,
# strictly verifies their SHA256 checksums, and stages them into .local-runtime/

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGING_DIR="${REPO_ROOT}/.local-runtime"

SHERPA_VERSION="v1.13.6"
SHERPA_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/${SHERPA_VERSION}/sherpa-onnx-${SHERPA_VERSION}-linux-x64-shared-lib.tar.bz2"
SHERPA_SHA256="bbeb203da0f69e37235b50e168d61d1f64ad2de256490cc64ed5535957415a97"

XASR_REV="689ff18c584d29910da37b6fe904db0c1489c9d1"
XASR_BASE="https://huggingface.co/GilgameshWind/X-ASR-zh-en/resolve/${XASR_REV}/deployment/models/chunk-480ms-model"

ENCODER_SHA256="0c3454033d249081df124ddcd7adaf3deca07d0b999b26f2ee5d2475d37abc74"
DECODER_SHA256="3658368d274a5d5fd39a7ac20c46bed0ad9cfea1f0feddef30d5d89797c1f499"
JOINER_SHA256="03781c98165a2385024c9cecdd2b6b13310d81db23a62c7da420782c2915cf81"
TOKENS_SHA256="b818a60878b9aae978cbb8ad594acbd403d76d1af2e31ef4197c84e2dbdba27c"

TEST_WAV_URL="https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20/resolve/main/test_wavs/0.wav"
TEST_WAV_SHA256="7d93384ca14702cc584a7a33fe2fed92e89e708549161cb12ea38c916882103b"

echo "=== Downloading & Verifying Official Echolet Assets (X-ASR 2026 Default) ==="
echo "Repo root:    ${REPO_ROOT}"
echo "Staging dir:  ${STAGING_DIR}"

TMP_WORK_DIR=$(mktemp -d)
trap 'rm -rf "${TMP_WORK_DIR}"' EXIT

mkdir -p "${STAGING_DIR}/runtime/lib"
mkdir -p "${STAGING_DIR}/models/bilingual-zh-en/test_wavs"

SHERPA_ARCHIVE="${TMP_WORK_DIR}/sherpa-onnx-${SHERPA_VERSION}-linux-x64-shared-lib.tar.bz2"

# 1. Download and verify official sherpa-onnx runtime
echo "--> Downloading official sherpa-onnx runtime (${SHERPA_VERSION})..."
curl -L --fail --retry 3 --retry-delay 2 -s -o "${SHERPA_ARCHIVE}" "${SHERPA_URL}"
echo "--> Verifying SHA256 of sherpa runtime archive..."
echo "${SHERPA_SHA256}  ${SHERPA_ARCHIVE}" | sha256sum -c -

echo "--> Staging native libraries into .local-runtime/runtime/lib..."
tar -xjf "${SHERPA_ARCHIVE}" -C "${TMP_WORK_DIR}"
cp -a "${TMP_WORK_DIR}/sherpa-onnx-${SHERPA_VERSION}-linux-x64-shared-lib/lib"/* "${STAGING_DIR}/runtime/lib/"

# 2. Download and verify X-ASR 480ms model files
echo "--> Downloading X-ASR 480ms model files (pinned revision: ${XASR_REV})..."

curl -L --fail --retry 3 --retry-delay 2 -s -o "${TMP_WORK_DIR}/encoder-480ms.onnx" "${XASR_BASE}/encoder-480ms.onnx"
echo "${ENCODER_SHA256}  ${TMP_WORK_DIR}/encoder-480ms.onnx" | sha256sum -c -

curl -L --fail --retry 3 --retry-delay 2 -s -o "${TMP_WORK_DIR}/decoder-480ms.onnx" "${XASR_BASE}/decoder-480ms.onnx"
echo "${DECODER_SHA256}  ${TMP_WORK_DIR}/decoder-480ms.onnx" | sha256sum -c -

curl -L --fail --retry 3 --retry-delay 2 -s -o "${TMP_WORK_DIR}/joiner-480ms.onnx" "${XASR_BASE}/joiner-480ms.onnx"
echo "${JOINER_SHA256}  ${TMP_WORK_DIR}/joiner-480ms.onnx" | sha256sum -c -

curl -L --fail --retry 3 --retry-delay 2 -s -o "${TMP_WORK_DIR}/tokens.txt" "${XASR_BASE}/tokens.txt"
echo "${TOKENS_SHA256}  ${TMP_WORK_DIR}/tokens.txt" | sha256sum -c -

# 3. Download test wav for stream testing
echo "--> Downloading test wav..."
curl -L --fail --retry 3 --retry-delay 2 -s -o "${TMP_WORK_DIR}/0.wav" "${TEST_WAV_URL}"
echo "${TEST_WAV_SHA256}  ${TMP_WORK_DIR}/0.wav" | sha256sum -c -

# 4. Copy model files into .local-runtime/
echo "--> Staging X-ASR model files into .local-runtime/models/bilingual-zh-en..."
cp "${TMP_WORK_DIR}/encoder-480ms.onnx" "${STAGING_DIR}/models/bilingual-zh-en/"
cp "${TMP_WORK_DIR}/decoder-480ms.onnx" "${STAGING_DIR}/models/bilingual-zh-en/"
cp "${TMP_WORK_DIR}/joiner-480ms.onnx" "${STAGING_DIR}/models/bilingual-zh-en/"
cp "${TMP_WORK_DIR}/tokens.txt" "${STAGING_DIR}/models/bilingual-zh-en/"
cp "${TMP_WORK_DIR}/0.wav" "${STAGING_DIR}/models/bilingual-zh-en/test_wavs/"

# 5. Copy manifest & registry
cp "${REPO_ROOT}/model.json" "${STAGING_DIR}/models/bilingual-zh-en/model.json"
cp "${REPO_ROOT}/model.json" "${STAGING_DIR}/model.json"
mkdir -p "${STAGING_DIR}/models"
cp "${REPO_ROOT}/models/registry.json" "${STAGING_DIR}/models/registry.json"

echo "=== Official assets staged successfully into .local-runtime/ ==="
ls -lh "${STAGING_DIR}/runtime/lib"
ls -lh "${STAGING_DIR}/models/bilingual-zh-en"
