#!/usr/bin/env bash
set -euo pipefail

# This script packages an immutable Echolet Base Model archive (.tar.zst) from pinned upstream assets,
# calculates its SHA256 checksum, and prints candidate lock metadata.
# NOTE: This script does NOT modify Git repository files automatically.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODEL_ID="xasr-zh-en-480ms"
ECHOLET_REV="${1:-r1}"
UPSTREAM_REV="689ff18c584d29910da37b6fe904db0c1489c9d1"
UPSTREAM_BASE="https://huggingface.co/GilgameshWind/X-ASR-zh-en/resolve/${UPSTREAM_REV}/deployment/models/chunk-480ms-model"

ENCODER_SHA256="0c3454033d249081df124ddcd7adaf3deca07d0b999b26f2ee5d2475d37abc74"
DECODER_SHA256="3658368d274a5d5fd39a7ac20c46bed0ad9cfea1f0feddef30d5d89797c1f499"
JOINER_SHA256="03781c98165a2385024c9cecdd2b6b13310d81db23a62c7da420782c2915cf81"
TOKENS_SHA256="b818a60878b9aae978cbb8ad594acbd403d76d1af2e31ef4197c84e2dbdba27c"

PKG_NAME="echolet-model-${MODEL_ID}-${ECHOLET_REV}"
OUTPUT_ARCHIVE="${REPO_ROOT}/dist/${PKG_NAME}.tar.zst"

echo "=== Packaging Echolet Base Model Artifact ==="
echo "Model ID:         ${MODEL_ID}"
echo "Echolet Revision: ${ECHOLET_REV}"
echo "Upstream Rev:     ${UPSTREAM_REV}"

TMP_WORK_DIR=$(mktemp -d)
trap 'rm -rf "${TMP_WORK_DIR}"' EXIT

STAGE_DIR="${TMP_WORK_DIR}/${PKG_NAME}"
mkdir -p "${STAGE_DIR}"
mkdir -p "${REPO_ROOT}/dist"

# 1. Acquire files (copy from local verified staging if available, otherwise download from Hugging Face)
download_or_copy() {
    local file_name="$1"
    local expected_sha="$2"
    local local_candidate="${REPO_ROOT}/.local-runtime/models/bilingual-zh-en/${file_name}"
    local target="${STAGE_DIR}/${file_name}"

    if [[ -f "${local_candidate}" ]]; then
        local calc_sha
        calc_sha=$(sha256sum "${local_candidate}" | awk '{print $1}')
        if [[ "${calc_sha}" == "${expected_sha}" ]]; then
            echo "--> Using local verified file: ${file_name}"
            cp "${local_candidate}" "${target}"
            return 0
        fi
    fi

    echo "--> Downloading ${file_name} from Hugging Face (${UPSTREAM_REV})..."
    curl -L --fail --retry 3 --retry-delay 2 -s -o "${target}" "${UPSTREAM_BASE}/${file_name}"
    echo "${expected_sha}  ${target}" | sha256sum -c -
}

download_or_copy "encoder-480ms.onnx" "${ENCODER_SHA256}"
download_or_copy "decoder-480ms.onnx" "${DECODER_SHA256}"
download_or_copy "joiner-480ms.onnx" "${JOINER_SHA256}"
download_or_copy "tokens.txt" "${TOKENS_SHA256}"

# 2. Add metadata & licenses
cp "${REPO_ROOT}/model.json" "${STAGE_DIR}/model.json"
cp "${REPO_ROOT}/licenses/model-LICENSE" "${STAGE_DIR}/LICENSE"

cat << EOF > "${STAGE_DIR}/UPSTREAM.json"
{
  "model_id": "${MODEL_ID}",
  "echolet_revision": "${ECHOLET_REV}",
  "upstream_project": "X-ASR",
  "upstream_repository": "GilgameshWind/X-ASR-zh-en",
  "upstream_revision": "${UPSTREAM_REV}",
  "language": ["zh", "en"],
  "chunk_ms": 480,
  "license": "Apache-2.0"
}
EOF

# 3. Create .tar.zst archive
echo "--> Compressing ${PKG_NAME}.tar.zst using zstd..."
(cd "${TMP_WORK_DIR}" && tar --zstd -cf "${OUTPUT_ARCHIVE}" "${PKG_NAME}")

ARCHIVE_SHA256=$(sha256sum "${OUTPUT_ARCHIVE}" | awk '{print $1}')

echo "============================================================"
echo " Packaging Complete!"
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
  "url": "https://github.com/SentimentalK/echolet/releases/download/model-${MODEL_ID}-${ECHOLET_REV}/${PKG_NAME}.tar.zst",
  "sha256": "${ARCHIVE_SHA256}",
  "upstream_repository": "GilgameshWind/X-ASR-zh-en",
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
