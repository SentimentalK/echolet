#!/usr/bin/env bash
set -euo pipefail

# This script verifies that the staged release bundle satisfies all production release contracts.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

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

echo "=== Verifying Echolet Linux Production Release Bundle (${DIST_NAME}) ==="
echo "Target directory: ${DIST_DIR}"

if [[ ! -d "${DIST_DIR}" ]]; then
    echo "[Error] Staged directory not found at: ${DIST_DIR}" >&2
    exit 1
fi

# 1. Check binary executable permission
echo "--> Checking binary executable permission..."
if [[ ! -x "${DIST_DIR}/echolet" ]]; then
    echo "[Error] ${DIST_DIR}/echolet is not executable!" >&2
    exit 1
fi

# 2. Check RUNPATH
echo "--> Verifying clean production RUNPATH..."
RUNPATH=$(readelf -d "${DIST_DIR}/echolet" | grep -E "RPATH|RUNPATH" || true)
echo "    Found: ${RUNPATH}"

if [[ -z "${RUNPATH}" ]]; then
    echo "[Error] RUNPATH is missing in release binary!" >&2
    exit 1
fi

if echo "${RUNPATH}" | grep -E "/home|\.local-runtime|target" >/dev/null; then
    echo "[Error] RUNPATH contains host machine / development paths!" >&2
    exit 1
fi

if ! echo "${RUNPATH}" | grep -F '$ORIGIN/runtime/lib' >/dev/null; then
    echo "[Error] RUNPATH does not point to \$ORIGIN/runtime/lib!" >&2
    exit 1
fi

# 3. Check dynamic library resolution closure (ldd)
echo "--> Checking dynamic library dependencies (ldd closure)..."
LDD_OUTPUT=$(LD_LIBRARY_PATH="${DIST_DIR}/runtime/lib" ldd "${DIST_DIR}/echolet" 2>&1)

if echo "${LDD_OUTPUT}" | grep "not found" >/dev/null; then
    echo "[Error] Unresolved native dependencies found:" >&2
    echo "${LDD_OUTPUT}" | grep "not found" >&2
    exit 1
fi

# 4. Check model registry and catalog metadata
echo "--> Checking model registry and catalog metadata..."
if [[ ! -f "${DIST_DIR}/models/registry.json" ]]; then
    echo "[Error] Missing models/registry.json!" >&2
    exit 1
fi

if grep -q '"bundled": true' "${DIST_DIR}/models/registry.json"; then
    echo "[Error] models/registry.json still marks model as bundled: true!" >&2
    exit 1
fi
if ! grep -q '"bundled": false' "${DIST_DIR}/models/registry.json"; then
    echo "[Error] models/registry.json does not mark model as bundled: false!" >&2
    exit 1
fi

if [[ -f "${DIST_DIR}/model.json" ]]; then
    echo "[Error] root model.json found in production release!" >&2
    exit 1
fi

# 5. Assert that model payload files/directories are absent
echo "--> Verifying absence of model weight payload..."
if [[ -d "${DIST_DIR}/models/bilingual-zh-en" ]]; then
    echo "[Error] models/bilingual-zh-en directory found in release bundle!" >&2
    exit 1
fi

if find "${DIST_DIR}" -name "*.onnx" | grep -q .; then
    echo "[Error] ONNX model files found in release bundle!" >&2
    find "${DIST_DIR}" -name "*.onnx" >&2
    exit 1
fi

if find "${DIST_DIR}" -name "tokens.txt" | grep -q .; then
    echo "[Error] tokens.txt found in release bundle!" >&2
    exit 1
fi

if [[ -d "${DIST_DIR}/models/test_wavs" || -d "${DIST_DIR}/models/bilingual-zh-en/test_wavs" ]]; then
    echo "[Error] test_wavs directory found in production release!" >&2
    exit 1
fi

# 6. Check license files
echo "--> Checking license notices..."
LICENSES=(
    "sherpa-onnx-LICENSE"
    "onnxruntime-LICENSE"
    "model-LICENSE"
    "lucide-LICENSE"
)

for lic in "${LICENSES[@]}"; do
    TARGET_LIC="${DIST_DIR}/licenses/${lic}"
    if [[ ! -f "${TARGET_LIC}" || ! -s "${TARGET_LIC}" ]]; then
        echo "[Error] Missing or empty license file: ${TARGET_LIC}" >&2
        exit 1
    fi
done

# 7. Check for host path leakage in text files
echo "--> Checking for host path leakage..."
if grep -rn "/home/sentimentalk/sherpa-onnx" "${DIST_DIR}/licenses" "${DIST_DIR}/models/registry.json" >/dev/null 2>&1; then
    echo "[Error] Host path leaked into release metadata/licenses!" >&2
    exit 1
fi

# 8. Print final bundle size
echo "--> Staged package size:"
du -sh "${DIST_DIR}"

echo "=== All production release verification checks PASSED (${DIST_NAME})! ==="
