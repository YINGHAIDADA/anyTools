//! The shared overlay window: non-activating, click-through, on all Spaces, with a native
//! NSVisualEffectView (glass) behind a transparent webview. Main thread only.
//!
//! Latency design (see docs/M0-RESULTS.md): the window stays ordered in at alpha 0 between uses and the
//! webview's occlusion detection is off, so the page keeps the next frame rendered. Showing is just a
//! native alpha animation; it never waits for the web process.

use super::context::set_screens;
use anytools_core::context::{Frame, Rect, ScreenInfo};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObjectProtocol};
use objc2::{define_class, extern_methods, msg_send, sel, ClassType, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSAppearance, NSAppearanceCustomization, NSAppearanceNameDarkAqua,
    NSAutoresizingMaskOptions, NSBezierPath, NSButton, NSColor, NSEvent, NSGlassEffectView, NSGlassEffectViewStyle, NSImage,
    NSPopUpMenuWindowLevel, NSScreen, NSTitlebarSeparatorStyle, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowCollectionBehavior, NSWindowOrderingMode,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use serde::Deserialize;
use std::cell::RefCell;
use std::ffi::c_void;

/// Glass mask: the union of polygons, circles and rounded rects, in window coordinates (y down).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Shape {
    pub size: [f64; 2],
    #[serde(default)]
    pub polys: Vec<Vec<[f64; 2]>>,
    #[serde(default)]
    pub circles: Vec<[f64; 3]>,
    /// `[x, y, w, h, corner_radius]`.
    #[serde(default)]
    pub rrects: Vec<[f64; 5]>,
    /// `[cx, cy, r]`: the disc the liquid glass covers (it can only be a circle or rounded rect).
    #[serde(default)]
    pub disc: Option<[f64; 3]>,
    /// `[x, y, w, h, corner_radius]`: a rounded rect for the liquid glass instead of `disc`.
    #[serde(default)]
    pub glass: Option<[f64; 5]>,
}

impl Shape {
    /// The liquid glass area as `[x, y, w, h, corner_radius]` (y down).
    fn glass_rect(&self) -> Option<[f64; 5]> {
        self.glass.or(self.disc.map(|[cx, cy, r]| [cx - r, cy - r, 2.0 * r, 2.0 * r, r]))
    }
}

struct Overlay {
    window: Retained<NSWindow>,
    effect: Retained<NSVisualEffectView>,
    /// macOS 26+ Liquid Glass (real refraction of what's behind). `None` on older systems.
    glass: Option<Retained<NSGlassEffectView>>,
    liquid: bool,
    shape: Shape,
    session: u64,
    primary_h: f64,
}

// Sits above the transparent title bar and turns a click there into a window drag. `mouseDownCanMoveWindow`
// would do the same, but it also swallows the double-click that zooms the window, which this keeps.
//
// SAFETY: `NSView` has no subclassing requirements, and this type does not implement `Drop`.
define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "AnytoolsTitlebarDragStrip"]
    struct TitlebarDragStrip;

    impl TitlebarDragStrip {
        // The strip only exists to catch clicks, so a click on a traffic-light button falls through to it.
        // Returned unretained, exactly as `NSView` returns `self`: the caller does not own the result.
        #[unsafe(method(hitTest:))]
        fn hit_test(this: *const Self, point: NSPoint) -> *mut NSView {
            let Some(view) = (unsafe { this.as_ref() }) else { return std::ptr::null_mut() };
            let local = view.convertPoint_fromView(point, None);
            let mine = NSRect::new(NSPoint::ZERO, view.bounds().size);
            if !rect_contains(mine, local) {
                return std::ptr::null_mut();
            }
            let Some(window) = view.window() else { return this.cast_mut().cast() };
            for button in traffic_lights(&window) {
                let rect = button.convertRect_toView(button.bounds(), Some(view));
                // A little padding: the buttons' tracking area is wider than the glyph they draw.
                if rect_contains(inset(rect, -4.0), local) {
                    return std::ptr::null_mut();
                }
            }
            this.cast_mut().cast()
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let Some(window) = self.window() else { return };
            // A double-click on the title bar zooms, matching the system default.
            if event.clickCount() == 2 && title_bar_double_click_zooms() {
                window.zoom(None);
                return;
            }
            window.performWindowDragWithEvent(event);
        }
    }
);

impl TitlebarDragStrip {
    extern_methods!(
        #[unsafe(method(initWithFrame:))]
        fn init_with_frame(this: objc2::rc::Allocated<Self>, frame: NSRect) -> Retained<Self>;
    );

    /// Cover whatever of the theme frame sticks out above the content view, i.e. the title bar.
    fn layout_over_titlebar(&self) {
        // SAFETY: the strip is in the window, so it has a superview for its whole lifetime.
        let Some(theme) = (unsafe { self.superview() }) else { return };
        let frame = theme.bounds();
        let title_h = self.window().map(|w| w.frame().size.height - w.contentLayoutRect().size.height).unwrap_or(0.0);
        let h = if title_h > 1.0 { title_h } else { 28.0 };
        self.setFrame(NSRect::new(NSPoint::new(0.0, frame.size.height - h), NSSize::new(frame.size.width, h)));
    }
}

fn traffic_lights(window: &NSWindow) -> Vec<Retained<NSButton>> {
    use objc2_app_kit::NSWindowButton;
    [NSWindowButton::CloseButton, NSWindowButton::MiniaturizeButton, NSWindowButton::ZoomButton]
        .into_iter()
        .filter_map(|b| window.standardWindowButton(b))
        .collect()
}

fn rect_contains(rect: NSRect, point: NSPoint) -> bool {
    point.x >= rect.origin.x
        && point.y >= rect.origin.y
        && point.x < rect.origin.x + rect.size.width
        && point.y < rect.origin.y + rect.size.height
}

fn inset(rect: NSRect, by: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(rect.origin.x - by, rect.origin.y - by),
        NSSize::new(rect.size.width + 2.0 * by, rect.size.height + 2.0 * by),
    )
}

/// The system setting for what a title-bar double-click does. `None` means it does nothing.
fn title_bar_double_click_zooms() -> bool {
    let key = NSString::from_str("AppleActionOnDoubleClick");
    objc2_foundation::NSUserDefaults::standardUserDefaults()
        .stringForKey(&key)
        .is_none_or(|v| v.to_string() != "Minimize")
}

/// The settings page fills the transparent title bar, and AppKit draws that title bar in its own view
/// layered above the page, so the page's drag regions never see the click. A transparent strip is laid
/// over that area instead: a click there starts a window drag, while the traffic-light buttons stay
/// clickable because the strip gives way wherever one of them is. Nothing in the existing view hierarchy
/// is moved, so the window's layout is left alone.
pub fn prepare_settings_window(ns_window: *mut c_void) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(window) = (unsafe { Retained::retain(ns_window as *mut NSWindow) }) else { return };
    window.setTitlebarSeparatorStyle(NSTitlebarSeparatorStyle::None);
    let Some(theme) = window.contentView().and_then(|content| unsafe { content.superview() }) else { return };

    // Reopening the window (it is kept and re-shown) must not stack a second strip on top.
    let already = theme.subviews().iter().any(|v| v.isKindOfClass(TitlebarDragStrip::class()));
    if !already {
        let strip = TitlebarDragStrip::init_with_frame(mtm.alloc(), NSRect::ZERO);
        // Pinned to the top edge (AppKit is y-up) and as wide as the window.
        strip.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        theme.addSubview(&strip);
        strip.layout_over_titlebar();
    }
}

pub fn liquid_glass_available() -> bool {
    objc2::runtime::AnyClass::get(c"NSGlassEffectView").is_some()
}

fn is_liquid(material: &str) -> bool {
    material.starts_with("liquid")
}

fn glass_style(material: &str) -> NSGlassEffectViewStyle {
    if material == "liquid-clear" {
        NSGlassEffectViewStyle::Clear
    } else {
        NSGlassEffectViewStyle::Regular
    }
}

fn glass_frame(g: [f64; 5], win_h: f64) -> NSRect {
    let [x, y, w, h, _] = g;
    // AppKit views are y-up; the shape is y-down.
    NSRect::new(NSPoint::new(x, win_h - y - h), NSSize::new(w, h))
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
        for [x, y, w, h, r] in &s.rrects {
            path.appendBezierPath(&NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                NSRect::new(NSPoint::new(*x, *y), NSSize::new(*w, *h)),
                *r,
                *r,
            ));
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

    // Liquid Glass sits below the webview too (above the classic effect view, which it replaces).
    let glass = liquid_glass_available().then(|| {
        let g = NSGlassEffectView::initWithFrame(mtm.alloc(), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0)));
        g.setStyle(glass_style(material_name));
        g.setHidden(true);
        content.addSubview_positioned_relativeTo(&g, NSWindowOrderingMode::Above, Some(&effect));
        g
    });

    let occlusion_spi = disable_occlusion_detection(wkwebview);
    window.setAlphaValue(0.0);
    window.orderFrontRegardless();

    let primary_h = NSScreen::screens(mtm).iter().next().map(|s| s.frame().size.height).unwrap_or(900.0);
    let mut ov = Overlay { window, effect, glass, liquid: false, shape: Shape::default(), session: 0, primary_h };
    apply_material(&mut ov, material_name);
    OVERLAY.with(|o| *o.borrow_mut() = Some(ov));
    refresh_screens(mtm);
    Ok(occlusion_spi)
}

/// Switches between Liquid Glass and the classic effect view. Returns whether Liquid Glass is in use.
fn apply_material(ov: &mut Overlay, name: &str) -> bool {
    ov.liquid = is_liquid(name) && ov.glass.is_some();
    match (&ov.glass, ov.liquid) {
        (Some(g), true) => {
            g.setStyle(glass_style(name));
            g.setHidden(false);
            ov.effect.setHidden(true);
            layout_glass(ov);
        }
        (g, _) => {
            if let Some(g) = g {
                g.setHidden(true);
            }
            ov.effect.setHidden(false);
            // A liquid material on an older macOS falls back to the default classic material.
            ov.effect.setMaterial(material(if is_liquid(name) { "hud" } else { name }));
        }
    }
    ov.liquid
}

fn layout_glass(ov: &Overlay) {
    let (Some(g), Some(r)) = (&ov.glass, ov.shape.glass_rect()) else { return };
    let h = ov.window.contentView().map(|v| v.bounds().size.height).unwrap_or(ov.shape.size[1]);
    g.setFrame(glass_frame(r, h));
    g.setCornerRadius(r[4]);
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
        layout_glass(ov);
    }
}

pub fn show(session: u64, frame: Frame, shape: Option<Shape>) {
    OVERLAY.with(|o| {
        let mut o = o.borrow_mut();
        let Some(ov) = o.as_mut() else { return };
        let rect = NSRect::new(NSPoint::new(frame.x, ov.primary_h - (frame.y + frame.h)), NSSize::new(frame.w, frame.h));
        ov.window.setFrame_display(rect, false);
        // After the frame: the glass is laid out against the window's (possibly new) height.
        if let Some(s) = shape {
            apply_shape(ov, s);
        }
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
            if let Some(g) = &ov.glass {
                let v: &NSView = g;
                v.setAppearance(appearance.as_deref());
            }
        }
    });
}

/// Returns whether Liquid Glass is now in use (false for classic materials or on macOS < 26).
pub fn set_material(name: &str) -> bool {
    OVERLAY.with(|o| o.borrow_mut().as_mut().map(|ov| apply_material(ov, name)).unwrap_or(false))
}

pub fn app_is_active(mtm: MainThreadMarker) -> bool {
    objc2_app_kit::NSApplication::sharedApplication(mtm).isActive()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_parses() {
        let s: Shape = serde_json::from_str(r#"{"size":[420,420],"polys":[[[0,0],[10,0],[10,10]]],"circles":[[210,210,52]],"disc":[210,210,150]}"#).unwrap();
        assert_eq!(s.polys[0].len(), 3);
        assert_eq!(s.circles[0], [210.0, 210.0, 52.0]);
        assert_eq!(s.glass_rect(), Some([60.0, 60.0, 300.0, 300.0, 150.0]));
        let p: Shape = serde_json::from_str(r#"{"size":[400,460],"rrects":[[0,0,400,460,24]],"glass":[0,0,400,460,24]}"#).unwrap();
        assert_eq!(p.glass_rect(), Some([0.0, 0.0, 400.0, 460.0, 24.0]));
    }
}
