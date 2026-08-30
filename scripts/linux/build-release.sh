#!/usr/bin/env bash
set -euo pipefail

# This script orchestrates the full release build, verification, and packaging of echolet-linux-x64.tar.gz

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DIST_DIR="${REPO_ROOT}/dist"
ARCHIVE_PATH="${DIST_DIR}/echolet-linux-x64.tar.gz"

echo "============================================================"
echo " Building Echolet Linux Portable Release"
echo "============================================================"

# 1. Run staging script (builds release binary & stages dist/echolet-linux-x64)
"${REPO_ROOT}/scripts/stage-release.sh"

# 2. Run verification script
"${REPO_ROOT}/scripts/linux/verify-release.sh"

# 3. Create deterministic tar.gz archive
echo "--> Creating production archive: ${ARCHIVE_PATH}..."
rm -f "${ARCHIVE_PATH}"

cd "${DIST_DIR}"
tar --owner=0 --group=0 --numeric-owner -czf "${ARCHIVE_PATH}" "echolet-linux-x64"

echo "============================================================"
echo " Release Build Complete!"
echo " Output Archive: ${ARCHIVE_PATH}"
ls -lh "${ARCHIVE_PATH}"
echo -n " SHA256 Checksum: "
sha256sum "${ARCHIVE_PATH}"
echo "============================================================"
