#!/usr/bin/env bash
set -euo pipefail

# This script generates echolet.ico from assets/echolet.svg.
# Windows application icons include 16x16, 24x24, 32x32, 48x48, 64x64, 128x128, and 256x256.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SVG_SRC="${REPO_ROOT}/assets/echolet.svg"
OUTPUT_ICO="${REPO_ROOT}/assets/windows/echolet.ico"

mkdir -p "$(dirname "${OUTPUT_ICO}")"

python3 - << EOF
import io
import struct
import os
from PIL import Image

svg_path = "${SVG_SRC}"
output_ico_path = "${OUTPUT_ICO}"

# Render multi-resolution bitmaps from canonical SVG
sizes = [16, 24, 32, 48, 64, 128, 256]
rendered_images = {}

try:
    # Use native macOS Cocoa vector renderer if available
    from Cocoa import NSImage, NSBitmapImageRep, NSPNGFileType, NSGraphicsContext
    from Foundation import NSData

    with open(svg_path, "rb") as f:
        svg_bytes = f.read()

    data = NSData.dataWithBytes_length_(svg_bytes, len(svg_bytes))
    img = NSImage.alloc().initWithData_(data)
    if not img or not img.isValid():
        raise RuntimeError("Failed to load SVG with NSImage")

    for s in sizes:
        rep = NSBitmapImageRep.alloc().initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel_(
            None, s, s, 8, 4, True, False, "NSCalibratedRGBColorSpace", 0, 0
        )
        ctx = NSGraphicsContext.graphicsContextWithBitmapImageRep_(rep)
        NSGraphicsContext.setCurrentContext_(ctx)
        img.drawInRect_fromRect_operation_fraction_(((0, 0), (s, s)), ((0, 0), (24, 24)), 2, 1.0)
        NSGraphicsContext.setCurrentContext_(None)
        png_data = bytes(rep.representationUsingType_properties_(NSPNGFileType, {}))
        im = Image.open(io.BytesIO(png_data)).convert("RGBA")
        rendered_images[s] = (im, png_data)

except ImportError:
    # Fallback to cairosvg or resvg if Cocoa is not present
    import cairosvg
    for s in sizes:
        png_data = cairosvg.svg2png(url=svg_path, output_width=s, output_height=s)
        im = Image.open(io.BytesIO(png_data)).convert("RGBA")
        rendered_images[s] = (im, png_data)

frames_data = []
for s in sizes:
    im, png_data = rendered_images[s]
    if s == 256:
        # Standard Windows Vista+ PNG-compressed frame for 256x256
        frames_data.append((256, 256, png_data))
    else:
        # Standard uncompressed DIB frame for 16..128
        w, h = s, s
        bih = struct.pack("<IIIHHIIIIII", 40, w, h * 2, 1, 32, 0, w * h * 4, 0, 0, 0, 0)
        xor_bytes = bytearray()
        for y in reversed(range(h)):
            for x in range(w):
                r, g, b, a = im.getpixel((x, y))
                xor_bytes.extend([b, g, r, a])
        row_bytes_len = ((w + 31) // 32) * 4
        and_bytes = bytearray(row_bytes_len * h)
        frame_bytes = bih + bytes(xor_bytes) + bytes(and_bytes)
        frames_data.append((w, h, frame_bytes))

num_images = len(frames_data)
ico_header = struct.pack("<HHH", 0, 1, num_images)
entries = []
offset = 6 + 16 * num_images

for w, h, data_bytes in frames_data:
    bw = 0 if w >= 256 else w
    bh = 0 if h >= 256 else h
    dwBytesInRes = len(data_bytes)
    entry = struct.pack("<BBBBHHII", bw, bh, 0, 0, 1, 32, dwBytesInRes, offset)
    entries.append(entry)
    offset += dwBytesInRes

with open(output_ico_path, "wb") as f:
    f.write(ico_header)
    for e in entries:
        f.write(e)
    for _, _, data_bytes in frames_data:
        f.write(data_bytes)

print(f"Generated {output_ico_path} successfully ({os.path.getsize(output_ico_path)} bytes).")
EOF
