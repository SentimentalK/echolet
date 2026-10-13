#!/usr/bin/env bash
set -euo pipefail

# Echolet Linux Desktop Integration & Installation Script
# Usage:
#   ./install.sh               Install into ~/.local (default, user-level)
#   ./install.sh --in-place    Register desktop entry & icon using current directory in-place
#   ./install.sh /custom/path  Install into custom prefix

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ARG="${1:-}"

IN_PLACE=false
if [[ "${ARG}" == "--in-place" || "${ARG}" == "-i" ]]; then
    IN_PLACE=true
    PREFIX="${HOME}/.local"
elif [[ -n "${ARG}" && "${ARG}" != --* ]]; then
    PREFIX="${ARG}"
else
    PREFIX="${HOME}/.local"
fi

BIN_DIR="${PREFIX}/bin"
APP_DIR="${PREFIX}/share/echolet"
DESKTOP_DIR="${PREFIX}/share/applications"
ICON_DIR="${PREFIX}/share/icons/hicolor"

echo "============================================================"
echo " Installing Echolet Voice Assistant"
echo "============================================================"

if [[ "${IN_PLACE}" == true ]]; then
    echo "Mode: In-place registration (source: ${SCRIPT_DIR})"
    TARGET_BIN="${SCRIPT_DIR}/echolet"
    TARGET_DIR="${SCRIPT_DIR}"
else
    echo "Mode: User installation (prefix: ${PREFIX})"
    TARGET_BIN="${APP_DIR}/echolet"
    TARGET_DIR="${APP_DIR}"

    echo "--> Copying application files to ${APP_DIR}..."
    mkdir -p "${APP_DIR}"
    cp -r "${SCRIPT_DIR}/echolet" "${APP_DIR}/"
    cp -r "${SCRIPT_DIR}/runtime" "${APP_DIR}/"
    cp -r "${SCRIPT_DIR}/models" "${APP_DIR}/"
    cp -r "${SCRIPT_DIR}/licenses" "${APP_DIR}/"
    cp "${SCRIPT_DIR}/echolet.png" "${APP_DIR}/"
    cp "${SCRIPT_DIR}/echolet.png" "${APP_DIR}/icon.png"
    if [[ -f "${SCRIPT_DIR}/echolet.svg" ]]; then
        cp "${SCRIPT_DIR}/echolet.svg" "${APP_DIR}/"
    fi
    chmod +x "${APP_DIR}/echolet"
fi

# Ensure bin link directory exists
mkdir -p "${BIN_DIR}"
echo "--> Creating executable symlink at ${BIN_DIR}/echolet..."
ln -sf "${TARGET_BIN}" "${BIN_DIR}/echolet"

# Install icons into FreeDesktop hicolor theme
echo "--> Installing desktop icons..."
mkdir -p "${ICON_DIR}/512x512/apps"
cp "${SCRIPT_DIR}/echolet.png" "${ICON_DIR}/512x512/apps/echolet.png"

if [[ -f "${SCRIPT_DIR}/echolet.svg" ]]; then
    mkdir -p "${ICON_DIR}/scalable/apps"
    cp "${SCRIPT_DIR}/echolet.svg" "${ICON_DIR}/scalable/apps/echolet.svg"
fi

# Also place icon.png in TARGET_DIR if in-place mode
if [[ "${IN_PLACE}" == true && ! -f "${TARGET_DIR}/icon.png" ]]; then
    cp "${SCRIPT_DIR}/echolet.png" "${TARGET_DIR}/icon.png"
fi

# Install desktop entry
echo "--> Creating desktop shortcut at ${DESKTOP_DIR}/echolet.desktop..."
mkdir -p "${DESKTOP_DIR}"
cat > "${DESKTOP_DIR}/echolet.desktop" << EOF
[Desktop Entry]
Type=Application
Name=Echolet
Comment=Real-time Voice Typing & Speech Recognition
Exec=${TARGET_BIN}
Icon=echolet
Terminal=false
Categories=Utility;AudioVideo;Audio;
StartupWMClass=echolet
StartupNotify=true
Keywords=voice;typing;speech;recognition;asr;dictation;
EOF
chmod +x "${DESKTOP_DIR}/echolet.desktop"

# Update desktop & icon caches if tools exist
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "${DESKTOP_DIR}" 2>/dev/null || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "${ICON_DIR}" 2>/dev/null || true
fi

echo "============================================================"
echo " Installation Complete!"
echo " - Executable: ${TARGET_BIN}"
echo " - Symlink:    ${BIN_DIR}/echolet"
echo " - Desktop:    ${DESKTOP_DIR}/echolet.desktop"
echo " - Icon:       ${ICON_DIR}/512x512/apps/echolet.png"
echo ""
echo " You can now launch Echolet from your application launcher,"
echo " or directly from terminal by typing: echolet"
echo "============================================================"
