#!/usr/bin/env bash
set -euo pipefail

# Reproducible packaging of the Echolet-owned frozen Nemotron 3.5 ASR Streaming
# 0.6B (560 ms, int8) model pack.
#
# The single source of truth is models/nemotron-model.json. This script:
#   1. acquires the pinned upstream sherpa-onnx archive and verifies its SHA256;
#   2. extracts ONLY the canonical model/runtime files (no test WAVs, no
#      unrelated payload);
#   3. generates the canonical Echolet model.json manifest, UPSTREAM.json,
#      NOTICE and the upstream README, and copies the full OpenMDW-1.1 text;
#   4. writes a deterministic .tar.zst (sorted entries, zeroed ownership,
#      normalized mode and fixed mtime) to dist/.
#
# It does NOT publish anything; publishing is a separate, explicit step.
#
# Usage:
#   scripts/package-nemotron-model.sh
#
# Env overrides:
#   ECHOLET_NEMOTRON_UPSTREAM_ARCHIVE  path to an already-downloaded upstream
#                                      .tar.bz2 (its SHA is still verified)
#   ECHOLET_NEMOTRON_OUT               output .tar.zst path

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEF="${REPO_ROOT}/models/nemotron-model.json"

if [[ ! -f "${DEF}" ]]; then
    echo "[Error] Model definition not found: ${DEF}" >&2
    exit 1
fi

for tool in jq python3; do
    if ! command -v "${tool}" >/dev/null 2>&1; then
        echo "[Error] Required tool not found: ${tool}" >&2
        exit 1
    fi
done

MODEL_ID=$(jq -r '.id' "${DEF}")
ECHOLET_REV=$(jq -r '.revision' "${DEF}")
UPSTREAM_ARCHIVE=$(jq -r '.upstream_source.archive' "${DEF}")
UPSTREAM_URL=$(jq -r '.upstream_source.url' "${DEF}")
UPSTREAM_SHA=$(jq -r '.upstream_source.sha256' "${DEF}")
EXTRACTED_DIR=$(jq -r '.upstream_source.extracted_dir' "${DEF}")

PKG_NAME="model-${MODEL_ID#echolet-}"
OUTPUT_ARCHIVE="${ECHOLET_NEMOTRON_OUT:-${REPO_ROOT}/dist/${PKG_NAME}.tar.zst}"

echo "=== Package Echolet Frozen Nemotron Model ==="
echo "Model ID:          ${MODEL_ID}"
echo "Echolet revision:  ${ECHOLET_REV}"
echo "Upstream archive:  ${UPSTREAM_ARCHIVE}"
echo "Output archive:    ${OUTPUT_ARCHIVE}"

compute_sha() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

# ---- 1. Acquire + verify the pinned upstream archive ----
UPSTREAM_PATH=""
CANDIDATE_PATHS=(
    "${ECHOLET_NEMOTRON_UPSTREAM_ARCHIVE:-}"
    "${REPO_ROOT}/dist/${UPSTREAM_ARCHIVE}"
    "${REPO_ROOT}/.local-runtime/${UPSTREAM_ARCHIVE}"
    "/tmp/${UPSTREAM_ARCHIVE}"
    "/tmp/echolet-j10/${UPSTREAM_ARCHIVE}"
    "${HOME}/.cache/echolet-models/${UPSTREAM_ARCHIVE}"
)
for cand in "${CANDIDATE_PATHS[@]}"; do
    [[ -n "${cand}" && -f "${cand}" ]] || continue
    if [[ "$(compute_sha "${cand}")" == "${UPSTREAM_SHA}" ]]; then
        UPSTREAM_PATH="${cand}"
        break
    fi
done

TMP_WORK_DIR=$(mktemp -d)
trap 'rm -rf "${TMP_WORK_DIR}"' EXIT

if [[ -z "${UPSTREAM_PATH}" ]]; then
    UPSTREAM_PATH="${TMP_WORK_DIR}/${UPSTREAM_ARCHIVE}"
    echo "--> Downloading ${UPSTREAM_URL}"
    curl -L --fail --retry 3 --retry-delay 2 -o "${UPSTREAM_PATH}" "${UPSTREAM_URL}"
fi

echo "--> Verifying upstream SHA256"
ACTUAL_UPSTREAM_SHA="$(compute_sha "${UPSTREAM_PATH}")"
if [[ "${ACTUAL_UPSTREAM_SHA}" != "${UPSTREAM_SHA}" ]]; then
    echo "[Error] Upstream archive SHA256 mismatch" >&2
    echo "        Expected: ${UPSTREAM_SHA}" >&2
    echo "        Got:      ${ACTUAL_UPSTREAM_SHA}" >&2
    exit 1
fi
echo "    OK: ${UPSTREAM_SHA}"

# ---- 2. Extract and stage only canonical files ----
EXTRACT_DIR="${TMP_WORK_DIR}/extracted"
mkdir -p "${EXTRACT_DIR}"
tar -xjf "${UPSTREAM_PATH}" -C "${EXTRACT_DIR}"

SRC_DIR="${EXTRACT_DIR}/${EXTRACTED_DIR}"
if [[ ! -d "${SRC_DIR}" ]]; then
    SRC_DIR=$(find "${EXTRACT_DIR}" -mindepth 1 -maxdepth 1 -type d | head -n 1)
fi
if [[ ! -d "${SRC_DIR}" ]]; then
    echo "[Error] Could not locate extracted upstream model directory" >&2
    exit 1
fi

STAGE_ROOT="${TMP_WORK_DIR}/stage"
STAGE_DIR="${STAGE_ROOT}/${MODEL_ID}"
mkdir -p "${STAGE_DIR}"

# Verify every model file's pinned SHA before staging.
for key in encoder decoder joiner tokens; do
    FILE_NAME=$(jq -r ".files.${key}" "${DEF}")
    EXPECTED=$(jq -r ".file_sha256[\"${FILE_NAME}\"]" "${DEF}")
    SRC="${SRC_DIR}/${FILE_NAME}"
    if [[ ! -f "${SRC}" ]]; then
        echo "[Error] Missing upstream model file: ${FILE_NAME}" >&2
        exit 1
    fi
    ACTUAL="$(compute_sha "${SRC}")"
    if [[ "${ACTUAL}" != "${EXPECTED}" ]]; then
        echo "[Error] SHA256 mismatch for ${FILE_NAME}" >&2
        echo "        Expected: ${EXPECTED}" >&2
        echo "        Got:      ${ACTUAL}" >&2
        exit 1
    fi
    cp "${SRC}" "${STAGE_DIR}/${FILE_NAME}"
done

# Required license / origin notices.
cp "${REPO_ROOT}/licenses/openmdw-1.1.txt" "${STAGE_DIR}/LICENSE"
if [[ -f "${SRC_DIR}/README.md" ]]; then
    cp "${SRC_DIR}/README.md" "${STAGE_DIR}/UPSTREAM_README.md"
fi

# NOTICE of origin (OpenMDW-1.1 obligation: retain applicable origin notices).
cat > "${STAGE_DIR}/NOTICE" <<EOF
${MODEL_ID}

This archive is an Echolet redistribution of the Model Materials distributed by
NVIDIA as nvidia/nemotron-3.5-asr-streaming-0.6b (NVIDIA Nemotron 3.5 ASR
Streaming 0.6B), exported to ONNX and published by the k2-fsa/sherpa-onnx project.

Origin:
  Upstream model:   ${UPSTREAM_URL}
  Upstream project: $(jq -r '.upstream_project' "${DEF}")
  Sherpa export:    $(jq -r '.export_project' "${DEF}")
  Export revision:  $(jq -r '.export_revision' "${DEF}")
  Upstream archive: ${UPSTREAM_URL}
  Upstream SHA256:  ${UPSTREAM_SHA}

The full OpenMDW-1.1 agreement is included in this distribution as LICENSE.
All copyright notices and notices of origin applicable to this distribution are
retained here and in UPSTREAM.json / UPSTREAM_README.md.
EOF

# UPSTREAM.json provenance (everything except the Echolet archive's own SHA,
# which is recorded in the repository-side release lock to avoid a hash cycle).
python3 - "${DEF}" "${STAGE_DIR}/UPSTREAM.json" <<'PY'
import json, sys
defn = json.load(open(sys.argv[1], encoding="utf-8"))
out = {
    "echolet_model_id": defn["id"],
    "echolet_revision": defn["revision"],
    "upstream_project": defn["upstream_project"],
    "upstream_repository": defn["upstream_repository"],
    "upstream_revision": defn["upstream_revision"],
    "export_project": defn["export_project"],
    "export_revision": defn["export_revision"],
    "release_date": defn["release_date"],
    "chunk_ms": defn["chunk_ms"],
    "license": defn["license"],
    "upstream_source": defn["upstream_source"],
    "files": defn["files"],
    "file_sha256": defn["file_sha256"],
    "canonical_files": defn["canonical_files"],
    "note": "Echolet-owned archive name/URL/SHA256 live in models/nemotron-model.lock.json",
}
json.dump(out, open(sys.argv[2], "w", encoding="utf-8"), indent=2, ensure_ascii=False)
open(sys.argv[2], "a", encoding="utf-8").write("\n")
PY

# Canonical Echolet manifest (ModelManifest-compatible) generated from the def.
python3 - "${DEF}" "${STAGE_DIR}/model.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
manifest = {
    "id": d["id"],
    "display_name": d["display_name"],
    "version": d["version"],
    "languages": d["languages"],
    "family": d["family"],
    "encoder": d["files"]["encoder"],
    "decoder": d["files"]["decoder"],
    "joiner": d["files"]["joiner"],
    "tokens": d["files"]["tokens"],
    "sample_rate": d["sample_rate"],
    "feature_dim": d["feature_dim"],
    "num_threads": d["num_threads"],
    "provider": d["provider"],
    "decoding_method": d["decoding_method"],
    "max_active_paths": d["max_active_paths"],
    "language_options": d["language_options"],
}
if d.get("model_type") is not None:
    manifest["model_type"] = d["model_type"]
json.dump(manifest, open(sys.argv[2], "w", encoding="utf-8"), indent=2, ensure_ascii=False)
open(sys.argv[2], "a", encoding="utf-8").write("\n")
PY

# ---- 3. Normalize permissions ----
find "${STAGE_ROOT}" -type d -exec chmod 755 {} +
find "${STAGE_ROOT}" -type f -exec chmod 644 {} +

# ---- 4. Deterministic tar + zstd ----
mkdir -p "$(dirname "${OUTPUT_ARCHIVE}")"
echo "--> Building deterministic tar.zst"
python3 - "${STAGE_ROOT}" "${MODEL_ID}" "${TMP_WORK_DIR}/pack.tar" <<'PY'
import os, sys, tarfile
stage_root, top, out = sys.argv[1], sys.argv[2], sys.argv[3]
entries = []
for dirpath, dirnames, filenames in os.walk(os.path.join(stage_root, top)):
    dirnames.sort()
    for name in sorted(filenames):
        full = os.path.join(dirpath, name)
        rel = os.path.relpath(full, stage_root)
        entries.append(rel)
entries.sort()
with tarfile.open(out, "w", format=tarfile.GNU_FORMAT) as tf:
    for rel in entries:
        full = os.path.join(stage_root, rel)
        ti = tf.gettarinfo(full, arcname=rel)
        ti.uid = ti.gid = 0
        ti.uname = ti.gname = ""
        ti.mtime = 0
        ti.mode = 0o644
        with open(full, "rb") as fh:
            tf.addfile(ti, fh)
PY
zstd -3 -T1 -q -f -o "${OUTPUT_ARCHIVE}" "${TMP_WORK_DIR}/pack.tar"

ARCHIVE_SHA256="$(compute_sha "${OUTPUT_ARCHIVE}")"
ARCHIVE_SIZE="$(wc -c < "${OUTPUT_ARCHIVE}" | tr -d ' ')"

echo "============================================================"
echo " Frozen Echolet Nemotron model pack built"
echo " Archive:       ${OUTPUT_ARCHIVE}"
echo " SHA256:        ${ARCHIVE_SHA256}"
echo " Size bytes:    ${ARCHIVE_SIZE}"
echo "============================================================"
echo ""
echo "models/nemotron-model.lock.json snippet:"
cat <<EOF
{
  "schema_version": 1,
  "id": "${MODEL_ID}",
  "revision": "${ECHOLET_REV}",
  "archive": "${PKG_NAME}.tar.zst",
  "url": "https://github.com/SentimentalK/echolet/releases/download/${PKG_NAME}/${PKG_NAME}.tar.zst",
  "sha256": "${ARCHIVE_SHA256}",
  "size_bytes": ${ARCHIVE_SIZE}
}
EOF
