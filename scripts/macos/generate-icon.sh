#!/usr/bin/env bash
set -euo pipefail

# This script generates Echolet.icns from assets/echolet.svg using native macOS APIs and iconutil.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SVG_SRC="${REPO_ROOT}/assets/echolet.svg"
OUTPUT_ICNS="${REPO_ROOT}/assets/macos/Echolet.icns"
ICONSET_DIR="$(mktemp -d)/Echolet.iconset"

trap 'rm -rf "${ICONSET_DIR}"' EXIT

mkdir -p "${ICONSET_DIR}"

python3 - << EOF
from Cocoa import NSImage, NSBitmapImageRep, NSPNGFileType, NSGraphicsContext
from Foundation import NSData
import os

svg_path = "${SVG_SRC}"
iconset_dir = "${ICONSET_DIR}"

with open(svg_path, 'rb') as f:
    svg_bytes = f.read()

data = NSData.dataWithBytes_length_(svg_bytes, len(svg_bytes))
img = NSImage.alloc().initWithData_(data)
if not img or not img.isValid():
    raise RuntimeError("Failed to load SVG with NSImage")

targets = [
    (16, "icon_16x16.png"),
    (32, "icon_16x16@2x.png"),
    (32, "icon_32x32.png"),
    (64, "icon_32x32@2x.png"),
    (128, "icon_128x128.png"),
    (256, "icon_128x128@2x.png"),
    (256, "icon_256x256.png"),
    (512, "icon_256x256@2x.png"),
    (512, "icon_512x512.png"),
    (1024, "icon_512x512@2x.png"),
]

for size, filename in targets:
    rep = NSBitmapImageRep.alloc().initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel_(
        None, size, size, 8, 4, True, False, 'NSCalibratedRGBColorSpace', 0, 0
    )
    ctx = NSGraphicsContext.graphicsContextWithBitmapImageRep_(rep)
    NSGraphicsContext.setCurrentContext_(ctx)
    img.drawInRect_fromRect_operation_fraction_(((0, 0), (size, size)), ((0, 0), (24, 24)), 2, 1.0)
    NSGraphicsContext.setCurrentContext_(None)
    png_data = rep.representationUsingType_properties_(NSPNGFileType, {})
    out_path = os.path.join(iconset_dir, filename)
    png_data.writeToFile_atomically_(out_path, False)

EOF

mkdir -p "$(dirname "${OUTPUT_ICNS}")"
iconutil -c icns "${ICONSET_DIR}" -o "${OUTPUT_ICNS}"
echo "Generated ${OUTPUT_ICNS} successfully."
