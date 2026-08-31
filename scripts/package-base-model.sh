#!/usr/bin/env bash
set -euo pipefail

# This script packages a 100% reproducible Echolet Base Model archive (.tar.zst)
# driven entirely by declarative definitions in models/base-model.json.
# NOTE: This script does NOT modify Git repository files automatically.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE_MODEL_DEF="${REPO_ROOT}/models/base-model.json"

if [[ ! -f "${BASE_MODEL_DEF}" ]]; then
    echo "[Error] Base model definition not found at: ${BASE_MODEL_DEF}" >&2
    exit 1
fi

MODEL_ID=$(jq -r '.id' "${BASE_MODEL_DEF}")
ECHOLET_REV="${1:-$(jq -r '.revision' "${BASE_MODEL_DEF}")}"
UPSTREAM_PROJECT=$(jq -r '.upstream_project' "${BASE_MODEL_DEF}")
UPSTREAM_REPO=$(jq -r '.upstream_repository' "${BASE_MODEL_DEF}")
UPSTREAM_REV=$(jq -r '.upstream_revision' "${BASE_MODEL_DEF}")

PKG_NAME="model-${MODEL_ID}-${ECHOLET_REV}"
OUTPUT_ARCHIVE="${REPO_ROOT}/dist/${PKG_NAME}.tar.zst"

echo "=== Reproducible Packaging of Echolet Base Model Artifact ==="
echo "Model ID:         ${MODEL_ID}"
echo "Echolet Revision: ${ECHOLET_REV}"
echo "Upstream Project: ${UPSTREAM_PROJECT}"
echo "Upstream Repo:    ${UPSTREAM_REPO}"
echo "Upstream Rev:     ${UPSTREAM_REV}"
echo "Archive Target:   ${OUTPUT_ARCHIVE}"

TMP_WORK_DIR=$(mktemp -d)
trap 'rm -rf "${TMP_WORK_DIR}"' EXIT

STAGE_DIR="${TMP_WORK_DIR}/${PKG_NAME}"
mkdir -p "${STAGE_DIR}"
mkdir -p "${REPO_ROOT}/dist"

# 1. Dynamically acquire and verify all declared model files
KEYS=$(jq -r '.files | keys[]' "${BASE_MODEL_DEF}")

for key in ${KEYS}; do
    FILE_NAME=$(jq -r ".files[\"${key}\"].name" "${BASE_MODEL_DEF}")
    UPSTREAM_PATH=$(jq -r ".files[\"${key}\"].upstream_path" "${BASE_MODEL_DEF}")
    EXPECTED_SHA=$(jq -r ".files[\"${key}\"].sha256" "${BASE_MODEL_DEF}")
    TARGET="${STAGE_DIR}/${FILE_NAME}"
    LOCAL_CANDIDATE="${REPO_ROOT}/.local-runtime/models/bilingual-zh-en/${FILE_NAME}"

    if [[ -f "${LOCAL_CANDIDATE}" ]]; then
        CALC_SHA=$(sha256sum "${LOCAL_CANDIDATE}" | awk '{print $1}')
        if [[ "${CALC_SHA}" == "${EXPECTED_SHA}" ]]; then
            echo "--> Using verified local staging file: ${FILE_NAME}"
            cp "${LOCAL_CANDIDATE}" "${TARGET}"
            continue
        fi
    fi

    DOWNLOAD_URL="https://huggingface.co/${UPSTREAM_REPO}/resolve/${UPSTREAM_REV}/${UPSTREAM_PATH}"
    echo "--> Downloading ${FILE_NAME} from ${DOWNLOAD_URL}..."
    curl -L --fail --retry 3 --retry-delay 2 -s -o "${TARGET}" "${DOWNLOAD_URL}"
    echo "${EXPECTED_SHA}  ${TARGET}" | sha256sum -c -
done

# 2. Add metadata & licenses
cp "${REPO_ROOT}/model.json" "${STAGE_DIR}/model.json"
cp "${REPO_ROOT}/licenses/model-LICENSE" "${STAGE_DIR}/LICENSE"
cp "${BASE_MODEL_DEF}" "${STAGE_DIR}/UPSTREAM.json"

# 3. Normalize directory and file permissions
echo "--> Normalizing file and directory permissions (dir: 0755, file: 0644)..."
find "${STAGE_DIR}" -type d -exec chmod 755 {} +
find "${STAGE_DIR}" -type f -exec chmod 644 {} +

# 4. Create 100% reproducible .tar.zst archive
echo "--> Compressing ${PKG_NAME}.tar.zst using normalized tar metadata + zstd-3 (single-thread)..."
(cd "${TMP_WORK_DIR}" && tar --sort=name --mtime="2026-01-01 00:00:00Z" --owner=0 --group=0 --numeric-owner -I "zstd -3 -T1" -cf "${OUTPUT_ARCHIVE}" "${PKG_NAME}")

ARCHIVE_SHA256=$(sha256sum "${OUTPUT_ARCHIVE}" | awk '{print $1}')

echo "============================================================"
echo " Reproducible Packaging Complete!"
echo " Output Archive: ${OUTPUT_ARCHIVE}"
echo " Archive SHA256: ${ARCHIVE_SHA256}"
echo "============================================================"
echo ""
echo "Candidate models/base-model.lock.json snippet:"
cat << EOF
{
  "schema_version": 1,
  "id": "${MODEL_ID}",
  "revision": "${ECHOLET_REV}",
  "archive": "${PKG_NAME}.tar.zst",
  "url": "https://github.com/SentimentalK/echolet/releases/download/${PKG_NAME}/${PKG_NAME}.tar.zst",
  "sha256": "${ARCHIVE_SHA256}",
  "upstream_repository": "${UPSTREAM_REPO}",
  "upstream_revision": "${UPSTREAM_REV}",
  "files": {
    "encoder": "encoder-480ms.onnx",
    "decoder": "decoder-480ms.onnx",
    "joiner": "joiner-480ms.onnx",
    "tokens": "tokens.txt"
  }
}
EOF
echo ""
