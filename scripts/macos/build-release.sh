#!/usr/bin/env bash
set -euo pipefail

# This script orchestrates the full release build, verification, and packaging of echolet-macos-${ARCH}.zip

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
        echo "[Error] Unsupported macOS architecture: ${RAW_ARCH}" >&2
        exit 1
        ;;
esac

DIST_DIR="${REPO_ROOT}/dist"
ARCHIVE_NAME="echolet-macos-${ARCH}.zip"
ARCHIVE_PATH="${DIST_DIR}/${ARCHIVE_NAME}"

compute_sha256() {
    local file_path="$1"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "${file_path}" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "${file_path}" | awk '{print $1}'
    else
        echo "[Error] Neither sha256sum nor shasum found!" >&2
        exit 1
    fi
}

echo "============================================================"
echo " Building Echolet macOS Portable Release (${ARCH})"
echo "============================================================"

# 1. Stage Echolet.app
"${REPO_ROOT}/scripts/macos/stage-release.sh" "${ARCH}"

# 2. Verify Echolet.app
"${REPO_ROOT}/scripts/macos/verify-release.sh"

# 3. Create zip archive
echo "--> Creating release archive: ${ARCHIVE_PATH}..."
rm -f "${ARCHIVE_PATH}"

cd "${DIST_DIR}"
if command -v zip >/dev/null 2>&1; then
    zip -q -r -y "${ARCHIVE_NAME}" "Echolet.app"
else
    tar -czf "${DIST_DIR}/echolet-macos-${ARCH}.tar.gz" "Echolet.app"
    ARCHIVE_PATH="${DIST_DIR}/echolet-macos-${ARCH}.tar.gz"
fi

echo "============================================================"
echo " macOS Release Build Complete (${ARCH})!"
echo " Output Archive: ${ARCHIVE_PATH}"
ls -lh "${ARCHIVE_PATH}"
echo -n " SHA256 Checksum: "
compute_sha256 "${ARCHIVE_PATH}"
echo "============================================================"
