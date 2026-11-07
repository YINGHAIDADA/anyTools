//! The quick wheel: hold a key (or mouse button), pick a sector, release to run its action.

pub mod config;
pub mod controller;
pub mod geometry;

use anytools_core::platform::{InputEvent, Keyboard, Permission};
use anytools_core::{ConfigFile, CoreEvent, Context, Host, Module, OverlayCmd, Pt, Registry};
use base64::Engine;
use config::{Compiled, ItemDef, ItemKind, WheelDef};
use controller::{Cmd, Controller, Env, Level};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

/// Selection probes give up after this; the wheel then stays as resolved from the front app.
const PROBE_TIMEOUT: Duration = Duration::from_millis(600);
/// The copy fallback waits this long for the front app to put the selection on the clipboard.
const TEXT_PROBE_TIMEOUT: Duration = Duration::from_millis(250);
const FADE_EXEC_MS: u64 = 140;
const FADE_CANCEL_MS: u64 = 90;

enum Job {
    Begin { seq: u64, needs_title: bool, needs_text: bool },
    Exec { label: String, action: anytools_core::ActionSpec, ctx: Context, t0: Instant },
    Replay(anytools_core::Chord),
    Hidden { seq: u64, fade_ms: u64 },
    Icons,
}

struct Inner {
    host: Host,
    ctl: Mutex<Controller>,
    cfg: RwLock<Arc<Compiled>>,
    esc_code: u32,
    jobs: Sender<Job>,
}

pub struct WheelModule {
    /// Maps trigger key names to codes; needed to validate configs even before `start`.
    keyboard: Arc<dyn Keyboard>,
    inner: OnceLock<Arc<Inner>>,
}

impl WheelModule {
    pub fn new(keyboard: Arc<dyn Keyboard>) -> Self {
        Self { keyboard, inner: OnceLock::new() }
    }
}

/// `dynamic`: computed when opened (a menu action), so the page builds it on the spot.
fn wheel_view(w: &WheelDef, dynamic: bool) -> Value {
    json!({
        "key": w.id,
        "name": w.name,
        "dynamic": dynamic,
        "items": w.items.iter().map(|i| json!({
            "id": i.id,
            "label": i.label,
            "icon": i.icon,
            "submenu": i.kind.opens_submenu(),
        })).collect::<Vec<_>>(),
    })
}

fn level_view(cfg: &Compiled, l: &Level) -> Value {
    wheel_view(l.def(cfg), matches!(l, Level::Menu(_)))
}

impl Inner {
    fn cfg(&self) -> Arc<Compiled> {
        self.cfg.read().unwrap().clone()
    }

    /// Runs the controller with a ready environment and dispatches its commands.
    fn drive(&self, f: impl FnOnce(&mut Controller, &Env, &mut Vec<Cmd>)) {
        let cfg = self.cfg();
        // Only read the screen list when a wheel actually opens (this runs for every mouse move).
        let place = |p: Pt| geometry::place(&self.host.services.context.screens(), p);
        let reg = &self.host.registry;
        let avail = |it: &ItemDef, ctx: &Context| match &it.kind {
            ItemKind::Action(a) | ItemKind::Menu(a) => reg.available(a, ctx),
            ItemKind::Submenu(_) => true,
        };
        let menu = |a: &anytools_core::ActionSpec, ctx: &Context| reg.menu(a, ctx);
        let env = Env { cfg: &cfg, place: &place, avail: &avail, menu: &menu, esc_code: self.esc_code };
        let mut out = Vec::new();
        {
            let mut ctl = self.ctl.lock().unwrap();
            f(&mut ctl, &env, &mut out);
        }
        for cmd in out {
            self.dispatch(cmd, &cfg);
        }
    }

    fn dispatch(&self, cmd: Cmd, cfg: &Compiled) {
        let ui = &self.host.ui;
        match cmd {
            Cmd::Begin { seq, needs_title, needs_text } => {
                let _ = self.jobs.send(Job::Begin { seq, needs_title, needs_text });
            }
            Cmd::Show { seq, t0, frame, wheel, disabled, click } => {
                let n = wheel.def(cfg).items.len();
                ui.overlay(OverlayCmd::Show {
                    session: seq,
                    frame,
                    shape: geometry::shape(n),
                    channel: "wheel:show",
                    payload: json!({
                        "seq": seq,
                        "wheel": level_view(cfg, &wheel),
                        "disabled": disabled,
                        "click": click,
                        "depth": 0,
                        "hz": frame.hz,
                        "t0_ms": t0.elapsed().as_secs_f64() * 1000.0,
                    }),
                });
            }
            Cmd::Hover { seq, idx, angle, dist } => {
                ui.overlay_event("wheel:hover", json!({ "seq": seq, "idx": idx, "angle": angle, "dist": dist }));
            }
            Cmd::Mode { seq, click } => ui.overlay_event("wheel:mode", json!({ "seq": seq, "click": click })),
            Cmd::Swap { seq, wheel, disabled, depth } => {
                ui.overlay(OverlayCmd::Shape { session: seq, shape: geometry::shape(wheel.def(cfg).items.len()) });
                ui.overlay_event("wheel:swap", json!({ "seq": seq, "wheel": level_view(cfg, &wheel), "disabled": disabled, "depth": depth }));
            }
            Cmd::Disabled { seq, disabled } => ui.overlay_event("wheel:disabled", json!({ "seq": seq, "disabled": disabled })),
            Cmd::Exec { seq, label, action, ctx, t0 } => {
                self.hide(seq, true);
                let _ = self.jobs.send(Job::Exec { label, action, ctx, t0 });
            }
            Cmd::Replay { chord } => {
                let _ = self.jobs.send(Job::Replay(chord));
            }
            Cmd::Cancel { seq } => self.hide(seq, false),
        }
    }

    fn hide(&self, seq: u64, flash: bool) {
        let fade_ms = if flash { FADE_EXEC_MS } else { FADE_CANCEL_MS };
        self.host.ui.overlay_event("wheel:hide", json!({ "seq": seq, "flash": flash }));
        self.host.ui.overlay(OverlayCmd::Hide { session: seq, fade_ms });
        let _ = self.jobs.send(Job::Hidden { seq, fade_ms });
    }

    // --------------------------------------------------------------------------------------------

    fn worker(self: Arc<Self>, rx: Receiver<Job>) {
        loop {
            // While a session is open, wake up regularly for dwell timers.
            let job = if self.ctl.lock().unwrap().active_seq().is_some() {
                match rx.recv_timeout(Duration::from_millis(40)) {
                    Ok(j) => Some(j),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            } else {
                match rx.recv() {
                    Ok(j) => Some(j),
                    Err(_) => return,
                }
            };
            match job {
                None => self.drive(|c, env, out| c.tick(Instant::now(), env, out)),
                Some(Job::Begin { seq, needs_title, needs_text }) => self.begin(seq, needs_title, needs_text),
                Some(Job::Exec { label, action, ctx, t0 }) => {
                    let me = self.clone();
                    // Actions may block (dialogs, scripts); never hold up the next wheel.
                    std::thread::spawn(move || me.exec(label, action, ctx, t0));
                }
                Some(Job::Replay(chord)) => {
                    let kb = &self.host.services.keyboard;
                    kb.wait_modifiers_released(Duration::from_millis(400));
                    if let Err(e) = kb.send_chord(&chord) {
                        tracing::warn!(error = %e, "wheel: replaying the trigger key failed");
                    }
                }
                Some(Job::Hidden { seq, fade_ms }) => {
                    let me = self.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(fade_ms + 30));
                        // Only if no newer session started meanwhile.
                        let ctl = me.ctl.lock().unwrap();
                        if ctl.active_seq().is_none() && ctl.last_seq() == seq {
                            drop(ctl);
                            me.host.ui.overlay_event("wheel:hidden", json!({ "seq": seq }));
                        }
                    });
                }
                Some(Job::Icons) => self.send_icons(),
            }
        }
    }

    fn begin(self: &Arc<Self>, seq: u64, needs_title: bool, needs_text: bool) {
        let ctxp = &self.host.services.context;
        let mut app = ctxp.frontmost();
        if !needs_title {
            if let Some(a) = app.as_mut() {
                a.title = None;
            }
        }
        let files = app.as_ref().is_some_and(|a| ctxp.has_file_provider(a));
        // Reading selected text is only worth it if some rule uses it (it may briefly touch the clipboard).
        let text = !files && needs_text && app.is_some();
        let a2 = app.clone();
        self.drive(|c, env, out| c.resolved(seq, a2, files || text, Instant::now(), env, out));
        let Some(app) = app else { return };
        if files {
            let me = self.clone();
            std::thread::spawn(move || {
                let res = me.host.services.context.file_selection(&app, PROBE_TIMEOUT);
                if let Err(e) = &res {
                    tracing::info!(error = %e, "wheel: selection probe failed");
                }
                me.drive(|c, env, out| c.probed(seq, res.ok(), Instant::now(), env, out));
            });
        } else if text {
            let me = self.clone();
            std::thread::spawn(move || {
                let res = me.host.services.context.text_selection(&app, TEXT_PROBE_TIMEOUT, true);
                if let Err(e) = &res {
                    tracing::info!(error = %e, "wheel: text probe failed");
                }
                me.drive(|c, env, out| c.probed_text(seq, res.map_err(|_| ()), Instant::now(), env, out));
            });
        }
    }

    fn exec(&self, label: String, action: anytools_core::ActionSpec, ctx: Context, t0: Instant) {
        let s = &self.host.services;
        let waited = s.keyboard.wait_modifiers_released(Duration::from_millis(800));
        let t = Instant::now();
        let res = self.host.registry.run(&action, &ctx, s);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        tracing::info!(label, kind = %action.kind, ok = res.is_ok(), run_ms = ms, waited_ms = waited.as_millis() as u64, session_ms = t0.elapsed().as_millis() as u64, "wheel: action");
        if let Err(e) = &res {
            s.shell.notify(&format!("「{label}」执行失败"), e);
        }
        self.host.bus.emit(CoreEvent::ActionFinished { source: "wheel".into(), label, kind: action.kind, ok: res.is_ok(), error: res.err(), ms });
    }

    /// Renders every `app:` icon once (they are cached by the overlay page).
    fn send_icons(&self) {
        self.host.ui.overlay_event("wheel:icons", json!(self.icons()));
    }

    fn icons(&self) -> HashMap<String, String> {
        let cfg = self.cfg();
        let mut icons = HashMap::new();
        for w in &cfg.wheels {
            for it in &w.items {
                let Some(target) = it.icon.strip_prefix("app:") else { continue };
                if icons.contains_key(&it.icon) {
                    continue;
                }
                if let Some(png) = self.host.services.icons.app_icon_png(target, 64) {
                    icons.insert(it.icon.clone(), format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)));
                }
            }
        }
        icons
    }
}

impl Module for WheelModule {
    fn id(&self) -> &'static str {
        "wheel"
    }

    fn name(&self) -> &'static str {
        "快捷轮盘"
    }

    fn permissions(&self) -> &'static [Permission] {
        &[Permission::Accessibility, Permission::AutomationFinder]
    }

    fn validate(&self, cfg: &ConfigFile, reg: &Registry) -> Result<(), String> {
        config::compile(&cfg.module("wheel"), reg, self.keyboard.as_ref()).map(|_| ())
    }

    fn start(&self, host: Host) -> Result<(), String> {
        let cfg = config::compile(&host.config.get().module("wheel"), &host.registry, self.keyboard.as_ref())?;
        let esc_code = self.keyboard.code_of("esc").ok_or("无法映射 Esc 键")?;
        let (tx, rx) = mpsc::channel();
        let inner = Arc::new(Inner { host, ctl: Mutex::new(Controller::default()), cfg: RwLock::new(Arc::new(cfg)), esc_code, jobs: tx });
        let w = inner.clone();
        std::thread::Builder::new().name("wheel".into()).spawn(move || w.worker(rx)).map_err(|e| e.to_string())?;
        self.inner.set(inner.clone()).map_err(|_| "wheel module started twice".to_string())?;
        let _ = inner.jobs.send(Job::Icons);
        Ok(())
    }

    fn apply_config(&self, cfg: &ConfigFile) {
        let Some(inner) = self.inner.get() else { return };
        match config::compile(&cfg.module("wheel"), &inner.host.registry, self.keyboard.as_ref()) {
            Ok(c) => {
                *inner.cfg.write().unwrap() = Arc::new(c);
                inner.host.ui.overlay_event("wheel:config", json!({}));
                let _ = inner.jobs.send(Job::Icons);
            }
            // validate() already ran on this config; this would be a bug.
            Err(e) => tracing::error!(error = %e, "wheel: validated config failed to apply"),
        }
    }

    fn exclusive(&self) -> bool {
        self.inner.get().is_some_and(|i| i.ctl.lock().unwrap().active_seq().is_some())
    }

    fn on_input(&self, ev: &InputEvent, now: Instant) -> bool {
        let Some(inner) = self.inner.get() else { return false };
        // Cheap fast path: when idle, only trigger keys/buttons matter.
        let mut swallow = false;
        inner.drive(|c, env, out| swallow = c.on_input(ev, now, env, out));
        swallow
    }

    fn on_overlay(&self, msg: &Value) -> Value {
        let Some(inner) = self.inner.get() else { return Value::Null };
        match msg["type"].as_str() {
            // The overlay pre-builds every wheel so showing one never builds DOM on the critical path.
            Some("snapshot") => {
                let _ = inner.jobs.send(Job::Icons);
                let cfg = inner.cfg();
                json!({
                    "geom": geometry::GEOM,
                    "fallback": cfg.wheels[cfg.fallback].id,
                    "wheels": cfg.wheels.iter().map(|w| wheel_view(w, false)).collect::<Vec<_>>(),
                })
            }
            Some("icons") => json!(inner.icons()),
            // For the settings window: wheels plus what opens them.
            Some("overview") => {
                let cfg = inner.cfg();
                json!({
                    "geom": geometry::GEOM,
                    "wheels": cfg.wheels.iter().map(|w| wheel_view(w, false)).collect::<Vec<_>>(),
                    "fallback": cfg.wheels[cfg.fallback].id,
                    "profiles": cfg.profiles.iter().map(|(_, w)| cfg.wheels[*w].id.clone()).collect::<Vec<_>>(),
                    "triggers": cfg.triggers.iter().map(|t| json!({
                        "input": match &t.input {
                            config::TriggerInput::Key { chord, .. } => json!({ "keys": chord }),
                            config::TriggerInput::Button(b) => json!({ "button": b }),
                        },
                        "tap": t.tap,
                        "wheel": t.wheel.map(|w| cfg.wheels[w].id.clone()),
                    })).collect::<Vec<_>>(),
                })
            }
            _ => Value::Null,
        }
    }

    fn status(&self) -> Value {
        let Some(inner) = self.inner.get() else { return Value::Null };
        let cfg = inner.cfg();
        json!({
            "wheels": cfg.wheels.iter().map(|w| json!({ "id": w.id, "name": w.name, "items": w.items.len() })).collect::<Vec<_>>(),
            "triggers": cfg.triggers.len(),
            "profiles": cfg.profiles.len(),
        })
    }
}
