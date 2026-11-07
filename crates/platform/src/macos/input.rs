//! Global input hook (CGEventTap) and key injection.

use super::ffi::*;
use anytools_core::context::Pt;
use anytools_core::keys::{Chord, Mods};
use anytools_core::platform::{Button, InputEvent, InputFilter, InputHook, Keyboard};
use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

// ------------------------------------------------------------------------------------------------
// Key codes (US ANSI virtual key codes; positions are layout-independent)
// ------------------------------------------------------------------------------------------------

const KEYS: &[(&str, u16)] = &[
    ("a", 0), ("s", 1), ("d", 2), ("f", 3), ("h", 4), ("g", 5), ("z", 6), ("x", 7), ("c", 8), ("v", 9),
    ("b", 11), ("q", 12), ("w", 13), ("e", 14), ("r", 15), ("y", 16), ("t", 17), ("1", 18), ("2", 19),
    ("3", 20), ("4", 21), ("6", 22), ("5", 23), ("=", 24), ("9", 25), ("7", 26), ("-", 27), ("8", 28),
    ("0", 29), ("]", 30), ("o", 31), ("u", 32), ("[", 33), ("i", 34), ("p", 35), ("enter", 36), ("l", 37),
    ("j", 38), ("'", 39), ("k", 40), (";", 41), ("\\", 42), (",", 43), ("/", 44), ("n", 45), ("m", 46),
    (".", 47), ("tab", 48), ("space", 49), ("`", 50), ("delete", 51), ("esc", 53), ("capslock", 57),
    ("f5", 96), ("f6", 97), ("f7", 98), ("f3", 99), ("f8", 100), ("f9", 101), ("f11", 103), ("f13", 105),
    ("f14", 107), ("f10", 109), ("f12", 111), ("f15", 113), ("home", 115), ("pageup", 116),
    ("forwarddelete", 117), ("f4", 118), ("end", 119), ("f2", 120), ("pagedown", 121), ("f1", 122),
    ("left", 123), ("right", 124), ("down", 125), ("up", 126),
];

pub fn keycode(name: &str) -> Option<u16> {
    KEYS.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

fn mods_from_flags(f: u64) -> Mods {
    let mut m = Mods::NONE;
    if f & FLAG_CMD != 0 {
        m |= Mods::CMD;
    }
    if f & FLAG_CTRL != 0 {
        m |= Mods::CTRL;
    }
    if f & FLAG_ALT != 0 {
        m |= Mods::ALT;
    }
    if f & FLAG_SHIFT != 0 {
        m |= Mods::SHIFT;
    }
    m
}

fn flags_from_mods(m: Mods) -> u64 {
    let mut f = 0;
    if m.contains(Mods::CMD) {
        f |= FLAG_CMD;
    }
    if m.contains(Mods::CTRL) {
        f |= FLAG_CTRL;
    }
    if m.contains(Mods::ALT) {
        f |= FLAG_ALT;
    }
    if m.contains(Mods::SHIFT) {
        f |= FLAG_SHIFT;
    }
    f
}

// ------------------------------------------------------------------------------------------------
// Event tap
// ------------------------------------------------------------------------------------------------

struct Tap {
    filter: RwLock<Option<Arc<dyn InputFilter>>>,
    port: AtomicPtr<c_void>,
}

static TAP: OnceLock<Tap> = OnceLock::new();

fn tap() -> &'static Tap {
    TAP.get_or_init(|| Tap { filter: RwLock::new(None), port: AtomicPtr::new(std::ptr::null_mut()) })
}

extern "C" fn tap_callback(_proxy: *mut c_void, ty: u32, e: CGEventRef, _info: *mut c_void) -> CGEventRef {
    let now = Instant::now();
    let t = tap();
    if ty == EV_TAP_DISABLED_TIMEOUT || ty == EV_TAP_DISABLED_USER {
        let port = t.port.load(Ordering::Acquire);
        if !port.is_null() {
            unsafe { CGEventTapEnable(port, true) };
        }
        tracing::warn!(reason = if ty == EV_TAP_DISABLED_TIMEOUT { "timeout" } else { "user-input" }, "event tap re-enabled");
        return e;
    }
    let Some(filter) = t.filter.read().unwrap().clone() else { return e };
    unsafe {
        if CGEventGetIntegerValueField(e, F_USERDATA) == MAGIC {
            return e;
        }
        let at = || {
            let p = CGEventGetLocation(e);
            Pt { x: p.x, y: p.y }
        };
        let mods = mods_from_flags(CGEventGetFlags(e));
        let other_button = || match CGEventGetIntegerValueField(e, F_BUTTON_NUMBER) {
            2 => Button::Middle,
            3 => Button::Back,
            4 => Button::Forward,
            n => Button::Other(n.clamp(0, 255) as u8),
        };
        let ev = match ty {
            EV_KEY_DOWN => InputEvent::KeyDown {
                code: CGEventGetIntegerValueField(e, F_KEYCODE) as u32,
                mods,
                repeat: CGEventGetIntegerValueField(e, F_AUTOREPEAT) != 0,
                at: at(),
                ch: typed_char(e),
            },
            EV_KEY_UP => InputEvent::KeyUp { code: CGEventGetIntegerValueField(e, F_KEYCODE) as u32, mods },
            EV_FLAGS => InputEvent::Flags { mods },
            EV_MOVED | EV_LEFT_DRAG | EV_RIGHT_DRAG | EV_OTHER_DRAG => InputEvent::Move { at: at() },
            EV_LEFT_DOWN => InputEvent::Down { button: Button::Left, at: at() },
            EV_LEFT_UP => InputEvent::Up { button: Button::Left },
            EV_RIGHT_DOWN => InputEvent::Down { button: Button::Right, at: at() },
            EV_RIGHT_UP => InputEvent::Up { button: Button::Right },
            EV_OTHER_DOWN => InputEvent::Down { button: other_button(), at: at() },
            EV_OTHER_UP => InputEvent::Up { button: other_button() },
            EV_SCROLL => {
                let precise = CGEventGetIntegerValueField(e, F_SCROLL_CONTINUOUS) != 0;
                let dy = if precise { CGEventGetIntegerValueField(e, F_SCROLL_POINT_DELTA_Y) } else { CGEventGetIntegerValueField(e, F_SCROLL_DELTA_Y) };
                if dy == 0 {
                    return e;
                }
                InputEvent::Scroll { dy: dy as f64, precise, at: at() }
            }
            _ => return e,
        };
        if filter.handle(ev, now) {
            std::ptr::null_mut()
        } else {
            e
        }
    }
}

/// The printable character a key event types (layout-aware, no input method), if any.
unsafe fn typed_char(e: CGEventRef) -> Option<char> {
    let mut buf = [0u16; 4];
    let mut n = 0usize;
    CGEventKeyboardGetUnicodeString(e, buf.len(), &mut n, buf.as_mut_ptr());
    let mut it = char::decode_utf16(buf[..n.min(buf.len())].iter().copied());
    match (it.next(), it.next()) {
        (Some(Ok(c)), None) if !c.is_control() && !('\u{F700}'..='\u{F8FF}').contains(&c) => Some(c),
        _ => None,
    }
}

pub struct MacInput;

impl InputHook for MacInput {
    fn start(&self, filter: Arc<dyn InputFilter>) -> Result<(), String> {
        let t = tap();
        *t.filter.write().unwrap() = Some(filter);
        if self.running() {
            return Ok(());
        }
        if !unsafe { AXIsProcessTrusted() } {
            return Err("尚未授予辅助功能权限".into());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("event-tap".into())
            .spawn(move || unsafe {
                let mask = [
                    EV_LEFT_DOWN, EV_LEFT_UP, EV_RIGHT_DOWN, EV_RIGHT_UP, EV_MOVED, EV_LEFT_DRAG, EV_RIGHT_DRAG,
                    EV_KEY_DOWN, EV_KEY_UP, EV_FLAGS, EV_OTHER_DOWN, EV_OTHER_UP, EV_OTHER_DRAG, EV_SCROLL,
                ]
                .iter()
                .fold(0u64, |m, t| m | (1u64 << t));
                let port = CGEventTapCreate(SESSION_TAP, 0, 0, mask, tap_callback, std::ptr::null_mut());
                if port.is_null() {
                    let _ = tx.send(Err("无法创建全局事件钩子（辅助功能权限可能未生效）".to_string()));
                    return;
                }
                tap().port.store(port, Ordering::Release);
                let src = CFMachPortCreateRunLoopSource(std::ptr::null(), port, 0);
                CFRunLoopAddSource(CFRunLoopGetCurrent(), src, kCFRunLoopCommonModes);
                CGEventTapEnable(port, true);
                let _ = tx.send(Ok(()));
                CFRunLoopRun();
            })
            .map_err(|e| e.to_string())?;
        rx.recv_timeout(Duration::from_secs(3)).map_err(|e| e.to_string())?
    }

    fn running(&self) -> bool {
        !tap().port.load(Ordering::Acquire).is_null()
    }
}

/// While a password field has focus macOS hides keystrokes from all event taps.
pub fn secure_input_enabled() -> bool {
    unsafe { IsSecureEventInputEnabled() }
}

// ------------------------------------------------------------------------------------------------
// Injection
// ------------------------------------------------------------------------------------------------

struct Src(*mut c_void);

impl Src {
    fn new() -> Src {
        Src(unsafe { CGEventSourceCreate(STATE_PRIVATE) })
    }
}

impl Drop for Src {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0 as CFTypeRef) }
        }
    }
}

fn post_key(src: &Src, code: u16, down: bool, flags: u64) {
    unsafe {
        let e = CGEventCreateKeyboardEvent(src.0, code, down);
        if e.is_null() {
            return;
        }
        CGEventSetFlags(e, flags);
        CGEventSetIntegerValueField(e, F_USERDATA, MAGIC);
        CGEventPost(HID_TAP, e);
        CFRelease(e as CFTypeRef);
    }
}

pub struct MacKeyboard;

impl Keyboard for MacKeyboard {
    fn code_of(&self, name: &str) -> Option<u32> {
        keycode(name).map(u32::from)
    }

    fn send_chord(&self, chord: &Chord) -> Result<(), String> {
        let code = keycode(&chord.key).ok_or_else(|| format!("不支持的按键: {}", chord.key))?;
        let mut flags = flags_from_mods(chord.mods);
        if (123..=126).contains(&code) {
            flags |= FLAG_FN | FLAG_NUMPAD;
        } else if (96..=122).contains(&code) || matches!(code, 115..=121) {
            flags |= FLAG_FN;
        }
        let src = Src::new();
        post_key(&src, code, true, flags);
        post_key(&src, code, false, flags);
        Ok(())
    }

    fn type_text(&self, text: &str) -> Result<(), String> {
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let src = Src::new();
        // CGEventKeyboardSetUnicodeString silently truncates long strings; 20 UTF-16 units is safe.
        let mut i = 0;
        while i < utf16.len() {
            let mut end = (i + 20).min(utf16.len());
            // Don't split a surrogate pair.
            if end < utf16.len() && (0xD800..0xDC00).contains(&utf16[end - 1]) {
                end -= 1;
            }
            let chunk = &utf16[i..end];
            for down in [true, false] {
                unsafe {
                    let e = CGEventCreateKeyboardEvent(src.0, 0, down);
                    if e.is_null() {
                        return Err("无法创建键盘事件".into());
                    }
                    CGEventSetFlags(e, 0);
                    CGEventKeyboardSetUnicodeString(e, chunk.len(), chunk.as_ptr());
                    CGEventSetIntegerValueField(e, F_USERDATA, MAGIC);
                    CGEventPost(HID_TAP, e);
                    CFRelease(e as CFTypeRef);
                }
            }
            i = end;
        }
        Ok(())
    }

    fn wait_modifiers_released(&self, max: Duration) -> Duration {
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
}

pub fn cursor_location() -> Pt {
    unsafe {
        let e = CGEventCreate(std::ptr::null_mut());
        if e.is_null() {
            return Pt::default();
        }
        let p = CGEventGetLocation(e);
        CFRelease(e as CFTypeRef);
        Pt { x: p.x, y: p.y }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_table_has_no_duplicates() {
        let mut codes: Vec<u16> = KEYS.iter().map(|(_, c)| *c).collect();
        codes.sort();
        let n = codes.len();
        codes.dedup();
        assert_eq!(n, codes.len());
        assert_eq!(keycode("space"), Some(49));
        assert_eq!(keycode("c"), Some(8));
    }

    #[test]
    fn flags_roundtrip() {
        let m = Mods::CMD | Mods::SHIFT;
        assert_eq!(mods_from_flags(flags_from_mods(m)), m);
        assert_eq!(mods_from_flags(FLAG_ALT | 0x100 /* device bits */), Mods::ALT);
    }
}
