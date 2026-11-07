//! Raw C APIs not covered by objc2: CoreGraphics events, CoreFoundation, Accessibility, Apple Events.
#![allow(non_upper_case_globals, dead_code, clippy::upper_case_acronyms)]

use std::ffi::c_void;

pub type CGEventRef = *mut c_void;
pub type CFTypeRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type AXUIElementRef = *const c_void;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

pub type TapCallback = extern "C" fn(*mut c_void, u32, CGEventRef, *mut c_void) -> CGEventRef;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    pub fn CGEventTapCreate(tap: u32, place: u32, options: u32, mask: u64, cb: TapCallback, info: *mut c_void) -> *mut c_void;
    pub fn CGEventTapEnable(tap: *mut c_void, enable: bool);
    pub fn CGEventCreate(src: *mut c_void) -> CGEventRef;
    pub fn CGEventGetLocation(e: CGEventRef) -> CGPoint;
    pub fn CGEventGetIntegerValueField(e: CGEventRef, field: u32) -> i64;
    pub fn CGEventSetIntegerValueField(e: CGEventRef, field: u32, v: i64);
    pub fn CGEventGetFlags(e: CGEventRef) -> u64;
    pub fn CGEventSetFlags(e: CGEventRef, flags: u64);
    pub fn CGEventCreateKeyboardEvent(src: *mut c_void, key: u16, down: bool) -> CGEventRef;
    pub fn CGEventKeyboardSetUnicodeString(e: CGEventRef, len: usize, s: *const u16);
    pub fn CGEventPost(tap: u32, e: CGEventRef);
    pub fn CGEventSourceCreate(state: i32) -> *mut c_void;
    pub fn CGEventSourceFlagsState(state: i32) -> u64;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub fn CFMachPortCreateRunLoopSource(alloc: CFTypeRef, port: *mut c_void, order: isize) -> *mut c_void;
    pub fn CFRunLoopGetCurrent() -> *mut c_void;
    pub fn CFRunLoopAddSource(rl: *mut c_void, src: *mut c_void, mode: CFTypeRef);
    pub fn CFRunLoopRun();
    pub fn CFRelease(p: CFTypeRef);
    pub fn CFGetTypeID(p: CFTypeRef) -> usize;
    pub fn CFStringGetTypeID() -> usize;
    pub fn CFStringCreateWithBytes(alloc: CFTypeRef, bytes: *const u8, len: isize, enc: u32, ext: bool) -> CFStringRef;
    pub fn CFStringGetLength(s: CFStringRef) -> isize;
    pub fn CFStringGetCString(s: CFStringRef, buf: *mut u8, size: isize, enc: u32) -> bool;
    pub fn CFDictionaryCreate(
        alloc: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        n: isize,
        kcb: *const c_void,
        vcb: *const c_void,
    ) -> CFTypeRef;
    pub static kCFRunLoopCommonModes: CFTypeRef;
    pub static kCFBooleanTrue: CFTypeRef;
    pub static kCFTypeDictionaryKeyCallBacks: c_void;
    pub static kCFTypeDictionaryValueCallBacks: c_void;
}

pub const kCFStringEncodingUTF8: u32 = 0x0800_0100;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    pub fn AXIsProcessTrusted() -> bool;
    pub fn AXIsProcessTrustedWithOptions(options: CFTypeRef) -> bool;
    pub static kAXTrustedCheckOptionPrompt: CFTypeRef;
    pub fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    pub fn AXUIElementCopyAttributeValue(el: AXUIElementRef, attr: CFStringRef, value: *mut CFTypeRef) -> i32;
    pub fn AXUIElementSetMessagingTimeout(el: AXUIElementRef, seconds: f32) -> i32;
}

#[repr(C)]
pub struct AEDesc {
    pub descriptor_type: u32,
    pub data_handle: *mut c_void,
}

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    pub fn AECreateDesc(ty: u32, data: *const c_void, size: isize, out: *mut AEDesc) -> i16;
    pub fn AEDisposeDesc(d: *mut AEDesc) -> i16;
    pub fn AEDeterminePermissionToAutomateTarget(target: *const AEDesc, class: u32, id: u32, ask: bool) -> i32;
}

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    pub fn IsSecureEventInputEnabled() -> bool;
}

pub const typeApplicationBundleID: u32 = u32::from_be_bytes(*b"bund");
pub const typeWildCard: u32 = u32::from_be_bytes(*b"****");
pub const ERR_AE_NOT_PERMITTED: i32 = -1743;
pub const ERR_AE_NEEDS_CONSENT: i32 = -1744;
pub const PROC_NOT_FOUND: i32 = -600;

// CGEventType
pub const EV_LEFT_DOWN: u32 = 1;
pub const EV_LEFT_UP: u32 = 2;
pub const EV_RIGHT_DOWN: u32 = 3;
pub const EV_RIGHT_UP: u32 = 4;
pub const EV_MOVED: u32 = 5;
pub const EV_LEFT_DRAG: u32 = 6;
pub const EV_RIGHT_DRAG: u32 = 7;
pub const EV_KEY_DOWN: u32 = 10;
pub const EV_KEY_UP: u32 = 11;
pub const EV_FLAGS: u32 = 12;
pub const EV_OTHER_DOWN: u32 = 25;
pub const EV_OTHER_UP: u32 = 26;
pub const EV_OTHER_DRAG: u32 = 27;
pub const EV_TAP_DISABLED_TIMEOUT: u32 = 0xFFFF_FFFE;
pub const EV_TAP_DISABLED_USER: u32 = 0xFFFF_FFFF;

// CGEventField
pub const F_BUTTON_NUMBER: u32 = 3;
pub const F_AUTOREPEAT: u32 = 8;
pub const F_KEYCODE: u32 = 9;
pub const F_USERDATA: u32 = 42;

// CGEventFlags
pub const FLAG_SHIFT: u64 = 0x0002_0000;
pub const FLAG_CTRL: u64 = 0x0004_0000;
pub const FLAG_ALT: u64 = 0x0008_0000;
pub const FLAG_CMD: u64 = 0x0010_0000;
pub const FLAG_NUMPAD: u64 = 0x0020_0000;
pub const FLAG_FN: u64 = 0x0080_0000;

pub const HID_TAP: u32 = 0;
pub const SESSION_TAP: u32 = 1;
pub const STATE_PRIVATE: i32 = -1;
pub const STATE_HID: i32 = 1;

/// Marks events we inject so our own tap ignores them.
pub const MAGIC: i64 = 0x616E_7954;

/// Owned CF object, released on drop.
pub struct Cf(pub CFTypeRef);

impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}

pub fn cfstr(s: &str) -> Cf {
    Cf(unsafe { CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as isize, kCFStringEncodingUTF8, false) })
}

pub fn cfstring_to_string(s: CFTypeRef) -> Option<String> {
    unsafe {
        if s.is_null() || CFGetTypeID(s) != CFStringGetTypeID() {
            return None;
        }
        let cap = CFStringGetLength(s) * 4 + 1;
        let mut buf = vec![0u8; cap as usize];
        if !CFStringGetCString(s, buf.as_mut_ptr(), cap, kCFStringEncodingUTF8) {
            return None;
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        buf.truncate(end);
        String::from_utf8(buf).ok()
    }
}
