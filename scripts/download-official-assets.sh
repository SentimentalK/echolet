#!/usr/bin/env bash
set -euo pipefail

# This script downloads official platform runtime libraries and acquires the frozen Echolet Base Model.
# NOTE: This script does NOT touch Hugging Face. All ASR model assets are acquired through acquire-base-model.sh.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGING_DIR="${REPO_ROOT}/.local-runtime"

SHERPA_VERSION="v1.13.6"
SHERPA_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/${SHERPA_VERSION}/sherpa-onnx-${SHERPA_VERSION}-linux-x64-shared-lib.tar.bz2"
SHERPA_SHA256="bbeb203da0f69e37235b50e168d61d1f64ad2de256490cc64ed5535957415a97"

TEST_WAV_URL="https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20/resolve/main/test_wavs/0.wav"
TEST_WAV_SHA256="7d93384ca14702cc584a7a33fe2fed92e89e708549161cb12ea38c916882103b"

echo "=== Downloading & Verifying Official Echolet Assets ==="
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

# 2. Acquire Echolet Base Model via frozen model lock (No upstream Hugging Face!)
echo "--> Acquiring Echolet Base Model (r1)..."
"${REPO_ROOT}/scripts/acquire-base-model.sh" "${STAGING_DIR}/models/bilingual-zh-en"

# 3. Download test wav for stream testing
echo "--> Downloading test wav..."
curl -L --fail --retry 3 --retry-delay 2 -s -o "${TMP_WORK_DIR}/0.wav" "${TEST_WAV_URL}"
echo "${TEST_WAV_SHA256}  ${TMP_WORK_DIR}/0.wav" | sha256sum -c -
cp "${TMP_WORK_DIR}/0.wav" "${STAGING_DIR}/models/bilingual-zh-en/test_wavs/"

# 4. Copy manifest & registry
cp "${REPO_ROOT}/model.json" "${STAGING_DIR}/models/bilingual-zh-en/model.json"
cp "${REPO_ROOT}/model.json" "${STAGING_DIR}/model.json"
mkdir -p "${STAGING_DIR}/models"
cp "${REPO_ROOT}/models/registry.json" "${STAGING_DIR}/models/registry.json"

echo "=== Official assets staged successfully into .local-runtime/ ==="
ls -lh "${STAGING_DIR}/runtime/lib"
ls -lh "${STAGING_DIR}/models/bilingual-zh-en"
