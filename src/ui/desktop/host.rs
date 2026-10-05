//! Desktop Panel Host Abstraction and Platform Geometry Seams (PROJECT-041 / J11.1d).
//!
//! Provides mockable platform anchor calculations and standard host contracts.

use crate::ui::control_surface::ControlSurfaceState;

/// Shared host abstraction for operating the desktop panel across platforms.
pub trait DesktopPanelHost: Send + Sync {
    /// Toggles the panel between visible and hidden states.
    fn toggle_panel(&self);

    /// Shows the panel and anchors it to the system tray/status item.
    fn show_panel(&self);

    /// Hides the panel.
    fn hide_panel(&self);

    /// Updates the panel state from canonical [`ControlSurfaceState`].
    fn update_state(&self, state: &ControlSurfaceState);
}

/// 2D rectangle in logical pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// 2D point in logical pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// Calculates clamped panel position for macOS status bar item.
///
/// On macOS, menu bar is at top of screen.
/// Panel is horizontally centered on the status item button, clamped to screen bounds,
/// and positioned immediately below the menu bar.
pub fn calculate_macos_panel_position(
    status_item_frame: Rect,
    screen_bounds: Rect,
    panel_width: u32,
    panel_height: u32,
) -> Point {
    let center_x = status_item_frame.x + (status_item_frame.width as i32 / 2);
    let mut x = center_x - (panel_width as i32 / 2);

    let min_x = screen_bounds.x;
    let max_x = screen_bounds.x + screen_bounds.width as i32 - panel_width as i32;
    if max_x >= min_x {
        x = x.clamp(min_x, max_x);
    }

    // Place directly below status bar item
    let y = status_item_frame.y + status_item_frame.height as i32;

    // Safety vertical clamp
    let max_y = screen_bounds.y + screen_bounds.height as i32 - panel_height as i32;
    let clamped_y = if max_y >= screen_bounds.y {
        y.min(max_y)
    } else {
        y
    };

    Point { x, y: clamped_y }
}

/// Calculates clamped panel position for Windows notification area tray icon.
///
/// Windows taskbar can be docked at bottom, top, left, or right.
/// Clamps strictly within the available work area.
pub fn calculate_windows_panel_position(
    tray_icon_rect: Rect,
    work_area: Rect,
    panel_width: u32,
    panel_height: u32,
) -> Point {
    let is_bottom_taskbar = tray_icon_rect.y + (tray_icon_rect.height as i32 / 2)
        >= work_area.y + (work_area.height as i32 / 2);

    let mut x = tray_icon_rect.x + (tray_icon_rect.width as i32 / 2) - (panel_width as i32 / 2);
    let min_x = work_area.x;
    let max_x = work_area.x + work_area.width as i32 - panel_width as i32;
    if max_x >= min_x {
        x = x.clamp(min_x, max_x);
    }

    let y = if is_bottom_taskbar {
        // Place above taskbar
        (tray_icon_rect.y - panel_height as i32).max(work_area.y)
    } else {
        // Place below taskbar
        (tray_icon_rect.y + tray_icon_rect.height as i32)
            .min(work_area.y + work_area.height as i32 - panel_height as i32)
    };

    Point { x, y }
}

/// Calculates clamped panel position for Linux system tray.
///
/// If tray anchor is provided (X11 / ksni coordinates), positions adjacent to the tray.
/// On native Wayland where absolute coordinate mapping is unavailable, uses a predictable
/// top-right workspace fallback.
pub fn calculate_linux_panel_position(
    tray_anchor: Option<Rect>,
    screen_bounds: Rect,
    panel_width: u32,
    panel_height: u32,
) -> Point {
    if let Some(anchor) = tray_anchor {
        let is_bottom = anchor.y >= screen_bounds.y + (screen_bounds.height as i32 / 2);
        let mut x = anchor.x + (anchor.width as i32 / 2) - (panel_width as i32 / 2);
        let min_x = screen_bounds.x;
        let max_x = screen_bounds.x + screen_bounds.width as i32 - panel_width as i32;
        if max_x >= min_x {
            x = x.clamp(min_x, max_x);
        }

        let y = if is_bottom {
            (anchor.y - panel_height as i32).max(screen_bounds.y)
        } else {
            (anchor.y + anchor.height as i32)
                .min(screen_bounds.y + screen_bounds.height as i32 - panel_height as i32)
        };

        Point { x, y }
    } else {
        // Wayland fallback: predictable top-right placement with 16px right margin, 32px top margin
        let x = (screen_bounds.x + screen_bounds.width as i32 - panel_width as i32 - 16)
            .max(screen_bounds.x);
        let y = screen_bounds.y + 32;
        Point { x, y }
    }
}
