#!/usr/bin/env bash
set -euo pipefail

# This script verifies that the staged release bundle satisfies all production release contracts.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DIST_DIR="${REPO_ROOT}/dist/echolet-linux-x64"

echo "=== Verifying Echolet Linux Production Release Bundle ==="
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

# 4. Check model files and manifest
echo "--> Checking model manifest and files..."
if [[ ! -f "${DIST_DIR}/model.json" ]]; then
    echo "[Error] Missing model.json manifest!" >&2
    exit 1
fi

MODEL_FILES=(
    "encoder-epoch-99-avg-1.int8.onnx"
    "decoder-epoch-99-avg-1.onnx"
    "joiner-epoch-99-avg-1.int8.onnx"
    "tokens.txt"
)

for mf in "${MODEL_FILES[@]}"; do
    TARGET_MF="${DIST_DIR}/models/bilingual-zh-en/${mf}"
    if [[ ! -f "${TARGET_MF}" || ! -s "${TARGET_MF}" ]]; then
        echo "[Error] Missing or empty model file: ${TARGET_MF}" >&2
        exit 1
    fi
done

# 5. Ensure test_wavs is excluded
if [[ -d "${DIST_DIR}/models/bilingual-zh-en/test_wavs" ]]; then
    echo "[Error] test_wavs directory found in production release!" >&2
    exit 1
fi

# 6. Check license files
echo "--> Checking license notices..."
LICENSES=(
    "sherpa-onnx-LICENSE"
    "onnxruntime-LICENSE"
    "model-LICENSE"
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
if grep -rn "/home/sentimentalk/sherpa-onnx" "${DIST_DIR}/model.json" "${DIST_DIR}/licenses" >/dev/null 2>&1; then
    echo "[Error] Host path leaked into release metadata/licenses!" >&2
    exit 1
fi

echo "=== All production release verification checks PASSED! ==="
