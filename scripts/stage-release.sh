#!/usr/bin/env bash
set -euo pipefail

# This script creates a self-contained production bundle at dist/echolet-linux-x64/

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="${REPO_ROOT}/dist/echolet-linux-x64"
LOCAL_RUNTIME="${REPO_ROOT}/.local-runtime"

echo "=== Building & Staging Echolet Production Bundle ==="
echo "Repo root: ${REPO_ROOT}"
echo "Dist target: ${DIST_DIR}"

# 1. Ensure local staging assets exist
if [[ ! -d "${LOCAL_RUNTIME}/runtime/lib" || ! -d "${LOCAL_RUNTIME}/models/bilingual-zh-en" ]]; then
    echo "--> Local assets not found. Running prepare-local-assets.sh first..."
    "${REPO_ROOT}/scripts/prepare-local-assets.sh"
fi

# 2. Build release binary with pure production RPATH ($ORIGIN/runtime/lib)
echo "--> Compiling release binary with ECHOLET_BUNDLE_BUILD=1..."
cd "${REPO_ROOT}"
ECHOLET_BUNDLE_BUILD=1 cargo build --release

# 3. Clean and create dist directory structure
rm -rf "${DIST_DIR}"
mkdir -p "${DIST_DIR}/runtime/lib"
mkdir -p "${DIST_DIR}/models/bilingual-zh-en"
mkdir -p "${DIST_DIR}/licenses"

# 4. Copy binary
echo "--> Copying executable..."
cp "${REPO_ROOT}/target/release/echolet" "${DIST_DIR}/echolet"
chmod +x "${DIST_DIR}/echolet"

# 5. Copy native libraries
echo "--> Copying native runtime libraries..."
cp -a "${LOCAL_RUNTIME}/runtime/lib"/*.so* "${DIST_DIR}/runtime/lib/"

# 6. Copy models (excluding test_wavs for clean production bundle)
echo "--> Copying model files (excluding test_wavs)..."
cp "${LOCAL_RUNTIME}/models/bilingual-zh-en/encoder-epoch-99-avg-1.int8.onnx" "${DIST_DIR}/models/bilingual-zh-en/"
cp "${LOCAL_RUNTIME}/models/bilingual-zh-en/decoder-epoch-99-avg-1.onnx" "${DIST_DIR}/models/bilingual-zh-en/"
cp "${LOCAL_RUNTIME}/models/bilingual-zh-en/joiner-epoch-99-avg-1.int8.onnx" "${DIST_DIR}/models/bilingual-zh-en/"
cp "${LOCAL_RUNTIME}/models/bilingual-zh-en/tokens.txt" "${DIST_DIR}/models/bilingual-zh-en/"

# 7. Copy model manifest
echo "--> Copying manifest..."
cp "${REPO_ROOT}/model.json" "${DIST_DIR}/model.json"

# 8. Copy open-source licenses
echo "--> Copying licenses..."
cp -a "${REPO_ROOT}/licenses"/* "${DIST_DIR}/licenses/"

# 9. Sanity check bundle completeness
echo "--> Validating production bundle structure..."
REQUIRED_FILES=(
    "${DIST_DIR}/echolet"
    "${DIST_DIR}/model.json"
    "${DIST_DIR}/runtime/lib/libsherpa-onnx-c-api.so"
    "${DIST_DIR}/runtime/lib/libonnxruntime.so"
    "${DIST_DIR}/models/bilingual-zh-en/encoder-epoch-99-avg-1.int8.onnx"
    "${DIST_DIR}/models/bilingual-zh-en/decoder-epoch-99-avg-1.onnx"
    "${DIST_DIR}/models/bilingual-zh-en/joiner-epoch-99-avg-1.int8.onnx"
    "${DIST_DIR}/models/bilingual-zh-en/tokens.txt"
    "${DIST_DIR}/licenses/sherpa-onnx-LICENSE"
    "${DIST_DIR}/licenses/onnxruntime-LICENSE"
    "${DIST_DIR}/licenses/model-LICENSE"
)

for file in "${REQUIRED_FILES[@]}"; do
    if [[ ! -f "${file}" ]]; then
        echo "[Error] Missing expected bundle file: ${file}" >&2
        exit 1
    fi
done

# Ensure test_wavs is NOT in production bundle
if [[ -d "${DIST_DIR}/models/bilingual-zh-en/test_wavs" ]]; then
    echo "[Error] test_wavs should not be present in production release bundle!" >&2
    exit 1
fi

echo "=== Echolet Production Bundle staged successfully at: ${DIST_DIR} ==="
ls -lh "${DIST_DIR}"
ls -lh "${DIST_DIR}/runtime/lib"
ls -lh "${DIST_DIR}/models/bilingual-zh-en"
ls -lh "${DIST_DIR}/licenses"
