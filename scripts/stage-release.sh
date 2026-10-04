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

# 2. Ensure local staging assets exist (runtime libraries only)
if [[ ! -d "${LOCAL_RUNTIME}/runtime/lib" || ! -f "${LOCAL_RUNTIME}/runtime/lib/libsherpa-onnx-c-api.so" ]]; then
    echo "--> Local runtime libraries not found. Running prepare-local-assets.sh --runtime-only first..."
    "${REPO_ROOT}/scripts/prepare-local-assets.sh" "${ARCH}" --runtime-only
fi

# 3. Build release binary with pure production RPATH ($ORIGIN/runtime/lib)
echo "--> Compiling release binary with ECHOLET_BUNDLE_BUILD=1..."
cd "${REPO_ROOT}"
ECHOLET_BUNDLE_BUILD=1 cargo build --release

# 4. Clean and create dist directory structure
rm -rf "${DIST_DIR}"
mkdir -p "${DIST_DIR}/runtime/lib"
mkdir -p "${DIST_DIR}/models"
mkdir -p "${DIST_DIR}/licenses"

# 5. Copy binary
echo "--> Copying executable..."
cp "${REPO_ROOT}/target/release/echolet" "${DIST_DIR}/echolet"
chmod +x "${DIST_DIR}/echolet"

# 6. Copy native libraries
echo "--> Copying native runtime libraries..."
cp -a "${LOCAL_RUNTIME}/runtime/lib"/*.so* "${DIST_DIR}/runtime/lib/"

# 7. Copy model catalog metadata (registry.json only, no weights)
echo "--> Copying model registry..."
cp "${REPO_ROOT}/models/registry.json" "${DIST_DIR}/models/registry.json"

# 8. Copy open-source licenses
echo "--> Copying licenses..."
cp -a "${REPO_ROOT}/licenses"/* "${DIST_DIR}/licenses/"

# 9. Sanity check bundle completeness
echo "--> Validating production bundle structure..."
REQUIRED_FILES=(
    "${DIST_DIR}/echolet"
    "${DIST_DIR}/models/registry.json"
    "${DIST_DIR}/runtime/lib/libsherpa-onnx-c-api.so"
    "${DIST_DIR}/runtime/lib/libonnxruntime.so"
    "${DIST_DIR}/licenses/sherpa-onnx-LICENSE"
    "${DIST_DIR}/licenses/onnxruntime-LICENSE"
    "${DIST_DIR}/licenses/model-LICENSE"
    "${DIST_DIR}/licenses/lucide-LICENSE"
)

for file in "${REQUIRED_FILES[@]}"; do
    if [[ ! -f "${file}" ]]; then
        echo "[Error] Missing expected bundle file: ${file}" >&2
        exit 1
    fi
done

# Ensure model payload files and directories are NOT in production bundle
if [[ -f "${DIST_DIR}/model.json" ]]; then
    echo "[Error] root model.json found in production release!" >&2
    exit 1
fi

if [[ -d "${DIST_DIR}/models/bilingual-zh-en" ]]; then
    echo "[Error] models/bilingual-zh-en directory found in production release!" >&2
    exit 1
fi

if find "${DIST_DIR}" -name "*.onnx" | grep -q .; then
    echo "[Error] ONNX model files found in production release bundle!" >&2
    exit 1
fi

if find "${DIST_DIR}" -name "tokens.txt" | grep -q .; then
    echo "[Error] tokens.txt found in production release bundle!" >&2
    exit 1
fi

if [[ -d "${DIST_DIR}/models/test_wavs" || -d "${DIST_DIR}/models/bilingual-zh-en/test_wavs" ]]; then
    echo "[Error] test_wavs directory found in production release!" >&2
    exit 1
fi

echo "=== Echolet Production Bundle staged successfully at: ${DIST_DIR} ==="
ls -lh "${DIST_DIR}"
ls -lh "${DIST_DIR}/runtime/lib"
ls -lh "${DIST_DIR}/models"
ls -lh "${DIST_DIR}/licenses"
