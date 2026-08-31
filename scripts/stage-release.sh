#!/usr/bin/env bash
set -euo pipefail

# This script creates a self-contained production bundle at dist/echolet-linux-${ARCH}/

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# 1. Normalize architecture
RAW_ARCH="${1:-${ECHOLET_ARCH:-$(uname -m)}}"
case "${RAW_ARCH}" in
    x86_64|x64|amd64)
        ARCH="x64"
        ;;
    aarch64|arm64)
        ARCH="arm64"
        ;;
    *)
        echo "[Error] Unsupported architecture: ${RAW_ARCH}" >&2
        exit 1
        ;;
esac

DIST_NAME="echolet-linux-${ARCH}"
DIST_DIR="${REPO_ROOT}/dist/${DIST_NAME}"
LOCAL_RUNTIME="${REPO_ROOT}/.local-runtime"

echo "=== Building & Staging Echolet Production Bundle (${DIST_NAME}) ==="
echo "Repo root:   ${REPO_ROOT}"
echo "Dist target: ${DIST_DIR}"

# 2. Ensure local staging assets exist
if [[ ! -d "${LOCAL_RUNTIME}/runtime/lib" || ! -f "${LOCAL_RUNTIME}/models/bilingual-zh-en/encoder-480ms.onnx" ]]; then
    echo "--> Local assets not found. Running prepare-local-assets.sh first..."
    "${REPO_ROOT}/scripts/prepare-local-assets.sh" "${ARCH}"
fi

# 3. Build release binary with pure production RPATH ($ORIGIN/runtime/lib)
echo "--> Compiling release binary with ECHOLET_BUNDLE_BUILD=1..."
cd "${REPO_ROOT}"
ECHOLET_BUNDLE_BUILD=1 cargo build --release

# 4. Clean and create dist directory structure
rm -rf "${DIST_DIR}"
mkdir -p "${DIST_DIR}/runtime/lib"
mkdir -p "${DIST_DIR}/models/bilingual-zh-en"
mkdir -p "${DIST_DIR}/licenses"

# 5. Copy binary
echo "--> Copying executable..."
cp "${REPO_ROOT}/target/release/echolet" "${DIST_DIR}/echolet"
chmod +x "${DIST_DIR}/echolet"

# 6. Copy native libraries
echo "--> Copying native runtime libraries..."
cp -a "${LOCAL_RUNTIME}/runtime/lib"/*.so* "${DIST_DIR}/runtime/lib/"

# 7. Copy models (excluding test_wavs for clean production bundle)
echo "--> Copying model files (excluding test_wavs)..."
cp "${LOCAL_RUNTIME}/models/bilingual-zh-en/encoder-480ms.onnx" "${DIST_DIR}/models/bilingual-zh-en/"
cp "${LOCAL_RUNTIME}/models/bilingual-zh-en/decoder-480ms.onnx" "${DIST_DIR}/models/bilingual-zh-en/"
cp "${LOCAL_RUNTIME}/models/bilingual-zh-en/joiner-480ms.onnx" "${DIST_DIR}/models/bilingual-zh-en/"
cp "${LOCAL_RUNTIME}/models/bilingual-zh-en/tokens.txt" "${DIST_DIR}/models/bilingual-zh-en/"

# 8. Copy model manifest and registry
echo "--> Copying manifest and registry..."
cp "${REPO_ROOT}/model.json" "${DIST_DIR}/model.json"
cp "${REPO_ROOT}/models/registry.json" "${DIST_DIR}/models/registry.json"

# 9. Copy open-source licenses
echo "--> Copying licenses..."
cp -a "${REPO_ROOT}/licenses"/* "${DIST_DIR}/licenses/"

# 10. Sanity check bundle completeness
echo "--> Validating production bundle structure..."
REQUIRED_FILES=(
    "${DIST_DIR}/echolet"
    "${DIST_DIR}/model.json"
    "${DIST_DIR}/models/registry.json"
    "${DIST_DIR}/runtime/lib/libsherpa-onnx-c-api.so"
    "${DIST_DIR}/runtime/lib/libonnxruntime.so"
    "${DIST_DIR}/models/bilingual-zh-en/encoder-480ms.onnx"
    "${DIST_DIR}/models/bilingual-zh-en/decoder-480ms.onnx"
    "${DIST_DIR}/models/bilingual-zh-en/joiner-480ms.onnx"
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
    echo "[Error] test_wavs directory found in production release!" >&2
    exit 1
fi

echo "=== Echolet Production Bundle staged successfully at: ${DIST_DIR} ==="
ls -lh "${DIST_DIR}"
ls -lh "${DIST_DIR}/runtime/lib"
ls -lh "${DIST_DIR}/models/bilingual-zh-en"
ls -lh "${DIST_DIR}/licenses"
