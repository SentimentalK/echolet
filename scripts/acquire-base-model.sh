#!/usr/bin/env bash
set -euo pipefail

# This script acquires the frozen Echolet Base Model defined in models/base-model.lock.json.
# It strictly enforces verification against the immutable Echolet Model Release archive (.tar.zst) and GitHub Actions cache.
# NOTE: Normal CI strictly enforces this single source of truth and does NOT fall back to upstream Hugging Face.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOCK_FILE="${REPO_ROOT}/models/base-model.lock.json"
TARGET_DIR="${1:-${REPO_ROOT}/.local-runtime/models/bilingual-zh-en}"

if [[ ! -f "${LOCK_FILE}" ]]; then
    echo "[Error] Model lock file not found at: ${LOCK_FILE}" >&2
    exit 1
fi

MODEL_ID=$(jq -r '.id' "${LOCK_FILE}")
MODEL_REV=$(jq -r '.revision' "${LOCK_FILE}")
ARCHIVE_NAME=$(jq -r '.archive' "${LOCK_FILE}")
ARCHIVE_URL=$(jq -r '.url' "${LOCK_FILE}")
EXPECTED_SHA256=$(jq -r '.sha256' "${LOCK_FILE}")

echo "=== Acquiring Echolet Base Model (${MODEL_ID} - ${MODEL_REV}) ==="
echo "Target directory: ${TARGET_DIR}"

# 1. Fast path: check if target directory is already populated and valid
REQUIRED_FILES=("encoder-480ms.onnx" "decoder-480ms.onnx" "joiner-480ms.onnx" "tokens.txt")
ALL_PRESENT=true
for f in "${REQUIRED_FILES[@]}"; do
    if [[ ! -f "${TARGET_DIR}/${f}" || ! -s "${TARGET_DIR}/${f}" ]]; then
        ALL_PRESENT=false
        break
    fi
done

if [[ "${ALL_PRESENT}" == "true" ]]; then
    echo "--> Target model directory already contains all required files. Reusing existing model."
    exit 0
fi

mkdir -p "${TARGET_DIR}"

TMP_WORK_DIR=$(mktemp -d)
trap 'rm -rf "${TMP_WORK_DIR}"' EXIT

ARCHIVE_PATH=""

# 2. Check local artifact caches (e.g. dist/ or /tmp/ or Actions cache)
CANDIDATE_PATHS=(
    "${REPO_ROOT}/dist/${ARCHIVE_NAME}"
    "/tmp/${ARCHIVE_NAME}"
    "${HOME}/.cache/echolet-models/${ARCHIVE_NAME}"
)

for cand in "${CANDIDATE_PATHS[@]}"; do
    if [[ -f "${cand}" ]]; then
        cand_sha=$(sha256sum "${cand}" | awk '{print $1}')
        if [[ "${cand_sha}" == "${EXPECTED_SHA256}" ]]; then
            echo "--> Found verified local archive at: ${cand}"
            ARCHIVE_PATH="${cand}"
            break
        fi
    fi
done

# 3. If not in local cache, download from official Echolet Model Release
if [[ -z "${ARCHIVE_PATH}" ]]; then
    CAND_DOWNLOAD="${TMP_WORK_DIR}/${ARCHIVE_NAME}"
    echo "--> Downloading official model archive from: ${ARCHIVE_URL}..."
    if ! curl -L --fail --retry 3 --retry-delay 2 -s -o "${CAND_DOWNLOAD}" "${ARCHIVE_URL}"; then
        echo "[Error] Failed to download official Echolet Base Model archive from: ${ARCHIVE_URL}" >&2
        echo "[Error] Make sure the model release workflow has published ${ARCHIVE_NAME} to GitHub Releases." >&2
        exit 1
    fi

    echo "--> Verifying SHA256 checksum of model archive..."
    calc_sha=$(sha256sum "${CAND_DOWNLOAD}" | awk '{print $1}')
    if [[ "${calc_sha}" != "${EXPECTED_SHA256}" ]]; then
        echo "[Error] SHA256 checksum mismatch for ${ARCHIVE_NAME}!" >&2
        echo "        Expected: ${EXPECTED_SHA256}" >&2
        echo "        Got:      ${calc_sha}" >&2
        exit 1
    fi
    echo "--> SHA256 checksum verified: OK"
    ARCHIVE_PATH="${CAND_DOWNLOAD}"
fi

# 4. Extract verified archive
echo "--> Extracting model archive into ${TARGET_DIR}..."
EXTRACT_DIR="${TMP_WORK_DIR}/extracted"
mkdir -p "${EXTRACT_DIR}"
tar --zstd -xf "${ARCHIVE_PATH}" -C "${EXTRACT_DIR}"

PACKAGE_SUBDIR="${EXTRACT_DIR}/echolet-model-${MODEL_ID}-${MODEL_REV}"
if [[ ! -d "${PACKAGE_SUBDIR}" ]]; then
    PACKAGE_SUBDIR=$(find "${EXTRACT_DIR}" -mindepth 1 -maxdepth 1 -type d | head -n 1)
fi

cp -a "${PACKAGE_SUBDIR}"/* "${TARGET_DIR}/"

# 5. Ensure model.json & LICENSE are present
if [[ ! -f "${TARGET_DIR}/model.json" ]]; then
    cp "${REPO_ROOT}/model.json" "${TARGET_DIR}/model.json"
fi
if [[ ! -f "${TARGET_DIR}/LICENSE" ]]; then
    cp "${REPO_ROOT}/licenses/model-LICENSE" "${TARGET_DIR}/LICENSE"
fi

echo "=== Base model acquired and verified successfully ==="
ls -lh "${TARGET_DIR}"
