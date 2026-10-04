#!/usr/bin/env bash
set -euo pipefail

# Echolet Model Lifecycle Benchmark & Instrumentation Runner (PROJECT-041 Stage 1 / J3)
# Reuses real ModelManager and OnlineRecognizer/OnlineStream paths.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

LOCAL_RUNTIME="${REPO_ROOT}/.local-runtime"
if [[ ! -d "${LOCAL_RUNTIME}/runtime" || ! -f "${LOCAL_RUNTIME}/models/bilingual-zh-en/encoder-480ms.onnx" ]]; then
    echo "[Benchmark] Local runtime assets not found in .local-runtime."
    echo "[Benchmark] Running asset acquisition first..."
    if [[ "$(uname)" == "Darwin" ]]; then
        "${REPO_ROOT}/scripts/macos/prepare-assets.sh"
    else
        ARCH="$(uname -m)"
        case "${ARCH}" in
            x86_64|amd64) ARCH="x64" ;;
            aarch64|arm64) ARCH="arm64" ;;
        esac
        "${REPO_ROOT}/scripts/download-official-assets.sh" "${ARCH}"
    fi
fi

# Run release benchmark binary
cargo run --release --bin model_benchmark -- "$@"
