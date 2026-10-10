#!/usr/bin/env bash
# verify-device.sh - Echolet iOS/iPadOS device and environment discovery script.
#
# Defaults to read-only discovery (SDKs, runtimes, connected devices via devicectl / xcrun).
# Supports safe optional build/install dry-runs once Xcode is configured.
# STRICT SAFETY: Does not execute sudo, modify system licenses, erase devices, or log user data.

set -euo pipefail

MODE="discover"
TARGET_DEVICE=""
VERBOSE=0

print_usage() {
    cat <<EOF
Usage: $(basename "$0") [options]

Modes:
  --discover            (Default) Read-only environment, SDK, simulator, and connected device inspection.
  --check-toolchain     Verify presence of Xcode developer directory and required CLIs.
  --help                Show this message.

Options:
  --device <ID/NAME>    Target device identifier or name for targeted queries.
  -v, --verbose         Enable verbose command output.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --discover)
            MODE="discover"
            shift
            ;;
        --check-toolchain)
            MODE="check-toolchain"
            shift
            ;;
        --device)
            TARGET_DEVICE="${2:-}"
            if [[ -z "$TARGET_DEVICE" ]]; then
                echo "Error: --device requires an argument" >&2
                exit 1
            fi
            shift 2
            ;;
        -v|--verbose)
            VERBOSE=1
            shift
            ;;
        --help|-h)
            print_usage
            exit 0
            ;;
        *)
            echo "Unknown argument: $1" >&2
            print_usage
            exit 1
            ;;
    esac
done

if [[ $VERBOSE -eq 1 ]]; then
    set -x
fi

echo "=== Echolet iOS/iPadOS Host & Device Discovery ==="
echo "Timestamp: $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
echo "Host OS: $(uname -s) $(uname -r) ($(uname -m))"
echo "macOS Product: $(sw_vers -productVersion 2>/dev/null || echo 'Unknown')"
echo ""

# 1. Developer Directory & Toolchain Check
ACTIVE_DEV_DIR="$(xcode-select -p 2>/dev/null || echo 'Not Set')"
echo "[1] Developer Directory: $ACTIVE_DEV_DIR"

XCODEBUILD_AVAILABLE=0
if command -v xcodebuild >/dev/null 2>&1; then
    if xcodebuild -version >/dev/null 2>&1; then
        XCODEBUILD_VERSION="$(xcodebuild -version | tr '\n' ' ')"
        echo "    Xcode Version: $XCODEBUILD_VERSION"
        XCODEBUILD_AVAILABLE=1
    else
        echo "    xcodebuild present but active developer dir is CommandLineTools (requires Xcode.app)."
    fi
else
    echo "    xcodebuild not found in PATH."
fi

# 2. XcodeGen CLI Check
if command -v xcodegen >/dev/null 2>&1; then
    echo "[2] XcodeGen: $(xcodegen --version 2>/dev/null || echo 'Installed')"
else
    echo "[2] XcodeGen: Not installed (optional; install via 'brew install xcodegen' when Xcode is ready)."
fi

# 3. SDKs Inspection
echo ""
echo "[3] Available SDKs (xcodebuild -showsdks):"
if [[ $XCODEBUILD_AVAILABLE -eq 1 ]]; then
    xcodebuild -showsdks || true
else
    echo "    Skipped: Full Xcode required to enumerate SDKs."
fi

# 4. Physical Devices via devicectl
echo ""
echo "[4] Connected Physical Devices (xcrun devicectl list devices):"
if command -v xcrun >/dev/null 2>&1 && xcrun --find devicectl >/dev/null 2>&1; then
    xcrun devicectl list devices || true
else
    echo "    xcrun devicectl not available yet in current developer directory."
fi

# 5. Simulators via simctl
echo ""
echo "[5] Available Simulators (xcrun simctl list devices available):"
if command -v xcrun >/dev/null 2>&1 && xcrun --find simctl >/dev/null 2>&1; then
    xcrun simctl list devices available | grep -E "iPhone|iPad" | head -n 20 || true
else
    echo "    xcrun simctl not available yet in current developer directory."
fi

# 6. Physical iPad Guidance
echo ""
echo "=== Physical iPad (iPadOS 18.7.7) Setup Checklist ==="
echo "1. Connect physical iPad via USB-C cable to Mac."
echo "2. Unlock iPad and tap 'Trust This Computer'."
echo "3. On iPad: Settings -> Privacy & Security -> Developer Mode -> Turn On (requires restart)."
echo "4. In Xcode: Select Personal Team (Free Apple ID) in Signing & Capabilities."
echo "   NOTE: Free Personal Teams may reject App Groups ('group.com.mainstayx.echolet.dev')."
echo "   If provisioning fails due to App Groups, test app standalone or verify Apple ID entitlement capabilities."
echo "5. Discover connected device ID: xcrun devicectl list devices"
echo ""
echo "Discovery complete."
