#!/usr/bin/env bash
set -euo pipefail

# This script orchestrates the full release build, verification, and packaging of echolet-linux-${ARCH}.tar.gz

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
DIST_DIR="${REPO_ROOT}/dist"
ARCHIVE_PATH="${DIST_DIR}/${DIST_NAME}.tar.gz"

echo "============================================================"
echo " Building Echolet Linux Portable Release (${DIST_NAME})"
echo "============================================================"

# 1. Run staging script (builds release binary & stages dist/echolet-linux-${ARCH})
"${REPO_ROOT}/scripts/stage-release.sh" "${ARCH}"

# 2. Run verification script
"${REPO_ROOT}/scripts/linux/verify-release.sh" "${ARCH}"

# 3. Create deterministic tar.gz archive
echo "--> Creating production archive: ${ARCHIVE_PATH}..."
rm -f "${ARCHIVE_PATH}"

cd "${DIST_DIR}"
tar --owner=0 --group=0 --numeric-owner -czf "${ARCHIVE_PATH}" "${DIST_NAME}"

echo "============================================================"
echo " Release Build Complete (${DIST_NAME})!"
echo " Output Archive: ${ARCHIVE_PATH}"
ls -lh "${ARCHIVE_PATH}"
echo -n " SHA256 Checksum: "
sha256sum "${ARCHIVE_PATH}"
echo "============================================================"
