//! Global input hook (low-level keyboard and mouse hooks) and key injection.
//!
//! Both hooks run on one thread with its own message loop. The callbacks only look the event up and
//! hand it to the filter, which has to stay well under a millisecond: Windows silently removes a
//! low-level hook whose callback takes too long.

use super::util::cursor;
use anytools_core::context::Pt;
use anytools_core::keys::{Chord, Mods};
use anytools_core::platform::{Button, InputEvent, InputFilter, InputHook, Keyboard};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, MapVirtualKeyW, SendInput, ToUnicode, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, VIRTUAL_KEY, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_HOME, VK_INSERT, VK_LEFT, VK_LWIN, VK_MENU, VK_NEXT, VK_OEM_1, VK_OEM_2, VK_OEM_3, VK_OEM_4, VK_OEM_5, VK_OEM_6, VK_OEM_7, VK_OEM_COMMA, VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SNAPSHOT, VK_SPACE, VK_TAB, VK_UP};
use windows_sys::Win32::UI::WindowsAndMessaging::{CallNextHookEx, GetMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLMHF_INJECTED, MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_XBUTTONDOWN, WM_XBUTTONUP, XBUTTON1, XBUTTON2};

/// Written into `dwExtraInfo` of every event we inject, so the hook can tell them from real input and
/// never swallow its own replay.
const MAGIC: usize = 0xA790;

/// Posted to the hook thread to make it leave its message loop.
const WM_STOP: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 1;

// -------------------------------------------------------------------------------------------------
// Key names. Codes are virtual-key codes, which (unlike scan codes) don't move with the layout, so a
// configured "c" stays the physical C key on an AZERTY keyboard too.
// -------------------------------------------------------------------------------------------------

fn key_table() -> &'static [(&'static str, u32)] {
    &[("space", VK_SPACE as u32), ("enter", VK_RETURN as u32), ("tab", VK_TAB as u32), ("esc", VK_ESCAPE as u32), ("delete", VK_BACK as u32), ("forwarddelete", VK_DELETE as u32), ("up", VK_UP as u32), ("down", VK_DOWN as u32), ("left", VK_LEFT as u32), ("right", VK_RIGHT as u32), ("home", VK_HOME as u32), ("end", VK_END as u32), ("pageup", VK_PRIOR as u32), ("pagedown", VK_NEXT as u32), ("capslock", VK_CAPITAL as u32), ("insert", VK_INSERT as u32), ("printscreen", VK_SNAPSHOT as u32), ("-", VK_OEM_MINUS as u32), ("=", VK_OEM_PLUS as u32), ("[", VK_OEM_4 as u32), ("]", VK_OEM_6 as u32), ("\\", VK_OEM_5 as u32), (";", VK_OEM_1 as u32), ("'", VK_OEM_7 as u32), (",", VK_OEM_COMMA as u32), (".", VK_OEM_PERIOD as u32), ("/", VK_OEM_2 as u32), ("`", VK_OEM_3 as u32)]
}

pub fn keycode(name: &str) -> Option<u32> {
    if name.len() == 1 {
        let c = name.as_bytes()[0];
        if c.is_ascii_alphabetic() {
            // 'A' is 0x41, which is VK_A; the letters are contiguous.
            return Some(c.to_ascii_uppercase() as u32);
        }
        if c.is_ascii_digit() {
            return Some(c as u32);
        }
    }
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()).filter(|n| (1..=24).contains(n)) {
        return Some(VK_F1 as u32 + n - 1);
    }
    key_table().iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

/// Keys whose scan code Windows only honours with `KEYEVENTF_EXTENDEDKEY` (the grey cursor keys, not
/// the numpad copies).
fn is_extended(vk: u32) -> bool {
    matches!(vk, x if x == VK_INSERT as u32 || x == VK_DELETE as u32 || x == VK_HOME as u32 || x == VK_END as u32 || x == VK_PRIOR as u32 || x == VK_NEXT as u32 || x == VK_LEFT as u32 || x == VK_RIGHT as u32 || x == VK_UP as u32 || x == VK_DOWN as u32 || x == VK_LWIN as u32 || x == VK_SNAPSHOT as u32 || x == vk_rmenu())
}

fn vk_rmenu() -> u32 {
    windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_RMENU as u32
}

fn mods_now() -> Mods {
    let down = |vk: VIRTUAL_KEY| unsafe { GetAsyncKeyState(vk as i32) } < 0;
    let mut m = Mods::NONE;
    if down(VK_LWIN) || down(windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_RWIN) {
        m |= Mods::CMD;
    }
    if down(VK_CONTROL) || down(windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_LCONTROL) || down(windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_RCONTROL) {
        m |= Mods::CTRL;
    }
    if down(VK_MENU) || down(windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_LMENU) || down(windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_RMENU) {
        m |= Mods::ALT;
    }
    if down(VK_SHIFT) || down(windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_LSHIFT) || down(windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_RSHIFT) {
        m |= Mods::SHIFT;
    }
    m
}

/// The character `vk` types with the modifiers currently held, before any input method. `None` for keys
/// that type nothing printable. Called from the hook callback, so it must not allocate much or block.
fn typed_char(vk: u32, scan: u32) -> Option<char> {
    let mut state = [0u8; 256];
    for vk in [VK_SHIFT, VK_CONTROL, VK_MENU, windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_LSHIFT, windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_RSHIFT, windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_LCONTROL, windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_RCONTROL, windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_LMENU, windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_RMENU] {
        if unsafe { GetAsyncKeyState(vk as i32) } < 0 {
            state[vk as usize] = 0x80;
        }
    }
    let mut buf = [0u16; 8];
    let n = unsafe { ToUnicode(vk, scan, state.as_ptr(), buf.as_mut_ptr(), buf.len() as i32, 0) };
    if n == -1 {
        // A dead key: the call consumed it, so type it again to put the keyboard state back.
        unsafe { ToUnicode(vk, scan, state.as_ptr(), buf.as_mut_ptr(), buf.len() as i32, 0) };
        return None;
    }
    if n != 1 {
        return None;
    }
    char::from_u32(buf[0] as u32).filter(|c| !c.is_control())
}

// -------------------------------------------------------------------------------------------------
// The hooks
// -------------------------------------------------------------------------------------------------

struct Hooks {
    filter: RwLock<Option<std::sync::Arc<dyn InputFilter>>>,
    keyboard: AtomicPtr<std::ffi::c_void>,
    mouse: AtomicPtr<std::ffi::c_void>,
    thread: AtomicU32,
    /// The previous key-down, to tell an auto-repeat from a fresh press (the hook doesn't say).
    last_down: AtomicU32,
    /// The modifier set last reported as a `Flags` event, so a chord commits when its modifier is
    /// released before its key (the low-level hook has no flags-changed message of its own).
    last_mods: AtomicU32,
}

fn ptr(p: &AtomicPtr<std::ffi::c_void>) -> HHOOK {
    p.load(Ordering::Acquire) as HHOOK
}

static HOOKS: OnceLock<Hooks> = OnceLock::new();

fn hooks() -> &'static Hooks {
    HOOKS.get_or_init(|| Hooks {
        filter: RwLock::new(None),
        keyboard: AtomicPtr::new(std::ptr::null_mut()),
        mouse: AtomicPtr::new(std::ptr::null_mut()),
        thread: AtomicU32::new(0),
        last_down: AtomicU32::new(u32::MAX),
        last_mods: AtomicU32::new(u32::MAX),
    })
}

fn swallow(ev: InputEvent) -> bool {
    let Some(filter) = hooks().filter.read().unwrap().clone() else { return false };
    filter.handle(ev, Instant::now())
}

/// Reports a `Flags` event whenever the held modifiers differ from the previous report. A modifier
/// released before its key never arrives as its own key event the controller can see, because the
/// controller only watches the trigger key.
fn flags_changed() {
    let mods = mods_now();
    if hooks().last_mods.swap(mods.0 as u32, Ordering::Relaxed) != mods.0 as u32 {
        swallow(InputEvent::Flags { mods });
    }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        if info.dwExtraInfo != MAGIC && info.flags & LLKHF_INJECTED == 0 {
            let down = wparam as u32 == WM_KEYDOWN || wparam as u32 == WM_SYSKEYDOWN;
            let h = hooks();
            let repeat = down && h.last_down.swap(if down { info.vkCode } else { u32::MAX }, Ordering::Relaxed) == info.vkCode;
            // A modifier going up is reported as a key event first, so the controller can commit the
            // chord on it before the following `Flags` event (which always passes through).
            if !down {
                flags_changed();
            }
            let ev = if down {
                InputEvent::KeyDown { code: info.vkCode, mods: mods_now(), repeat, at: cursor(), ch: typed_char(info.vkCode, info.scanCode) }
            } else {
                InputEvent::KeyUp { code: info.vkCode, mods: mods_now() }
            };
            let eaten = swallow(ev);
            // A modifier going down is reported after its key event, so the chord's key event still
            // carries the modifiers held before it.
            if down {
                flags_changed();
            }
            if eaten {
                return 1;
            }
        }
    }
    unsafe { CallNextHookEx(ptr(&hooks().keyboard), code, wparam, lparam) }
}

fn button_of(msg: u32, data: u32) -> Option<(Button, bool)> {
    Some(match msg {
        WM_LBUTTONDOWN => (Button::Left, true),
        WM_LBUTTONUP => (Button::Left, false),
        WM_RBUTTONDOWN => (Button::Right, true),
        WM_RBUTTONUP => (Button::Right, false),
        WM_MBUTTONDOWN => (Button::Middle, true),
        WM_MBUTTONUP => (Button::Middle, false),
        WM_XBUTTONDOWN | WM_XBUTTONUP => {
            let b = match (data >> 16) as u16 {
                XBUTTON1 => Button::Back,
                XBUTTON2 => Button::Forward,
                _ => Button::Other(0),
            };
            (b, msg == WM_XBUTTONDOWN)
        }
        _ => return None,
    })
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
        if info.dwExtraInfo != MAGIC && info.flags & LLMHF_INJECTED == 0 {
            let at = Pt { x: info.pt.x as f64, y: info.pt.y as f64 };
            let msg = wparam as u32;
            let ev = if msg == WM_MOUSEMOVE {
                Some(InputEvent::Move { at })
            } else if msg == WM_MOUSEWHEEL {
                let dy = (info.mouseData >> 16) as i16 as f64 / 120.0;
                (dy != 0.0).then_some(InputEvent::Scroll { dy, precise: false, at })
            } else if let Some((button, down)) = button_of(msg, info.mouseData) {
                Some(if down { InputEvent::Down { button, at } } else { InputEvent::Up { button } })
            } else {
                None
            };
            if ev.is_some_and(swallow) {
                return 1;
            }
        }
    }
    unsafe { CallNextHookEx(ptr(&hooks().mouse), code, wparam, lparam) }
}

/// Installs both hooks and pumps messages until [`stop_hook_thread`] posts `WM_STOP`. The hooks only
/// receive events while this loop runs, so it must stay on this thread.
fn hook_thread(ready: std::sync::mpsc::Sender<Result<(), String>>) {
    unsafe {
        let module = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null());
        let kb = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0);
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0);
        if kb.is_null() || mouse.is_null() {
            if !kb.is_null() {
                UnhookWindowsHookEx(kb);
            }
            if !mouse.is_null() {
                UnhookWindowsHookEx(mouse);
            }
            let _ = ready.send(Err("无法安装全局键盘钩子".into()));
            return;
        }
        let h = hooks();
        h.keyboard.store(kb as *mut _, Ordering::Release);
        h.mouse.store(mouse as *mut _, Ordering::Release);
        h.thread.store(windows_sys::Win32::System::Threading::GetCurrentThreadId(), Ordering::Release);
        let _ = ready.send(Ok(()));

        let mut msg = windows_sys::Win32::UI::WindowsAndMessaging::MSG::default();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.message == WM_STOP {
                break;
            }
        }
        UnhookWindowsHookEx(kb);
        UnhookWindowsHookEx(mouse);
        h.keyboard.store(std::ptr::null_mut(), Ordering::Release);
        h.mouse.store(std::ptr::null_mut(), Ordering::Release);
    }
}

pub struct WinInput;

impl InputHook for WinInput {
    fn start(&self, filter: std::sync::Arc<dyn InputFilter>) -> Result<(), String> {
        *hooks().filter.write().unwrap() = Some(filter);
        if self.running() {
            return Ok(());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new().name("input-hook".into()).spawn(move || hook_thread(tx)).map_err(|e| e.to_string())?;
        rx.recv_timeout(Duration::from_secs(3)).map_err(|e| e.to_string())?
    }

    fn running(&self) -> bool {
        !hooks().keyboard.load(Ordering::Acquire).is_null()
    }
}

// -------------------------------------------------------------------------------------------------
// Injection
// -------------------------------------------------------------------------------------------------

fn key_input(vk: u32, up: bool) -> INPUT {
    let scan = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_VSC) } as u16;
    let mut flags = if up { KEYEVENTF_KEYUP } else { 0 };
    if is_extended(vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk as VIRTUAL_KEY, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: MAGIC } } }
}

fn send(inputs: &[INPUT]) -> Result<(), String> {
    let n = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
    if n as usize == inputs.len() {
        Ok(())
    } else {
        Err("发送按键失败（当前窗口可能以管理员身份运行）".into())
    }
}

fn modifier_keys(mods: Mods) -> Vec<u32> {
    let mut v = vec![];
    if mods.contains(Mods::CMD) {
        v.push(VK_LWIN as u32);
    }
    if mods.contains(Mods::CTRL) {
        v.push(VK_CONTROL as u32);
    }
    if mods.contains(Mods::ALT) {
        v.push(VK_MENU as u32);
    }
    if mods.contains(Mods::SHIFT) {
        v.push(VK_SHIFT as u32);
    }
    v
}

pub struct WinKeyboard;

impl Keyboard for WinKeyboard {
    fn code_of(&self, name: &str) -> Option<u32> {
        keycode(name)
    }

    fn send_chord(&self, chord: &Chord) -> Result<(), String> {
        let vk = keycode(&chord.key).ok_or_else(|| format!("不支持的按键: {}", chord.key))?;
        let mods = modifier_keys(chord.mods);
        let mut inputs: Vec<INPUT> = mods.iter().copied().map(|m| key_input(m, false)).collect();
        inputs.push(key_input(vk, false));
        inputs.push(key_input(vk, true));
        inputs.extend(mods.iter().rev().copied().map(|m| key_input(m, true)));
        send(&inputs)
    }

    fn type_text(&self, text: &str) -> Result<(), String> {
        let mut inputs = Vec::with_capacity(text.encode_utf16().count() * 2);
        for unit in text.encode_utf16() {
            for up in [false, true] {
                let mut flags = KEYEVENTF_UNICODE;
                if up {
                    flags |= KEYEVENTF_KEYUP;
                }
                inputs.push(INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: 0, wScan: unit, dwFlags: flags, time: 0, dwExtraInfo: MAGIC } } });
            }
        }
        send(&inputs)
    }

    fn wait_modifiers_released(&self, max: Duration) -> Duration {
        let start = Instant::now();
        while start.elapsed() < max {
            if mods_now().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        start.elapsed()
    }
}

/// Whether any physical modifier is held right now. Used to hold back injected keys until the trigger
/// key has actually gone up.
#[allow(dead_code)]
pub fn modifiers_held() -> bool {
    !mods_now().is_empty()
}

/// Stops the hook thread. Nothing calls this today (the hook lives as long as the process), but tests
/// and a future clean shutdown need a way to leave the loop.
#[allow(dead_code)]
pub fn stop() {
    let id = hooks().thread.load(Ordering::Acquire);
    if id != 0 {
        unsafe { PostThreadMessageW(id, WM_STOP, 0, 0) };
    }
}

/// Set while a password field has focus. Windows has no global equivalent of macOS secure input, so this
/// stays false; the flag exists so the settings page can share one status shape.
pub fn secure_input_enabled() -> bool {
    false
}

/// Keeps the compiler from dropping the `AtomicBool` import if a later edit stops using it.
#[allow(dead_code)]
fn _keep(b: &AtomicBool) {
    let _ = b.load(Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names_map_to_virtual_keys() {
        assert_eq!(keycode("c"), Some(b'C' as u32));
        assert_eq!(keycode("space"), Some(VK_SPACE as u32));
        assert_eq!(keycode("f5"), Some(VK_F1 as u32 + 4));
        assert_eq!(keycode("f25"), None);
        assert_eq!(keycode("capslock"), Some(VK_CAPITAL as u32));
        assert_eq!(keycode("nope"), None);
    }

    #[test]
    fn extended_keys_are_the_cursor_block() {
        assert!(is_extended(VK_LEFT as u32));
        assert!(is_extended(VK_DELETE as u32));
        assert!(!is_extended(VK_BACK as u32));
        assert!(!is_extended(b'A' as u32));
    }
}
