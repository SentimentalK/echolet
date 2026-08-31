#!/usr/bin/env bash
set -euo pipefail

# This script prepares native runtime libraries and pre-trained models in .local-runtime/
#
# Logic:
# 1. If .local-runtime/ is already fully populated with valid production runtime and X-ASR 480ms model, exit 0.
# 2. Otherwise, run download-official-assets.sh with requested architecture to acquire official pinned assets.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGING_DIR="${REPO_ROOT}/.local-runtime"

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

# 2. Check if already complete with current X-ASR 480ms model and native libraries
if [[ -f "${STAGING_DIR}/runtime/lib/libsherpa-onnx-c-api.so" && \
      -f "${STAGING_DIR}/runtime/lib/libonnxruntime.so" && \
      -f "${STAGING_DIR}/models/bilingual-zh-en/encoder-480ms.onnx" && \
      -f "${STAGING_DIR}/models/bilingual-zh-en/decoder-480ms.onnx" && \
      -f "${STAGING_DIR}/models/bilingual-zh-en/joiner-480ms.onnx" && \
      -f "${STAGING_DIR}/models/bilingual-zh-en/tokens.txt" ]]; then
    echo "[Assets] .local-runtime/ is already populated and valid (${ARCH})."
    exit 0
fi

echo "=== Staging official Echolet runtime and frozen model (${ARCH}) ==="
"${REPO_ROOT}/scripts/download-official-assets.sh" "${ARCH}"
