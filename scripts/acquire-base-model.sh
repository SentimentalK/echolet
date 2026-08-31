#!/usr/bin/env bash
set -euo pipefail

# This script acquires the frozen Echolet Base Model defined in models/base-model.lock.json.
# It prioritizes immutable Echolet Model Release archives (.tar.zst) and GitHub Actions cache,
# with robust fallback to pinned upstream assets with per-file SHA256 validation.

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
UPSTREAM_REV=$(jq -r '.upstream_revision' "${LOCK_FILE}")

ENCODER_SHA256="0c3454033d249081df124ddcd7adaf3deca07d0b999b26f2ee5d2475d37abc74"
DECODER_SHA256="3658368d274a5d5fd39a7ac20c46bed0ad9cfea1f0feddef30d5d89797c1f499"
JOINER_SHA256="03781c98165a2385024c9cecdd2b6b13310d81db23a62c7da420782c2915cf81"
TOKENS_SHA256="b818a60878b9aae978cbb8ad594acbd403d76d1af2e31ef4197c84e2dbdba27c"

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

# 2. Check local artifact caches (e.g. dist/ or /tmp/)
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

# 3. If not in local cache, attempt download from Echolet Model Release
if [[ -z "${ARCHIVE_PATH}" ]]; then
    CAND_DOWNLOAD="${TMP_WORK_DIR}/${ARCHIVE_NAME}"
    echo "--> Attempting download from Echolet Model Release: ${ARCHIVE_URL}..."
    if curl -L --fail --retry 3 --retry-delay 2 -s -o "${CAND_DOWNLOAD}" "${ARCHIVE_URL}"; then
        calc_sha=$(sha256sum "${CAND_DOWNLOAD}" | awk '{print $1}')
        if [[ "${calc_sha}" == "${EXPECTED_SHA256}" ]]; then
            echo "--> Echolet Model Release archive verified (SHA256: ${calc_sha})."
            ARCHIVE_PATH="${CAND_DOWNLOAD}"
        else
            echo "[Warning] Downloaded archive checksum mismatch: ${calc_sha} != ${EXPECTED_SHA256}"
        fi
    else
        echo "[Notice] Echolet Model Release not yet published or unreachable."
    fi
fi

# 4. Extract verified archive if available, or fall back to pinned per-file acquisition
if [[ -n "${ARCHIVE_PATH}" ]]; then
    echo "--> Extracting model archive into ${TARGET_DIR}..."
    EXTRACT_DIR="${TMP_WORK_DIR}/extracted"
    mkdir -p "${EXTRACT_DIR}"
    tar --zstd -xf "${ARCHIVE_PATH}" -C "${EXTRACT_DIR}"

    PACKAGE_SUBDIR="${EXTRACT_DIR}/echolet-model-${MODEL_ID}-${MODEL_REV}"
    if [[ ! -d "${PACKAGE_SUBDIR}" ]]; then
        PACKAGE_SUBDIR=$(find "${EXTRACT_DIR}" -mindepth 1 -maxdepth 1 -type d | head -n 1)
    fi

    cp -a "${PACKAGE_SUBDIR}"/* "${TARGET_DIR}/"
else
    echo "--> Fallback: Acquiring pinned upstream assets (${UPSTREAM_REV}) with per-file SHA256 validation..."
    UPSTREAM_BASE="https://huggingface.co/GilgameshWind/X-ASR-zh-en/resolve/${UPSTREAM_REV}/deployment/models/chunk-480ms-model"

    fetch_and_verify() {
        local file_name="$1"
        local expected_sha="$2"
        local dest="${TARGET_DIR}/${file_name}"

        echo "--> Downloading ${file_name}..."
        curl -L --fail --retry 3 --retry-delay 2 -s -o "${dest}" "${UPSTREAM_BASE}/${file_name}"
        echo "${expected_sha}  ${dest}" | sha256sum -c -
    }

    fetch_and_verify "encoder-480ms.onnx" "${ENCODER_SHA256}"
    fetch_and_verify "decoder-480ms.onnx" "${DECODER_SHA256}"
    fetch_and_verify "joiner-480ms.onnx" "${JOINER_SHA256}"
    fetch_and_verify "tokens.txt" "${TOKENS_SHA256}"
fi

# 5. Ensure model.json & LICENSE are present
if [[ ! -f "${TARGET_DIR}/model.json" ]]; then
    cp "${REPO_ROOT}/model.json" "${TARGET_DIR}/model.json"
fi
if [[ ! -f "${TARGET_DIR}/LICENSE" ]]; then
    cp "${REPO_ROOT}/licenses/model-LICENSE" "${TARGET_DIR}/LICENSE"
fi

echo "=== Base model acquired and verified successfully ==="
ls -lh "${TARGET_DIR}"
