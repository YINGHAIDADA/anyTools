//! Wheel geometry. The overlay page mirrors `sector_polygon` (ui/overlay/geometry.ts) so the native
//! glass mask and the drawn sectors line up exactly.

use anytools_core::{Frame, Pt, ScreenInfo};
use serde::Serialize;
use serde_json::{json, Value};
use std::f64::consts::{PI, TAU};

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

/// Clockwise angle from 12 o'clock in [0, 2π) for a y-down vector.
pub fn angle_of(dx: f64, dy: f64) -> f64 {
    let a = dx.atan2(-dy);
    if a < 0.0 {
        a + TAU
    } else {
        a
    }
}

/// The sector under `(dx, dy)` (relative to the wheel center), with hysteresis around `prev`.
pub fn sector_at(dx: f64, dy: f64, n: usize, prev: Option<usize>) -> Option<usize> {
    if n == 0 || dx.hypot(dy) < GEOM.dead {
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

/// Centers the wheel on the cursor, pushed inside the visible area of the cursor's screen.
pub fn place(screens: &[ScreenInfo], pt: Pt) -> Frame {
    let half = GEOM.win / 2.0;
    let Some(s) = screens.iter().find(|s| s.frame.contains(pt)).or(screens.first()) else {
        return Frame { x: pt.x - half, y: pt.y - half, w: GEOM.win, h: GEOM.win, hz: 60.0 };
    };
    let m = GEOM.r_out + 14.0;
    let v = s.visible;
    let cx = pt.x.clamp(v.x + m, (v.x + v.w - m).max(v.x + m));
    let cy = pt.y.clamp(v.y + m, (v.y + v.h - m).max(v.y + m));
    Frame { x: cx - half, y: cy - half, w: GEOM.win, h: GEOM.win, hz: if s.hz > 0.0 { s.hz } else { 60.0 } }
}

/// Outline of sector `i` of `n` in window coordinates, with straight-edged gaps.
pub fn sector_polygon(i: usize, n: usize, steps: usize) -> Vec<[f64; 2]> {
    let g = GEOM;
    let c = g.win / 2.0;
    let w = TAU / n as f64;
    let mid = i as f64 * w;
    let at = |r: f64, a: f64| [c + r * a.sin(), c - r * a.cos()];
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

/// Glass mask for an `n`-sector wheel, in the platform overlay's shape format.
pub fn shape(n: usize) -> Value {
    let c = GEOM.win / 2.0;
    let steps = (192 / n.max(1)).clamp(12, 64);
    json!({
        "size": [GEOM.win, GEOM.win],
        "polys": (0..n).map(|i| sector_polygon(i, n, steps)).collect::<Vec<_>>(),
        "circles": [[c, c, GEOM.r_center]],
        // Liquid Glass covers the whole wheel as one disc (it can't take the sector shape).
        "disc": [c, c, GEOM.r_out],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anytools_core::Rect;

    #[test]
    fn sectors_clockwise_from_top() {
        assert_eq!(sector_at(0.0, -100.0, 8, None), Some(0));
        assert_eq!(sector_at(100.0, 0.0, 8, None), Some(2));
        assert_eq!(sector_at(0.0, 100.0, 8, None), Some(4));
        assert_eq!(sector_at(-100.0, 0.0, 8, None), Some(6));
        assert_eq!(sector_at(-10.0, -100.0, 8, None), Some(0));
        assert_eq!(sector_at(100.0, 0.0, 3, None), Some(1));
    }

    #[test]
    fn dead_zone_and_hysteresis() {
        assert_eq!(sector_at(10.0, 10.0, 8, Some(3)), None);
        let at = |deg: f64| {
            let a = deg.to_radians();
            (100.0 * a.sin(), -100.0 * a.cos())
        };
        let (x, y) = at(24.0);
        assert_eq!(sector_at(x, y, 8, None), Some(1));
        assert_eq!(sector_at(x, y, 8, Some(0)), Some(0));
        let (x, y) = at(27.0);
        assert_eq!(sector_at(x, y, 8, Some(0)), Some(1));
    }

    #[test]
    fn place_clamps_to_cursor_screen() {
        let screens = [
            ScreenInfo { frame: Rect { x: 0.0, y: 0.0, w: 1440.0, h: 900.0 }, visible: Rect { x: 0.0, y: 25.0, w: 1440.0, h: 875.0 }, hz: 120.0 },
            ScreenInfo { frame: Rect { x: 1440.0, y: -200.0, w: 1920.0, h: 1080.0 }, visible: Rect { x: 1440.0, y: -200.0, w: 1920.0, h: 1080.0 }, hz: 60.0 },
        ];
        let f = place(&screens, Pt { x: 700.0, y: 400.0 });
        assert_eq!((f.center().x, f.center().y, f.hz), (700.0, 400.0, 120.0));
        let f = place(&screens, Pt { x: 3.0, y: 30.0 });
        assert_eq!((f.center().x, f.center().y), (164.0, 189.0));
        let f = place(&screens, Pt { x: 3350.0, y: -190.0 });
        assert_eq!((f.center().x, f.center().y, f.hz), (3196.0, -36.0, 60.0));
        assert_eq!(place(&[], Pt { x: 10.0, y: 10.0 }).center(), Pt { x: 10.0, y: 10.0 });
    }

    #[test]
    fn shape_has_one_polygon_per_sector() {
        let s = shape(6);
        assert_eq!(s["polys"].as_array().unwrap().len(), 6);
        assert_eq!(s["circles"][0][2], GEOM.r_center);
    }
}
