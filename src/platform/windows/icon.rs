//! Procedurally generated tray icon, matching the Linux/macOS style:
//!   - Standby (false): gray outline circle  ○  (#E0E0E0)
//!   - Listening (true):  red filled circle  ●  (#FF3B30)
//!
//! Built at runtime from in-memory ARGB pixels via `CreateDIBSection` +
//! `CreateIconIndirect`, so no .ico resource needs to be shipped.

use std::mem::size_of;
use std::ptr;
use windows_sys::Win32::Graphics::Gdi::{
    BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DeleteObject, DIB_RGB_COLORS,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

const ICON_SIZE: i32 = 32;

/// Creates a 32x32 tray icon. `listening` selects the red filled circle
/// (true) or the gray outline circle (false). Caller owns the returned
/// HICON and must `DestroyIcon` it when done.
pub fn create_echolet_icon(listening: bool) -> HICON {
    let size = ICON_SIZE;
    let center = size as f32 / 2.0;
    let radius = size as f32 * 0.38;
    let stroke_width = 2.5f32;

    // 32bpp BI_RGB DIB pixel layout is B, G, R, A (bottom-up rows; symmetric icon).
    let mut pixels: Vec<u8> = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let dist = (dx * dx + dy * dy).sqrt();

            // (alpha, r, g, b) — same values as the Linux ksni icon.
            let (a, r, g, b) = if listening {
                // Solid red circle ● (#FF3B30)
                if dist <= radius {
                    (255u8, 255u8, 59u8, 48u8)
                } else if dist < radius + 1.0 {
                    let alpha = ((1.0 - (dist - radius)) * 255.0) as u8;
                    (alpha, 255, 59, 48)
                } else {
                    (0, 0, 0, 0)
                }
            } else {
                // Outline circle ○ (#E0E0E0)
                let diff = (dist - radius).abs();
                if diff <= stroke_width / 2.0 {
                    (230u8, 220u8, 220u8, 220u8)
                } else if diff < stroke_width / 2.0 + 1.0 {
                    let alpha = ((1.0 - (diff - stroke_width / 2.0)) * 230.0) as u8;
                    (alpha, 220, 220, 220)
                } else {
                    (0, 0, 0, 0)
                }
            };

            pixels.push(b);
            pixels.push(g);
            pixels.push(r);
            pixels.push(a);
        }
    }

    // 1) Color bitmap: 32bpp ARGB DIB section.
    let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
    bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = size;
    bmi.bmiHeader.biHeight = size; // bottom-up; symmetric icon, flip is invisible
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = 0; // BI_RGB

    let mut bits: *mut std::ffi::c_void = ptr::null_mut();
    let color_bmp = unsafe {
        CreateDIBSection(
            ptr::null_mut(),
            &bmi,
            DIB_RGB_COLORS,
            &mut bits,
            ptr::null_mut(),
            0,
        )
    };
    if color_bmp.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());
    }

    // 2) Mask bitmap: monochrome, all zeros (opaque everywhere; alpha channel
    //    on the color bitmap does the real per-pixel transparency).
    let mask_stride = ((size + 7) / 8) as usize * size as usize;
    let mask_pixels = vec![0u8; mask_stride];
    let mask_bmp = unsafe {
        CreateBitmap(
            size,
            size,
            1,
            1,
            mask_pixels.as_ptr() as *const std::ffi::c_void,
        )
    };

    // 3) Combine into an icon.
    let mut ii: ICONINFO = unsafe { std::mem::zeroed() };
    ii.fIcon = 1; // TRUE
    ii.xHotspot = 0;
    ii.yHotspot = 0;
    ii.hbmMask = mask_bmp;
    ii.hbmColor = color_bmp;

    let icon = unsafe { CreateIconIndirect(&mut ii) };

    unsafe {
        DeleteObject(color_bmp);
        DeleteObject(mask_bmp);
    }

    icon
}
