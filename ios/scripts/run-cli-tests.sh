#!/usr/bin/env bash
#
# run-cli-tests.sh — compile + run every pure-Foundation Swift CLI test suite
# from ios/protocol on the HOST (macOS). This is the single runner used by
# CI (.github/workflows/ios.yml) and locally; there are no hidden steps.
#
# Each suite is a @main test binary compiled from its own protocol sources.
# A failing suite exits non-zero and stops the run (fail-closed).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$BASH_SOURCE")/../.." && pwd)"
P="$REPO_ROOT/ios/protocol"
OUT="${TMPDIR:-/tmp}/echolet-cli-tests"
mkdir -p "$OUT"

command -v swiftc >/dev/null 2>&1 || { echo "Blocker: swiftc not found (XcoTools/Xcode not selected)."; exit 1; }

# Sources shared by the admission/launch/session suites.
CORE=(EcholetIPC.swift EcholetAdmission.swift)
LAUNCH_EPIC=("${CORE[@]}" EcholetLaunch.swift EcholetKeyboardSessionController.swift)
CAPTURE_SET=("${CORE[@]}" CaptureLifecycleGate.swift DeterministicResampler.swift WarmCaptureFlow.swift)
MODEL_SET=(EcholetHomePresenter.swift EcholetModelGate.swift EcholetModelSwitch.swift EcholetModelDownloadPolicy.swift EcholetModelError.swift)

run_suite() {
  local name="$1"; shift
  local bin="$1"; shift
  echo "[run-cli-tests] building $name"
  # Prepend $P/ to every source path (relative protocol sources).
  local sources=()
  local src
  for src in "$@"; do
    sources+=("$P/$src")
  done
  swiftc "${sources[@]}" -o "$OUT/$bin" >/dev/null
  echo "[run-cli-tests] running $name"
  if [ "$name" = "ipc-codec-smoke" ]; then
    "$OUT/$bin" "$P/fixtures"
  else
    "$OUT/$bin"
  fi
  echo "[run-cli-tests] $name PASS"
}

run_suite admission           admission.bin          "${CORE[@]}"            AdmissionTests.swift
run_suite adversarial         adversarial.bin        "${CORE[@]}" CaptureLifecycleGate.swift AdversarialTests.swift
run_suite capture-lifecycle   capture.bin            CaptureLifecycleGate.swift CaptureLifecycleTests.swift
run_suite warm-capture-flow   warmflow.bin           "${CAPTURE_SET[@]}"     WarmCaptureFlowTests.swift
run_suite launch              launch.bin             "${LAUNCH_EPIC[@]}"     EcholetLaunchTests.swift
run_suite keyboard-session    session.bin            "${LAUNCH_EPIC[@]}"     EcholetKeyboardSessionTests.swift
run_suite model-flow          modelflow.bin          "${MODEL_SET[@]}"       EcholetModelFlowTests.swift
run_suite home-presenter      homepresenter.bin      EcholetHomePresenter.swift EcholetHomePresenterTests.swift
run_suite ipc-codec-smoke     ipccodec.bin           EcholetIPC.swift        IPCCodecSmoke.swift

echo "[run-cli-tests] ALL SUITES PASS"
