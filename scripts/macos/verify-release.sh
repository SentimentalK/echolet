#!/usr/bin/env bash
set -euo pipefail

# This script verifies that the staged macOS Echolet.app satisfies all release contracts.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
APP_DIR="${REPO_ROOT}/dist/Echolet.app"

echo "=== Verifying Echolet macOS Release Bundle ==="
echo "Target app: ${APP_DIR}"

if [[ ! -d "${APP_DIR}" ]]; then
    echo "[Error] Staged Echolet.app not found at: ${APP_DIR}" >&2
    exit 1
fi

# 1. Check binary executable permission
echo "--> Checking binary executable permission..."
if [[ ! -x "${APP_DIR}/Contents/MacOS/echolet" ]]; then
    echo "[Error] ${APP_DIR}/Contents/MacOS/echolet is not executable!" >&2
    exit 1
fi

# 2. Check Info.plist
echo "--> Validating Info.plist..."
if [[ ! -f "${APP_DIR}/Contents/Info.plist" ]]; then
    echo "[Error] Missing Info.plist!" >&2
    exit 1
fi
if command -v plutil >/dev/null 2>&1; then
    plutil -lint "${APP_DIR}/Contents/Info.plist"
fi

# 3. Check dynamic library resolution (otool -L)
if command -v otool >/dev/null 2>&1; then
    echo "--> Checking dynamic library dependencies via otool..."

    check_dependencies() {
        local binary="$1"
        local output deps

        output=$(otool -L "${binary}" 2>&1)
        echo "${output}"

        # First line is the target binary itself, not a dependency.
        deps=$(printf '%s\n' "${output}" | sed '1d')

        if printf '%s\n' "${deps}" | grep -E "/Users/|/home/|\.local-runtime|/usr/local/Cellar|/opt/homebrew" >/dev/null; then
            echo "[Error] Leaked developer or build paths detected in: ${binary}!" >&2
            exit 1
        fi
    }

    check_dependencies "${APP_DIR}/Contents/MacOS/echolet"

    for dylib in "${APP_DIR}/Contents/Frameworks"/*.dylib*; do
        if [[ -f "${dylib}" ]]; then
            check_dependencies "${dylib}"
        fi
    done
fi

# 4. Check model files, manifest, and registry
echo "--> Checking model manifest, registry, and files..."
if [[ ! -f "${APP_DIR}/Contents/Resources/model.json" ]]; then
    echo "[Error] Missing model.json manifest!" >&2
    exit 1
fi

if [[ ! -f "${APP_DIR}/Contents/Resources/models/registry.json" ]]; then
    echo "[Error] Missing models/registry.json!" >&2
    exit 1
fi

MODEL_FILES=(
    "encoder-480ms.onnx"
    "decoder-480ms.onnx"
    "joiner-480ms.onnx"
    "tokens.txt"
)

for mf in "${MODEL_FILES[@]}"; do
    TARGET_MF="${APP_DIR}/Contents/Resources/models/bilingual-zh-en/${mf}"
    if [[ ! -f "${TARGET_MF}" || ! -s "${TARGET_MF}" ]]; then
        echo "[Error] Missing or empty model file: ${TARGET_MF}" >&2
        exit 1
    fi
done

# 5. Ensure test_wavs is excluded
if [[ -d "${APP_DIR}/Contents/Resources/models/bilingual-zh-en/test_wavs" ]]; then
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
    TARGET_LIC="${APP_DIR}/Contents/Resources/licenses/${lic}"
    if [[ ! -f "${TARGET_LIC}" || ! -s "${TARGET_LIC}" ]]; then
        echo "[Error] Missing or empty license file: ${TARGET_LIC}" >&2
        exit 1
    fi
done

# 7. Check code signature
if command -v codesign >/dev/null 2>&1; then
    echo "--> Verifying code signature..."
    codesign --verify --deep --strict "${APP_DIR}"
    echo "--> Code signature verification: PASSED"
fi

echo "=== All Echolet macOS release verification checks PASSED! ==="
