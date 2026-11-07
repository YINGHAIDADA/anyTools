//! Front app, window title (AX), screens, Finder selection (AppleScript), Automation permission.

use super::ffi::*;
use super::input::cursor_location;
use anytools_core::context::{AppInfo, Pt, ScreenInfo};
use anytools_core::platform::{ContextProbe, FileSelection};
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::AnyObject;
use objc2::{msg_send, AllocAnyThread};
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSAppleEventDescriptor, NSAppleScript, NSDictionary, NSString};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::Duration;

pub const FINDER: &str = "com.apple.finder";

// ------------------------------------------------------------------------------------------------
// Screens: refreshed on the main thread (NSScreen is main-thread only), read from anywhere.
// ------------------------------------------------------------------------------------------------

static SCREENS: RwLock<Vec<ScreenInfo>> = RwLock::new(Vec::new());

pub fn set_screens(s: Vec<ScreenInfo>) {
    *SCREENS.write().unwrap() = s;
}

// ------------------------------------------------------------------------------------------------
// Window title via Accessibility
// ------------------------------------------------------------------------------------------------

fn focused_window_title(pid: i32) -> Option<String> {
    unsafe {
        let app = Cf(AXUIElementCreateApplication(pid));
        if app.0.is_null() {
            return None;
        }
        // A hung app must not stall us.
        AXUIElementSetMessagingTimeout(app.0, 0.05);
        let mut win: CFTypeRef = std::ptr::null();
        if AXUIElementCopyAttributeValue(app.0, cfstr("AXFocusedWindow").0, &mut win) != 0 || win.is_null() {
            return None;
        }
        let win = Cf(win);
        let mut title: CFTypeRef = std::ptr::null();
        if AXUIElementCopyAttributeValue(win.0, cfstr("AXTitle").0, &mut title) != 0 || title.is_null() {
            return None;
        }
        let title = Cf(title);
        cfstring_to_string(title.0).filter(|s| !s.is_empty())
    }
}

// ------------------------------------------------------------------------------------------------
// Selected text: Accessibility first (no side effects), ⌘C as a fallback (clipboard restored).
// ------------------------------------------------------------------------------------------------

fn ax_copy(el: CFTypeRef, attr: &str) -> Option<Cf> {
    unsafe {
        let mut v: CFTypeRef = std::ptr::null();
        if AXUIElementCopyAttributeValue(el, cfstr(attr).0, &mut v) != 0 || v.is_null() {
            return None;
        }
        Some(Cf(v))
    }
}

/// `AXSelectedText` of the focused element of `pid`. `Ok(None)` = the element answered and nothing is
/// selected; `Err` = the app doesn't expose it (fall back to copying).
fn ax_selected_text(pid: i32) -> Result<Option<String>, ()> {
    unsafe {
        let app = Cf(AXUIElementCreateApplication(pid));
        if app.0.is_null() {
            return Err(());
        }
        AXUIElementSetMessagingTimeout(app.0, 0.08);
        let focused = ax_copy(app.0, "AXFocusedUIElement").ok_or(())?;
        match ax_copy(focused.0, "AXSelectedText") {
            Some(v) => Ok(cfstring_to_string(v.0).filter(|s| !s.trim().is_empty())),
            None => Err(()),
        }
    }
}

fn copy_selection(timeout: Duration) -> Result<Option<String>, String> {
    let files = super::system::MacFiles;
    super::system::set_busy(true);
    let res = copy_selection_inner(&files, timeout);
    super::system::set_busy(false);
    res
}

fn copy_selection_inner(files: &super::system::MacFiles, timeout: Duration) -> Result<Option<String>, String> {
    use super::input::MacKeyboard;
    use anytools_core::keys::{Chord, Mods};
    use anytools_core::platform::{FileOps, Keyboard};
    let snap = files.clipboard_snapshot();
    let before = files.clipboard_change_count();
    MacKeyboard.send_chord(&Chord { mods: Mods::CMD, key: "c".into() })?;
    let start = std::time::Instant::now();
    let mut text = None;
    while start.elapsed() < timeout {
        std::thread::sleep(Duration::from_millis(10));
        if files.clipboard_change_count() != before {
            text = files.clipboard_text().filter(|s| !s.trim().is_empty());
            break;
        }
    }
    // Only restore if the copy actually changed the clipboard (otherwise we'd bump changeCount for nothing).
    if files.clipboard_change_count() != before {
        files.clipboard_restore(&snap)?;
    }
    Ok(text)
}

// ------------------------------------------------------------------------------------------------
// Finder probe: one dedicated thread owns the compiled NSAppleScript (it is not thread-safe).
// ------------------------------------------------------------------------------------------------

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

type Reply = Sender<Result<FileSelection, String>>;

fn probe_thread(rx: Receiver<Reply>) {
    let script = autoreleasepool(|_| {
        let s = NSAppleScript::initWithSource(NSAppleScript::alloc(), &NSString::from_str(FINDER_SCRIPT))?;
        let mut err: Option<Retained<NSDictionary<NSString, AnyObject>>> = None;
        unsafe { s.compileAndReturnError(Some(&mut err)) }.then_some(s)
    });
    while let Ok(first) = rx.recv() {
        // Coalesce: everyone waiting gets the same fresh answer.
        let mut waiting = vec![first];
        while let Ok(more) = rx.try_recv() {
            waiting.push(more);
        }
        let res = match &script {
            Some(s) => run_finder_script(s),
            None => Err("AppleScript 编译失败".into()),
        };
        for w in waiting {
            // The caller may have timed out and gone; that's fine.
            let _ = w.send(res.clone());
        }
    }
}

fn run_finder_script(script: &NSAppleScript) -> Result<FileSelection, String> {
    autoreleasepool(|_| {
        let mut err: Option<Retained<NSDictionary<NSString, AnyObject>>> = None;
        let res: Option<Retained<NSAppleEventDescriptor>> = unsafe { msg_send![script, executeAndReturnError: Some(&mut err)] };
        let Some(res) = res else {
            let code = err
                .as_ref()
                .and_then(|d| d.objectForKey(&NSString::from_str("NSAppleScriptErrorNumber")))
                .map(|n| unsafe { msg_send![&*n, integerValue] })
                .unwrap_or(0isize);
            return Err(match code as i32 {
                ERR_AE_NOT_PERMITTED => "未授权控制 Finder（系统设置 › 隐私与安全性 › 自动化）".to_string(),
                c => format!("读取 Finder 选中项失败（{c}）"),
            });
        };
        let text = res.stringValue().map(|s| s.to_string()).unwrap_or_default();
        Ok(parse_finder_output(&text))
    })
}

fn parse_finder_output(text: &str) -> FileSelection {
    let mut lines = text.split(['\n', '\r']);
    let dir = lines.next().filter(|s| !s.is_empty()).map(PathBuf::from);
    let paths = lines.filter(|s| !s.is_empty()).map(PathBuf::from).collect();
    FileSelection { dir, paths }
}

fn probe_tx() -> &'static Mutex<Sender<Reply>> {
    static TX: OnceLock<Mutex<Sender<Reply>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new().name("finder-probe".into()).spawn(move || probe_thread(rx)).expect("spawn finder-probe");
        Mutex::new(tx)
    })
}

// ------------------------------------------------------------------------------------------------
// Automation permission (Apple Events to Finder)
// ------------------------------------------------------------------------------------------------

/// `ask`: show the system prompt if not decided yet (blocks until the user answers).
pub fn automation_permission(bundle: &str, ask: bool) -> i32 {
    unsafe {
        let mut desc = AEDesc { descriptor_type: 0, data_handle: std::ptr::null_mut() };
        if AECreateDesc(typeApplicationBundleID, bundle.as_ptr().cast(), bundle.len() as isize, &mut desc) != 0 {
            return -1;
        }
        let r = AEDeterminePermissionToAutomateTarget(&desc, typeWildCard, typeWildCard, ask);
        AEDisposeDesc(&mut desc);
        r
    }
}

// ------------------------------------------------------------------------------------------------

pub struct MacContext;

impl ContextProbe for MacContext {
    fn frontmost(&self) -> Option<AppInfo> {
        autoreleasepool(|_| {
            let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
            let pid = app.processIdentifier();
            Some(AppInfo {
                id: app.bundleIdentifier().map(|s| s.to_string()).unwrap_or_default(),
                name: app.localizedName().map(|s| s.to_string()).unwrap_or_default(),
                pid,
                title: focused_window_title(pid),
            })
        })
    }

    fn cursor(&self) -> Pt {
        cursor_location()
    }

    fn screens(&self) -> Vec<ScreenInfo> {
        SCREENS.read().unwrap().clone()
    }

    fn has_file_provider(&self, app: &AppInfo) -> bool {
        app.id == FINDER
    }

    fn file_selection(&self, app: &AppInfo, timeout: Duration) -> Result<FileSelection, String> {
        if app.id != FINDER {
            return Err(format!("不支持读取 {} 的选中项", app.name));
        }
        let (tx, rx) = mpsc::channel();
        probe_tx().lock().unwrap().send(tx).map_err(|_| "Finder 探测线程已退出".to_string())?;
        rx.recv_timeout(timeout).map_err(|_| format!("读取 Finder 选中项超时（>{}ms）", timeout.as_millis()))?
    }

    fn text_selection(&self, app: &AppInfo, timeout: Duration, allow_copy: bool) -> Result<Option<String>, String> {
        match ax_selected_text(app.pid) {
            Ok(t) => Ok(t),
            Err(()) if allow_copy => copy_selection(timeout),
            Err(()) => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_finder_output() {
        let s = parse_finder_output("/Users/me/Downloads/\n/Users/me/Downloads/a.png\n/Users/me/Downloads/dir/\n");
        assert_eq!(s.dir, Some(PathBuf::from("/Users/me/Downloads/")));
        assert_eq!(s.paths.len(), 2);
        let empty = parse_finder_output("");
        assert_eq!(empty, FileSelection::default());
        let no_dir = parse_finder_output("\n/a");
        assert_eq!(no_dir.dir, None);
        assert_eq!(no_dir.paths, vec![PathBuf::from("/a")]);
    }
}
