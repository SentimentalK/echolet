#!/usr/bin/env bash
set -euo pipefail

# This script creates a self-contained macOS Application Bundle at dist/Echolet.app

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

DIST_DIR="${REPO_ROOT}/dist"
APP_DIR="${DIST_DIR}/Echolet.app"
LOCAL_RUNTIME="${REPO_ROOT}/.local-runtime"

echo "=== Building & Staging Echolet macOS App Bundle (${ARCH}) ==="
echo "Repo root:  ${REPO_ROOT}"
echo "App target: ${APP_DIR}"

# 2. Ensure local staging assets exist (runtime libraries only)
if [[ ! -d "${LOCAL_RUNTIME}/runtime/lib" || ! -f "${LOCAL_RUNTIME}/runtime/lib/libsherpa-onnx-c-api.dylib" ]]; then
    echo "--> Local runtime libraries not found. Running prepare-assets.sh --runtime-only first..."
    "${REPO_ROOT}/scripts/macos/prepare-assets.sh" "${ARCH}" --runtime-only
fi

# 3. Build release binary with bundle RPATH (@executable_path/../Frameworks)
echo "--> Compiling release binary with ECHOLET_BUNDLE_BUILD=1..."
cd "${REPO_ROOT}"
ECHOLET_BUNDLE_BUILD=1 cargo build --release

# 4. Clean and create bundle directory layout
rm -rf "${APP_DIR}"
mkdir -p "${APP_DIR}/Contents/MacOS"
mkdir -p "${APP_DIR}/Contents/Frameworks"
mkdir -p "${APP_DIR}/Contents/Resources/models"
mkdir -p "${APP_DIR}/Contents/Resources/licenses"

# 5. Copy executable
echo "--> Copying executable..."
cp "${REPO_ROOT}/target/release/echolet" "${APP_DIR}/Contents/MacOS/echolet"
chmod +x "${APP_DIR}/Contents/MacOS/echolet"

# 6. Copy native dynamic libraries (full dependency closure)
echo "--> Copying native runtime libraries into Contents/Frameworks/..."
cp -a "${LOCAL_RUNTIME}/runtime/lib"/*.dylib* "${APP_DIR}/Contents/Frameworks/"

# 7. Copy model catalog metadata (registry.json only, no weights)
echo "--> Copying model registry..."
cp "${REPO_ROOT}/models/registry.json" "${APP_DIR}/Contents/Resources/models/registry.json"

# 8. Copy licenses
echo "--> Copying licenses..."
cp -a "${REPO_ROOT}/licenses"/* "${APP_DIR}/Contents/Resources/licenses/"

# 9. Copy application icon
if [[ -f "${REPO_ROOT}/assets/macos/Echolet.icns" ]]; then
    echo "--> Copying application icon..."
    cp "${REPO_ROOT}/assets/macos/Echolet.icns" "${APP_DIR}/Contents/Resources/Echolet.icns"
fi

# 10. Generate Info.plist
echo "--> Writing Info.plist..."
cat << 'EOF' > "${APP_DIR}/Contents/Info.plist"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Echolet</string>
    <key>CFBundleDisplayName</key>
    <string>Echolet</string>
    <key>CFBundleIdentifier</key>
    <string>com.echolet.app</string>
    <key>CFBundleExecutable</key>
    <string>echolet</string>
    <key>CFBundleIconFile</key>
    <string>Echolet</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>1.0.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSMicrophoneUsageDescription</key>
    <string>Echolet uses the microphone for local real-time speech recognition.</string>
</dict>
</plist>
EOF

# 11. Nested Code Signing (Frameworks -> Executable -> App Bundle)
if command -v codesign >/dev/null 2>&1; then
    echo "--> Performing nested ad-hoc code signing..."
    for dylib in "${APP_DIR}/Contents/Frameworks"/*.dylib*; do
        if [[ -f "${dylib}" ]]; then
            codesign --force --sign - "${dylib}"
        fi
    done
    codesign --force --sign - "${APP_DIR}/Contents/MacOS/echolet"
    codesign --force --sign - "${APP_DIR}"
fi

# 12. Sanity check bundle completeness
echo "--> Validating macOS app bundle structure..."
REQUIRED_FILES=(
    "${APP_DIR}/Contents/MacOS/echolet"
    "${APP_DIR}/Contents/Info.plist"
    "${APP_DIR}/Contents/Resources/models/registry.json"
    "${APP_DIR}/Contents/Frameworks/libsherpa-onnx-c-api.dylib"
    "${APP_DIR}/Contents/Frameworks/libonnxruntime.dylib"
    "${APP_DIR}/Contents/Resources/licenses/sherpa-onnx-LICENSE"
    "${APP_DIR}/Contents/Resources/licenses/onnxruntime-LICENSE"
    "${APP_DIR}/Contents/Resources/licenses/model-LICENSE"
    "${APP_DIR}/Contents/Resources/licenses/lucide-LICENSE"
)

for file in "${REQUIRED_FILES[@]}"; do
    if [[ ! -f "${file}" ]]; then
        echo "[Error] Missing expected bundle file: ${file}" >&2
        exit 1
    fi
done

# Ensure model payload files and directories are NOT in production bundle
if [[ -f "${APP_DIR}/Contents/Resources/model.json" ]]; then
    echo "[Error] root model.json found in macOS App Bundle!" >&2
    exit 1
fi

if [[ -d "${APP_DIR}/Contents/Resources/models/bilingual-zh-en" ]]; then
    echo "[Error] models/bilingual-zh-en directory found in macOS App Bundle!" >&2
    exit 1
fi

if find "${APP_DIR}" -name "*.onnx" | grep -q .; then
    echo "[Error] ONNX model files found in macOS App Bundle!" >&2
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

echo "=== Echolet macOS App Bundle staged successfully at: ${APP_DIR} ==="
ls -lh "${APP_DIR}/Contents/MacOS"
ls -lh "${APP_DIR}/Contents/Frameworks"
ls -lh "${APP_DIR}/Contents/Resources/models"
ls -lh "${APP_DIR}/Contents/Resources/licenses"
