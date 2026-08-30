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

# 2. Build release binary
echo "--> Compiling release binary..."
cd "${REPO_ROOT}"
cargo build --release

# 3. Clean and create dist directory
rm -rf "${DIST_DIR}"
mkdir -p "${DIST_DIR}/runtime/lib"
mkdir -p "${DIST_DIR}/models/bilingual-zh-en/test_wavs"

# 4. Copy binary
echo "--> Copying executable..."
cp "${REPO_ROOT}/target/release/echolet" "${DIST_DIR}/echolet"
chmod +x "${DIST_DIR}/echolet"

# 5. Copy native libraries
echo "--> Copying native runtime libraries..."
cp -a "${LOCAL_RUNTIME}/runtime/lib"/*.so* "${DIST_DIR}/runtime/lib/"

# 6. Copy models
echo "--> Copying models..."
cp -a "${LOCAL_RUNTIME}/models/bilingual-zh-en"/* "${DIST_DIR}/models/bilingual-zh-en/"

# 7. Copy model manifest
echo "--> Copying manifest..."
cp "${REPO_ROOT}/model.json" "${DIST_DIR}/model.json"

# 8. Sanity check bundle completeness
echo "--> Validating bundle structure..."
REQUIRED_FILES=(
    "${DIST_DIR}/echolet"
    "${DIST_DIR}/model.json"
    "${DIST_DIR}/runtime/lib/libsherpa-onnx-c-api.so"
    "${DIST_DIR}/runtime/lib/libonnxruntime.so"
    "${DIST_DIR}/models/bilingual-zh-en/encoder-epoch-99-avg-1.int8.onnx"
    "${DIST_DIR}/models/bilingual-zh-en/decoder-epoch-99-avg-1.onnx"
    "${DIST_DIR}/models/bilingual-zh-en/joiner-epoch-99-avg-1.int8.onnx"
    "${DIST_DIR}/models/bilingual-zh-en/tokens.txt"
)

for file in "${REQUIRED_FILES[@]}"; do
    if [[ ! -f "${file}" ]]; then
        echo "[Error] Missing expected bundle file: ${file}" >&2
        exit 1
    fi
done

echo "=== Echolet Bundle created successfully at: ${DIST_DIR} ==="
ls -lh "${DIST_DIR}"
ls -lh "${DIST_DIR}/runtime/lib"
ls -lh "${DIST_DIR}/models/bilingual-zh-en"
