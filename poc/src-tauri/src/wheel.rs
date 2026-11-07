//! Platform-neutral wheel logic: geometry, wheel definitions, and the input state machine.
//! Everything here is pure so it can be unit-tested without a window server.

use serde::Serialize;
use std::f64::consts::{PI, TAU};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Geom {
    pub win: f64,
    pub r_out: f64,
    pub r_in: f64,
    pub r_center: f64,
    pub gap: f64,
    pub dead: f64,
}

pub const GEOM: Geom = Geom { win: 420.0, r_out: 150.0, r_in: 62.0, r_center: 52.0, gap: 3.0, dead: 36.0 };

const HYSTERESIS: f64 = 3.0 * PI / 180.0;
pub const TAP_MAX: Duration = Duration::from_millis(180);
const TAP_MOVE_MAX: f64 = 6.0;
const HOVER_MIN_INTERVAL: Duration = Duration::from_millis(8);
/// A late file-probe result may still swap the wheel only if the user has not started selecting.
pub const SWAP_BUDGET: Duration = Duration::from_millis(80);

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

/// Window frame in global top-left-origin coordinates (points).
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub hz: f64,
}

impl Frame {
    pub fn center(&self) -> Pt {
        Pt { x: self.x + self.w / 2.0, y: self.y + self.h / 2.0 }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn contains(&self, p: Pt) -> bool {
        p.x >= self.x && p.x < self.x + self.w && p.y >= self.y && p.y < self.y + self.h
    }
}

/// A display, in global top-left-origin coordinates (points).
#[derive(Clone, Copy, Debug, Serialize)]
pub struct ScreenInfo {
    pub frame: Rect,
    pub visible: Rect,
    pub hz: f64,
}

/// Centers the wheel on the cursor, pushed back inside the visible area of the cursor's screen.
pub fn place(screens: &[ScreenInfo], pt: Pt) -> Frame {
    let g = GEOM;
    let half = g.win / 2.0;
    let Some(s) = screens.iter().find(|s| s.frame.contains(pt)).or(screens.first()) else {
        return Frame { x: pt.x - half, y: pt.y - half, w: g.win, h: g.win, hz: 60.0 };
    };
    let m = g.r_out + 14.0;
    let v = s.visible;
    let cx = pt.x.clamp(v.x + m, (v.x + v.w - m).max(v.x + m));
    let cy = pt.y.clamp(v.y + m, (v.y + v.h - m).max(v.y + m));
    Frame { x: cx - half, y: cy - half, w: g.win, h: g.win, hz: if s.hz > 0.0 { s.hz } else { 60.0 } }
}

/// Outline of sector `i` of `n` (window coordinates, y-down) with straight-edged gaps between sectors.
/// The native glass mask and the SVG in the overlay use the same formula, so they line up.
pub fn sector_polygon(i: usize, n: usize, steps: usize) -> Vec<Pt> {
    let g = GEOM;
    let c = g.win / 2.0;
    let w = TAU / n as f64;
    let mid = i as f64 * w;
    let at = |r: f64, a: f64| Pt { x: c + r * a.sin(), y: c - r * a.cos() };
    let off = |r: f64| (g.gap / 2.0 / r).asin();
    let mut pts = Vec::with_capacity(2 * steps + 2);
    let (o0, o1) = (mid - w / 2.0 + off(g.r_out), mid + w / 2.0 - off(g.r_out));
    for k in 0..=steps {
        pts.push(at(g.r_out, o0 + (o1 - o0) * k as f64 / steps as f64));
    }
    let (i0, i1) = (mid - w / 2.0 + off(g.r_in), mid + w / 2.0 - off(g.r_in));
    for k in (0..=steps).rev() {
        pts.push(at(g.r_in, i0 + (i1 - i0) * k as f64 / steps as f64));
    }
    pts
}

/// Clockwise angle from 12 o'clock, in [0, 2π), for a y-down vector.
pub fn angle_of(dx: f64, dy: f64) -> f64 {
    let a = dx.atan2(-dy);
    if a < 0.0 { a + TAU } else { a }
}

pub fn sector_at(dx: f64, dy: f64, n: usize, prev: Option<usize>, dead: f64) -> Option<usize> {
    if n == 0 || dx.hypot(dy) < dead {
        return None;
    }
    let a = angle_of(dx, dy);
    let w = TAU / n as f64;
    if let Some(p) = prev.filter(|&p| p < n) {
        let mut d = (a - p as f64 * w).abs() % TAU;
        if d > PI {
            d = TAU - d;
        }
        if d <= w / 2.0 + HYSTERESIS {
            return Some(p);
        }
    }
    Some((((a + w / 2.0) / w).floor() as usize) % n)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WheelId {
    Default,
    Files,
}

#[derive(Clone, Copy, Debug)]
pub enum Action {
    Keys(&'static str),
    Text(&'static str),
    Launch(&'static str),
    Url(&'static str),
    AppleScript(&'static str),
    TerminalHere,
    Cut,
    CopyPath,
    Zip,
    Trash,
    OpenLab,
}

pub struct Item {
    pub label: &'static str,
    pub icon: &'static str,
    pub action: Action,
}

pub struct Wheel {
    pub name: &'static str,
    pub items: &'static [Item],
}

const DEFAULT_ITEMS: &[Item] = &[
    Item { label: "输入文本", icon: "Type", action: Action::Text("Hello from anyTools ✨ ") },
    Item { label: "截图", icon: "Crop", action: Action::Keys("cmd+shift+4") },
    Item { label: "计算器", icon: "Calculator", action: Action::Launch("Calculator") },
    Item { label: "调度中心", icon: "LayoutDashboard", action: Action::Keys("ctrl+up") },
    Item {
        label: "AppleScript 通知",
        icon: "Bell",
        action: Action::AppleScript("display notification \"来自轮盘的 AppleScript 动作\" with title \"anyTools\""),
    },
    Item { label: "打开网址", icon: "Globe", action: Action::Url("https://tauri.app") },
    Item { label: "终端", icon: "SquareTerminal", action: Action::Launch("Terminal") },
    Item { label: "PoC 面板", icon: "AppWindow", action: Action::OpenLab },
];

const FILE_ITEMS: &[Item] = &[
    Item { label: "在此打开终端", icon: "SquareTerminal", action: Action::TerminalHere },
    Item { label: "复制", icon: "Copy", action: Action::Keys("cmd+c") },
    Item { label: "剪切", icon: "Scissors", action: Action::Cut },
    Item { label: "快速查看", icon: "Eye", action: Action::Keys("space") },
    Item { label: "复制路径", icon: "Link", action: Action::CopyPath },
    Item { label: "压缩", icon: "FileArchive", action: Action::Zip },
    Item { label: "新建文件夹", icon: "FolderPlus", action: Action::Keys("cmd+shift+n") },
    Item { label: "移到废纸篓", icon: "Trash2", action: Action::Trash },
];

pub fn wheel(id: WheelId) -> Wheel {
    match id {
        WheelId::Default => Wheel { name: "快捷轮盘", items: DEFAULT_ITEMS },
        WheelId::Files => Wheel { name: "文件", items: FILE_ITEMS },
    }
}

#[derive(Serialize)]
pub struct ItemView {
    pub label: &'static str,
    pub icon: &'static str,
}

#[derive(Serialize)]
pub struct WheelView {
    pub id: WheelId,
    pub name: &'static str,
    pub items: Vec<ItemView>,
}

pub fn wheel_view(id: WheelId) -> WheelView {
    let w = wheel(id);
    WheelView { id, name: w.name, items: w.items.iter().map(|i| ItemView { label: i.label, icon: i.icon }).collect() }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct FileCtx {
    pub dir: Option<String>,
    pub files: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct AppInfo {
    pub bundle: String,
    pub name: String,
    pub pid: i32,
}

#[derive(Clone, Copy, Debug)]
pub enum Input {
    TriggerDown(Pt),
    TriggerRepeat,
    TriggerUp,
    ModRelease,
    EscDown,
    EscUp,
    Move(Pt),
    LeftDown,
    LeftUp,
    RightDown,
    RightUp,
}

#[derive(Debug)]
pub enum Cmd {
    Show { seq: u64, t0: Instant, frame: Frame, wheel: WheelId },
    Hover { seq: u64, idx: Option<usize>, angle: f64, dist: f64 },
    Mode { seq: u64, click: bool },
    Exec { seq: u64, wheel: WheelId, idx: usize, files: Option<FileCtx>, front: Option<AppInfo> },
    Cancel { seq: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Hold,
    Click,
}

pub struct Session {
    pub seq: u64,
    pub t0: Instant,
    pub trigger_pt: Pt,
    pub center: Pt,
    pub wheel: WheelId,
    pub hovered: Option<usize>,
    pub max_move: f64,
    pub files: Option<FileCtx>,
    pub front: Option<AppInfo>,
    last_hover_emit: Option<Instant>,
}

pub struct Controller {
    pub phase: Phase,
    pub seq: u64,
    pub session: Option<Session>,
    trigger_held: bool,
    key_swallowed: bool,
    swallow_esc_up: bool,
    swallow_left_up: bool,
    swallow_right_up: bool,
}

impl Default for Controller {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            seq: 0,
            session: None,
            trigger_held: false,
            key_swallowed: false,
            swallow_esc_up: false,
            swallow_left_up: false,
            swallow_right_up: false,
        }
    }
}

impl Controller {
    /// Returns true when the event must be swallowed. Must stay O(1): it runs inside the input hook.
    pub fn on_input(&mut self, input: Input, now: Instant, placer: &dyn Fn(Pt) -> Frame, out: &mut Vec<Cmd>) -> bool {
        match input {
            Input::TriggerDown(pt) => {
                self.key_swallowed = true;
                if self.phase != Phase::Idle {
                    // Pressing the trigger again while the wheel is open (click mode) closes it.
                    self.trigger_held = false;
                    self.cancel(out);
                    return true;
                }
                self.trigger_held = true;
                self.seq += 1;
                let frame = placer(pt);
                self.session = Some(Session {
                    seq: self.seq,
                    t0: now,
                    trigger_pt: pt,
                    center: frame.center(),
                    wheel: WheelId::Default,
                    hovered: None,
                    max_move: 0.0,
                    files: None,
                    front: None,
                    last_hover_emit: None,
                });
                self.phase = Phase::Hold;
                out.push(Cmd::Show { seq: self.seq, t0: now, frame, wheel: WheelId::Default });
                // The cursor may already sit inside a sector when the window was clamped at a screen edge.
                self.update_hover(pt, now, true, out);
                true
            }
            // Repeats and the key-up of a swallowed trigger key are swallowed even after the hold ended
            // (e.g. ⌥ released first), otherwise the app would receive stray spaces.
            Input::TriggerRepeat => self.key_swallowed,
            Input::TriggerUp => {
                self.release(now, out);
                std::mem::take(&mut self.key_swallowed)
            }
            // The trigger modifier was released. Modifier changes are never swallowed.
            Input::ModRelease => {
                self.release(now, out);
                false
            }
            Input::EscDown => {
                if self.phase == Phase::Idle {
                    return false;
                }
                self.swallow_esc_up = true;
                self.cancel(out);
                true
            }
            Input::EscUp => std::mem::take(&mut self.swallow_esc_up),
            Input::Move(pt) => {
                if self.phase != Phase::Idle {
                    self.update_hover(pt, now, false, out);
                }
                false
            }
            Input::LeftDown => {
                if self.phase == Phase::Idle {
                    return false;
                }
                self.swallow_left_up = true;
                if self.phase == Phase::Click {
                    self.commit(out);
                }
                true
            }
            Input::LeftUp => std::mem::take(&mut self.swallow_left_up),
            Input::RightDown => {
                if self.phase == Phase::Idle {
                    return false;
                }
                self.swallow_right_up = true;
                self.cancel(out);
                true
            }
            Input::RightUp => std::mem::take(&mut self.swallow_right_up),
        }
    }

    fn release(&mut self, now: Instant, out: &mut Vec<Cmd>) {
        if !std::mem::take(&mut self.trigger_held) || self.phase != Phase::Hold {
            return;
        }
        let s = self.session.as_ref().expect("hold phase has a session");
        if now.duration_since(s.t0) < TAP_MAX && s.max_move < TAP_MOVE_MAX {
            self.phase = Phase::Click;
            out.push(Cmd::Mode { seq: s.seq, click: true });
        } else {
            self.commit(out);
        }
    }

    fn update_hover(&mut self, pt: Pt, now: Instant, force: bool, out: &mut Vec<Cmd>) {
        let Some(s) = self.session.as_mut() else { return };
        let (dx, dy) = (pt.x - s.center.x, pt.y - s.center.y);
        s.max_move = s.max_move.max((pt.x - s.trigger_pt.x).hypot(pt.y - s.trigger_pt.y));
        let n = wheel(s.wheel).items.len();
        let idx = sector_at(dx, dy, n, s.hovered, GEOM.dead);
        let due = s.last_hover_emit.map_or(true, |t| now.duration_since(t) >= HOVER_MIN_INTERVAL);
        if force || idx != s.hovered || due {
            s.hovered = idx;
            s.last_hover_emit = Some(now);
            out.push(Cmd::Hover { seq: s.seq, idx, angle: angle_of(dx, dy).to_degrees(), dist: dx.hypot(dy) });
        }
    }

    fn commit(&mut self, out: &mut Vec<Cmd>) {
        let s = self.session.as_ref().expect("commit needs a session");
        match s.hovered {
            Some(idx) => {
                out.push(Cmd::Exec { seq: s.seq, wheel: s.wheel, idx, files: s.files.clone(), front: s.front.clone() });
                self.phase = Phase::Idle;
            }
            None => self.cancel(out),
        }
    }

    fn cancel(&mut self, out: &mut Vec<Cmd>) {
        if let Some(s) = &self.session {
            out.push(Cmd::Cancel { seq: s.seq });
        }
        self.phase = Phase::Idle;
    }

    /// Applies a file-probe result. Returns the wheel to swap to, if any.
    pub fn on_probe(&mut self, seq: u64, ctx: FileCtx, now: Instant) -> Option<WheelId> {
        if self.phase == Phase::Idle {
            return None;
        }
        let s = self.session.as_mut().filter(|s| s.seq == seq)?;
        let has_files = !ctx.files.is_empty();
        s.files = Some(ctx);
        let in_budget = now.duration_since(s.t0) <= SWAP_BUDGET;
        if has_files && s.wheel != WheelId::Files && (in_budget || s.hovered.is_none()) {
            s.wheel = WheelId::Files;
            s.hovered = None;
            return Some(WheelId::Files);
        }
        None
    }

    pub fn set_front(&mut self, seq: u64, app: AppInfo) {
        if let Some(s) = self.session.as_mut().filter(|s| s.seq == seq) {
            s.front = Some(app);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn centered(pt: Pt) -> Frame {
        Frame { x: pt.x - 210.0, y: pt.y - 210.0, w: 420.0, h: 420.0, hz: 60.0 }
    }

    fn clamped(_: Pt) -> Frame {
        Frame { x: 0.0, y: 0.0, w: 420.0, h: 420.0, hz: 60.0 }
    }

    #[test]
    fn sectors_clockwise_from_top() {
        assert_eq!(sector_at(0.0, -100.0, 8, None, 36.0), Some(0));
        assert_eq!(sector_at(100.0, 0.0, 8, None, 36.0), Some(2));
        assert_eq!(sector_at(0.0, 100.0, 8, None, 36.0), Some(4));
        assert_eq!(sector_at(-100.0, 0.0, 8, None, 36.0), Some(6));
        assert_eq!(sector_at(-10.0, -100.0, 8, None, 36.0), Some(0), "just left of top wraps to sector 0");
    }

    #[test]
    fn dead_zone_and_hysteresis() {
        assert_eq!(sector_at(10.0, 10.0, 8, Some(3), 36.0), None);
        // Boundary between 0 and 1 is at 22.5°. At 24° we'd normally be in 1, but hysteresis keeps 0.
        let a = 24f64.to_radians();
        let (dx, dy) = (100.0 * a.sin(), -100.0 * a.cos());
        assert_eq!(sector_at(dx, dy, 8, None, 36.0), Some(1));
        assert_eq!(sector_at(dx, dy, 8, Some(0), 36.0), Some(0));
        let a = 27f64.to_radians();
        assert_eq!(sector_at(100.0 * a.sin(), -100.0 * a.cos(), 8, Some(0), 36.0), Some(1));
    }

    fn run(c: &mut Controller, i: Input, t: Instant) -> (bool, Vec<Cmd>) {
        let mut out = vec![];
        let s = c.on_input(i, t, &centered, &mut out);
        (s, out)
    }

    #[test]
    fn hold_move_release_executes_hovered() {
        let mut c = Controller::default();
        let t = Instant::now();
        let p = Pt { x: 500.0, y: 500.0 };
        assert!(run(&mut c, Input::TriggerDown(p), t).0);
        assert!(run(&mut c, Input::TriggerRepeat, t).0);
        let (sw, out) = run(&mut c, Input::Move(Pt { x: 600.0, y: 500.0 }), t + Duration::from_millis(100));
        assert!(!sw, "mouse moves are never swallowed");
        assert!(matches!(out.last(), Some(Cmd::Hover { idx: Some(2), .. })));
        let (sw, out) = run(&mut c, Input::TriggerUp, t + Duration::from_millis(400));
        assert!(sw);
        assert!(matches!(out.as_slice(), [Cmd::Exec { idx: 2, wheel: WheelId::Default, .. }]));
        assert_eq!(c.phase, Phase::Idle);
    }

    #[test]
    fn release_in_dead_zone_cancels() {
        let mut c = Controller::default();
        let t = Instant::now();
        run(&mut c, Input::TriggerDown(Pt { x: 500.0, y: 500.0 }), t);
        run(&mut c, Input::Move(Pt { x: 510.0, y: 500.0 }), t + Duration::from_millis(50));
        let (_, out) = run(&mut c, Input::TriggerUp, t + Duration::from_millis(400));
        assert!(matches!(out.as_slice(), [Cmd::Cancel { .. }]));
    }

    #[test]
    fn tap_enters_click_mode_then_click_executes() {
        let mut c = Controller::default();
        let t = Instant::now();
        run(&mut c, Input::TriggerDown(Pt { x: 500.0, y: 500.0 }), t);
        let (_, out) = run(&mut c, Input::TriggerUp, t + Duration::from_millis(90));
        assert!(matches!(out.as_slice(), [Cmd::Mode { click: true, .. }]));
        assert_eq!(c.phase, Phase::Click);
        run(&mut c, Input::Move(Pt { x: 500.0, y: 400.0 }), t + Duration::from_millis(300));
        let (sw, out) = run(&mut c, Input::LeftDown, t + Duration::from_millis(400));
        assert!(sw);
        assert!(matches!(out.as_slice(), [Cmd::Exec { idx: 0, .. }]));
        assert!(run(&mut c, Input::LeftUp, t + Duration::from_millis(450)).0, "matching mouse-up is swallowed");
        assert!(!run(&mut c, Input::LeftUp, t + Duration::from_millis(500)).0);
    }

    #[test]
    fn esc_cancels_and_swallows_its_key_up() {
        let mut c = Controller::default();
        let t = Instant::now();
        assert!(!run(&mut c, Input::EscDown, t).0, "Esc passes through when idle");
        assert!(!run(&mut c, Input::EscUp, t).0);
        run(&mut c, Input::TriggerDown(Pt { x: 500.0, y: 500.0 }), t);
        let (sw, out) = run(&mut c, Input::EscDown, t);
        assert!(sw && matches!(out.as_slice(), [Cmd::Cancel { .. }]));
        assert!(run(&mut c, Input::EscUp, t).0);
        assert!(run(&mut c, Input::TriggerUp, t).0, "trigger key-up is still swallowed after cancel");
    }

    #[test]
    fn modifier_released_first_commits_and_swallows_trailing_space() {
        let mut c = Controller::default();
        let t = Instant::now();
        run(&mut c, Input::TriggerDown(Pt { x: 500.0, y: 500.0 }), t);
        run(&mut c, Input::Move(Pt { x: 500.0, y: 600.0 }), t + Duration::from_millis(100));
        let (sw, out) = run(&mut c, Input::ModRelease, t + Duration::from_millis(300));
        assert!(!sw, "modifier changes always pass through");
        assert!(matches!(out.as_slice(), [Cmd::Exec { idx: 4, .. }]));
        assert!(run(&mut c, Input::TriggerRepeat, t + Duration::from_millis(320)).0, "no stray auto-repeat spaces");
        let (sw, out) = run(&mut c, Input::TriggerUp, t + Duration::from_millis(350));
        assert!(sw && out.is_empty());
        assert!(!run(&mut c, Input::TriggerUp, t + Duration::from_millis(400)).0);
    }

    #[test]
    fn plain_space_passes_through() {
        let mut c = Controller::default();
        let t = Instant::now();
        assert!(!run(&mut c, Input::TriggerRepeat, t).0);
        assert!(!run(&mut c, Input::TriggerUp, t).0);
    }

    #[test]
    fn selection_uses_clamped_wheel_center() {
        let mut c = Controller::default();
        let t = Instant::now();
        let mut out = vec![];
        // Trigger near the top-left corner; window is clamped so the wheel center is (210, 210).
        c.on_input(Input::TriggerDown(Pt { x: 5.0, y: 5.0 }), t, &clamped, &mut out);
        assert!(matches!(out.last(), Some(Cmd::Hover { idx: Some(7), .. })), "cursor is up-left of center");
        // Released quickly without moving: that's a tap, not an accidental execute.
        out.clear();
        c.on_input(Input::TriggerUp, t + Duration::from_millis(60), &clamped, &mut out);
        assert!(matches!(out.as_slice(), [Cmd::Mode { click: true, .. }]));
    }

    #[test]
    fn place_clamps_to_cursor_screen() {
        let screens = [
            ScreenInfo {
                frame: Rect { x: 0.0, y: 0.0, w: 1440.0, h: 900.0 },
                visible: Rect { x: 0.0, y: 25.0, w: 1440.0, h: 875.0 },
                hz: 120.0,
            },
            ScreenInfo {
                frame: Rect { x: 1440.0, y: -200.0, w: 1920.0, h: 1080.0 },
                visible: Rect { x: 1440.0, y: -200.0, w: 1920.0, h: 1080.0 },
                hz: 60.0,
            },
        ];
        let f = place(&screens, Pt { x: 700.0, y: 400.0 });
        assert_eq!((f.center().x, f.center().y, f.hz), (700.0, 400.0, 120.0));
        let f = place(&screens, Pt { x: 3.0, y: 30.0 });
        assert_eq!((f.center().x, f.center().y), (164.0, 189.0), "pushed in from the top-left, below the menu bar");
        let f = place(&screens, Pt { x: 3350.0, y: -190.0 });
        assert_eq!((f.center().x, f.center().y, f.hz), (3196.0, -36.0, 60.0), "second screen, above the primary");
    }

    #[test]
    fn probe_swaps_only_in_budget_or_before_hover() {
        let mut c = Controller::default();
        let t = Instant::now();
        run(&mut c, Input::TriggerDown(Pt { x: 500.0, y: 500.0 }), t);
        let ctx = FileCtx { dir: Some("/tmp".into()), files: vec!["/tmp/a".into()] };
        // Late, but nothing hovered yet: swap.
        assert_eq!(c.on_probe(1, ctx.clone(), t + Duration::from_millis(300)), Some(WheelId::Files));

        run(&mut c, Input::EscDown, t);
        run(&mut c, Input::EscUp, t);
        run(&mut c, Input::TriggerUp, t);
        run(&mut c, Input::TriggerDown(Pt { x: 500.0, y: 500.0 }), t);
        run(&mut c, Input::Move(Pt { x: 600.0, y: 500.0 }), t + Duration::from_millis(50));
        // Late and the user is already pointing at a sector: keep the current wheel.
        assert_eq!(c.on_probe(2, ctx.clone(), t + Duration::from_millis(300)), None);
        // Stale sequence numbers are ignored.
        assert_eq!(c.on_probe(1, ctx, t), None);
    }
}
