//! Clipboard history: records copied text, shows it in a glass panel near the cursor (type to
//! search, arrows / Enter / click to paste), and offers recent entries as a wheel submenu
//! (`clipboard.recent`).
//!
//! Privacy: only text is kept; copies that the source app marks as concealed or transient
//! (password managers do), copies made in ignored apps, and anyTools' own clipboard borrowing are
//! never recorded. The history file is readable by the user only.

pub mod history;
pub mod panel;

use anytools_core::action::params;
use anytools_core::keys::parse_chord;
use anytools_core::platform::{InputEvent, Keyboard};
use anytools_core::{ActionCtx, ActionHandler, ActionSpec, ConfigFile, Context, Host, MenuItem, Module, Mods, OverlayCmd, Params, PerPlatform, Platform, Registry};
use history::{kind, kind_icon, preview, History};
use panel::{Cmd, Keys, Panel};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const POLL: Duration = Duration::from_millis(250);
/// Overlay sessions of the panel live in their own range so they never collide with the wheel's.
const SESSION_BASE: u64 = 1 << 40;
const FADE_MS: u64 = 90;
/// If the page hasn't confirmed it rendered the panel by then, show it anyway.
const READY_TIMEOUT: Duration = Duration::from_millis(150);

// ------------------------------------------------------------------------------------------------
// Config
// ------------------------------------------------------------------------------------------------

fn yes() -> bool {
    true
}
fn default_max() -> usize {
    200
}
fn default_recent() -> usize {
    8
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipboardCfg {
    /// Record copies. Off: nothing new is recorded (the existing history stays until cleared).
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Opens the panel. `null` / absent for this platform: no shortcut. (Without a `clipboard`
    /// section at all, configs from before this module get [`default_trigger`].)
    #[serde(default)]
    pub trigger: Option<PerPlatform<String>>,
    /// How many entries to keep.
    #[serde(default = "default_max")]
    pub max: usize,
    /// How many entries `clipboard.recent` shows in a wheel (2–12).
    #[serde(default = "default_recent")]
    pub recent: usize,
    /// Bundle ids (macOS) / exe names (Windows) whose copies are never recorded.
    #[serde(default)]
    pub ignore_apps: Vec<String>,
}

pub fn default_trigger() -> PerPlatform<String> {
    PerPlatform::Split(anytools_core::per_platform::Split { mac: Some("ctrl+cmd+v".into()), win: Some("ctrl+alt+v".into()) })
}

impl Default for ClipboardCfg {
    fn default() -> Self {
        ClipboardCfg { enabled: true, trigger: Some(default_trigger()), max: default_max(), recent: default_recent(), ignore_apps: vec![] }
    }
}

#[derive(Clone, Debug, Default)]
struct Compiled {
    cfg: ClipboardCfg,
    trigger: Option<(u32, Mods)>,
}

fn compile(raw: &Value, kb: &dyn Keyboard, p: Platform) -> Result<Compiled, String> {
    let cfg: ClipboardCfg = if raw.is_null() { ClipboardCfg::default() } else { serde_json::from_value(raw.clone()).map_err(|e| format!("modules.clipboard 格式错误: {e}"))? };
    if !(1..=5000).contains(&cfg.max) {
        return Err("max 应在 1–5000 之间".into());
    }
    if !(2..=12).contains(&cfg.recent) {
        return Err("recent 应在 2–12 之间".into());
    }
    let trigger = match cfg.trigger.as_ref().and_then(|t| t.get_for(p)) {
        None => None,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => {
            let c = parse_chord(s, p).map_err(|e| format!("唤起键：{e}"))?;
            if c.mods.is_empty() {
                return Err("唤起键需要至少一个修饰键（如 cmd+shift+v）".into());
            }
            let code = kb.code_of(&c.key).ok_or_else(|| format!("唤起键：不支持的按键 \"{}\"", c.key))?;
            Some((code, c.mods))
        }
    };
    Ok(Compiled { cfg, trigger })
}

// ------------------------------------------------------------------------------------------------
// Shared state
// ------------------------------------------------------------------------------------------------

enum Job {
    /// Put an entry on the clipboard, and paste it into the front app.
    Use { id: u64, paste: bool },
    Remove(u64),
    /// Show the panel natively if the page hasn't confirmed by now.
    ShowFallback { seq: u64, frame: anytools_core::Frame },
    Hidden { seq: u64 },
}

#[derive(Default)]
struct Ui {
    panel: Panel,
    keys: Keys,
    /// The page hasn't confirmed rendering this panel session yet.
    pending: Option<(u64, anytools_core::Frame)>,
}

struct Started {
    host: Host,
    jobs: Sender<Job>,
    save: Sender<()>,
}

struct State {
    hist: Mutex<History>,
    cfg: RwLock<Compiled>,
    ui: Mutex<Ui>,
    started: OnceLock<Started>,
}

impl State {
    fn cfg(&self) -> ClipboardCfg {
        self.cfg.read().unwrap().cfg.clone()
    }

    fn changed(&self) {
        if let Some(s) = self.started.get() {
            let _ = s.save.send(());
        }
        self.refresh_panel();
    }

    /// Re-render an open panel after the history changed.
    fn refresh_panel(&self) {
        let Some(st) = self.started.get() else { return };
        let mut ui = self.ui.lock().unwrap();
        if ui.panel.is_open() {
            let h = self.hist.lock().unwrap();
            ui.panel.refilter(&h);
            st.host.ui.overlay_event("clip:render", ui.panel.view(&h, now_ms()));
        }
    }

    fn dispatch(&self, st: &Started, ui: &mut Ui, h: &History, cmds: Vec<Cmd>) {
        for c in cmds {
            match c {
                Cmd::Show { seq, frame } => {
                    // The page renders first and confirms (`ready`), then the window fades in.
                    ui.pending = Some((seq, frame));
                    let mut v = ui.panel.view(h, now_ms());
                    v["frame"] = json!(frame);
                    st.host.ui.overlay_event("clip:show", v);
                    let _ = st.jobs.send(Job::ShowFallback { seq, frame });
                }
                Cmd::Render { .. } => st.host.ui.overlay_event("clip:render", ui.panel.view(h, now_ms())),
                Cmd::Hide { seq } => {
                    ui.pending = None;
                    st.host.ui.overlay_event("clip:hide", json!({ "seq": seq }));
                    st.host.ui.overlay(OverlayCmd::Hide { session: SESSION_BASE + seq, fade_ms: FADE_MS });
                    let _ = st.jobs.send(Job::Hidden { seq });
                }
                Cmd::Use { id, paste } => {
                    let _ = st.jobs.send(Job::Use { id, paste });
                }
                Cmd::Remove { id } => {
                    let _ = st.jobs.send(Job::Remove(id));
                }
            }
        }
    }

    fn show_native(&self, st: &Started, seq: u64, frame: anytools_core::Frame) {
        st.host.ui.overlay(OverlayCmd::Show { session: SESSION_BASE + seq, frame, shape: panel::shape(), channel: "clip:show", payload: Value::Null });
    }

    /// Puts entry `id` on the clipboard (it becomes the newest entry) and pastes it if asked.
    fn use_entry(&self, s: &anytools_core::Services, id: u64, paste: bool) -> Result<(), String> {
        let text = self.hist.lock().unwrap().get(id).map(|e| e.text.clone()).ok_or("这条剪贴板记录已不存在")?;
        s.files.set_clipboard_text(&text)?;
        if paste {
            s.keyboard.wait_modifiers_released(Duration::from_millis(800));
            s.keyboard.send_chord(&parse_chord("mod+v", Platform::current())?)?;
        }
        Ok(())
    }

    fn worker(self: Arc<Self>, rx: Receiver<Job>) {
        let st = self.started.get().expect("started");
        while let Ok(job) = rx.recv() {
            match job {
                Job::Use { id, paste } => {
                    if let Err(e) = self.use_entry(&st.host.services, id, paste) {
                        st.host.services.shell.notify("剪贴板", &e);
                    }
                }
                Job::Remove(id) => {
                    if self.hist.lock().unwrap().remove(id) {
                        self.changed();
                    }
                }
                Job::ShowFallback { seq, frame } => {
                    let me = self.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(READY_TIMEOUT);
                        let Some(st) = me.started.get() else { return };
                        let mut ui = me.ui.lock().unwrap();
                        if ui.pending == Some((seq, frame)) {
                            ui.pending = None;
                            tracing::warn!(seq, "clipboard: overlay page did not confirm the panel; showing anyway");
                            me.show_native(st, seq, frame);
                        }
                    });
                }
                Job::Hidden { seq } => {
                    let me = self.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(FADE_MS + 30));
                        let Some(st) = me.started.get() else { return };
                        let ui = me.ui.lock().unwrap();
                        if !ui.panel.is_open() && ui.panel.seq() == seq {
                            st.host.ui.overlay_event("clip:hidden", json!({ "seq": seq }));
                        }
                    });
                }
            }
        }
    }

    /// Polls the clipboard and records new text.
    fn watcher(self: Arc<Self>) {
        let st = self.started.get().expect("started");
        let s = &st.host.services;
        let mut seen = s.files.clipboard_change_count();
        loop {
            std::thread::sleep(POLL);
            let n = s.files.clipboard_change_count();
            if n == seen || s.files.clipboard_busy() {
                continue;
            }
            seen = n;
            // anyTools putting back what was there before (after borrowing the clipboard).
            if n == s.files.clipboard_restored_at() {
                continue;
            }
            let cfg = self.cfg();
            if !cfg.enabled {
                continue;
            }
            let app = s.context.frontmost();
            if app.as_ref().is_some_and(|a| cfg.ignore_apps.iter().any(|i| i.eq_ignore_ascii_case(&a.id))) {
                continue;
            }
            let Some(text) = s.files.clipboard_text_for_history() else { continue };
            if self.hist.lock().unwrap().add(text, app.map(|a| a.name), now_ms()) {
                self.changed();
            }
        }
    }

    /// Writes the history to disk shortly after changes (coalesced), off the input path.
    fn saver(self: Arc<Self>, rx: Receiver<()>, path: PathBuf) {
        while rx.recv().is_ok() {
            std::thread::sleep(Duration::from_millis(400));
            while rx.try_recv().is_ok() {}
            let snap = self.hist.lock().unwrap().snapshot();
            if let Err(e) = history::save(&path, &snap) {
                tracing::warn!(error = %e, "clipboard: saving history failed");
            }
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

// ------------------------------------------------------------------------------------------------
// Actions
// ------------------------------------------------------------------------------------------------

/// `clipboard.recent`: in a wheel, a submenu of the latest entries; elsewhere, opens the panel.
struct Recent(Arc<State>);

impl ActionHandler for Recent {
    fn kind(&self) -> &'static str {
        "clipboard.recent"
    }
    fn validate(&self, p: &Params, _: &Registry) -> Result<(), String> {
        if p.is_empty() {
            Ok(())
        } else {
            Err("clipboard.recent 不需要参数".into())
        }
    }
    fn available(&self, _: &Params, _: &Context) -> bool {
        !self.0.hist.lock().unwrap().is_empty()
    }
    fn is_menu(&self) -> bool {
        true
    }
    fn menu(&self, _: &Params, _: &Context) -> Vec<MenuItem> {
        let n = self.0.cfg().recent;
        let h = self.0.hist.lock().unwrap();
        h.entries()
            .iter()
            .take(n)
            .map(|e| MenuItem {
                label: preview(&e.text, 24),
                icon: kind_icon(kind(&e.text)).into(),
                action: ActionSpec::new("clipboard.paste", json!({ "id": e.id })),
            })
            .collect()
    }
    fn run(&self, _: &Params, cx: &ActionCtx) -> Result<(), String> {
        open_panel(&self.0, cx.context.cursor);
        Ok(())
    }
}

/// `clipboard.panel`: opens the clipboard panel.
struct OpenPanel(Arc<State>);

impl ActionHandler for OpenPanel {
    fn kind(&self) -> &'static str {
        "clipboard.panel"
    }
    fn run(&self, _: &Params, cx: &ActionCtx) -> Result<(), String> {
        open_panel(&self.0, cx.context.cursor);
        Ok(())
    }
}

fn open_panel(state: &Arc<State>, at: anytools_core::Pt) {
    let Some(st) = state.started.get() else { return };
    let frame = panel::place(&st.host.services.context.screens(), at);
    let mut ui = state.ui.lock().unwrap();
    let h = state.hist.lock().unwrap();
    let mut out = vec![];
    ui.panel.open(frame, &h, &mut out);
    state.dispatch(st, &mut ui, &h, out);
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PasteP {
    id: u64,
}

/// `clipboard.paste { id }`: pastes one history entry (what the wheel submenu items run).
struct Paste(Arc<State>);

impl ActionHandler for Paste {
    fn kind(&self) -> &'static str {
        "clipboard.paste"
    }
    fn validate(&self, p: &Params, _: &Registry) -> Result<(), String> {
        params::<PasteP>(self.kind(), p).map(|_| ())
    }
    fn available(&self, p: &Params, _: &Context) -> bool {
        params::<PasteP>(self.kind(), p).is_ok_and(|p| self.0.hist.lock().unwrap().get(p.id).is_some())
    }
    fn run(&self, p: &Params, cx: &ActionCtx) -> Result<(), String> {
        let p: PasteP = params(self.kind(), p)?;
        self.0.use_entry(cx.services, p.id, true)
    }
}

// ------------------------------------------------------------------------------------------------
// Module
// ------------------------------------------------------------------------------------------------

pub struct ClipboardModule {
    keyboard: Arc<dyn Keyboard>,
    state: Arc<State>,
}

impl ClipboardModule {
    pub fn new(keyboard: Arc<dyn Keyboard>) -> Self {
        let state = Arc::new(State { hist: Mutex::new(History::new(default_max())), cfg: RwLock::default(), ui: Mutex::default(), started: OnceLock::new() });
        ClipboardModule { keyboard, state }
    }
}

impl Module for ClipboardModule {
    fn id(&self) -> &'static str {
        "clipboard"
    }

    fn name(&self) -> &'static str {
        "剪贴板历史"
    }

    fn register_actions(&self, reg: &Registry) {
        reg.register(Arc::new(Recent(self.state.clone())));
        reg.register(Arc::new(OpenPanel(self.state.clone())));
        reg.register(Arc::new(Paste(self.state.clone())));
    }

    fn validate(&self, cfg: &ConfigFile, _: &Registry) -> Result<(), String> {
        compile(&cfg.module("clipboard"), self.keyboard.as_ref(), Platform::current()).map(|_| ())
    }

    fn start(&self, host: Host) -> Result<(), String> {
        let c = compile(&host.config.get().module("clipboard"), self.keyboard.as_ref(), Platform::current())?;
        let path = host.config.path().parent().map(|d| d.join("state/clipboard.json")).ok_or("配置目录未知")?;
        *self.state.hist.lock().unwrap() = History::load(&path, c.cfg.max);
        self.state.ui.lock().unwrap().keys = Keys::new(|n| self.keyboard.code_of(n))?;
        *self.state.cfg.write().unwrap() = c;

        let (jobs, jobs_rx) = mpsc::channel();
        let (save, save_rx) = mpsc::channel();
        self.state.started.set(Started { host, jobs, save }).map_err(|_| "clipboard module started twice".to_string())?;
        let spawn = |name: &str, f: Box<dyn FnOnce() + Send>| std::thread::Builder::new().name(name.into()).spawn(f).map(|_| ()).map_err(|e| e.to_string());
        let s = self.state.clone();
        spawn("clipboard", Box::new(move || s.worker(jobs_rx)))?;
        let s = self.state.clone();
        spawn("clipboard-watch", Box::new(move || s.watcher()))?;
        let s = self.state.clone();
        spawn("clipboard-save", Box::new(move || s.saver(save_rx, path)))?;
        Ok(())
    }

    fn apply_config(&self, cfg: &ConfigFile) {
        match compile(&cfg.module("clipboard"), self.keyboard.as_ref(), Platform::current()) {
            Ok(c) => {
                let max = c.cfg.max;
                *self.state.cfg.write().unwrap() = c;
                if self.state.hist.lock().unwrap().set_max(max) {
                    self.state.changed();
                }
            }
            Err(e) => tracing::error!(error = %e, "clipboard: validated config failed to apply"),
        }
    }

    fn exclusive(&self) -> bool {
        self.state.ui.lock().unwrap().panel.is_open()
    }

    fn on_input(&self, ev: &InputEvent, _now: Instant) -> bool {
        let Some(st) = self.state.started.get() else { return false };
        let trigger = self.state.cfg.read().unwrap().trigger;
        let mut ui = self.state.ui.lock().unwrap();
        // Fast path: closed, and not the trigger.
        if !ui.panel.is_open() {
            let relevant = match ev {
                InputEvent::KeyDown { code, mods, .. } => trigger == Some((*code, *mods)),
                InputEvent::KeyUp { .. } | InputEvent::Up { .. } => true,
                _ => false,
            };
            if !relevant {
                return false;
            }
        }
        let h = self.state.hist.lock().unwrap();
        let place = |p: anytools_core::Pt| panel::place(&st.host.services.context.screens(), p);
        let mut out = vec![];
        let ui = &mut *ui;
        let swallow = ui.panel.on_input(ev, trigger, &ui.keys, &h, &place, &mut out);
        self.state.dispatch(st, ui, &h, out);
        swallow
    }

    fn on_overlay(&self, msg: &Value) -> Value {
        let st = self.state.started.get();
        match msg["type"].as_str() {
            // The page rendered the panel: fade the window in.
            Some("ready") => {
                let (Some(st), Some(seq)) = (st, msg["seq"].as_u64()) else { return Value::Null };
                let mut ui = self.state.ui.lock().unwrap();
                if let Some((s, frame)) = ui.pending.filter(|(s, _)| *s == seq) {
                    ui.pending = None;
                    self.state.show_native(st, s, frame);
                }
                Value::Null
            }
            Some("clear") => {
                self.state.hist.lock().unwrap().clear();
                self.state.changed();
                json!(true)
            }
            Some("stats") => json!({ "count": self.state.hist.lock().unwrap().len() }),
            _ => Value::Null,
        }
    }

    fn status(&self) -> Value {
        json!({ "entries": self.state.hist.lock().unwrap().len(), "enabled": self.state.cfg().enabled })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anytools_platform::mock::Mock;

    fn yaml(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn config_defaults_and_errors() {
        let kb = Mock::new();
        let c = compile(&Value::Null, &kb, Platform::Mac).unwrap();
        assert!(c.cfg.enabled && c.cfg.max == 200);
        assert_eq!(c.trigger, Some((u32::from(b'v'), Mods::CTRL | Mods::CMD)), "no section: default shortcut");
        assert!(compile(&yaml(r#"{ "max": 10 }"#), &kb, Platform::Mac).unwrap().trigger.is_none(), "section without trigger: none");
        let c = compile(&yaml(r#"{ "trigger": { "mac": "cmd+shift+v" }, "max": 50 }"#), &kb, Platform::Mac).unwrap();
        assert_eq!(c.trigger, Some((u32::from(b'v'), Mods::CMD | Mods::SHIFT)));
        assert!(compile(&yaml(r#"{ "trigger": { "mac": "cmd+shift+v" } }"#), &kb, Platform::Win).unwrap().trigger.is_none());
        assert!(compile(&yaml(r#"{ "trigger": "v" }"#), &kb, Platform::Mac).unwrap_err().contains("修饰键"));
        assert!(compile(&yaml(r#"{ "max": 0 }"#), &kb, Platform::Mac).is_err());
        assert!(compile(&yaml(r#"{ "recent": 20 }"#), &kb, Platform::Mac).is_err());
        assert!(compile(&yaml(r#"{ "nope": 1 }"#), &kb, Platform::Mac).unwrap_err().contains("格式错误"));
    }

    #[test]
    fn recent_menu_and_paste() {
        let mock = Mock::new();
        let m = ClipboardModule::new(Arc::new(mock.clone()));
        let reg = Registry::default();
        m.register_actions(&reg);
        let recent = ActionSpec::new("clipboard.recent", json!({}));
        let ctx = Context::default();
        assert!(reg.is_menu(&recent));
        assert!(!reg.available(&recent, &ctx), "nothing copied yet");
        {
            let mut h = m.state.hist.lock().unwrap();
            h.add("https://example.com".into(), None, 1);
            h.add("second\nline".into(), None, 2);
        }
        assert!(reg.available(&recent, &ctx));
        let items = reg.menu(&recent, &ctx);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "second line");
        assert_eq!(items[0].icon, "lucide:AlignLeft");
        assert_eq!(items[1].icon, "lucide:Link");
        reg.validate(&items[0].action).unwrap();
        reg.run(&items[1].action, &ctx, &mock.services()).unwrap();
        let calls = mock.calls();
        assert_eq!(calls[0], anytools_platform::mock::Call::ClipText("https://example.com".into()));
        assert!(matches!(calls[1], anytools_platform::mock::Call::Chord(_)));
        assert!(!reg.available(&ActionSpec::new("clipboard.paste", json!({ "id": 99 })), &ctx));
        assert!(reg.validate(&ActionSpec::new("clipboard.paste", json!({}))).is_err());
    }
}
