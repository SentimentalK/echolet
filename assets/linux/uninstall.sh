#!/usr/bin/env bash
set -euo pipefail

# Echolet Linux Desktop Integration Uninstaller

PREFIX="${1:-${HOME}/.local}"
APP_DIR="${PREFIX}/share/echolet"
BIN_DIR="${PREFIX}/bin"
DESKTOP_DIR="${PREFIX}/share/applications"
ICON_DIR="${PREFIX}/share/icons/hicolor"

echo "============================================================"
echo " Uninstalling Echolet from ${PREFIX}..."
echo "============================================================"

# Remove symlink if it points to echolet
if [[ -L "${BIN_DIR}/echolet" ]]; then
    rm -f "${BIN_DIR}/echolet"
    echo "--> Removed ${BIN_DIR}/echolet symlink"
fi

# Remove application directory
if [[ -d "${APP_DIR}" ]]; then
    rm -rf "${APP_DIR}"
    echo "--> Removed ${APP_DIR}"
fi

# Remove desktop entry
if [[ -f "${DESKTOP_DIR}/echolet.desktop" ]]; then
    rm -f "${DESKTOP_DIR}/echolet.desktop"
    echo "--> Removed ${DESKTOP_DIR}/echolet.desktop"
fi

# Remove icons
rm -f "${ICON_DIR}/512x512/apps/echolet.png"
rm -f "${ICON_DIR}/scalable/apps/echolet.svg"
echo "--> Removed desktop icons"

# Update caches
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "${DESKTOP_DIR}" 2>/dev/null || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "${ICON_DIR}" 2>/dev/null || true
fi

echo "============================================================"
echo " Echolet uninstalled successfully."
echo "============================================================"
