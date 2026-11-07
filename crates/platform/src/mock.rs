//! An in-memory platform for tests: records what actions did instead of touching the OS.

use anytools_core::context::{AppInfo, Pt, Rect, ScreenInfo};
use anytools_core::keys::Chord;
use anytools_core::platform::*;
use anytools_core::Services;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    Chord(Chord),
    Text(String),
    Launch(String),
    Open(String),
    OpenWith(String, Vec<PathBuf>),
    Terminal(PathBuf, Option<String>),
    Script(ScriptLang, String),
    Trash(Vec<PathBuf>),
    ClipFiles(Vec<PathBuf>),
    ClipText(String),
    Zip(PathBuf, Vec<PathBuf>, PathBuf),
    Confirm(String),
    Notify(String, String),
}

#[derive(Default)]
pub struct MockState {
    pub calls: Vec<Call>,
    pub front: Option<AppInfo>,
    pub selection: Option<Result<FileSelection, String>>,
    pub confirm_answer: bool,
    pub clip_files: Vec<PathBuf>,
    pub clip_text: Option<String>,
    pub script_output: ScriptOutput,
}

#[derive(Clone, Default)]
pub struct Mock {
    pub state: Arc<Mutex<MockState>>,
}

impl Mock {
    pub fn new() -> Self {
        let m = Mock::default();
        m.state.lock().unwrap().confirm_answer = true;
        m
    }

    pub fn services(&self) -> Services {
        let a = Arc::new(self.clone());
        Services {
            input: a.clone(),
            keyboard: a.clone(),
            context: a.clone(),
            shell: a.clone(),
            files: a.clone(),
            perms: a.clone(),
            icons: a,
        }
    }

    pub fn calls(&self) -> Vec<Call> {
        self.state.lock().unwrap().calls.clone()
    }

    fn push(&self, c: Call) {
        self.state.lock().unwrap().calls.push(c);
    }
}

impl InputHook for Mock {
    fn start(&self, _filter: Arc<dyn InputFilter>) -> Result<(), String> {
        Ok(())
    }
    fn running(&self) -> bool {
        true
    }
}

impl Keyboard for Mock {
    fn code_of(&self, name: &str) -> Option<u32> {
        // Stable fake codes: the byte value for single characters, a table for named keys.
        const NAMED: &[&str] = &[
            "space", "enter", "tab", "esc", "delete", "forwarddelete", "up", "down", "left", "right", "home", "end",
            "pageup", "pagedown", "capslock",
        ];
        if name.len() == 1 {
            return name.bytes().next().map(u32::from);
        }
        if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()).filter(|n| (1..=20).contains(n)) {
            return Some(2000 + n);
        }
        NAMED.iter().position(|k| *k == name).map(|i| 1000 + i as u32)
    }
    fn send_chord(&self, chord: &Chord) -> Result<(), String> {
        self.push(Call::Chord(chord.clone()));
        Ok(())
    }
    fn type_text(&self, text: &str) -> Result<(), String> {
        self.push(Call::Text(text.into()));
        Ok(())
    }
    fn wait_modifiers_released(&self, _max: Duration) -> Duration {
        Duration::ZERO
    }
}

impl ContextProbe for Mock {
    fn frontmost(&self) -> Option<AppInfo> {
        self.state.lock().unwrap().front.clone()
    }
    fn cursor(&self) -> Pt {
        Pt { x: 500.0, y: 500.0 }
    }
    fn screens(&self) -> Vec<ScreenInfo> {
        let r = Rect { x: 0.0, y: 0.0, w: 1440.0, h: 900.0 };
        vec![ScreenInfo { frame: r, visible: r, hz: 60.0 }]
    }
    fn has_file_provider(&self, app: &AppInfo) -> bool {
        app.id == "com.apple.finder" || app.id.eq_ignore_ascii_case("explorer.exe")
    }
    fn file_selection(&self, _app: &AppInfo, _timeout: Duration) -> Result<FileSelection, String> {
        self.state.lock().unwrap().selection.clone().unwrap_or_else(|| Ok(FileSelection::default()))
    }
}

impl Shell for Mock {
    fn launch(&self, target: &str) -> Result<(), String> {
        self.push(Call::Launch(target.into()));
        Ok(())
    }
    fn open(&self, target: &str) -> Result<(), String> {
        self.push(Call::Open(target.into()));
        Ok(())
    }
    fn open_with(&self, app: &str, paths: &[PathBuf]) -> Result<(), String> {
        self.push(Call::OpenWith(app.into(), paths.to_vec()));
        Ok(())
    }
    fn open_terminal(&self, dir: &Path, terminal: Option<&str>) -> Result<(), String> {
        self.push(Call::Terminal(dir.into(), terminal.map(str::to_string)));
        Ok(())
    }
    fn run_script(&self, run: &ScriptRun) -> Result<ScriptOutput, String> {
        self.push(Call::Script(run.lang, run.source.into()));
        Ok(self.state.lock().unwrap().script_output.clone())
    }
    fn confirm(&self, _title: &str, message: &str) -> bool {
        self.push(Call::Confirm(message.into()));
        self.state.lock().unwrap().confirm_answer
    }
    fn notify(&self, title: &str, message: &str) {
        self.push(Call::Notify(title.into(), message.into()));
    }
}

impl FileOps for Mock {
    fn trash(&self, paths: &[PathBuf]) -> Result<(), String> {
        self.push(Call::Trash(paths.to_vec()));
        Ok(())
    }
    fn set_clipboard_files(&self, paths: &[PathBuf]) -> Result<(), String> {
        self.state.lock().unwrap().clip_files = paths.to_vec();
        self.push(Call::ClipFiles(paths.to_vec()));
        Ok(())
    }
    fn clipboard_files(&self) -> Vec<PathBuf> {
        self.state.lock().unwrap().clip_files.clone()
    }
    fn set_clipboard_text(&self, text: &str) -> Result<(), String> {
        self.state.lock().unwrap().clip_text = Some(text.into());
        self.push(Call::ClipText(text.into()));
        Ok(())
    }
    fn clipboard_text(&self) -> Option<String> {
        self.state.lock().unwrap().clip_text.clone()
    }
    fn zip(&self, base: &Path, paths: &[PathBuf], dest: &Path) -> Result<(), String> {
        self.push(Call::Zip(base.into(), paths.to_vec(), dest.into()));
        Ok(())
    }
}

impl Permissions for Mock {
    fn status(&self, _p: Permission) -> PermState {
        PermState::Granted
    }
    fn request(&self, _p: Permission) {}
    fn open_settings(&self, _p: Permission) {}
}

impl Icons for Mock {
    fn app_icon_png(&self, _target: &str, _size: u32) -> Option<Vec<u8>> {
        None
    }
}
