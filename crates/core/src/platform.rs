//! Platform abstraction. Implementations live in `anytools-platform` (`macos`, `mock`, later `windows`).
//!
//! Threading: unless noted, every method may be called from any thread. Implementations that need the
//! main thread (AppKit) handle that internally or document it.

use crate::context::{AppInfo, Pt, ScreenInfo};
use crate::keys::{Chord, Mods};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum Button {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u8),
}

impl Button {
    pub fn parse(name: &str) -> Option<Button> {
        Some(match name {
            "left" => Button::Left,
            "right" => Button::Right,
            "middle" => Button::Middle,
            "back" | "x1" => Button::Back,
            "forward" | "x2" => Button::Forward,
            _ => return None,
        })
    }
}

/// A raw input event as seen by the global hook. Key codes are platform-specific; use
/// [`Keyboard::code_of`] to translate key names.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputEvent {
    KeyDown { code: u32, mods: Mods, repeat: bool, at: Pt },
    KeyUp { code: u32, mods: Mods },
    /// Modifier state changed (no other key involved).
    Flags { mods: Mods },
    Move { at: Pt },
    Down { button: Button, at: Pt },
    Up { button: Button },
}

/// Called on the hook thread for every event. Return `true` to swallow it. Must be fast (well under
/// 1 ms); the OS disables slow hooks.
pub trait InputFilter: Send + Sync {
    fn handle(&self, ev: InputEvent, now: Instant) -> bool;
}

pub trait InputHook: Send + Sync {
    fn start(&self, filter: Arc<dyn InputFilter>) -> Result<(), String>;
    fn running(&self) -> bool;
}

pub trait Keyboard: Send + Sync {
    /// Platform key code for a normalized key name (see [`crate::keys::normalize_key`]).
    fn code_of(&self, name: &str) -> Option<u32>;
    fn send_chord(&self, chord: &Chord) -> Result<(), String>;
    fn type_text(&self, text: &str) -> Result<(), String>;
    /// Blocks until no physical modifier is held (or `max` elapsed) so injected keys are not combined
    /// with a still-held trigger modifier. Returns how long it waited.
    fn wait_modifiers_released(&self, max: Duration) -> Duration;
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FileSelection {
    pub dir: Option<PathBuf>,
    pub paths: Vec<PathBuf>,
}

pub trait ContextProbe: Send + Sync {
    /// The frontmost application. Fast (cached where possible); safe to call from the input hook.
    fn frontmost(&self) -> Option<AppInfo>;
    fn cursor(&self) -> Pt;
    /// Current display layout. Fast (cached).
    fn screens(&self) -> Vec<ScreenInfo>;
    /// Whether `file_selection` can read the selection of this app (Finder, Explorer, …).
    fn has_file_provider(&self, app: &AppInfo) -> bool;
    /// Reads the selected files and current folder of a file manager. Blocking; gives up after
    /// `timeout`.
    fn file_selection(&self, app: &AppInfo, timeout: Duration) -> Result<FileSelection, String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ScriptLang {
    Shell,
    AppleScript,
    PowerShell,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ScriptOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub struct ScriptRun<'a> {
    pub lang: ScriptLang,
    /// Inline source, or a path when `is_file`.
    pub source: &'a str,
    pub is_file: bool,
    pub env: &'a [(String, String)],
    pub cwd: Option<&'a Path>,
    pub timeout: Duration,
}

pub trait Shell: Send + Sync {
    /// Launches or activates an application: bundle id / exe name, app name, or path.
    fn launch(&self, target: &str) -> Result<(), String>;
    /// Opens a file, folder or URL with its default handler.
    fn open(&self, target: &str) -> Result<(), String>;
    /// Opens files with a specific application (same `app` forms as [`Shell::launch`]).
    fn open_with(&self, app: &str, paths: &[PathBuf]) -> Result<(), String>;
    fn open_terminal(&self, dir: &Path, terminal: Option<&str>) -> Result<(), String>;
    fn run_script(&self, run: &ScriptRun) -> Result<ScriptOutput, String>;
    /// Modal yes/no question. Blocking.
    fn confirm(&self, title: &str, message: &str) -> bool;
    fn notify(&self, title: &str, message: &str);
}

pub trait FileOps: Send + Sync {
    fn trash(&self, paths: &[PathBuf]) -> Result<(), String>;
    fn set_clipboard_files(&self, paths: &[PathBuf]) -> Result<(), String>;
    fn clipboard_files(&self) -> Vec<PathBuf>;
    fn set_clipboard_text(&self, text: &str) -> Result<(), String>;
    fn clipboard_text(&self) -> Option<String>;
    /// Creates `dest` (a .zip) from `paths`, stored relative to `base`.
    fn zip(&self, base: &Path, paths: &[PathBuf], dest: &Path) -> Result<(), String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// macOS Accessibility: global hook + key injection. Required.
    Accessibility,
    /// macOS Automation of Finder: reading the file selection.
    AutomationFinder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermState {
    Granted,
    Denied,
    /// Never asked yet (the system prompts on first use).
    NotDetermined,
    /// Not applicable on this platform.
    NotNeeded,
    Unknown,
}

pub trait Permissions: Send + Sync {
    fn status(&self, p: Permission) -> PermState;
    /// Triggers the system prompt where possible.
    fn request(&self, p: Permission);
    fn open_settings(&self, p: Permission);
}

pub trait Icons: Send + Sync {
    /// PNG of an application's icon. `target` is the same as for [`Shell::launch`].
    fn app_icon_png(&self, target: &str, size: u32) -> Option<Vec<u8>>;
}

/// All platform services, handed to modules and actions.
#[derive(Clone)]
pub struct Services {
    pub input: Arc<dyn InputHook>,
    pub keyboard: Arc<dyn Keyboard>,
    pub context: Arc<dyn ContextProbe>,
    pub shell: Arc<dyn Shell>,
    pub files: Arc<dyn FileOps>,
    pub perms: Arc<dyn Permissions>,
    pub icons: Arc<dyn Icons>,
}
