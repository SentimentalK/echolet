#!/usr/bin/env bash
set -euo pipefail

# This script generates Linux desktop icon assets from assets/echolet.png and assets/echolet.svg

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SRC_PNG="${REPO_ROOT}/assets/echolet.png"
SRC_SVG="${REPO_ROOT}/assets/echolet.svg"
LINUX_ASSETS="${REPO_ROOT}/assets/linux"

mkdir -p "${LINUX_ASSETS}"

echo "--> Generating Linux icon assets..."

python3 - << EOF
from PIL import Image
import os

src_png = "${SRC_PNG}"
out_png = os.path.join("${LINUX_ASSETS}", "echolet.png")

if not os.path.exists(src_png):
    raise FileNotFoundError(f"Source icon not found at {src_png}")

im = Image.open(src_png)
im_resized = im.resize((512, 512), Image.Resampling.LANCZOS)
im_resized.save(out_png, optimize=True)
print(f"Generated {out_png} (512x512, {os.path.getsize(out_png)} bytes)")
EOF

if [[ -f "${SRC_SVG}" ]]; then
    cp "${SRC_SVG}" "${LINUX_ASSETS}/echolet.svg"
    echo "Copied scalable vector icon to ${LINUX_ASSETS}/echolet.svg"
fi

echo "--> Linux icon assets generated successfully."
