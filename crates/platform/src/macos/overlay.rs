//! The shared overlay window: non-activating, click-through, on all Spaces, with a native
//! NSVisualEffectView (glass) behind a transparent webview. Main thread only.
//!
//! Latency design (see poc/M0-RESULTS.md): the window stays ordered in at alpha 0 between uses and the
//! webview's occlusion detection is off, so the page keeps the next frame rendered. Showing is just a
//! native alpha animation; it never waits for the web process.

use super::context::set_screens;
use anytools_core::context::{Frame, Rect, ScreenInfo};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{msg_send, sel, MainThreadMarker};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSAppearance, NSAppearanceCustomization, NSAppearanceNameDarkAqua,
    NSAutoresizingMaskOptions, NSBezierPath, NSColor, NSImage, NSPopUpMenuWindowLevel, NSScreen, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowCollectionBehavior,
    NSWindowOrderingMode,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use serde::Deserialize;
use std::cell::RefCell;
use std::ffi::c_void;

/// Glass mask: the union of polygons and circles, in window coordinates (y down).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Shape {
    pub size: [f64; 2],
    #[serde(default)]
    pub polys: Vec<Vec<[f64; 2]>>,
    #[serde(default)]
    pub circles: Vec<[f64; 3]>,
}

struct Overlay {
    window: Retained<NSWindow>,
    effect: Retained<NSVisualEffectView>,
    shape: Shape,
    session: u64,
    primary_h: f64,
}

thread_local! {
    static OVERLAY: RefCell<Option<Overlay>> = const { RefCell::new(None) };
}

pub fn material(name: &str) -> NSVisualEffectMaterial {
    match name {
        "popover" => NSVisualEffectMaterial::Popover,
        "menu" => NSVisualEffectMaterial::Menu,
        "sidebar" => NSVisualEffectMaterial::Sidebar,
        "fullscreen-ui" => NSVisualEffectMaterial::FullScreenUI,
        "under-window" => NSVisualEffectMaterial::UnderWindowBackground,
        _ => NSVisualEffectMaterial::HUDWindow,
    }
}

fn mask_image(shape: &Shape) -> Retained<NSImage> {
    let s = shape.clone();
    let handler = RcBlock::new(move |_r: NSRect| -> Bool {
        let path = NSBezierPath::bezierPath();
        for poly in &s.polys {
            for (k, [x, y]) in poly.iter().enumerate() {
                let pt = NSPoint::new(*x, *y);
                if k == 0 {
                    path.moveToPoint(pt);
                } else {
                    path.lineToPoint(pt);
                }
            }
            path.closePath();
        }
        for [cx, cy, r] in &s.circles {
            path.appendBezierPath(&NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
                NSPoint::new(cx - r, cy - r),
                NSSize::new(2.0 * r, 2.0 * r),
            )));
        }
        NSColor::blackColor().setFill();
        path.fill();
        Bool::YES
    });
    NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(shape.size[0], shape.size[1]), true, &handler)
}

/// Screen layout in global top-left coordinates; also refreshes the cache used by `ContextProbe`.
pub fn refresh_screens(mtm: MainThreadMarker) -> Vec<ScreenInfo> {
    let list = NSScreen::screens(mtm);
    let Some(primary) = list.iter().next() else { return vec![] };
    let ph = primary.frame().size.height;
    let conv = |r: NSRect| Rect { x: r.origin.x, y: ph - (r.origin.y + r.size.height), w: r.size.width, h: r.size.height };
    let v: Vec<ScreenInfo> = list
        .iter()
        .map(|s| ScreenInfo { frame: conv(s.frame()), visible: conv(s.visibleFrame()), hz: s.maximumFramesPerSecond() as f64 })
        .collect();
    set_screens(v.clone());
    OVERLAY.with(|o| {
        if let Some(ov) = o.borrow_mut().as_mut() {
            ov.primary_h = ph;
        }
    });
    v
}

/// Turns the (hidden) Tauri overlay window into the glass popup. `ns_window`: the NSWindow,
/// `wkwebview`: its WKWebView. Returns whether the occlusion SPI was available.
pub fn setup(mtm: MainThreadMarker, ns_window: *mut c_void, wkwebview: *mut c_void, material_name: &str) -> Result<bool, String> {
    let window: Retained<NSWindow> =
        unsafe { Retained::retain(ns_window as *mut NSWindow) }.ok_or("overlay NSWindow is null")?;
    window.setLevel(NSPopUpMenuWindowLevel);
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    window.setIgnoresMouseEvents(true);
    window.setOpaque(false);
    window.setHasShadow(false);
    window.setBackgroundColor(Some(&NSColor::clearColor()));

    let content = window.contentView().ok_or("overlay has no contentView")?;
    let effect = NSVisualEffectView::initWithFrame(mtm.alloc(), content.bounds());
    effect.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable);
    effect.setMaterial(material(material_name));
    effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    effect.setState(NSVisualEffectState::Active);
    if let Some(dark) = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }) {
        effect.setAppearance(Some(&dark));
    }
    // Fully masked until the first shape arrives.
    effect.setMaskImage(Some(&mask_image(&Shape { size: [1.0, 1.0], ..Default::default() })));
    content.addSubview_positioned_relativeTo(&effect, NSWindowOrderingMode::Below, None);

    let occlusion_spi = disable_occlusion_detection(wkwebview);
    window.setAlphaValue(0.0);
    window.orderFrontRegardless();

    let primary_h = NSScreen::screens(mtm).iter().next().map(|s| s.frame().size.height).unwrap_or(900.0);
    OVERLAY.with(|o| *o.borrow_mut() = Some(Overlay { window, effect, shape: Shape::default(), session: 0, primary_h }));
    refresh_screens(mtm);
    Ok(occlusion_spi)
}

fn disable_occlusion_detection(wkwebview: *mut c_void) -> bool {
    let wk = wkwebview as *mut AnyObject;
    if wk.is_null() {
        return false;
    }
    unsafe {
        let ok: bool = msg_send![wk, respondsToSelector: sel!(_setWindowOcclusionDetectionEnabled:)];
        if ok {
            let _: () = msg_send![wk, _setWindowOcclusionDetectionEnabled: false];
        } else {
            tracing::warn!("WKWebView occlusion SPI missing: overlay first paint will be slower");
        }
        ok
    }
}

fn animate_alpha(window: &NSWindow, to: f64, secs: f64, ease_out: bool) {
    NSAnimationContext::beginGrouping();
    let ctx = NSAnimationContext::currentContext();
    ctx.setDuration(secs);
    unsafe {
        let name = NSString::from_str(if ease_out { "easeOut" } else { "easeIn" });
        let f: *mut AnyObject = msg_send![objc2::class!(CAMediaTimingFunction), functionWithName: &*name];
        let _: () = msg_send![&*ctx, setTimingFunction: f];
    }
    window.animator().setAlphaValue(to);
    NSAnimationContext::endGrouping();
}

fn apply_shape(ov: &mut Overlay, shape: Shape) {
    if ov.shape != shape {
        ov.effect.setMaskImage(Some(&mask_image(&shape)));
        ov.shape = shape;
    }
}

pub fn show(session: u64, frame: Frame, shape: Option<Shape>) {
    OVERLAY.with(|o| {
        let mut o = o.borrow_mut();
        let Some(ov) = o.as_mut() else { return };
        if let Some(s) = shape {
            apply_shape(ov, s);
        }
        let rect = NSRect::new(NSPoint::new(frame.x, ov.primary_h - (frame.y + frame.h)), NSSize::new(frame.w, frame.h));
        ov.window.setFrame_display(rect, false);
        ov.window.setAlphaValue(0.0);
        if !ov.window.isVisible() {
            ov.window.orderFrontRegardless();
        }
        animate_alpha(&ov.window, 1.0, 0.11, true);
        ov.session = session;
    });
}

pub fn set_shape(session: u64, shape: Shape) {
    OVERLAY.with(|o| {
        if let Some(ov) = o.borrow_mut().as_mut().filter(|ov| ov.session == session) {
            apply_shape(ov, shape);
        }
    });
}

/// Starts the fade-out. Returns false if a newer session owns the window.
pub fn hide(session: u64, fade_ms: u64) -> bool {
    OVERLAY.with(|o| match o.borrow().as_ref().filter(|ov| ov.session == session) {
        Some(ov) => {
            animate_alpha(&ov.window, 0.0, fade_ms as f64 / 1000.0, false);
            true
        }
        None => false,
    })
}

/// `dark` | `light` | `auto` (follow the system).
pub fn set_theme(theme: &str) {
    OVERLAY.with(|o| {
        if let Some(ov) = o.borrow().as_ref() {
            let name = match theme {
                "light" => Some(unsafe { objc2_app_kit::NSAppearanceNameAqua }),
                "dark" => Some(unsafe { NSAppearanceNameDarkAqua }),
                _ => None,
            };
            let appearance = name.and_then(NSAppearance::appearanceNamed);
            ov.effect.setAppearance(appearance.as_deref());
        }
    });
}

pub fn set_material(name: &str) {
    OVERLAY.with(|o| {
        if let Some(ov) = o.borrow().as_ref() {
            ov.effect.setMaterial(material(name));
        }
    });
}

pub fn app_is_active(mtm: MainThreadMarker) -> bool {
    objc2_app_kit::NSApplication::sharedApplication(mtm).isActive()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_parses() {
        let s: Shape = serde_json::from_str(r#"{"size":[420,420],"polys":[[[0,0],[10,0],[10,10]]],"circles":[[210,210,52]]}"#).unwrap();
        assert_eq!(s.polys[0].len(), 3);
        assert_eq!(s.circles[0], [210.0, 210.0, 52.0]);
    }
}
