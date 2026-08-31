#!/usr/bin/env bash
set -euo pipefail

# This script downloads official platform runtime libraries and acquires the frozen Echolet Base Model.
# NOTE: This script does NOT touch Hugging Face. All ASR model assets are acquired through acquire-base-model.sh.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGING_DIR="${REPO_ROOT}/.local-runtime"

# 1. Normalize architecture
RAW_ARCH="${1:-${ECHOLET_ARCH:-$(uname -m)}}"
case "${RAW_ARCH}" in
    x86_64|x64|amd64)
        ARCH="x64"
        SHERPA_ASSET="sherpa-onnx-v1.13.6-linux-x64-shared-lib.tar.bz2"
        SHERPA_SHA256="bbeb203da0f69e37235b50e168d61d1f64ad2de256490cc64ed5535957415a97"
        SHERPA_DIR_NAME="sherpa-onnx-v1.13.6-linux-x64-shared-lib"
        ;;
    aarch64|arm64)
        ARCH="arm64"
        SHERPA_ASSET="sherpa-onnx-v1.13.6-linux-aarch64-shared-cpu-lib.tar.bz2"
        SHERPA_SHA256="3575bde0543da12fc626c814c14287455f70a22b72caa483c7398d5f20f4cb12"
        SHERPA_DIR_NAME="sherpa-onnx-v1.13.6-linux-aarch64-shared-cpu-lib"
        ;;
    *)
        echo "[Error] Unsupported architecture: ${RAW_ARCH}" >&2
        exit 1
        ;;
esac

SHERPA_VERSION="v1.13.6"
SHERPA_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/${SHERPA_VERSION}/${SHERPA_ASSET}"

TEST_WAV_URL="https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20/resolve/main/test_wavs/0.wav"
TEST_WAV_SHA256="7d93384ca14702cc584a7a33fe2fed92e89e708549161cb12ea38c916882103b"

echo "=== Downloading & Verifying Official Echolet Assets (${ARCH}) ==="
echo "Repo root:    ${REPO_ROOT}"
echo "Staging dir:  ${STAGING_DIR}"
echo "Architecture: ${ARCH} (raw: ${RAW_ARCH})"

TMP_WORK_DIR=$(mktemp -d)
trap 'rm -rf "${TMP_WORK_DIR}"' EXIT

mkdir -p "${STAGING_DIR}/runtime/lib"
mkdir -p "${STAGING_DIR}/models/bilingual-zh-en/test_wavs"

SHERPA_ARCHIVE="${TMP_WORK_DIR}/${SHERPA_ASSET}"

# 1. Download and verify official sherpa-onnx runtime
echo "--> Downloading official sherpa-onnx runtime (${SHERPA_VERSION} for linux-${ARCH})..."
curl -L --fail --retry 3 --retry-delay 2 -s -o "${SHERPA_ARCHIVE}" "${SHERPA_URL}"
echo "--> Verifying SHA256 of sherpa runtime archive..."
echo "${SHERPA_SHA256}  ${SHERPA_ARCHIVE}" | sha256sum -c -

echo "--> Staging native libraries into .local-runtime/runtime/lib..."
tar -xjf "${SHERPA_ARCHIVE}" -C "${TMP_WORK_DIR}"
cp -a "${TMP_WORK_DIR}/${SHERPA_DIR_NAME}/lib"/* "${STAGING_DIR}/runtime/lib/"

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

echo "=== Official assets staged successfully into .local-runtime/ (${ARCH}) ==="
ls -lh "${STAGING_DIR}/runtime/lib"
ls -lh "${STAGING_DIR}/models/bilingual-zh-en"
