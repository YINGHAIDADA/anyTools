//! The overlay window on Windows: a borderless, non-activating, topmost window whose outline is the
//! wheel itself.
//!
//! macOS masks the glass with an image and the window stays rectangular. Windows has no per-pixel
//! glass mask, so the window region is cut to the wheel's shape and the acrylic backdrop shows through
//! the page's transparent background. The shape arrives in the same physical pixels the window is
//! positioned in (the hook reports `GetCursorPos` and the screens are read the same way), so the region
//! is not scaled again.

use super::context::monitor_at;
use anytools_core::Frame;
use serde::Deserialize;
use std::sync::Mutex;
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND};
use windows_sys::Win32::Graphics::Gdi::{CombineRgn, CreateEllipticRgn, CreatePolygonRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject, SetWindowRgn, HRGN, RGN_OR, WINDING};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, HWND_TOPMOST, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNOACTIVATE, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT};

/// Re-declared so this module doesn't depend on the macOS overlay's shape type. Same JSON shape.
#[derive(Clone, Debug, Default, Deserialize)]
struct Mask {
    /// The window size the coordinates were authored against. Unused: the region is scaled by DPI.
    #[allow(dead_code)]
    size: [f64; 2],
    #[serde(default)]
    polys: Vec<Vec<[f64; 2]>>,
    #[serde(default)]
    circles: Vec<[f64; 3]>,
    /// `[x, y, w, h, corner radius]`, in window points.
    #[serde(default)]
    rrects: Vec<[f64; 5]>,
}

struct Overlay {
    hwnd: isize,
    session: u64,
}

static OVERLAY: Mutex<Option<Overlay>> = Mutex::new(None);

fn with_overlay(f: impl FnOnce(&mut Overlay)) {
    if let Some(ov) = OVERLAY.lock().unwrap().as_mut() {
        f(ov);
    }
}

/// Remembers the window and makes it a click-through, never-activating popup. Called once, on the UI
/// thread, after the window exists.
pub fn setup(hwnd: isize) {
    let hwnd = hwnd as HWND;
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        // TOOLWINDOW keeps it off the taskbar and out of Alt-Tab, NOACTIVATE stops it taking focus,
        // TRANSPARENT lets clicks fall through to the app underneath (the wheel is driven by the hook).
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT) as isize);
        let corner = DWMWCP_ROUND;
        DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE as u32, &corner as *const _ as *const _, 4);
    }
    *OVERLAY.lock().unwrap() = Some(Overlay { hwnd: hwnd as isize, session: 0 });
}

fn px(v: f64) -> i32 {
    v.round() as i32
}

/// The window region for a mask: the union of its polygons, circles and rounded rects, in the same
/// pixels the window is sized in.
fn region_for(mask: &Mask) -> HRGN {
    unsafe {
        let acc = CreateRectRgn(0, 0, 0, 0);
        let add = |r: HRGN| {
            if !r.is_null() {
                CombineRgn(acc, acc, r, RGN_OR);
                DeleteObject(r as _);
            }
        };
        for poly in &mask.polys {
            let pts: Vec<POINT> = poly.iter().map(|[x, y]| POINT { x: px(*x), y: px(*y) }).collect();
            if pts.len() >= 3 {
                add(CreatePolygonRgn(pts.as_ptr(), pts.len() as i32, WINDING));
            }
        }
        for [cx, cy, r] in &mask.circles {
            let (cx, cy, r) = (px(*cx), px(*cy), px(*r));
            add(CreateEllipticRgn(cx - r, cy - r, cx + r, cy + r));
        }
        for [x, y, w, h, rad] in &mask.rrects {
            let (x, y, w, h) = (px(*x), px(*y), px(*w), px(*h));
            let d = px(*rad) * 2;
            add(CreateRoundRectRgn(x, y, x + w, y + h, d, d));
        }
        acc
    }
}

fn apply_region(ov: &Overlay, shape: &serde_json::Value) {
    let Ok(mask) = serde_json::from_value::<Mask>(shape.clone()) else { return };
    let hwnd = ov.hwnd as HWND;
    unsafe {
        let rgn = region_for(&mask);
        // Windows takes ownership of the region.
        SetWindowRgn(hwnd, rgn, 1);
    }
}

/// Moves the window over `frame` and reveals it without activating. `frame` is in physical pixels,
/// top-left origin, the same space `GetCursorPos` reports.
pub fn show(session: u64, frame: Frame, shape: Option<serde_json::Value>) {
    with_overlay(|ov| {
        let hwnd = ov.hwnd as HWND;
        unsafe {
            SetWindowPos(hwnd, HWND_TOPMOST, frame.x.round() as i32, frame.y.round() as i32, frame.w.round() as i32, frame.h.round() as i32, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        if let Some(shape) = shape {
            apply_region(ov, &shape);
        }
        ov.session = session;
    });
}

/// Replaces the window outline. Ignored when a newer session owns the window.
pub fn set_shape(session: u64, shape: serde_json::Value) {
    with_overlay(|ov| {
        if ov.session == session {
            apply_region(ov, &shape);
        }
    });
}

/// Hides the window unless a newer session has taken it over. Returns whether it hid it. `fade_ms` is
/// unused: the window has no per-window alpha animation, so it disappears at once.
pub fn hide(session: u64, _fade_ms: u64) -> bool {
    let mut hidden = false;
    with_overlay(|ov| {
        if ov.session == session {
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(ov.hwnd as HWND, SW_HIDE) };
            hidden = true;
        }
    });
    hidden
}

/// `frame` with the refresh rate of the monitor under the cursor, so the page can pace its animation.
/// Placement itself happens in the shared geometry, which already reads the screen list in the same
/// physical pixels the window is positioned in.
#[allow(dead_code)]
pub fn place(frame: Frame, cursor_x: f64, cursor_y: f64) -> Frame {
    let Some((_mon, _work, hz)) = monitor_at(cursor_x.round() as i32, cursor_y.round() as i32) else {
        return frame;
    };
    Frame { x: frame.x, y: frame.y, w: frame.w, h: frame.h, hz: if hz > 1.0 { hz } else { frame.hz } }
}
