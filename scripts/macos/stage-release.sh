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

# 2. Ensure local staging assets exist
if [[ ! -d "${LOCAL_RUNTIME}/runtime/lib" || ! -f "${LOCAL_RUNTIME}/models/bilingual-zh-en/encoder-480ms.onnx" ]]; then
    echo "--> Local assets not found. Running prepare-assets.sh first..."
    "${REPO_ROOT}/scripts/macos/prepare-assets.sh" "${ARCH}"
fi

# 3. Build release binary with bundle RPATH (@executable_path/../Frameworks)
echo "--> Compiling release binary with ECHOLET_BUNDLE_BUILD=1..."
cd "${REPO_ROOT}"
ECHOLET_BUNDLE_BUILD=1 cargo build --release

# 4. Clean and create bundle directory layout
rm -rf "${APP_DIR}"
mkdir -p "${APP_DIR}/Contents/MacOS"
mkdir -p "${APP_DIR}/Contents/Frameworks"
mkdir -p "${APP_DIR}/Contents/Resources/models/bilingual-zh-en"
mkdir -p "${APP_DIR}/Contents/Resources/licenses"

# 5. Copy executable
echo "--> Copying executable..."
cp "${REPO_ROOT}/target/release/echolet" "${APP_DIR}/Contents/MacOS/echolet"
chmod +x "${APP_DIR}/Contents/MacOS/echolet"

# 6. Copy native dynamic libraries (full dependency closure)
echo "--> Copying native runtime libraries into Contents/Frameworks/..."
cp -a "${LOCAL_RUNTIME}/runtime/lib"/*.dylib* "${APP_DIR}/Contents/Frameworks/"

# 7. Copy models (excluding test_wavs)
echo "--> Copying model files..."
cp "${LOCAL_RUNTIME}/models/bilingual-zh-en/encoder-480ms.onnx" "${APP_DIR}/Contents/Resources/models/bilingual-zh-en/"
cp "${LOCAL_RUNTIME}/models/bilingual-zh-en/decoder-480ms.onnx" "${APP_DIR}/Contents/Resources/models/bilingual-zh-en/"
cp "${LOCAL_RUNTIME}/models/bilingual-zh-en/joiner-480ms.onnx" "${APP_DIR}/Contents/Resources/models/bilingual-zh-en/"
cp "${LOCAL_RUNTIME}/models/bilingual-zh-en/tokens.txt" "${APP_DIR}/Contents/Resources/models/bilingual-zh-en/"

# 8. Copy model manifest and registry
echo "--> Copying manifest and registry..."
cp "${REPO_ROOT}/model.json" "${APP_DIR}/Contents/Resources/model.json"
cp "${REPO_ROOT}/model.json" "${APP_DIR}/Contents/Resources/models/bilingual-zh-en/model.json"
cp "${REPO_ROOT}/models/registry.json" "${APP_DIR}/Contents/Resources/models/registry.json"

# 9. Copy licenses
echo "--> Copying licenses..."
cp -a "${REPO_ROOT}/licenses"/* "${APP_DIR}/Contents/Resources/licenses/"

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

echo "=== Echolet macOS App Bundle staged successfully at: ${APP_DIR} ==="
ls -lh "${APP_DIR}/Contents/MacOS"
ls -lh "${APP_DIR}/Contents/Frameworks"
ls -lh "${APP_DIR}/Contents/Resources/models/bilingual-zh-en"
