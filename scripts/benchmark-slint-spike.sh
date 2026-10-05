#!/usr/bin/env bash
set -euo pipefail

# Benchmark and measurement script for Echolet Slint UI Spike (PROJECT-041 / J11.1c).

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

echo "============================================================"
echo " Benchmarking Echolet Desktop UI Spike (Slint 1.18.1)"
echo "============================================================"

# 1. Build release binaries
echo "--> Compiling release binaries..."
cargo build --release --bin echolet
cargo build --release --features slint-ui-spike --bin echolet-ui-spike

# 2. Binary sizes
ECHOLET_BIN="target/release/echolet"
SPIKE_BIN="target/release/echolet-ui-spike"

if [[ "$(uname)" == "Darwin" ]]; then
    ECHOLET_SIZE=$(stat -f "%z" "${ECHOLET_BIN}")
    SPIKE_SIZE=$(stat -f "%z" "${SPIKE_BIN}")
else
    ECHOLET_SIZE=$(stat -c "%s" "${ECHOLET_BIN}")
    SPIKE_SIZE=$(stat -c "%s" "${SPIKE_BIN}")
fi

DELTA_SIZE=$((SPIKE_SIZE - ECHOLET_SIZE))

echo "--> Build Artifact Sizes:"
echo "    Normal Echolet release binary : ${ECHOLET_SIZE} bytes ($(echo "scale=2; ${ECHOLET_SIZE}/1048576" | bc) MiB)"
echo "    Slint Spike release binary    : ${SPIKE_SIZE} bytes ($(echo "scale=2; ${SPIKE_SIZE}/1048576" | bc) MiB)"
echo "    Incremental Slint footprint   : ${DELTA_SIZE} bytes ($(echo "scale=2; ${DELTA_SIZE}/1048576" | bc) MiB)"

# 3. Repeated open/hide latency & memory benchmark
echo ""
echo "--> Running latency and memory benchmark suite..."
"${SPIKE_BIN}" --bench --iterations 10

# 4. Steady-state idle RSS and CPU
echo ""
echo "--> Measuring 3-second steady-state idle CPU and memory..."
"${SPIKE_BIN}" --close-after-ms 3000 &
SPIKE_PID=$!
sleep 1.5
if ps -p "${SPIKE_PID}" >/dev/null 2>&1; then
    ps -p "${SPIKE_PID}" -o %cpu,rss,command
fi
wait "${SPIKE_PID}" || true

echo "============================================================"
echo " Benchmark Complete."
echo "============================================================"
