//! macOS platform layer for the M0 PoC: event tap, key injection, overlay window + glass, Finder probe.
#![allow(non_upper_case_globals, non_snake_case)]

use crate::wheel::{self, AppInfo, FileCtx, Frame, Input, Pt, Rect, ScreenInfo, GEOM};
use block2::RcBlock;
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::{AnyObject, Bool};
use objc2::{msg_send, sel, AllocAnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSAppearance, NSAppearanceCustomization,
    NSAppearanceNameDarkAqua, NSAutoresizingMaskOptions, NSBezierPath, NSColor, NSImage, NSPasteboard,
    NSPasteboardTypeString, NSPopUpMenuWindowLevel, NSScreen, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowCollectionBehavior, NSWindowOrderingMode,
    NSWorkspace,
};
use objc2_foundation::{NSAppleEventDescriptor, NSAppleScript, NSDictionary, NSPoint, NSRect, NSSize, NSString};
use std::cell::RefCell;
use std::ffi::c_void;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------------------------
// CoreGraphics / CoreFoundation / ApplicationServices FFI
// ---------------------------------------------------------------------------------------------

type CGEventRef = *mut c_void;
type CFTypeRef = *const c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

type TapCallback = extern "C" fn(*mut c_void, u32, CGEventRef, *mut c_void) -> CGEventRef;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapCreate(tap: u32, place: u32, options: u32, mask: u64, cb: TapCallback, info: *mut c_void) -> *mut c_void;
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);
    fn CGEventGetLocation(e: CGEventRef) -> CGPoint;
    fn CGEventGetIntegerValueField(e: CGEventRef, field: u32) -> i64;
    fn CGEventSetIntegerValueField(e: CGEventRef, field: u32, v: i64);
    fn CGEventGetFlags(e: CGEventRef) -> u64;
    fn CGEventSetFlags(e: CGEventRef, flags: u64);
    fn CGEventGetTimestamp(e: CGEventRef) -> u64;
    fn CGEventCreateKeyboardEvent(src: *mut c_void, key: u16, down: bool) -> CGEventRef;
    fn CGEventKeyboardSetUnicodeString(e: CGEventRef, len: usize, s: *const u16);
    fn CGEventPost(tap: u32, e: CGEventRef);
    fn CGEventSourceCreate(state: i32) -> *mut c_void;
    fn CGEventSourceFlagsState(state: i32) -> u64;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFMachPortCreateRunLoopSource(alloc: CFTypeRef, port: *mut c_void, order: isize) -> *mut c_void;
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopAddSource(rl: *mut c_void, src: *mut c_void, mode: CFTypeRef);
    fn CFRunLoopRun();
    fn CFRelease(p: CFTypeRef);
    fn CFDictionaryCreate(
        alloc: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        n: isize,
        kcb: *const c_void,
        vcb: *const c_void,
    ) -> CFTypeRef;
    static kCFRunLoopCommonModes: CFTypeRef;
    static kCFBooleanTrue: CFTypeRef;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: CFTypeRef) -> bool;
    static kAXTrustedCheckOptionPrompt: CFTypeRef;
}

extern "C" {
    fn clock_gettime_nsec_np(clock: u32) -> u64;
}
const CLOCK_UPTIME_RAW: u32 = 8;

const EV_LEFT_DOWN: u32 = 1;
const EV_LEFT_UP: u32 = 2;
const EV_RIGHT_DOWN: u32 = 3;
const EV_RIGHT_UP: u32 = 4;
const EV_MOVED: u32 = 5;
const EV_LEFT_DRAG: u32 = 6;
const EV_RIGHT_DRAG: u32 = 7;
const EV_KEY_DOWN: u32 = 10;
const EV_KEY_UP: u32 = 11;
const EV_FLAGS: u32 = 12;
const EV_OTHER_DRAG: u32 = 27;
const EV_TAP_DISABLED_TIMEOUT: u32 = 0xFFFF_FFFE;
const EV_TAP_DISABLED_USER: u32 = 0xFFFF_FFFF;

const F_AUTOREPEAT: u32 = 8;
const F_KEYCODE: u32 = 9;
const F_USERDATA: u32 = 42;

const FLAG_SHIFT: u64 = 0x0002_0000;
const FLAG_CTRL: u64 = 0x0004_0000;
const FLAG_ALT: u64 = 0x0008_0000;
const FLAG_CMD: u64 = 0x0010_0000;
const FLAG_NUMPAD: u64 = 0x0020_0000;
const FLAG_FN: u64 = 0x0080_0000;

const KC_SPACE: i64 = 49;
const KC_ESC: i64 = 53;

/// Marks events we inject so the tap lets them through untouched.
const MAGIC: i64 = 0x616E_7954;

const HID_TAP: u32 = 0;
const SESSION_TAP: u32 = 1;
const STATE_PRIVATE: i32 = -1;
const STATE_HID: i32 = 1;

// ---------------------------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------------------------

pub fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

pub fn request_accessibility() -> bool {
    unsafe {
        let keys = [kAXTrustedCheckOptionPrompt];
        let vals = [kCFBooleanTrue];
        let dict = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            vals.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks as *const c_void,
            &kCFTypeDictionaryValueCallBacks as *const c_void,
        );
        let ok = AXIsProcessTrustedWithOptions(dict);
        CFRelease(dict);
        ok
    }
}

// ---------------------------------------------------------------------------------------------
// Event tap
// ---------------------------------------------------------------------------------------------

pub struct TapEvent {
    pub input: Input,
    /// Event timestamp → tap callback, when the event carries a plausible uptime timestamp.
    pub hid_to_tap_ms: Option<f64>,
}

/// Called on the tap thread for every relevant event. Returns true to swallow the event.
pub type TapHandler = Box<dyn Fn(TapEvent, Instant) -> bool + Send + Sync>;

struct TapCtx {
    handler: TapHandler,
    port: std::sync::atomic::AtomicPtr<c_void>,
    on_reenable: Box<dyn Fn(&'static str) + Send + Sync>,
}

static TAP: std::sync::OnceLock<TapCtx> = std::sync::OnceLock::new();

extern "C" fn tap_callback(_proxy: *mut c_void, ty: u32, e: CGEventRef, _info: *mut c_void) -> CGEventRef {
    let now = Instant::now();
    let Some(ctx) = TAP.get() else { return e };
    if ty == EV_TAP_DISABLED_TIMEOUT || ty == EV_TAP_DISABLED_USER {
        let port = ctx.port.load(std::sync::atomic::Ordering::Acquire);
        if !port.is_null() {
            unsafe { CGEventTapEnable(port, true) };
        }
        (ctx.on_reenable)(if ty == EV_TAP_DISABLED_TIMEOUT { "timeout" } else { "user-input" });
        return e;
    }
    unsafe {
        if CGEventGetIntegerValueField(e, F_USERDATA) == MAGIC {
            return e;
        }
        let loc = || {
            let p = CGEventGetLocation(e);
            Pt { x: p.x, y: p.y }
        };
        let flags = CGEventGetFlags(e);
        let input = match ty {
            EV_KEY_DOWN => match CGEventGetIntegerValueField(e, F_KEYCODE) {
                KC_SPACE if CGEventGetIntegerValueField(e, F_AUTOREPEAT) != 0 => Some(Input::TriggerRepeat),
                KC_SPACE if flags & FLAG_ALT != 0 && flags & (FLAG_CMD | FLAG_CTRL) == 0 => {
                    Some(Input::TriggerDown(loc()))
                }
                KC_ESC => Some(Input::EscDown),
                _ => None,
            },
            EV_KEY_UP => match CGEventGetIntegerValueField(e, F_KEYCODE) {
                KC_SPACE => Some(Input::TriggerUp),
                KC_ESC => Some(Input::EscUp),
                _ => None,
            },
            EV_FLAGS if flags & FLAG_ALT == 0 => Some(Input::ModRelease),
            EV_MOVED | EV_LEFT_DRAG | EV_RIGHT_DRAG | EV_OTHER_DRAG => Some(Input::Move(loc())),
            EV_LEFT_DOWN => Some(Input::LeftDown),
            EV_LEFT_UP => Some(Input::LeftUp),
            EV_RIGHT_DOWN => Some(Input::RightDown),
            EV_RIGHT_UP => Some(Input::RightUp),
            _ => None,
        };
        let Some(input) = input else { return e };
        let hid_to_tap_ms = if matches!(input, Input::TriggerDown(_)) {
            let ts = CGEventGetTimestamp(e);
            let up = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
            let d = up.wrapping_sub(ts) as f64 / 1e6;
            (ts > 0 && (0.0..500.0).contains(&d)).then_some(d)
        } else {
            None
        };
        if (ctx.handler)(TapEvent { input, hid_to_tap_ms }, now) {
            std::ptr::null_mut()
        } else {
            e
        }
    }
}

/// Starts the event tap on its own CFRunLoop thread. Fails when Accessibility is not granted.
pub fn start_tap(handler: TapHandler, on_reenable: Box<dyn Fn(&'static str) + Send + Sync>) -> Result<(), String> {
    if tap_running() {
        return Ok(());
    }
    // The handler is installed once; a failed CGEventTapCreate leaves `port` null so a later call retries.
    let _ = TAP.set(TapCtx { handler, port: Default::default(), on_reenable });
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("event-tap".into())
        .spawn(move || unsafe {
            let mask: u64 = [
                EV_LEFT_DOWN,
                EV_LEFT_UP,
                EV_RIGHT_DOWN,
                EV_RIGHT_UP,
                EV_MOVED,
                EV_LEFT_DRAG,
                EV_RIGHT_DRAG,
                EV_KEY_DOWN,
                EV_KEY_UP,
                EV_FLAGS,
                EV_OTHER_DRAG,
            ]
            .iter()
            .fold(0, |m, t| m | (1u64 << t));
            let port = CGEventTapCreate(SESSION_TAP, 0, 0, mask, tap_callback, std::ptr::null_mut());
            if port.is_null() {
                let _ = tx.send(Err("CGEventTapCreate 失败（未授予辅助功能权限？）".to_string()));
                return;
            }
            TAP.get().unwrap().port.store(port, std::sync::atomic::Ordering::Release);
            let src = CFMachPortCreateRunLoopSource(std::ptr::null(), port, 0);
            CFRunLoopAddSource(CFRunLoopGetCurrent(), src, kCFRunLoopCommonModes);
            CGEventTapEnable(port, true);
            let _ = tx.send(Ok(()));
            CFRunLoopRun();
        })
        .map_err(|e| e.to_string())?;
    rx.recv_timeout(Duration::from_secs(3)).map_err(|e| e.to_string())?
}

pub fn tap_running() -> bool {
    TAP.get().is_some_and(|t| !t.port.load(std::sync::atomic::Ordering::Acquire).is_null())
}

// ---------------------------------------------------------------------------------------------
// Key injection
// ---------------------------------------------------------------------------------------------

fn keycode(name: &str) -> Option<u16> {
    const LETTERS: &str = "asdfhgzxcv\0bqweryt";
    if name.len() == 1 {
        let ch = name.chars().next()?;
        if let Some(i) = LETTERS.find(ch) {
            return Some(i as u16);
        }
    }
    Some(match name {
        "1" => 18,
        "2" => 19,
        "3" => 20,
        "4" => 21,
        "6" => 22,
        "5" => 23,
        "9" => 25,
        "7" => 26,
        "8" => 28,
        "0" => 29,
        "o" => 31,
        "u" => 32,
        "i" => 34,
        "p" => 35,
        "return" | "enter" => 36,
        "l" => 37,
        "j" => 38,
        "k" => 40,
        "n" => 45,
        "m" => 46,
        "tab" => 48,
        "space" => 49,
        "delete" => 51,
        "esc" => 53,
        "left" => 123,
        "right" => 124,
        "down" => 125,
        "up" => 126,
        _ => return None,
    })
}

fn post_key(src: *mut c_void, code: u16, down: bool, flags: u64) {
    unsafe {
        let e = CGEventCreateKeyboardEvent(src, code, down);
        CGEventSetFlags(e, flags);
        CGEventSetIntegerValueField(e, F_USERDATA, MAGIC);
        CGEventPost(HID_TAP, e);
        CFRelease(e as CFTypeRef);
    }
}

pub fn send_chord(chord: &str) -> Result<(), String> {
    let mut flags = 0u64;
    let mut key = None;
    for part in chord.split('+').map(str::trim) {
        match part {
            "cmd" | "mod" => flags |= FLAG_CMD,
            "shift" => flags |= FLAG_SHIFT,
            "alt" | "opt" => flags |= FLAG_ALT,
            "ctrl" => flags |= FLAG_CTRL,
            k => key = Some(keycode(k).ok_or_else(|| format!("未知按键: {k}"))?),
        }
    }
    let code = key.ok_or("缺少主键")?;
    if (123..=126).contains(&code) {
        flags |= FLAG_FN | FLAG_NUMPAD;
    }
    unsafe {
        let src = CGEventSourceCreate(STATE_PRIVATE);
        post_key(src, code, true, flags);
        post_key(src, code, false, flags);
        if !src.is_null() {
            CFRelease(src as CFTypeRef);
        }
    }
    Ok(())
}

pub fn type_text(text: &str) {
    let utf16: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        let src = CGEventSourceCreate(STATE_PRIVATE);
        for chunk in utf16.chunks(20) {
            for down in [true, false] {
                let e = CGEventCreateKeyboardEvent(src, 0, down);
                CGEventSetFlags(e, 0);
                CGEventKeyboardSetUnicodeString(e, chunk.len(), chunk.as_ptr());
                CGEventSetIntegerValueField(e, F_USERDATA, MAGIC);
                CGEventPost(HID_TAP, e);
                CFRelease(e as CFTypeRef);
            }
        }
        if !src.is_null() {
            CFRelease(src as CFTypeRef);
        }
    }
}

/// Waits (up to `max`) until the physical trigger modifiers are released, so injected chords are not
/// combined with a still-held ⌥. Returns how long it waited.
pub fn wait_modifiers_released(max: Duration) -> Duration {
    let start = Instant::now();
    while start.elapsed() < max {
        let f = unsafe { CGEventSourceFlagsState(STATE_HID) };
        if f & (FLAG_ALT | FLAG_CMD | FLAG_CTRL | FLAG_SHIFT) == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    start.elapsed()
}

// ---------------------------------------------------------------------------------------------
// Screens
// ---------------------------------------------------------------------------------------------

/// Screen list in global top-left coordinates. Main thread only.
pub fn screens(mtm: MainThreadMarker) -> Vec<ScreenInfo> {
    let list = NSScreen::screens(mtm);
    let Some(primary) = list.iter().next() else { return vec![] };
    let ph = primary.frame().size.height;
    let conv = |r: NSRect| Rect { x: r.origin.x, y: ph - (r.origin.y + r.size.height), w: r.size.width, h: r.size.height };
    list.iter()
        .map(|s| ScreenInfo { frame: conv(s.frame()), visible: conv(s.visibleFrame()), hz: s.maximumFramesPerSecond() as f64 })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Overlay window (main thread only)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShowStrategy {
    /// orderOut when hidden, orderFrontRegardless when shown.
    OrderOut,
    /// Keep the window ordered in with alpha 0, so WebKit never treats it as hidden.
    Alpha,
}

struct Overlay {
    window: Retained<NSWindow>,
    effect: Retained<NSVisualEffectView>,
    mask_n: usize,
    shown_seq: u64,
    strategy: ShowStrategy,
    primary_h: f64,
}

thread_local! {
    static OVERLAY: RefCell<Option<Overlay>> = const { RefCell::new(None) };
}

pub fn material_from_name(name: &str) -> NSVisualEffectMaterial {
    match name {
        "popover" => NSVisualEffectMaterial::Popover,
        "menu" => NSVisualEffectMaterial::Menu,
        "sidebar" => NSVisualEffectMaterial::Sidebar,
        "fullscreen-ui" => NSVisualEffectMaterial::FullScreenUI,
        "under-window" => NSVisualEffectMaterial::UnderWindowBackground,
        _ => NSVisualEffectMaterial::HUDWindow,
    }
}

fn mask_image(n: usize) -> Retained<NSImage> {
    let polys: Vec<Vec<Pt>> = (0..n).map(|i| wheel::sector_polygon(i, n, 48)).collect();
    let handler = RcBlock::new(move |_r: NSRect| -> Bool {
        let path = NSBezierPath::bezierPath();
        for poly in &polys {
            for (k, p) in poly.iter().enumerate() {
                let pt = NSPoint::new(p.x, p.y);
                if k == 0 {
                    path.moveToPoint(pt);
                } else {
                    path.lineToPoint(pt);
                }
            }
            path.closePath();
        }
        let c = GEOM.win / 2.0;
        let r = GEOM.r_center;
        path.appendBezierPath(&NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
            NSPoint::new(c - r, c - r),
            NSSize::new(2.0 * r, 2.0 * r),
        )));
        NSColor::blackColor().setFill();
        path.fill();
        Bool::YES
    });
    NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(GEOM.win, GEOM.win), true, &handler)
}

/// Configures the Tauri overlay window as a non-activating, click-through, all-spaces popup with a
/// wheel-shaped NSVisualEffectView behind the (transparent) webview.
pub fn setup_overlay(mtm: MainThreadMarker, ns_window: *mut c_void, material: &str, strategy: ShowStrategy) -> Result<(), String> {
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
    effect.setMaterial(material_from_name(material));
    effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    effect.setState(NSVisualEffectState::Active);
    if let Some(dark) = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }) {
        effect.setAppearance(Some(&dark));
    }
    effect.setMaskImage(Some(&mask_image(8)));
    content.addSubview_positioned_relativeTo(&effect, NSWindowOrderingMode::Below, None);

    let primary_h = NSScreen::screens(mtm).iter().next().map(|s| s.frame().size.height).unwrap_or(900.0);
    if strategy == ShowStrategy::Alpha {
        window.setAlphaValue(0.0);
        window.orderFrontRegardless();
    } else {
        window.orderOut(None);
    }
    OVERLAY.with(|o| {
        *o.borrow_mut() = Some(Overlay { window, effect, mask_n: 8, shown_seq: 0, strategy, primary_h })
    });
    Ok(())
}

/// Animates the whole window's alpha (glass + already-rendered webview) on the main thread.
/// The webview content is never on the critical path: it is pre-rendered while the window is invisible.
fn animate_window_alpha(window: &NSWindow, to: f64, secs: f64, ease_out: bool) {
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

pub fn overlay_show(mtm: MainThreadMarker, seq: u64, frame: Frame, n: usize) {
    OVERLAY.with(|o| {
        let mut o = o.borrow_mut();
        let Some(ov) = o.as_mut() else { return };
        ov.primary_h = NSScreen::screens(mtm).iter().next().map(|s| s.frame().size.height).unwrap_or(ov.primary_h);
        if ov.mask_n != n {
            ov.effect.setMaskImage(Some(&mask_image(n)));
            ov.mask_n = n;
        }
        let rect = NSRect::new(
            NSPoint::new(frame.x, ov.primary_h - (frame.y + frame.h)),
            NSSize::new(frame.w, frame.h),
        );
        ov.window.setFrame_display(rect, false);
        ov.window.setAlphaValue(0.0);
        if ov.strategy == ShowStrategy::OrderOut || !ov.window.isVisible() {
            ov.window.orderFrontRegardless();
        }
        animate_window_alpha(&ov.window, 1.0, 0.11, true);
        ov.shown_seq = seq;
    });
}

pub fn overlay_set_shape(n: usize) {
    OVERLAY.with(|o| {
        if let Some(ov) = o.borrow_mut().as_mut() {
            if ov.mask_n != n {
                ov.effect.setMaskImage(Some(&mask_image(n)));
                ov.mask_n = n;
            }
        }
    });
}

/// Starts the window fade-out immediately; `overlay_hide_finish` takes it off screen afterwards.
pub fn overlay_hide_begin(seq: u64, fade: f64) {
    OVERLAY.with(|o| {
        if let Some(ov) = o.borrow().as_ref().filter(|ov| ov.shown_seq == seq) {
            animate_window_alpha(&ov.window, 0.0, fade, false);
        }
    });
}

pub fn overlay_hide_finish(seq: u64) {
    OVERLAY.with(|o| {
        if let Some(ov) = o.borrow().as_ref().filter(|ov| ov.shown_seq == seq) {
            match ov.strategy {
                ShowStrategy::OrderOut => ov.window.orderOut(None),
                ShowStrategy::Alpha => ov.window.setAlphaValue(0.0),
            }
        }
    });
}

pub fn overlay_set_material(name: &str) {
    OVERLAY.with(|o| {
        if let Some(ov) = o.borrow().as_ref() {
            ov.effect.setMaterial(material_from_name(name));
        }
    });
}

pub fn overlay_set_strategy(strategy: ShowStrategy) {
    OVERLAY.with(|o| {
        if let Some(ov) = o.borrow_mut().as_mut() {
            ov.strategy = strategy;
            match strategy {
                ShowStrategy::Alpha => {
                    ov.window.setAlphaValue(0.0);
                    ov.window.orderFrontRegardless();
                }
                ShowStrategy::OrderOut => ov.window.orderOut(None),
            }
        }
    });
}

/// WebKit marks the page hidden (no rAF, no rendering) whenever AppKit reports the window occluded, which
/// includes an alpha-0 window. Turning off occlusion detection keeps the pre-rendered wheel live while the
/// window is invisible, so revealing it needs no web-process round trip. Private SPI, checked at runtime.
pub fn disable_occlusion_detection(wkwebview: *mut c_void) -> bool {
    unsafe {
        let wk = wkwebview as *mut AnyObject;
        if wk.is_null() {
            return false;
        }
        let ok: bool = msg_send![wk, respondsToSelector: sel!(_setWindowOcclusionDetectionEnabled:)];
        if ok {
            let _: () = msg_send![wk, _setWindowOcclusionDetectionEnabled: false];
        }
        ok
    }
}

/// Tries to lift WebKit's "prefer ~60fps page rendering" cap on ProMotion displays via private SPI.
/// Returns whether the SPI exists. Must run on the main thread.
pub fn unlock_high_refresh(wkwebview: *mut c_void) -> bool {
    unsafe {
        let wk = wkwebview as *mut AnyObject;
        if wk.is_null() {
            return false;
        }
        let cfg: *mut AnyObject = msg_send![wk, configuration];
        let prefs: *mut AnyObject = msg_send![cfg, preferences];
        let ok: bool = msg_send![prefs, respondsToSelector: sel!(_setPreferPageRenderingUpdatesNear60FPSEnabled:)];
        if ok {
            let _: () = msg_send![prefs, _setPreferPageRenderingUpdatesNear60FPSEnabled: false];
        }
        ok
    }
}

// ---------------------------------------------------------------------------------------------
// Context probes
// ---------------------------------------------------------------------------------------------

pub fn frontmost_app() -> Option<AppInfo> {
    autoreleasepool(|_| {
        let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        Some(AppInfo {
            bundle: app.bundleIdentifier().map(|s| s.to_string()).unwrap_or_default(),
            name: app.localizedName().map(|s| s.to_string()).unwrap_or_default(),
            pid: app.processIdentifier(),
        })
    })
}

const FINDER_SCRIPT: &str = r#"
tell application "Finder"
  set out to ""
  try
    set out to POSIX path of (insertion location as alias)
  end try
  try
    set sel to selection as alias list
    repeat with f in sel
      set out to out & linefeed & POSIX path of f
    end repeat
  end try
  return out
end tell
"#;

/// Owns a compiled NSAppleScript. Lives on one dedicated thread (NSAppleScript is not thread-safe).
pub struct FinderProbe {
    script: Retained<NSAppleScript>,
}

impl FinderProbe {
    pub fn new() -> Result<Self, String> {
        let script = NSAppleScript::initWithSource(NSAppleScript::alloc(), &NSString::from_str(FINDER_SCRIPT))
            .ok_or("NSAppleScript init failed")?;
        let mut err: Option<Retained<NSDictionary<NSString, AnyObject>>> = None;
        if !unsafe { script.compileAndReturnError(Some(&mut err)) } {
            return Err(format!("compile: {err:?}"));
        }
        Ok(Self { script })
    }

    pub fn run(&self) -> Result<FileCtx, String> {
        autoreleasepool(|_| {
            let mut err: Option<Retained<NSDictionary<NSString, AnyObject>>> = None;
            let res: Option<Retained<NSAppleEventDescriptor>> =
                unsafe { msg_send![&*self.script, executeAndReturnError: Some(&mut err)] };
            let Some(res) = res else {
                let msg = err.map(|d| format!("{d:?}")).unwrap_or_else(|| "unknown error".into());
                return Err(msg.split_whitespace().collect::<Vec<_>>().join(" "));
            };
            let text = res.stringValue().map(|s| s.to_string()).unwrap_or_default();
            let mut lines = text.split(['\n', '\r']);
            let dir = lines.next().filter(|s| !s.is_empty()).map(str::to_string);
            let files = lines.filter(|s| !s.is_empty()).map(str::to_string).collect();
            Ok(FileCtx { dir, files })
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Misc actions
// ---------------------------------------------------------------------------------------------

/// Current cursor position in global top-left coordinates. Any thread.
pub fn cursor_location() -> Pt {
    extern "C" {
        fn CGEventCreate(src: *mut c_void) -> CGEventRef;
    }
    unsafe {
        let e = CGEventCreate(std::ptr::null_mut());
        let p = CGEventGetLocation(e);
        CFRelease(e as CFTypeRef);
        Pt { x: p.x, y: p.y }
    }
}

pub fn app_is_active(mtm: MainThreadMarker) -> bool {
    objc2_app_kit::NSApplication::sharedApplication(mtm).isActive()
}

pub fn set_clipboard_text(text: &str) {
    autoreleasepool(|_| {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        pb.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
    });
}
