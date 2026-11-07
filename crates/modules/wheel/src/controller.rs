//! The wheel interaction state machine. Pure: inputs are events + time, outputs are [`Cmd`]s.
//!
//! Flow of one session:
//! 1. Trigger down (hook thread) → `Begin`. The worker looks up the front app and calls
//!    [`Controller::resolved`], which picks the wheel and emits `Show`.
//! 2. If the front app is a file manager, the worker reads the selection and calls
//!    [`Controller::probed`], which may swap to another wheel (only early, or before anything is hovered).
//! 3. Release / click → `Exec` or `Cancel`. Submenus open by crossing the outer ring, dwelling, or
//!    releasing on them; the center (or Esc) goes back.

use crate::config::{Compiled, ItemDef, ItemKind, TapMode, TriggerInput, WheelId};
use crate::geometry::{angle_of, sector_at, GEOM};
use anytools_core::keys::Chord;
use anytools_core::platform::{Button, FileSelection, InputEvent};
use anytools_core::{ActionSpec, AppInfo, Context, Frame, Pt, Selection};
use std::time::{Duration, Instant};

pub const TAP_MAX: Duration = Duration::from_millis(180);
const TAP_MOVE_MAX: f64 = 6.0;
const HOVER_MIN_INTERVAL: Duration = Duration::from_millis(8);
/// A late selection result may still swap the wheel within this time, or while nothing is hovered.
pub const SWAP_BUDGET: Duration = Duration::from_millis(150);
pub const DWELL: Duration = Duration::from_millis(320);
const MAX_DEPTH: usize = 8;
/// How far past the outer ring the cursor must go on a submenu item to open it.
const CROSS_MARGIN: f64 = 12.0;

pub struct Env<'a> {
    pub cfg: &'a Compiled,
    pub place: &'a dyn Fn(Pt) -> Frame,
    pub avail: &'a dyn Fn(&ItemDef, &Context) -> bool,
    pub esc_code: u32,
}

#[derive(Debug, Clone)]
pub enum Cmd {
    /// Look up the front app (and maybe the selection), then call `resolved` / `probed`.
    Begin { seq: u64, needs_title: bool },
    Show { seq: u64, t0: Instant, frame: Frame, wheel: WheelId, disabled: Vec<bool>, click: bool },
    Hover { seq: u64, idx: Option<usize>, angle: f64, dist: f64 },
    Mode { seq: u64, click: bool },
    /// Different wheel in the same window: submenu in/out, or a late selection result.
    Swap { seq: u64, wheel: WheelId, disabled: Vec<bool>, depth: usize },
    Disabled { seq: u64, disabled: Vec<bool> },
    Exec { seq: u64, label: String, action: ActionSpec, ctx: Context, t0: Instant },
    /// Sends the trigger chord on (tap with `tap: replay`).
    Replay { chord: Chord },
    Cancel { seq: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Hold,
    Click,
}

struct Session {
    seq: u64,
    t0: Instant,
    trigger: usize,
    pt0: Pt,
    last_pt: Pt,
    max_move: f64,
    frame: Frame,
    center: Pt,
    /// `None` until the front app is known (unless the trigger forces a wheel).
    root: Option<WheelId>,
    forced: bool,
    stack: Vec<WheelId>,
    hovered: Option<usize>,
    hover_since: Instant,
    dead_since: Option<Instant>,
    /// Set after going back via the center, until the cursor leaves the center.
    dead_consumed: bool,
    cross_armed: bool,
    disabled: Vec<bool>,
    ctx: Context,
    /// A selection probe is running: don't grey out items that may need it.
    probing: bool,
    last_hover_emit: Option<Instant>,
}

impl Session {
    fn current(&self) -> Option<WheelId> {
        self.stack.last().copied().or(self.root)
    }
}

#[derive(Default)]
pub struct Controller {
    seq: u64,
    phase: Option<Phase>,
    s: Option<Session>,
    held: Option<usize>,
    key_swallowed: Option<u32>,
    button_swallowed: Option<Button>,
    swallow_esc_up: bool,
    swallow_left_up: bool,
    swallow_right_up: bool,
}

impl Controller {
    pub fn phase(&self) -> Phase {
        self.phase.unwrap_or(Phase::Idle)
    }

    pub fn active_seq(&self) -> Option<u64> {
        (self.phase() != Phase::Idle).then(|| self.s.as_ref().map(|s| s.seq)).flatten()
    }

    /// The latest session number (also after it ended).
    pub fn last_seq(&self) -> u64 {
        self.seq
    }

    fn live(&mut self, seq: u64) -> Option<&mut Session> {
        if self.phase() == Phase::Idle {
            return None;
        }
        self.s.as_mut().filter(|s| s.seq == seq)
    }

    // --------------------------------------------------------------------------------------------
    // Input (hook thread). Returns whether to swallow the event.
    // --------------------------------------------------------------------------------------------

    pub fn on_input(&mut self, ev: &InputEvent, now: Instant, env: &Env, out: &mut Vec<Cmd>) -> bool {
        match *ev {
            InputEvent::KeyDown { code, repeat: true, .. } => self.key_swallowed == Some(code),
            InputEvent::KeyDown { code, .. } if code == env.esc_code => {
                if self.phase() == Phase::Idle {
                    return false;
                }
                self.swallow_esc_up = true;
                if self.s.as_ref().is_some_and(|s| !s.stack.is_empty()) {
                    self.pop(now, env, out);
                } else {
                    self.cancel(out);
                }
                true
            }
            InputEvent::KeyDown { code, mods, at, .. } => {
                let hit = env.cfg.triggers.iter().position(|t| matches!(&t.input, TriggerInput::Key { code: c, mods: m, .. } if *c == code && *m == mods));
                let Some(i) = hit else { return false };
                self.key_swallowed = Some(code);
                self.trigger_down(i, at, now, env, out);
                true
            }
            InputEvent::KeyUp { code, .. } => {
                if code == env.esc_code && self.swallow_esc_up {
                    self.swallow_esc_up = false;
                    return true;
                }
                if self.key_swallowed == Some(code) {
                    self.key_swallowed = None;
                    self.release(now, env, out);
                    return true;
                }
                false
            }
            InputEvent::Flags { mods } => {
                if let Some(i) = self.held {
                    if let Some(TriggerInput::Key { mods: m, .. }) = env.cfg.triggers.get(i).map(|t| &t.input) {
                        if !m.is_empty() && !mods.contains(*m) {
                            self.release(now, env, out);
                        }
                    }
                }
                false
            }
            InputEvent::Move { at } => {
                if self.phase() != Phase::Idle {
                    self.update_hover(at, now, false, env, out);
                }
                false
            }
            InputEvent::Down { button, at } => {
                if let Some(i) = env.cfg.triggers.iter().position(|t| t.input == TriggerInput::Button(button)) {
                    self.button_swallowed = Some(button);
                    self.trigger_down(i, at, now, env, out);
                    return true;
                }
                if self.phase() == Phase::Idle {
                    return false;
                }
                match button {
                    Button::Left => {
                        self.swallow_left_up = true;
                        if self.phase() == Phase::Click {
                            self.click(now, env, out);
                        }
                        true
                    }
                    Button::Right => {
                        self.swallow_right_up = true;
                        self.cancel(out);
                        true
                    }
                    _ => false,
                }
            }
            InputEvent::Up { button } => {
                if self.button_swallowed == Some(button) {
                    self.button_swallowed = None;
                    self.release(now, env, out);
                    return true;
                }
                match button {
                    Button::Left => std::mem::take(&mut self.swallow_left_up),
                    Button::Right => std::mem::take(&mut self.swallow_right_up),
                    _ => false,
                }
            }
        }
    }

    fn trigger_down(&mut self, i: usize, at: Pt, now: Instant, env: &Env, out: &mut Vec<Cmd>) {
        if self.phase() != Phase::Idle {
            // Pressing the trigger while the wheel is open (click mode) closes it.
            self.held = None;
            self.cancel(out);
            return;
        }
        self.seq += 1;
        let frame = (env.place)(at);
        let forced = env.cfg.triggers[i].wheel;
        self.s = Some(Session {
            seq: self.seq,
            t0: now,
            trigger: i,
            pt0: at,
            last_pt: at,
            max_move: 0.0,
            frame,
            center: frame.center(),
            root: forced,
            forced: forced.is_some(),
            stack: vec![],
            hovered: None,
            hover_since: now,
            dead_since: None,
            dead_consumed: false,
            cross_armed: true,
            disabled: vec![],
            ctx: Context { cursor: at, ..Default::default() },
            probing: false,
            last_hover_emit: None,
        });
        self.phase = Some(Phase::Hold);
        self.held = Some(i);
        out.push(Cmd::Begin { seq: self.seq, needs_title: env.cfg.needs_title });
        if forced.is_some() {
            self.show(now, env, out);
        }
    }

    fn release(&mut self, now: Instant, env: &Env, out: &mut Vec<Cmd>) {
        if self.held.take().is_none() || self.phase() != Phase::Hold {
            return;
        }
        let s = self.s.as_ref().expect("hold phase has a session");
        let tap = now.duration_since(s.t0) < TAP_MAX && s.max_move < TAP_MOVE_MAX;
        if !tap {
            return self.commit(now, env, out);
        }
        let t = &env.cfg.triggers[s.trigger];
        match t.tap {
            TapMode::Click => {
                self.phase = Some(Phase::Click);
                if s.root.is_some() {
                    out.push(Cmd::Mode { seq: s.seq, click: true });
                }
            }
            TapMode::Replay => {
                let chord = match &t.input {
                    TriggerInput::Key { chord, .. } => Some(chord.clone()),
                    TriggerInput::Button(_) => None,
                };
                self.cancel(out);
                if let Some(chord) = chord {
                    out.push(Cmd::Replay { chord });
                }
            }
            TapMode::None => self.cancel(out),
        }
    }

    // --------------------------------------------------------------------------------------------
    // Worker callbacks
    // --------------------------------------------------------------------------------------------

    /// The front app is known. `probing`: a selection probe was started for it.
    pub fn resolved(&mut self, seq: u64, app: Option<AppInfo>, probing: bool, now: Instant, env: &Env, out: &mut Vec<Cmd>) {
        let Some(s) = self.live(seq) else { return };
        s.ctx.app = app;
        s.probing = probing;
        if s.root.is_none() {
            s.root = Some(env.cfg.resolve(&s.ctx));
            self.show(now, env, out);
        } else {
            // Forced wheel: already shown; availability may depend on the app.
            self.refresh_disabled(env, out);
        }
    }

    /// The selection probe finished (`None`: it failed or timed out).
    pub fn probed(&mut self, seq: u64, sel: Option<FileSelection>, now: Instant, env: &Env, out: &mut Vec<Cmd>) {
        let Some(s) = self.live(seq) else { return };
        s.probing = false;
        if let Some(sel) = sel {
            s.ctx.selection = Selection::Files { paths: sel.paths };
            s.ctx.cwd = sel.dir;
            s.ctx.selection_known = true;
        }
        let can_swap = !s.forced && s.stack.is_empty() && s.root.is_some();
        if can_swap {
            let new_root = env.cfg.resolve(&s.ctx);
            let early = now.duration_since(s.t0) <= SWAP_BUDGET;
            if Some(new_root) != s.root && (early || s.hovered.is_none()) {
                s.root = Some(new_root);
                s.hovered = None;
                s.disabled = disabled_for(env, new_root, &s.ctx, false);
                out.push(Cmd::Swap { seq, wheel: new_root, disabled: s.disabled.clone(), depth: 0 });
                let pt = s.last_pt;
                self.update_hover(pt, now, true, env, out);
                return;
            }
        }
        self.refresh_disabled(env, out);
    }

    /// Periodic (every ~40 ms while a session is open): dwell timers.
    pub fn tick(&mut self, now: Instant, env: &Env, out: &mut Vec<Cmd>) {
        let Some(s) = self.s.as_ref().filter(|_| self.phase() != Phase::Idle) else { return };
        let Some(cur) = s.current() else { return };
        if let Some(i) = s.hovered {
            if let ItemKind::Submenu(w) = env.cfg.wheels[cur].items[i].kind {
                if now.duration_since(s.hover_since) >= DWELL {
                    self.push(w, now, env, out);
                }
            }
        } else if !s.stack.is_empty() && !s.dead_consumed && s.dead_since.is_some_and(|d| now.duration_since(d) >= DWELL) {
            self.pop(now, env, out);
        }
    }

    // --------------------------------------------------------------------------------------------

    fn show(&mut self, now: Instant, env: &Env, out: &mut Vec<Cmd>) {
        let click = self.phase() == Phase::Click;
        let s = self.s.as_mut().expect("session");
        let wheel = s.current().expect("resolved");
        s.disabled = disabled_for(env, wheel, &s.ctx, s.probing);
        out.push(Cmd::Show { seq: s.seq, t0: s.t0, frame: s.frame, wheel, disabled: s.disabled.clone(), click });
        let pt = s.last_pt;
        // The cursor may already be over a sector when the window was pushed in from a screen edge.
        self.update_hover(pt, now, true, env, out);
    }

    fn refresh_disabled(&mut self, env: &Env, out: &mut Vec<Cmd>) {
        let Some(s) = self.s.as_mut() else { return };
        let Some(cur) = s.current() else { return };
        let d = disabled_for(env, cur, &s.ctx, s.probing);
        if d != s.disabled {
            s.disabled = d.clone();
            out.push(Cmd::Disabled { seq: s.seq, disabled: d });
        }
    }

    fn update_hover(&mut self, pt: Pt, now: Instant, force: bool, env: &Env, out: &mut Vec<Cmd>) {
        let Some(s) = self.s.as_mut() else { return };
        s.last_pt = pt;
        s.max_move = s.max_move.max((pt.x - s.pt0.x).hypot(pt.y - s.pt0.y));
        let Some(cur) = s.current() else { return };
        let items = &env.cfg.wheels[cur].items;
        let (dx, dy) = (pt.x - s.center.x, pt.y - s.center.y);
        let dist = dx.hypot(dy);
        let idx = sector_at(dx, dy, items.len(), s.hovered);

        if dist < GEOM.dead {
            s.dead_since.get_or_insert(now);
        } else {
            s.dead_since = None;
            s.dead_consumed = false;
        }
        if dist < GEOM.r_out {
            s.cross_armed = true;
        }
        let changed = idx != s.hovered;
        if changed {
            s.hovered = idx;
            s.hover_since = now;
        }
        let due = s.last_hover_emit.is_none_or(|t| now.duration_since(t) >= HOVER_MIN_INTERVAL);
        if force || changed || due {
            s.last_hover_emit = Some(now);
            out.push(Cmd::Hover { seq: s.seq, idx, angle: angle_of(dx, dy).to_degrees(), dist });
        }
        if let Some(i) = idx {
            if let ItemKind::Submenu(w) = items[i].kind {
                if s.cross_armed && dist > GEOM.r_out + CROSS_MARGIN {
                    self.push(w, now, env, out);
                }
            }
        }
    }

    fn push(&mut self, w: WheelId, now: Instant, env: &Env, out: &mut Vec<Cmd>) {
        let s = self.s.as_mut().expect("session");
        if s.stack.len() >= MAX_DEPTH {
            return;
        }
        s.stack.push(w);
        self.enter_current(now, env, out);
    }

    fn pop(&mut self, now: Instant, env: &Env, out: &mut Vec<Cmd>) {
        let s = self.s.as_mut().expect("session");
        if s.stack.pop().is_none() {
            return;
        }
        s.dead_consumed = true;
        self.enter_current(now, env, out);
    }

    fn enter_current(&mut self, now: Instant, env: &Env, out: &mut Vec<Cmd>) {
        let s = self.s.as_mut().expect("session");
        let Some(cur) = s.current() else { return };
        s.hovered = None;
        s.hover_since = now;
        s.cross_armed = false;
        s.dead_since = None;
        s.disabled = disabled_for(env, cur, &s.ctx, s.probing);
        out.push(Cmd::Swap { seq: s.seq, wheel: cur, disabled: s.disabled.clone(), depth: s.stack.len() });
        let pt = s.last_pt;
        self.update_hover(pt, now, true, env, out);
    }

    /// Release in hold mode.
    fn commit(&mut self, now: Instant, env: &Env, out: &mut Vec<Cmd>) {
        let s = self.s.as_ref().expect("session");
        let (Some(cur), Some(i)) = (s.current(), s.hovered) else { return self.cancel(out) };
        match &env.cfg.wheels[cur].items[i].kind {
            ItemKind::Submenu(w) => {
                let (w, seq) = (*w, s.seq);
                self.phase = Some(Phase::Click);
                self.push(w, now, env, out);
                out.push(Cmd::Mode { seq, click: true });
            }
            ItemKind::Action(_) if s.disabled.get(i).copied().unwrap_or(false) => self.cancel(out),
            ItemKind::Action(_) => self.exec(cur, i, env, out),
        }
    }

    /// Left click in click mode.
    fn click(&mut self, now: Instant, env: &Env, out: &mut Vec<Cmd>) {
        let s = self.s.as_ref().expect("session");
        let Some(cur) = s.current() else { return self.cancel(out) };
        match s.hovered {
            Some(i) => match &env.cfg.wheels[cur].items[i].kind {
                ItemKind::Submenu(w) => {
                    let w = *w;
                    self.push(w, now, env, out);
                }
                ItemKind::Action(_) if s.disabled.get(i).copied().unwrap_or(false) => {}
                ItemKind::Action(_) => self.exec(cur, i, env, out),
            },
            None if !s.stack.is_empty() => self.pop(now, env, out),
            None => self.cancel(out),
        }
    }

    fn exec(&mut self, cur: WheelId, i: usize, env: &Env, out: &mut Vec<Cmd>) {
        let s = self.s.as_ref().expect("session");
        let item = &env.cfg.wheels[cur].items[i];
        if let ItemKind::Action(spec) = &item.kind {
            out.push(Cmd::Exec { seq: s.seq, label: item.label.clone(), action: spec.clone(), ctx: s.ctx.clone(), t0: s.t0 });
        }
        self.phase = Some(Phase::Idle);
        self.held = None;
    }

    fn cancel(&mut self, out: &mut Vec<Cmd>) {
        if let Some(s) = &self.s {
            if self.phase() != Phase::Idle {
                out.push(Cmd::Cancel { seq: s.seq });
            }
        }
        self.phase = Some(Phase::Idle);
        self.held = None;
    }
}

fn disabled_for(env: &Env, wheel: WheelId, ctx: &Context, optimistic: bool) -> Vec<bool> {
    env.cfg.wheels[wheel]
        .items
        .iter()
        .map(|it| match it.kind {
            ItemKind::Submenu(_) => false,
            ItemKind::Action(_) => !optimistic && !(env.avail)(it, ctx),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::compile_for;
    use anytools_core::keys::Mods;
    use anytools_core::platform::Keyboard;
    use anytools_core::{Platform, Registry};
    use anytools_platform::mock::Mock;
    use std::path::PathBuf;

    const CFG: &str = r#"
triggers:
  - keys: "alt+space"
  - keys: "f13"
    tap: replay
  - button: middle
    wheel: tools
profiles:
  - when: { app: com.apple.finder, selection: files }
    wheel: files
wheels:
  default:
    items:
      - { id: a, label: A, action: { type: text, text: a } }
      - { id: b, label: B, action: { type: text, text: b } }
      - { id: more, label: More, action: { type: submenu, wheel: tools } }
      - { id: path, label: Path, action: { type: file.copy_path } }
  tools:
    items:
      - { label: T1, action: { type: text, text: t1 } }
      - { label: T2, action: { type: text, text: t2 } }
      - { label: Deep, action: { type: submenu, wheel: deep } }
  deep:
    items:
      - { label: D1, action: { type: text, text: d1 } }
      - { label: D2, action: { type: text, text: d2 } }
  files:
    items:
      - { label: F1, action: { type: file.copy_path } }
      - { label: F2, action: { type: file.trash } }
"#;

    struct T {
        c: Controller,
        cfg: Compiled,
        reg: Registry,
        mock: Mock,
        t: Instant,
    }

    const C: Pt = Pt { x: 500.0, y: 500.0 };

    fn at(deg: f64, r: f64) -> Pt {
        let a = deg.to_radians();
        Pt { x: C.x + r * a.sin(), y: C.y - r * a.cos() }
    }

    impl T {
        fn new() -> T {
            let reg = Registry::default();
            anytools_actions::register_all(&reg);
            let mock = Mock::new();
            let raw = serde_yaml_ng::from_str(CFG).unwrap();
            let cfg = compile_for(&raw, &reg, &mock, Platform::Mac).unwrap();
            T { c: Controller::default(), cfg, reg, mock, t: Instant::now() }
        }
        fn ms(&self, ms: u64) -> Instant {
            self.t + Duration::from_millis(ms)
        }
        fn with_env<R>(&mut self, f: impl FnOnce(&mut Controller, &Env) -> R) -> R {
            let place = |p: Pt| Frame { x: p.x - 210.0, y: p.y - 210.0, w: 420.0, h: 420.0, hz: 60.0 };
            let reg = &self.reg;
            let avail = |it: &ItemDef, ctx: &Context| match &it.kind {
                ItemKind::Action(a) => reg.available(a, ctx),
                _ => true,
            };
            let env = Env { cfg: &self.cfg, place: &place, avail: &avail, esc_code: self.mock.code_of("esc").unwrap() };
            f(&mut self.c, &env)
        }
        fn input(&mut self, ev: InputEvent, ms: u64) -> (bool, Vec<Cmd>) {
            let now = self.ms(ms);
            self.with_env(|c, env| {
                let mut out = vec![];
                let sw = c.on_input(&ev, now, env, &mut out);
                (sw, out)
            })
        }
        fn resolve(&mut self, app: &str, probing: bool, ms: u64) -> Vec<Cmd> {
            let now = self.ms(ms);
            let seq = self.c.last_seq();
            let app = AppInfo { id: app.into(), ..Default::default() };
            self.with_env(|c, env| {
                let mut out = vec![];
                c.resolved(seq, Some(app), probing, now, env, &mut out);
                out
            })
        }
        fn probe(&mut self, seq: u64, files: &[&str], ms: u64) -> Vec<Cmd> {
            let now = self.ms(ms);
            let sel = FileSelection { dir: Some("/tmp".into()), paths: files.iter().map(PathBuf::from).collect() };
            self.with_env(|c, env| {
                let mut out = vec![];
                c.probed(seq, Some(sel), now, env, &mut out);
                out
            })
        }
        fn tick(&mut self, ms: u64) -> Vec<Cmd> {
            let now = self.ms(ms);
            self.with_env(|c, env| {
                let mut out = vec![];
                c.tick(now, env, &mut out);
                out
            })
        }
        fn code(&self, k: &str) -> u32 {
            self.mock.code_of(k).unwrap()
        }
        fn down(&mut self, ms: u64) -> (bool, Vec<Cmd>) {
            let code = self.code("space");
            self.input(InputEvent::KeyDown { code, mods: Mods::ALT, repeat: false, at: C }, ms)
        }
        fn up(&mut self, ms: u64) -> (bool, Vec<Cmd>) {
            let code = self.code("space");
            self.input(InputEvent::KeyUp { code, mods: Mods::ALT }, ms)
        }
        fn mv(&mut self, p: Pt, ms: u64) -> Vec<Cmd> {
            let (sw, out) = self.input(InputEvent::Move { at: p }, ms);
            assert!(!sw, "moves are never swallowed");
            out
        }
        fn wheel(&self, id: &str) -> WheelId {
            self.cfg.wheel_index(id).unwrap()
        }
    }

    fn exec_label(out: &[Cmd]) -> Option<&str> {
        out.iter().find_map(|c| match c {
            Cmd::Exec { label, .. } => Some(label.as_str()),
            _ => None,
        })
    }

    fn swapped_to(out: &[Cmd]) -> Option<(WheelId, usize)> {
        out.iter().find_map(|c| match c {
            Cmd::Swap { wheel, depth, .. } => Some((*wheel, *depth)),
            _ => None,
        })
    }

    #[test]
    fn hold_move_release_executes() {
        let mut t = T::new();
        let (sw, out) = t.down(0);
        assert!(sw);
        assert!(matches!(out.as_slice(), [Cmd::Begin { seq: 1, needs_title: false }]), "nothing shown before the front app is known");
        let out = t.resolve("com.apple.TextEdit", false, 3);
        let def = t.wheel("default");
        assert!(matches!(&out[0], Cmd::Show { wheel, click: false, .. } if *wheel == def));
        assert!(matches!(out[1], Cmd::Hover { idx: None, .. }));
        let out = t.mv(at(90.0, 100.0), 50);
        assert!(matches!(out.as_slice(), [Cmd::Hover { idx: Some(1), .. }]));
        let (sw, out) = t.up(400);
        assert!(sw);
        assert_eq!(exec_label(&out), Some("B"));
        assert_eq!(t.c.phase(), Phase::Idle);
    }

    #[test]
    fn release_in_center_cancels() {
        let mut t = T::new();
        t.down(0);
        t.resolve("x", false, 2);
        t.mv(at(0.0, 10.0), 100);
        let (_, out) = t.up(400);
        assert!(matches!(out.as_slice(), [Cmd::Cancel { seq: 1 }]));
    }

    #[test]
    fn tap_opens_click_mode_and_click_executes() {
        let mut t = T::new();
        t.down(0);
        // Released before the front app was even known: show arrives later, already in click mode.
        let (_, out) = t.up(60);
        assert!(out.is_empty(), "no Mode before Show: {out:?}");
        let out = t.resolve("x", false, 70);
        assert!(matches!(&out[0], Cmd::Show { click: true, .. }));
        t.mv(at(0.0, 100.0), 200);
        let (sw, out) = t.input(InputEvent::Down { button: Button::Left, at: at(0.0, 100.0) }, 300);
        assert!(sw);
        assert_eq!(exec_label(&out), Some("A"));
        assert!(t.input(InputEvent::Up { button: Button::Left }, 350).0, "its mouse-up is swallowed too");
        assert!(!t.input(InputEvent::Up { button: Button::Left }, 400).0);
    }

    #[test]
    fn replay_tap_sends_the_key_on() {
        let mut t = T::new();
        let code = t.code("f13");
        t.input(InputEvent::KeyDown { code, mods: Mods::NONE, repeat: false, at: C }, 0);
        let (sw, out) = t.input(InputEvent::KeyUp { code, mods: Mods::NONE }, 50);
        assert!(sw);
        assert!(matches!(out.as_slice(), [Cmd::Cancel { .. }, Cmd::Replay { chord }] if chord.key == "f13"));
    }

    #[test]
    fn modifier_released_first_commits_and_swallows_trailing_space() {
        let mut t = T::new();
        t.down(0);
        t.resolve("x", false, 2);
        t.mv(at(180.0, 100.0), 100);
        let (sw, out) = t.input(InputEvent::Flags { mods: Mods::NONE }, 300);
        assert!(!sw, "modifier changes always pass through");
        assert_eq!(exec_label(&out), None, "the submenu item at 180° opens instead of executing");
        let space = t.code("space");
        assert!(t.input(InputEvent::KeyDown { code: space, mods: Mods::NONE, repeat: true, at: C }, 320).0, "no stray auto-repeat spaces");
        assert!(t.up(350).0);
        assert!(!t.up(400).0);
    }

    #[test]
    fn other_keys_and_other_modifiers_pass_through() {
        let mut t = T::new();
        let space = t.code("space");
        assert!(!t.input(InputEvent::KeyDown { code: space, mods: Mods::NONE, repeat: false, at: C }, 0).0);
        assert!(!t.input(InputEvent::KeyDown { code: space, mods: Mods::ALT | Mods::SHIFT, repeat: false, at: C }, 0).0);
        assert!(!t.input(InputEvent::KeyUp { code: space, mods: Mods::NONE }, 0).0);
        let esc = t.code("esc");
        assert!(!t.input(InputEvent::KeyDown { code: esc, mods: Mods::NONE, repeat: false, at: C }, 0).0);
        assert!(!t.input(InputEvent::Down { button: Button::Left, at: C }, 0).0);
    }

    #[test]
    fn submenu_by_crossing_the_ring_and_back_with_esc() {
        let mut t = T::new();
        t.down(0);
        t.resolve("x", false, 2);
        t.mv(at(180.0, 100.0), 50);
        let out = t.mv(at(180.0, 170.0), 80);
        assert_eq!(swapped_to(&out), Some((t.wheel("tools"), 1)));
        // Still beyond the ring, now over "Deep" (at 240° in a 3-item wheel): not opened by crossing
        // until the cursor came back inside the ring first.
        let out = t.mv(at(240.0, 170.0), 100);
        assert_eq!(swapped_to(&out), None);
        t.mv(at(240.0, 100.0), 120);
        let out = t.mv(at(240.0, 170.0), 140);
        assert_eq!(swapped_to(&out), Some((t.wheel("deep"), 2)));

        let esc = t.code("esc");
        let (sw, out) = t.input(InputEvent::KeyDown { code: esc, mods: Mods::NONE, repeat: false, at: C }, 200);
        assert!(sw);
        assert_eq!(swapped_to(&out), Some((t.wheel("tools"), 1)), "Esc in a submenu goes back");
        assert!(t.input(InputEvent::KeyUp { code: esc, mods: Mods::NONE }, 210).0);
        t.input(InputEvent::KeyDown { code: esc, mods: Mods::NONE, repeat: false, at: C }, 220);
        let (_, out) = t.input(InputEvent::KeyDown { code: esc, mods: Mods::NONE, repeat: false, at: C }, 230);
        assert!(matches!(out.as_slice(), [Cmd::Cancel { .. }]), "Esc at the root closes");
    }

    #[test]
    fn submenu_by_dwell_and_back_by_dwelling_in_center() {
        let mut t = T::new();
        t.down(0);
        t.resolve("x", false, 2);
        t.mv(at(180.0, 100.0), 50);
        assert!(t.tick(200).is_empty());
        assert_eq!(swapped_to(&t.tick(400)), Some((t.wheel("tools"), 1)));
        t.mv(at(0.0, 5.0), 450);
        assert!(t.tick(600).is_empty());
        assert_eq!(swapped_to(&t.tick(800)), Some((t.wheel("default"), 0)));
        assert!(t.tick(1500).is_empty(), "only one level per visit to the center");
        // Releasing in the center cancels.
        let (_, out) = t.up(1600);
        assert!(matches!(out.as_slice(), [Cmd::Cancel { .. }]));
    }

    #[test]
    fn releasing_on_a_submenu_opens_it_in_click_mode() {
        let mut t = T::new();
        t.down(0);
        t.resolve("x", false, 2);
        t.mv(at(180.0, 100.0), 50);
        let (_, out) = t.up(250);
        assert_eq!(swapped_to(&out), Some((t.wheel("tools"), 1)));
        assert!(out.iter().any(|c| matches!(c, Cmd::Mode { click: true, .. })));
        assert_eq!(t.c.phase(), Phase::Click);
        t.mv(at(0.0, 100.0), 300);
        let (_, out) = t.input(InputEvent::Down { button: Button::Left, at: C }, 400);
        assert_eq!(exec_label(&out), Some("T1"));
    }

    #[test]
    fn click_in_center_goes_back_then_closes() {
        let mut t = T::new();
        t.down(0);
        t.up(50);
        t.resolve("x", false, 60);
        t.mv(at(180.0, 100.0), 100);
        t.input(InputEvent::Down { button: Button::Left, at: C }, 150);
        t.mv(at(0.0, 5.0), 200);
        let (_, out) = t.input(InputEvent::Down { button: Button::Left, at: C }, 250);
        assert_eq!(swapped_to(&out), Some((t.wheel("default"), 0)));
        let (_, out) = t.input(InputEvent::Down { button: Button::Left, at: C }, 300);
        assert!(matches!(out.as_slice(), [Cmd::Cancel { .. }]));
    }

    #[test]
    fn selection_probe_swaps_only_early_or_before_hover() {
        let mut t = T::new();
        t.down(0);
        let out = t.resolve("com.apple.finder", true, 3);
        assert!(matches!(&out[0], Cmd::Show { disabled, .. } if disabled.iter().all(|d| !d)), "optimistic while probing");
        let out = t.probe(1, &["/tmp/a"], 60);
        assert_eq!(swapped_to(&out), Some((t.wheel("files"), 0)));

        t.input(InputEvent::Down { button: Button::Right, at: C }, 100);
        t.input(InputEvent::Up { button: Button::Right }, 110);
        t.up(120);
        t.down(1000);
        t.resolve("com.apple.finder", true, 1003);
        t.mv(at(90.0, 100.0), 1050);
        let out = t.probe(2, &["/tmp/a"], 1300);
        assert_eq!(swapped_to(&out), None, "late and the user is already pointing at something");
        assert!(t.probe(1, &["/tmp/a"], 1310).is_empty(), "stale sessions are ignored");
    }

    #[test]
    fn availability_updates_when_selection_arrives() {
        let mut t = T::new();
        t.down(0);
        t.resolve("com.apple.TextEdit", false, 3);
        // Not Finder, nothing selected: "Path" (file.copy_path) needs a selection or folder.
        t.mv(at(270.0, 100.0), 50);
        let (_, out) = t.up(300);
        assert!(matches!(out.as_slice(), [Cmd::Cancel { .. }]), "releasing on a greyed-out item does nothing");

        t.down(1000);
        t.resolve("com.apple.finder", true, 1003);
        let out = t.probe(2, &[], 1050);
        assert_eq!(swapped_to(&out), None, "empty selection: stays on the default wheel");
        assert!(out.is_empty(), "cwd is known, so Path stays enabled: {out:?}");
    }

    #[test]
    fn mouse_button_trigger_with_forced_wheel() {
        let mut t = T::new();
        let (sw, out) = t.input(InputEvent::Down { button: Button::Middle, at: C }, 0);
        assert!(sw);
        let tools = t.wheel("tools");
        assert!(matches!(&out[1], Cmd::Show { wheel, .. } if *wheel == tools), "forced wheel shows at once: {out:?}");
        t.mv(at(120.0, 100.0), 100);
        let (sw, out) = t.input(InputEvent::Up { button: Button::Middle }, 400);
        assert!(sw);
        assert_eq!(exec_label(&out), Some("T2"));
        // The front app arriving later doesn't re-show anything.
        assert!(t.resolve("x", false, 410).is_empty());
    }

    #[test]
    fn trigger_again_in_click_mode_closes() {
        let mut t = T::new();
        t.down(0);
        t.up(50);
        t.resolve("x", false, 60);
        let (sw, out) = t.down(500);
        assert!(sw && matches!(out.as_slice(), [Cmd::Cancel { .. }]));
        assert!(t.up(550).0);
        assert_eq!(t.c.phase(), Phase::Idle);
    }
}
