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

if [[ -f "${APP_DIR}/Contents/MacOS/echolet-ui-spike" ]]; then
    echo "[Error] echolet-ui-spike found in production Echolet.app!" >&2
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

# 4. Check model registry and catalog metadata
echo "--> Checking model registry and catalog metadata..."
if [[ ! -f "${APP_DIR}/Contents/Resources/models/registry.json" ]]; then
    echo "[Error] Missing models/registry.json!" >&2
    exit 1
fi

if grep -q '"bundled": true' "${APP_DIR}/Contents/Resources/models/registry.json"; then
    echo "[Error] models/registry.json still marks model as bundled: true!" >&2
    exit 1
fi
if ! grep -q '"bundled": false' "${APP_DIR}/Contents/Resources/models/registry.json"; then
    echo "[Error] models/registry.json does not mark model as bundled: false!" >&2
    exit 1
fi

if [[ -f "${APP_DIR}/Contents/Resources/model.json" ]]; then
    echo "[Error] root model.json found in macOS App Bundle!" >&2
    exit 1
fi

# 5. Assert that model payload files/directories are absent
echo "--> Verifying absence of model weight payload..."
if [[ -d "${APP_DIR}/Contents/Resources/models/bilingual-zh-en" ]]; then
    echo "[Error] models/bilingual-zh-en directory found in macOS App Bundle!" >&2
    exit 1
fi

if find "${APP_DIR}" -name "*.onnx" | grep -q .; then
    echo "[Error] ONNX model files found in macOS App Bundle!" >&2
    find "${APP_DIR}" -name "*.onnx" >&2
    exit 1
fi

if find "${APP_DIR}" -name "tokens.txt" | grep -q .; then
    echo "[Error] tokens.txt found in macOS App Bundle!" >&2
    exit 1
fi

if [[ -d "${APP_DIR}/Contents/Resources/models/test_wavs" || -d "${APP_DIR}/Contents/Resources/models/bilingual-zh-en/test_wavs" ]]; then
    echo "[Error] test_wavs directory found in macOS App Bundle!" >&2
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
    TARGET_LIC="${APP_DIR}/Contents/Resources/licenses/${lic}"
    if [[ ! -f "${TARGET_LIC}" || ! -s "${TARGET_LIC}" ]]; then
        echo "[Error] Missing or empty license file: ${TARGET_LIC}" >&2
        exit 1
    fi
done

# 7. Check application icon
echo "--> Checking application icon..."
if [[ ! -f "${APP_DIR}/Contents/Resources/Echolet.icns" || ! -s "${APP_DIR}/Contents/Resources/Echolet.icns" ]]; then
    echo "[Error] Missing or empty Echolet.icns in Contents/Resources!" >&2
    exit 1
fi

# 8. Check code signature
if command -v codesign >/dev/null 2>&1; then
    echo "--> Verifying code signature..."
    codesign --verify --deep --strict "${APP_DIR}"
    echo "--> Code signature verification: PASSED"
fi

# 9. Print final bundle size
echo "--> Staged macOS App Bundle size:"
du -sh "${APP_DIR}"

echo "=== All Echolet macOS release verification checks PASSED! ==="
