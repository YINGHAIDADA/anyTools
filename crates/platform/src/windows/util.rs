//! Small Windows helpers shared by the platform modules.

use anytools_core::context::Pt;
use windows_sys::Win32::Foundation::POINT;

/// Encodes `s` as a NUL-terminated UTF-16 buffer, for APIs that take `PCWSTR`.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Decodes a NUL-terminated UTF-16 buffer, dropping everything from the first NUL.
pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

pub fn cursor() -> Pt {
    let mut p = POINT { x: 0, y: 0 };
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p) };
    Pt { x: p.x as f64, y: p.y as f64 }
}

/// The last error from a Win32 call, as text.
pub fn last_error(what: &str) -> String {
    let code = std::io::Error::last_os_error();
    format!("{what}：{code}")
}

