//! Front application, screen layout and the file selection of Explorer (including the desktop).
//!
//! The selection is read by a small PowerShell script over the Shell COM objects. Doing it out of
//! process avoids apartment rules: `IShellWindows` is an STA object, and calling it from arbitrary
//! threads fails intermittently.

use super::system::run_hidden;
use super::util::{cursor, from_wide};
use anytools_core::context::{AppInfo, Pt, Rect, ScreenInfo};
use anytools_core::platform::{ContextProbe, FileSelection};
use std::path::PathBuf;
use std::sync::RwLock;
use std::time::Duration;
use windows_sys::Win32::Foundation::{CloseHandle, RECT};
use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW, MonitorFromPoint, DEVMODEW, ENUM_CURRENT_SETTINGS, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId};

/// The executable name the wheel's rules match Explorer (and the desktop) by.
pub const EXPLORER: &str = "explorer.exe";

/// Reads the selection of the front Explorer window, or of the desktop when it is in front. The first
/// line is the folder, the rest are the selected paths.
const SELECTION_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$folder = $null
$shell = New-Object -ComObject Shell.Application
if ($env:ANYTOOLS_DESKTOP -eq '1') {
  $folder = $shell.NameSpace('shell:Desktop')
} else {
  $want = [int]$env:ANYTOOLS_HWND
  foreach ($w in $shell.Windows()) {
    if ($w.HWND -eq $want) { $folder = $w.Document.Folder; break }
  }
}
if ($null -eq $folder) { exit 0 }
$folder.Self.Path
foreach ($item in @($folder.SelectedItems())) { $item.Path }
"#;

// -------------------------------------------------------------------------------------------------
// Screens. Refreshed from housekeeping, read from anywhere.
// -------------------------------------------------------------------------------------------------

static SCREENS: RwLock<Vec<ScreenInfo>> = RwLock::new(Vec::new());

fn rect_of(r: RECT) -> Rect {
    Rect { x: r.left as f64, y: r.top as f64, w: (r.right - r.left) as f64, h: (r.bottom - r.top) as f64 }
}

/// Refresh rate of the monitor named by a `MONITORINFOEX`, or 60 when it can't be read.
fn refresh_hz(device: &[u16]) -> f64 {
    unsafe {
        let mut dm = DEVMODEW::default();
        dm.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
        if EnumDisplaySettingsW(device.as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm) != 0 && dm.dmDisplayFrequency > 1 {
            return dm.dmDisplayFrequency as f64;
        }
    }
    60.0
}

unsafe extern "system" fn monitor_enum(monitor: HMONITOR, _hdc: HDC, _rect: *mut RECT, data: isize) -> i32 {
    let out = unsafe { &mut *(data as *mut Vec<ScreenInfo>) };
    unsafe {
        let mut info = MONITORINFOEXW {
            monitorInfo: MONITORINFO { cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32, ..MONITORINFO::default() },
            szDevice: [0; 32],
        };
        if GetMonitorInfoW(monitor, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) != 0 {
            out.push(ScreenInfo { frame: rect_of(info.monitorInfo.rcMonitor), visible: rect_of(info.monitorInfo.rcWork), hz: refresh_hz(&info.szDevice) });
        }
    }
    1
}

pub fn refresh_screens() -> Vec<ScreenInfo> {
    let mut out = Vec::new();
    unsafe {
        EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(monitor_enum), &mut out as *mut Vec<ScreenInfo> as isize);
    }
    // The primary monitor first, matching the macOS side where the menu-bar screen leads.
    let primary = |s: &&ScreenInfo| s.frame.x == 0.0 && s.frame.y == 0.0;
    out.sort_by(|a, b| primary(&b).cmp(&primary(&a)));
    *SCREENS.write().unwrap() = out.clone();
    out
}

fn screens_cached() -> Vec<ScreenInfo> {
    let v = SCREENS.read().unwrap().clone();
    if v.is_empty() {
        refresh_screens()
    } else {
        v
    }
}

/// The monitor under a point, in physical pixels. Falls back to the nearest monitor.
#[allow(dead_code)]
pub fn monitor_at(x: i32, y: i32) -> Option<(Rect, Rect, f64)> {
    unsafe {
        let m = MonitorFromPoint(windows_sys::Win32::Foundation::POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        if m.is_null() {
            return None;
        }
        let mut info = MONITORINFOEXW {
            monitorInfo: MONITORINFO { cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32, ..MONITORINFO::default() },
            szDevice: [0; 32],
        };
        if GetMonitorInfoW(m, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) == 0 {
            return None;
        }
        Some((rect_of(info.monitorInfo.rcMonitor), rect_of(info.monitorInfo.rcWork), refresh_hz(&info.szDevice)))
    }
}

// -------------------------------------------------------------------------------------------------
// Front application
// -------------------------------------------------------------------------------------------------

fn exe_of(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        let path = from_wide(&buf[..len as usize]);
        Some(path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string())
    }
}

fn window_text(hwnd: windows_sys::Win32::Foundation::HWND, class: bool) -> String {
    unsafe {
        let mut buf = [0u16; 512];
        let n = if class { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) } else { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        from_wide(&buf[..n.max(0) as usize])
    }
}

/// The window whose selection a file wheel should read: Explorer's own windows and the desktop.
fn is_file_window(class: &str) -> bool {
    matches!(class, "CabinetWClass" | "ExploreWClass" | "Progman" | "WorkerW")
}

fn front() -> Option<AppInfo> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let exe = exe_of(pid).unwrap_or_default();
        let title = window_text(hwnd, false);
        Some(AppInfo { id: exe.clone(), name: exe.trim_end_matches(".exe").trim_end_matches(".EXE").to_string(), pid: pid as i32, title: (!title.is_empty()).then_some(title) })
    }
}

// -------------------------------------------------------------------------------------------------
// Explorer selection
// -------------------------------------------------------------------------------------------------

fn explorer_selection(timeout: Duration) -> Result<FileSelection, String> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return Ok(FileSelection::default());
    }
    let class = window_text(hwnd, true);
    let desktop = class == "Progman" || class == "WorkerW";
    if !desktop && !is_file_window(&class) {
        return Ok(FileSelection::default());
    }
    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", SELECTION_SCRIPT]);
    cmd.env("ANYTOOLS_HWND", (hwnd as isize).to_string());
    cmd.env("ANYTOOLS_DESKTOP", if desktop { "1" } else { "0" });
    let out = run_hidden(&mut cmd, timeout).map_err(|e| format!("读取资源管理器选中项失败: {e}"))?;
    if out.code != 0 {
        return Err(format!("读取资源管理器选中项失败: {}", out.stderr.trim()));
    }
    Ok(parse_selection(&out.stdout))
}

/// First non-empty line is the folder, the rest are the selected paths.
fn parse_selection(text: &str) -> FileSelection {
    let mut lines = text.lines().map(str::trim).filter(|s| !s.is_empty());
    let dir = lines.next().map(PathBuf::from);
    FileSelection { dir, paths: lines.map(PathBuf::from).collect() }
}

// -------------------------------------------------------------------------------------------------

pub struct WinContext;

impl ContextProbe for WinContext {
    fn frontmost(&self) -> Option<AppInfo> {
        front()
    }

    fn cursor(&self) -> Pt {
        cursor()
    }

    fn screens(&self) -> Vec<ScreenInfo> {
        screens_cached()
    }

    fn has_file_provider(&self, app: &AppInfo) -> bool {
        app.id.eq_ignore_ascii_case(EXPLORER)
    }

    fn file_selection(&self, app: &AppInfo, timeout: Duration) -> Result<FileSelection, String> {
        if !app.id.eq_ignore_ascii_case(EXPLORER) {
            return Err(format!("不支持读取 {} 的选中项", app.name));
        }
        explorer_selection(timeout)
    }

    fn text_selection(&self, _app: &AppInfo, timeout: Duration, allow_copy: bool) -> Result<Option<String>, String> {
        if !allow_copy {
            return Ok(None);
        }
        super::system::copy_selection(timeout)
    }
}

/// Whether `class` is an Explorer file window or the desktop.
#[cfg(test)]
fn file_window_class(class: &str) -> bool {
    is_file_window(class)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_windows_are_explorer_and_the_desktop() {
        assert!(file_window_class("CabinetWClass"));
        assert!(file_window_class("Progman"));
        assert!(file_window_class("WorkerW"));
        assert!(!file_window_class("Chrome_WidgetWin_1"));
    }

    #[test]
    fn parses_explorer_output() {
        let s = parse_selection("C:\\Users\\me\\Downloads\r\nC:\\Users\\me\\Downloads\\a.png\r\nC:\\Users\\me\\Downloads\\dir\r\n");
        assert_eq!(s.dir, Some(PathBuf::from("C:\\Users\\me\\Downloads")));
        assert_eq!(s.paths.len(), 2);
        assert_eq!(parse_selection(""), FileSelection::default());
        assert_eq!(parse_selection("  \n"), FileSelection::default());
    }
}
