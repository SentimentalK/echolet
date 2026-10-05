#!/usr/bin/env bash
set -euo pipefail

# Verify an Echolet-owned frozen Nemotron 3.5 pack against
# models/nemotron-model.lock.json.
#
# Checks:
#   * archive SHA256 matches the lock;
#   * the archive contains exactly the canonical files;
#   * the four model files match their pinned SHA256;
#   * the full OpenMDW-1.1 text and origin NOTICE are present;
#   * model.json parses and its id matches the lock;
#   * no test WAVs or unrelated payload are present.
#
# Usage:
#   scripts/verify-nemotron-pack.sh [path-to-archive.tar.zst]
#
# With no argument the archive is downloaded from the lock URL.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOCK="${REPO_ROOT}/models/nemotron-model.lock.json"

[[ -f "${LOCK}" ]] || { echo "[Error] missing ${LOCK}" >&2; exit 1; }

for tool in jq python3; do
    command -v "${tool}" >/dev/null 2>&1 || { echo "[Error] required tool: ${tool}" >&2; exit 1; }
done

compute_sha() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

MODEL_ID=$(jq -r '.id' "${LOCK}")
EXPECTED_SHA=$(jq -r '.sha256' "${LOCK}")
EXPECTED_SIZE=$(jq -r '.size_bytes' "${LOCK}")
URL=$(jq -r '.url' "${LOCK}")

TMP_WORK_DIR=$(mktemp -d)
trap 'rm -rf "${TMP_WORK_DIR}"' EXIT

ARCHIVE_PATH="${1:-}"
if [[ -z "${ARCHIVE_PATH}" ]]; then
    ARCHIVE_PATH="${TMP_WORK_DIR}/pack.tar.zst"
    echo "--> Downloading ${URL}"
    curl -L --fail --retry 3 --retry-delay 2 -o "${ARCHIVE_PATH}" "${URL}"
fi
[[ -f "${ARCHIVE_PATH}" ]] || { echo "[Error] archive not found: ${ARCHIVE_PATH}" >&2; exit 1; }

echo "--> Verifying archive SHA256"
ACTUAL_SHA="$(compute_sha "${ARCHIVE_PATH}")"
if [[ "${ACTUAL_SHA}" != "${EXPECTED_SHA}" ]]; then
    echo "[Error] archive SHA256 mismatch" >&2
    echo "        Expected: ${EXPECTED_SHA}" >&2
    echo "        Got:      ${ACTUAL_SHA}" >&2
    exit 1
fi
ACTUAL_SIZE=$(wc -c < "${ARCHIVE_PATH}" | tr -d ' ')
if [[ "${ACTUAL_SIZE}" != "${EXPECTED_SIZE}" ]]; then
    echo "[Error] archive size mismatch: expected ${EXPECTED_SIZE}, got ${ACTUAL_SIZE}" >&2
    exit 1
fi
echo "    OK: ${EXPECTED_SHA} (${ACTUAL_SIZE} bytes)"

EXTRACT_DIR="${TMP_WORK_DIR}/extracted"
mkdir -p "${EXTRACT_DIR}"
zstd -dc "${ARCHIVE_PATH}" | tar -xf - -C "${EXTRACT_DIR}"

PACK_DIR="${EXTRACT_DIR}/${MODEL_ID}"
if [[ ! -d "${PACK_DIR}" ]]; then
    echo "[Error] expected top-level directory ${MODEL_ID} not found" >&2
    find "${EXTRACT_DIR}" -maxdepth 1 -mindepth 1 >&2
    exit 1
fi

# Exact canonical file set (no extra payload allowed).
python3 - "${LOCK}" "${PACK_DIR}" <<'PY'
import json, os, sys
lock = json.load(open(sys.argv[1], encoding="utf-8"))
pack = sys.argv[2]
expected = set(lock["canonical_files"])
actual = set(os.listdir(pack))
missing = expected - actual
extra = actual - expected
errors = []
if missing:
    errors.append(f"missing canonical files: {sorted(missing)}")
if extra:
    errors.append(f"unexpected payload: {sorted(extra)}")
for name in actual:
    if name.lower().endswith((".wav", ".flac", ".mp3")) or name == "test_wavs":
        errors.append(f"test audio payload must not ship: {name}")
if errors:
    print("[Error] " + "; ".join(errors), file=sys.stderr)
    sys.exit(1)
print(f"    OK: exactly {len(expected)} canonical files; no test WAVs/unrelated payload")
PY

# Model file SHAs.
for f in $(jq -r '.file_sha256 | keys[]' "${LOCK}"); do
    EXPECTED=$(jq -r ".file_sha256[\"${f}\"]" "${LOCK}")
    ACTUAL="$(compute_sha "${PACK_DIR}/${f}")"
    if [[ "${ACTUAL}" != "${EXPECTED}" ]]; then
        echo "[Error] ${f} SHA256 mismatch (expected ${EXPECTED}, got ${ACTUAL})" >&2
        exit 1
    fi
done
echo "    OK: all model file SHA256s match"

# License / origin notices.
grep -q "OpenMDW" "${PACK_DIR}/LICENSE" || { echo "[Error] LICENSE is not OpenMDW text" >&2; exit 1; }
grep -q "Model Materials" "${PACK_DIR}/LICENSE" || { echo "[Error] LICENSE missing OpenMDW terms" >&2; exit 1; }
[[ -s "${PACK_DIR}/NOTICE" ]] || { echo "[Error] NOTICE missing/empty" >&2; exit 1; }
grep -q "nvidia/nemotron-3.5-asr-streaming-0.6b" "${PACK_DIR}/NOTICE" || {
    echo "[Error] NOTICE missing upstream origin" >&2; exit 1;
}
[[ -s "${PACK_DIR}/UPSTREAM.json" ]] || { echo "[Error] UPSTREAM.json missing" >&2; exit 1; }
echo "    OK: OpenMDW-1.1 text + origin notices present"

# Manifest identity.
python3 - "${LOCK}" "${PACK_DIR}/model.json" <<'PY'
import json, sys
lock = json.load(open(sys.argv[1], encoding="utf-8"))
manifest = json.load(open(sys.argv[2], encoding="utf-8"))
if manifest.get("id") != lock["id"]:
    print(f"[Error] model.json id {manifest.get('id')!r} != lock id {lock['id']!r}", file=sys.stderr)
    sys.exit(1)
if len(manifest.get("language_options", {}).get("supported", [])) != 32:
    print("[Error] model.json must declare 32 supported language options", file=sys.stderr)
    sys.exit(1)
print("    OK: model.json identity + language options")
PY

echo "=== Nemotron pack verification PASSED (${MODEL_ID}) ==="
