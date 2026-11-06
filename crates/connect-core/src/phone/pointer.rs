//! The pointer on the phone (the prototype's `point_to`, host/phone.py).
//!
//! iOS takes only relative mouse moves from a Bluetooth mouse, and
//! accelerates them. Measured on the iPhone XR with AssistiveTouch's
//! default speed, moving one axis at a time at 20 units per report every
//! 8 ms: frame pixels = 24 + 1.79 * units from a corner. So the pointer is
//! homed in the nearest corner, moved one axis at a time, and its position
//! is remembered: the next move starts from there, until it is 15 moves
//! old or 5 minutes old, or a tap did not land (then it is forgotten and
//! the next move homes again).

use std::time::Duration;

use serde_json::{json, Value};
use tokio::time::Instant;

use crate::settings::PhoneTuning;

/// The calibration in the units the math uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Calib {
    pub corner: f64,
    pub gain: f64,
    pub step: i64,
    pub delay_ms: i64,
    pub corner_count: i64,
    pub max_hops: u32,
    pub max_age: Duration,
    pub settle: Duration,
}

impl Calib {
    pub fn from_tuning(t: &PhoneTuning) -> Calib {
        let d = PhoneTuning::default();
        let or = |v: u32, def: u32| if v == 0 { def } else { v };
        Calib {
            corner: t.corner_px as f64,
            gain: or(t.gain_milli, d.gain_milli) as f64 / 1000.0,
            step: or(t.step, d.step).min(127) as i64,
            delay_ms: t.delay_ms.min(1000) as i64,
            corner_count: or(t.corner_count, d.corner_count).min(500) as i64,
            max_hops: or(t.max_hops, d.max_hops),
            max_age: Duration::from_secs(or(t.max_age_s, d.max_age_s) as u64),
            settle: Duration::from_millis(t.settle_ms.min(5000) as u64),
        }
    }

    /// Mouse units for a distance in frame pixels.
    pub fn units(&self, px: f64) -> i64 {
        (px / self.gain).round() as i64
    }

    /// A `move` along one line, and how long the board takes for it.
    pub fn move_cmd(&self, dx: i64, dy: i64, step: i64) -> (Value, Duration) {
        let n = (dx.abs().max(dy.abs()) as f64 / step.max(1) as f64) + 1.0;
        let t = BOARD_BASE + Duration::from_secs_f64(n * (self.delay_ms + 10) as f64 / 1000.0);
        (json!({"cmd": "move", "dx": dx, "dy": dy, "step": step, "delay_ms": self.delay_ms}), t)
    }

    /// One axis at a time: a diagonal move gets more acceleration on iOS.
    pub fn moves(&self, dx_px: f64, dy_px: f64) -> Vec<(Value, Duration)> {
        let (ux, uy) = (self.units(dx_px), self.units(dy_px));
        let mut out = Vec::new();
        if ux != 0 {
            out.push(self.move_cmd(ux, 0, self.step));
        }
        if uy != 0 {
            out.push(self.move_cmd(0, uy, self.step));
        }
        out
    }

    fn corner_cmd(&self, which: &str) -> (Value, Duration) {
        let t = BOARD_BASE + Duration::from_secs_f64(self.corner_count as f64 * (self.delay_ms.max(8) + 10) as f64 / 1000.0);
        (json!({"cmd": "corner", "which": which, "count": self.corner_count}), t)
    }
}

/// What every board command may take on top of its own run time.
pub const BOARD_BASE: Duration = Duration::from_secs(5);

/// Where the pointer is believed to be.
#[derive(Debug, Clone, Copy)]
struct Known {
    x: i64,
    y: i64,
    at: Instant,
    hops: u32,
}

/// The remembered pointer.
#[derive(Debug, Default)]
pub struct Pointer {
    known: Option<Known>,
}

/// The board commands that bring the pointer to a point.
#[derive(Debug)]
pub struct Route {
    pub cmds: Vec<(Value, Duration)>,
    /// It went through a corner.
    pub homed: bool,
}

/// The corner nearest to a point: "tl", "tr", "bl" or "br".
pub fn nearest_corner(x: i64, y: i64, w: u32, h: u32) -> &'static str {
    let left = (x as f64) < w as f64 / 2.0;
    let top = (y as f64) < h as f64 / 2.0;
    match (top, left) {
        (true, true) => "tl",
        (true, false) => "tr",
        (false, true) => "bl",
        (false, false) => "br",
    }
}

impl Pointer {
    /// The commands that move the pointer to (x, y) on a screen of w x h
    /// frame pixels, and remember it there. `rehome` goes through a
    /// corner whatever is remembered.
    #[allow(clippy::too_many_arguments)]
    pub fn route(&mut self, x: i64, y: i64, w: u32, h: u32, rehome: bool, now: Instant, c: &Calib) -> Result<Route, String> {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            return Err(format!("bad request: ({x}, {y}) is outside the {w}x{h} screen"));
        }
        let fresh = self
            .known
            .filter(|k| !rehome && now.saturating_duration_since(k.at) < c.max_age && k.hops < c.max_hops);
        let (cmds, hops, homed) = match fresh {
            // Straight from where the pointer is: no trip to a corner.
            Some(k) => (c.moves((x - k.x) as f64, (y - k.y) as f64), k.hops + 1, false),
            None => {
                let which = nearest_corner(x, y, w, h);
                let cx = if which.ends_with('l') { c.corner } else { w as f64 - c.corner };
                let cy = if which.starts_with('t') { c.corner } else { h as f64 - c.corner };
                let mut cmds = vec![c.corner_cmd(which)];
                cmds.extend(c.moves(x as f64 - cx, y as f64 - cy));
                (cmds, 0, true)
            }
        };
        self.known = Some(Known { x, y, at: now, hops });
        Ok(Route { cmds, homed })
    }

    /// Nothing is known any more: the next move homes in a corner.
    pub fn forget(&mut self) {
        self.known = None;
    }

    /// Where the pointer is believed to be.
    pub fn position(&self) -> Option<(i64, i64)> {
        self.known.map(|k| (k.x, k.y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calib() -> Calib {
        Calib::from_tuning(&PhoneTuning::default())
    }

    fn summary(r: &Route) -> Vec<String> {
        r.cmds
            .iter()
            .map(|(v, _)| match v["cmd"].as_str().unwrap() {
                "corner" => format!("corner {} {}", v["which"].as_str().unwrap(), v["count"]),
                "move" => format!("move {} {} step {}", v["dx"], v["dy"], v["step"]),
                other => other.to_string(),
            })
            .collect()
    }

    #[test]
    fn the_prototype_constants_are_the_defaults() {
        let c = calib();
        assert_eq!(c.corner, 24.0);
        assert!((c.gain - 1.79).abs() < 1e-9);
        assert_eq!((c.step, c.delay_ms, c.corner_count, c.max_hops), (20, 8, 15, 15));
        assert_eq!(c.max_age, Duration::from_secs(300));
        assert_eq!(c.settle, Duration::from_millis(250));
        // px = 24 + 1.79 * units
        assert_eq!(c.units(179.0), 100);
        assert_eq!(c.units(-179.0), -100);
        assert_eq!(c.units(0.4), 0);
    }

    #[test]
    fn the_first_move_homes_in_the_nearest_corner_one_axis_at_a_time() {
        let c = calib();
        let mut p = Pointer::default();
        let now = Instant::now();
        // Top left: from (24, 24).
        let r = p.route(400, 800, 828, 1792, false, now, &c).unwrap();
        assert!(r.homed);
        assert_eq!(summary(&r), ["corner tl 15", "move 210 0 step 20", "move 0 434 step 20"]);
        assert_eq!(p.position(), Some((400, 800)));
        // Bottom right: from (828 - 24, 1792 - 24).
        let mut q = Pointer::default();
        let r = q.route(700, 1500, 828, 1792, false, now, &c).unwrap();
        assert_eq!(summary(&r), ["corner br 15", "move -58 0 step 20", "move 0 -150 step 20"]);
        assert_eq!(nearest_corner(600, 100, 828, 1792), "tr");
        assert_eq!(nearest_corner(100, 1700, 828, 1792), "bl");
    }

    #[test]
    fn later_moves_start_from_the_remembered_point() {
        let c = calib();
        let mut p = Pointer::default();
        let now = Instant::now();
        p.route(400, 800, 828, 1792, false, now, &c).unwrap();
        let r = p.route(579, 800, 828, 1792, false, now, &c).unwrap();
        assert!(!r.homed);
        assert_eq!(summary(&r), ["move 100 0 step 20"]);
        // Same point: nothing to send.
        let r = p.route(579, 800, 828, 1792, false, now, &c).unwrap();
        assert!(r.cmds.is_empty());
    }

    #[test]
    fn the_pointer_is_homed_again_after_15_moves_after_5_minutes_and_when_forgotten() {
        let c = calib();
        let mut p = Pointer::default();
        let now = Instant::now();
        assert!(p.route(400, 800, 828, 1792, false, now, &c).unwrap().homed);
        for i in 0..15 {
            let r = p.route(400 + i, 800, 828, 1792, false, now, &c).unwrap();
            assert!(!r.homed, "hop {i}");
        }
        assert!(p.route(400, 800, 828, 1792, false, now, &c).unwrap().homed, "the 16th move homes");

        let later = now + Duration::from_secs(301);
        assert!(p.route(410, 800, 828, 1792, false, later, &c).unwrap().homed, "5 minutes old");

        p.forget();
        assert_eq!(p.position(), None);
        assert!(p.route(410, 800, 828, 1792, false, later, &c).unwrap().homed);
        assert!(p.route(410, 800, 828, 1792, true, later, &c).unwrap().homed, "asked to rehome");
    }

    #[test]
    fn a_point_off_the_screen_is_refused_and_moves_nothing() {
        let c = calib();
        let mut p = Pointer::default();
        let now = Instant::now();
        for (x, y) in [(-1, 10), (828, 10), (10, 1792), (10, -5)] {
            let e = p.route(x, y, 828, 1792, false, now, &c).unwrap_err();
            assert!(e.starts_with("bad request: "), "{e}");
        }
        assert_eq!(p.position(), None);
    }

    #[test]
    fn long_moves_get_time_for_every_report() {
        let c = calib();
        let (_, t) = c.move_cmd(2000, 0, 20);
        // 101 reports at 18 ms each, on top of the base.
        assert!(t > BOARD_BASE + Duration::from_millis(1800), "{t:?}");
        let (v, _) = c.move_cmd(0, -50, 12);
        assert_eq!(v, json!({"cmd": "move", "dx": 0, "dy": -50, "step": 12, "delay_ms": 8}));
    }

    #[test]
    fn a_zero_tuning_field_falls_back_to_the_default() {
        let t = PhoneTuning { gain_milli: 0, step: 0, ..PhoneTuning::default() };
        let c = Calib::from_tuning(&t);
        assert!((c.gain - 1.79).abs() < 1e-9);
        assert_eq!(c.step, 20);
    }
}
