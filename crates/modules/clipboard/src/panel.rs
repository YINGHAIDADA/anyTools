//! The clipboard panel's interaction state machine. Pure: input events in, [`Cmd`]s out.
//!
//! The overlay window never takes focus or mouse events, so everything goes through the global
//! hook: typed characters build the search, arrows / Enter / ⌘1…9 choose, the mouse is hit-tested
//! against the row layout below (which the page mirrors).

use crate::history::{kind, preview, History};
use anytools_core::platform::{Button, InputEvent};
use anytools_core::{Frame, Mods, Pt, ScreenInfo};
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Geom {
    pub w: f64,
    pub pad: f64,
    pub search_h: f64,
    pub row_h: f64,
    pub rows: usize,
    pub foot_h: f64,
    pub radius: f64,
}

pub const GEOM: Geom = Geom { w: 404.0, pad: 8.0, search_h: 48.0, row_h: 42.0, rows: 8, foot_h: 30.0, radius: 24.0 };

impl Geom {
    pub fn h(&self) -> f64 {
        self.pad * 2.0 + self.search_h + self.rows as f64 * self.row_h + self.foot_h
    }
    fn list_top(&self) -> f64 {
        self.pad + self.search_h
    }
}

/// Near the cursor: horizontally centered on it, just below (or above, near the bottom edge), kept
/// inside the visible area of its screen.
pub fn place(screens: &[ScreenInfo], pt: Pt) -> Frame {
    let (w, h) = (GEOM.w, GEOM.h());
    let Some(s) = screens.iter().find(|s| s.frame.contains(pt)).or(screens.first()) else {
        return Frame { x: pt.x - w / 2.0, y: pt.y + 12.0, w, h, hz: 60.0 };
    };
    let v = s.visible;
    let m = 8.0;
    let x = (pt.x - w / 2.0).clamp(v.x + m, (v.x + v.w - w - m).max(v.x + m));
    let below = pt.y + 14.0;
    let y = if below + h <= v.y + v.h - m { below } else { (pt.y - 14.0 - h).max(v.y + m) };
    Frame { x, y, w, h, hz: if s.hz > 0.0 { s.hz } else { 60.0 } }
}

/// Glass for the panel: one rounded rect (both the classic mask and Liquid Glass).
pub fn shape() -> Value {
    let (w, h, r) = (GEOM.w, GEOM.h(), GEOM.radius);
    json!({ "size": [w, h], "rrects": [[0.0, 0.0, w, h, r]], "glass": [0.0, 0.0, w, h, r] })
}

/// Platform key codes the panel reacts to.
#[derive(Clone, Debug, Default)]
pub struct Keys {
    pub up: u32,
    pub down: u32,
    pub enter: u32,
    pub esc: u32,
    pub delete: u32,
    pub forward_delete: u32,
    pub tab: u32,
    pub page_up: u32,
    pub page_down: u32,
    pub home: u32,
    pub end: u32,
    /// `1` … `9`.
    pub digits: [u32; 9],
}

impl Keys {
    pub fn new(code_of: impl Fn(&str) -> Option<u32>) -> Result<Keys, String> {
        let k = |n: &str| code_of(n).ok_or_else(|| format!("无法映射按键 {n}"));
        let mut digits = [0; 9];
        for (i, d) in digits.iter_mut().enumerate() {
            *d = k(&(i + 1).to_string())?;
        }
        Ok(Keys {
            up: k("up")?,
            down: k("down")?,
            enter: k("enter")?,
            esc: k("esc")?,
            delete: k("delete")?,
            forward_delete: k("forwarddelete")?,
            tab: k("tab")?,
            page_up: k("pageup")?,
            page_down: k("pagedown")?,
            home: k("home")?,
            end: k("end")?,
            digits,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    Show { seq: u64, frame: Frame },
    /// The visible state changed: re-render.
    Render { seq: u64 },
    Hide { seq: u64 },
    /// `paste`: also send ⌘V / Ctrl+V (else only put it on the clipboard).
    Use { id: u64, paste: bool },
    Remove { id: u64 },
}

#[derive(Default)]
pub struct Panel {
    seq: u64,
    open: bool,
    frame: Frame,
    query: String,
    results: Vec<u64>,
    sel: usize,
    top: usize,
    /// Trackpad scrolling, in points not yet turned into whole rows.
    scroll_acc: f64,
    /// Swallowed key-downs whose key-up must be swallowed too.
    keys_down: Vec<u32>,
    swallow_up: Option<Button>,
}

impl Panel {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    pub fn open(&mut self, frame: Frame, hist: &History, out: &mut Vec<Cmd>) {
        if self.open {
            return;
        }
        self.seq += 1;
        self.open = true;
        self.frame = frame;
        self.query.clear();
        self.scroll_acc = 0.0;
        self.refilter(hist);
        out.push(Cmd::Show { seq: self.seq, frame });
    }

    pub fn close(&mut self, out: &mut Vec<Cmd>) {
        if self.open {
            self.open = false;
            out.push(Cmd::Hide { seq: self.seq });
        }
    }

    /// Re-runs the search (history changed, or the query did). Keeps the selection on the same entry
    /// when it is still there.
    pub fn refilter(&mut self, hist: &History) {
        let keep = self.results.get(self.sel).copied();
        self.results = hist.search(&self.query);
        self.sel = keep.and_then(|id| self.results.iter().position(|r| *r == id)).unwrap_or(0);
        self.top = self.top.min(self.results.len().saturating_sub(GEOM.rows));
        self.reveal();
    }

    fn reset_query(&mut self, q: String, hist: &History) {
        self.query = q;
        self.results = hist.search(&self.query);
        self.sel = 0;
        self.top = 0;
    }

    fn reveal(&mut self) {
        if self.sel < self.top {
            self.top = self.sel;
        } else if self.sel >= self.top + GEOM.rows {
            self.top = self.sel + 1 - GEOM.rows;
        }
    }

    fn select(&mut self, i: usize) -> bool {
        let i = i.min(self.results.len().saturating_sub(1));
        let changed = i != self.sel;
        self.sel = i;
        self.reveal();
        changed
    }

    fn local(&self, p: Pt) -> Option<(f64, f64)> {
        let (x, y) = (p.x - self.frame.x, p.y - self.frame.y);
        (x >= 0.0 && y >= 0.0 && x < self.frame.w && y < self.frame.h).then_some((x, y))
    }

    /// The result index of the row under `p`.
    fn row_at(&self, p: Pt) -> Option<usize> {
        let (x, y) = self.local(p)?;
        let g = GEOM;
        let dy = y - g.list_top();
        if x < g.pad || x >= g.w - g.pad || dy < 0.0 || dy >= g.rows as f64 * g.row_h {
            return None;
        }
        let i = self.top + (dy / g.row_h) as usize;
        (i < self.results.len()).then_some(i)
    }

    fn use_sel(&mut self, i: usize, paste: bool, out: &mut Vec<Cmd>) {
        if let Some(&id) = self.results.get(i) {
            self.close(out);
            out.push(Cmd::Use { id, paste });
        }
    }

    /// Returns whether to swallow the event. `trigger`: the key that opens the panel.
    pub fn on_input(&mut self, ev: &InputEvent, trigger: Option<(u32, Mods)>, keys: &Keys, hist: &History, place: &dyn Fn(Pt) -> Frame, out: &mut Vec<Cmd>) -> bool {
        match *ev {
            InputEvent::KeyDown { code, mods, repeat, at, ch } => {
                let is_trigger = trigger == Some((code, mods));
                if !self.open {
                    if !is_trigger {
                        return false;
                    }
                    if !repeat {
                        self.open(place(at), hist, out);
                    }
                } else if is_trigger {
                    if !repeat {
                        self.close(out);
                    }
                } else if !self.key(code, mods, ch, keys, hist, out) {
                    // A shortcut meant for the app (⌘Tab, ⌘Q, …): close and let it through.
                    self.close(out);
                    return false;
                }
                if !self.keys_down.contains(&code) {
                    self.keys_down.push(code);
                }
                true
            }
            InputEvent::KeyUp { code, .. } => match self.keys_down.iter().position(|c| *c == code) {
                Some(i) => {
                    self.keys_down.swap_remove(i);
                    true
                }
                None => false,
            },
            InputEvent::Move { at } if self.open => {
                if let Some(i) = self.row_at(at) {
                    if self.select(i) {
                        out.push(Cmd::Render { seq: self.seq });
                    }
                }
                false
            }
            InputEvent::Down { button, at } if self.open => {
                if self.local(at).is_none() {
                    // Clicking elsewhere closes the panel; the click itself goes through.
                    self.close(out);
                    return false;
                }
                self.swallow_up = Some(button);
                if button == Button::Left {
                    if let Some(i) = self.row_at(at) {
                        self.use_sel(i, true, out);
                    }
                }
                true
            }
            InputEvent::Up { button } => {
                if self.swallow_up == Some(button) {
                    self.swallow_up = None;
                    return true;
                }
                false
            }
            InputEvent::Scroll { dy, precise, at } if self.open => {
                if self.local(at).is_none() {
                    return false;
                }
                let rows = if precise {
                    self.scroll_acc += dy;
                    let n = (self.scroll_acc / GEOM.row_h).trunc();
                    self.scroll_acc -= n * GEOM.row_h;
                    n as i64
                } else {
                    dy.signum() as i64 * (dy.abs().round() as i64).max(1)
                };
                // Positive dy scrolls toward the top of the list.
                let max_top = self.results.len().saturating_sub(GEOM.rows) as i64;
                let top = (self.top as i64 - rows).clamp(0, max_top) as usize;
                if top != self.top {
                    self.top = top;
                    self.sel = self.sel.clamp(top, top + GEOM.rows - 1);
                    out.push(Cmd::Render { seq: self.seq });
                }
                true
            }
            _ => false,
        }
    }

    /// A key press while open. Returns false if the panel doesn't use it.
    fn key(&mut self, code: u32, mods: Mods, ch: Option<char>, keys: &Keys, hist: &History, out: &mut Vec<Cmd>) -> bool {
        let cmd = mods.contains(Mods::CMD) || mods.contains(Mods::CTRL);
        let n = self.results.len();
        let render = |p: &mut Panel, out: &mut Vec<Cmd>| out.push(Cmd::Render { seq: p.seq });
        if code == keys.esc {
            if self.query.is_empty() {
                self.close(out);
            } else {
                self.reset_query(String::new(), hist);
                render(self, out);
            }
        } else if code == keys.enter {
            let sel = self.sel;
            self.use_sel(sel, !mods.contains(Mods::SHIFT), out);
        } else if code == keys.up || code == keys.down || code == keys.page_up || code == keys.page_down || code == keys.home || code == keys.end {
            let page = GEOM.rows - 1;
            let to = match code {
                c if c == keys.up && cmd => 0,
                c if c == keys.down && cmd => n,
                c if c == keys.up => self.sel.saturating_sub(1),
                c if c == keys.down => self.sel + 1,
                c if c == keys.page_up => self.sel.saturating_sub(page),
                c if c == keys.page_down => self.sel + page,
                c if c == keys.home => 0,
                _ => n,
            };
            if self.select(to) {
                render(self, out);
            }
        } else if cmd && keys.digits.contains(&code) {
            let d = keys.digits.iter().position(|c| *c == code).unwrap_or(0);
            self.use_sel(self.top + d, true, out);
        } else if (code == keys.delete || code == keys.forward_delete) && cmd {
            if let Some(&id) = self.results.get(self.sel) {
                out.push(Cmd::Remove { id });
            }
        } else if code == keys.delete {
            if !self.query.is_empty() {
                let q = if mods.contains(Mods::ALT) {
                    // ⌥⌫: delete the last word.
                    let t = self.query.trim_end();
                    t[..t.rfind(char::is_whitespace).map(|i| i + 1).unwrap_or(0)].to_string()
                } else {
                    let mut q = self.query.clone();
                    q.pop();
                    q
                };
                self.reset_query(q, hist);
                render(self, out);
            }
        } else if cmd {
            return false;
        } else if code == keys.tab {
            // Nothing to move focus to.
        } else if let Some(c) = ch {
            let mut q = self.query.clone();
            q.push(c);
            self.reset_query(q, hist);
            render(self, out);
        }
        true
    }

    /// What the page draws: the query, counts, and the visible rows.
    pub fn view(&self, hist: &History, now_ms: i64) -> Value {
        let rows: Vec<Value> = self
            .results
            .iter()
            .enumerate()
            .skip(self.top)
            .take(GEOM.rows)
            .filter_map(|(i, id)| {
                let e = hist.get(*id)?;
                Some(json!({
                    "index": i,
                    "id": e.id,
                    "text": preview(&e.text, 160),
                    "kind": kind(&e.text),
                    "lines": e.text.trim().lines().count(),
                    "chars": e.text.chars().count(),
                    "app": e.app,
                    "age_ms": (now_ms - e.ts).max(0),
                }))
            })
            .collect();
        json!({
            "seq": self.seq,
            "geom": GEOM,
            "query": self.query,
            "total": hist.len(),
            "matches": self.results.len(),
            "top": self.top,
            "sel": self.sel,
            "rows": rows,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anytools_core::Rect;

    struct T {
        p: Panel,
        h: History,
        keys: Keys,
        trig: (u32, Mods),
    }

    const V: u32 = 9;
    const AT: Pt = Pt { x: 500.0, y: 300.0 };

    fn frame_at(p: Pt) -> Frame {
        Frame { x: p.x - GEOM.w / 2.0, y: p.y + 14.0, w: GEOM.w, h: GEOM.h(), hz: 60.0 }
    }

    impl T {
        fn new(texts: &[&str]) -> T {
            let mut h = History::new(100);
            for (i, t) in texts.iter().enumerate() {
                h.add(t.to_string(), None, i as i64);
            }
            let keys = Keys::new(|n| {
                Some(match n {
                    "up" => 126,
                    "down" => 125,
                    "enter" => 36,
                    "esc" => 53,
                    "delete" => 51,
                    "forwarddelete" => 117,
                    "tab" => 48,
                    "pageup" => 116,
                    "pagedown" => 121,
                    "home" => 115,
                    "end" => 119,
                    d => 18 + d.parse::<u32>().ok()?,
                })
            })
            .unwrap();
            T { p: Panel::default(), h, keys, trig: (V, Mods::CMD | Mods::SHIFT) }
        }
        fn ev(&mut self, ev: InputEvent) -> (bool, Vec<Cmd>) {
            let mut out = vec![];
            let sw = self.p.on_input(&ev, Some(self.trig), &self.keys, &self.h, &frame_at, &mut out);
            (sw, out)
        }
        fn key(&mut self, code: u32, mods: Mods, ch: Option<char>) -> (bool, Vec<Cmd>) {
            let r = self.ev(InputEvent::KeyDown { code, mods, repeat: false, at: AT, ch });
            let up = self.ev(InputEvent::KeyUp { code, mods });
            assert_eq!(up.0, r.0, "key-up swallowed iff key-down was");
            r
        }
        fn open(&mut self) {
            let (sw, out) = self.key(V, self.trig.1, Some('v'));
            assert!(sw);
            assert!(matches!(out.as_slice(), [Cmd::Show { seq: 1, .. }]), "{out:?}");
        }
        fn typed(&mut self, s: &str) {
            for c in s.chars() {
                assert!(self.key(0, Mods::NONE, Some(c)).0);
            }
        }
        fn sel_text(&self) -> String {
            self.h.get(self.p.results[self.p.sel]).unwrap().text.clone()
        }
        fn row_pt(&self, visible: usize) -> Pt {
            let f = frame_at(AT);
            Pt { x: f.x + 40.0, y: f.y + GEOM.list_top() + GEOM.row_h * (visible as f64 + 0.5) }
        }
    }

    #[test]
    fn opens_types_and_pastes_the_match() {
        let mut t = T::new(&["alpha", "beta", "gamma"]);
        assert!(!t.key(V, Mods::CMD, None).0, "other chords pass through");
        t.open();
        assert_eq!(t.sel_text(), "gamma", "newest first");
        t.typed("Al");
        assert_eq!(t.p.results.len(), 1);
        assert_eq!(t.p.view(&t.h, 0)["query"], "Al");
        let (_, out) = t.key(t.keys.enter, Mods::NONE, None);
        let id = t.h.search("alpha")[0];
        assert_eq!(out, [Cmd::Hide { seq: 1 }, Cmd::Use { id, paste: true }]);
        assert!(!t.p.is_open());
        assert!(!t.key(0, Mods::NONE, Some('x')).0, "closed: typing goes to the app");
    }

    #[test]
    fn navigation_and_editing() {
        let mut t = T::new(&["a1", "a2", "a3", "b"]);
        t.open();
        t.key(t.keys.down, Mods::NONE, None);
        t.key(t.keys.down, Mods::NONE, None);
        assert_eq!(t.sel_text(), "a2");
        t.key(t.keys.up, Mods::CMD, None);
        assert_eq!(t.sel_text(), "b");
        t.typed("a 9");
        assert!(t.p.results.is_empty());
        t.key(t.keys.delete, Mods::NONE, None);
        assert_eq!(t.p.results.len(), 3, "backspace widens the search again");
        t.key(t.keys.delete, Mods::ALT, None);
        assert_eq!(t.p.query, "");
        t.typed("zz");
        let (_, out) = t.key(t.keys.esc, Mods::NONE, None);
        assert!(t.p.is_open() && t.p.query.is_empty(), "Esc clears the search first");
        assert!(matches!(out.as_slice(), [Cmd::Render { .. }]));
        let (_, out) = t.key(t.keys.esc, Mods::NONE, None);
        assert_eq!(out, [Cmd::Hide { seq: 1 }]);
    }

    #[test]
    fn shortcuts() {
        let mut t = T::new(&["x", "y", "z"]);
        t.open();
        let (_, out) = t.key(t.keys.digits[1], Mods::CMD, None);
        assert_eq!(out[1], Cmd::Use { id: t.h.search("y")[0], paste: true }, "⌘2 = second row");
        t.open_again();
        let (_, out) = t.key(t.keys.enter, Mods::SHIFT, None);
        assert!(matches!(out[1], Cmd::Use { paste: false, .. }), "⇧↩ copies only");
        t.open_again();
        let (_, out) = t.key(t.keys.delete, Mods::CMD, None);
        assert!(matches!(out.as_slice(), [Cmd::Remove { .. }]));
        assert!(t.p.is_open());
        // An app shortcut closes the panel and goes through.
        let (sw, out) = t.key(12, Mods::CMD, Some('q'));
        assert!(!sw && matches!(out.as_slice(), [Cmd::Hide { .. }]));
        // The trigger toggles.
        t.open_again();
        let (sw, out) = t.key(V, t.trig.1, Some('v'));
        assert!(sw && matches!(out.as_slice(), [Cmd::Hide { .. }]));
    }

    impl T {
        fn open_again(&mut self) {
            let (sw, out) = self.key(V, self.trig.1, Some('v'));
            assert!(sw && matches!(out.as_slice(), [Cmd::Show { .. }]), "{out:?}");
        }
    }

    #[test]
    fn mouse_hover_click_scroll() {
        let texts: Vec<String> = (0..20).map(|i| format!("item {i}")).collect();
        let mut t = T::new(&texts.iter().map(String::as_str).collect::<Vec<_>>());
        t.open();
        let (sw, out) = t.ev(InputEvent::Move { at: t.row_pt(3) });
        assert!(!sw);
        assert_eq!(out, [Cmd::Render { seq: 1 }]);
        assert_eq!(t.sel_text(), "item 16");
        // Mouse wheel down two notches: the list scrolls, the selection stays visible.
        let (sw, _) = t.ev(InputEvent::Scroll { dy: -2.0, precise: false, at: t.row_pt(0) });
        assert!(sw);
        assert_eq!(t.p.top, 2);
        // Trackpad: points accumulate into rows.
        t.ev(InputEvent::Scroll { dy: 30.0, precise: true, at: t.row_pt(0) });
        assert_eq!(t.p.top, 2);
        t.ev(InputEvent::Scroll { dy: 30.0, precise: true, at: t.row_pt(0) });
        assert_eq!(t.p.top, 1);
        let (sw, out) = t.ev(InputEvent::Down { button: Button::Left, at: t.row_pt(0) });
        assert!(sw);
        assert_eq!(out[1], Cmd::Use { id: t.h.search("item 18")[0], paste: true });
        assert!(t.ev(InputEvent::Up { button: Button::Left }).0, "its mouse-up too");

        t.open_again();
        let outside = Pt { x: 10.0, y: 10.0 };
        let (sw, out) = t.ev(InputEvent::Down { button: Button::Left, at: outside });
        assert!(!sw && out == [Cmd::Hide { seq: 2 }], "click outside closes and goes through");
        assert!(!t.ev(InputEvent::Up { button: Button::Left }).0);
    }

    #[test]
    fn history_change_keeps_the_selection() {
        let mut t = T::new(&["a", "b", "c"]);
        t.open();
        t.key(t.keys.down, Mods::NONE, None);
        assert_eq!(t.sel_text(), "b");
        t.h.add("d".into(), None, 9);
        t.p.refilter(&t.h);
        assert_eq!(t.sel_text(), "b");
        assert_eq!(t.p.results.len(), 4);
    }

    #[test]
    fn placement_stays_on_screen() {
        let r = Rect { x: 0.0, y: 0.0, w: 1440.0, h: 900.0 };
        let screens = [ScreenInfo { frame: r, visible: Rect { y: 25.0, h: 875.0, ..r }, hz: 120.0 }];
        let f = place(&screens, Pt { x: 700.0, y: 200.0 });
        assert_eq!((f.x, f.y), (700.0 - GEOM.w / 2.0, 214.0));
        let f = place(&screens, Pt { x: 5.0, y: 880.0 });
        assert_eq!(f.x, 8.0);
        assert!(f.y + f.h <= 880.0, "above the cursor near the bottom");
        assert_eq!(f.hz, 120.0);
    }
}
