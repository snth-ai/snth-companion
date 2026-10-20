//! The owner's usual manner of speaking: a running average (and spread)
//! of each feature over past holds, kept in the app's data folder on this
//! computer only (`island-tone.json`). "Faster than usual" means faster
//! than this.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::prosody::Features;

/// The weight of a new hold once there are enough: about the last ten
/// holds count. Before that every hold counts the same (a plain mean).
const ALPHA: f32 = 0.1;
/// Holds needed before anything is called usual or unusual.
pub const MIN_HOLDS: u32 = 5;

/// One feature's running mean and variance.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Stat {
    pub n: u32,
    pub mean: f32,
    pub var: f32,
}

impl Stat {
    fn add(&mut self, x: f32) {
        if !x.is_finite() {
            return;
        }
        self.n = self.n.saturating_add(1);
        let a = (1.0 / self.n as f32).max(ALPHA);
        let diff = x - self.mean;
        self.mean += a * diff;
        // The exponentially weighted variance (West, 1979); exact for the
        // plain mean of the first holds.
        self.var = (1.0 - a) * (self.var + a * diff * diff);
    }

    /// How unusual `x` is, in spreads, with the spread never taken below
    /// `floor` (a very steady owner is not "unusual" over a trifle).
    /// None until there are `MIN_HOLDS`.
    pub fn z(&self, x: f32, floor: f32) -> Option<f32> {
        (self.n >= MIN_HOLDS).then(|| (x - self.mean) / self.var.sqrt().max(floor))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    /// Holds learned from.
    pub holds: u32,
    pub rate: Stat,
    pub pause_ratio: Stat,
    pub loud_db: Stat,
    /// Pitch in semitones over 100 Hz (so the spread is in semitones).
    pub f0_st: Stat,
    pub f0_range: Stat,
}

pub fn semitones(hz: f32) -> f32 {
    12.0 * (hz / 100.0).log2()
}

impl Baseline {
    /// Learn from one hold (nothing from one with too little speech).
    pub fn add(&mut self, f: &Features) {
        if !f.judgeable() {
            return;
        }
        self.holds = self.holds.saturating_add(1);
        if let Some(r) = f.rate {
            self.rate.add(r);
        }
        self.pause_ratio.add(f.pause_ratio);
        self.loud_db.add(f.loud_db);
        if let Some(hz) = f.f0 {
            self.f0_st.add(semitones(hz));
        }
        if let Some(r) = f.f0_range {
            self.f0_range.add(r);
        }
    }

    /// The saved baseline; a fresh one when there is none or it is
    /// unreadable.
    pub fn load(path: &Path) -> Baseline {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hold(rate: f32, loud: f32, f0: f32) -> Features {
        Features { secs: 4.0, speech_secs: 3.0, rate: Some(rate), loud_db: loud, f0: Some(f0), f0_range: Some(5.0), pause_ratio: 0.1, ..Features::default() }
    }

    #[test]
    fn the_first_holds_are_a_plain_mean() {
        let mut s = Stat::default();
        for x in [4.0, 6.0, 5.0] {
            s.add(x);
        }
        assert!((s.mean - 5.0).abs() < 1e-5);
        assert!((s.var - 2.0 / 3.0).abs() < 1e-5, "population variance: {s:?}");
        assert_eq!(s.z(9.0, 0.1), None, "too few holds to call anything unusual");
        s.add(5.0);
        s.add(5.0);
        assert!(s.z(9.0, 0.1).unwrap() > 3.0);
        // The floor keeps a tiny spread from blowing a trifle up.
        assert!(s.z(5.3, 1.0).unwrap() < 0.5);
    }

    #[test]
    fn later_holds_move_the_baseline_slowly() {
        let mut s = Stat::default();
        for _ in 0..30 {
            s.add(5.0);
        }
        s.add(8.0);
        assert!((s.mean - 5.3).abs() < 1e-4, "{s:?}");
        s.add(f32::NAN);
        assert!(s.mean.is_finite());
    }

    #[test]
    fn short_holds_teach_nothing_and_the_file_round_trips() {
        let mut b = Baseline::default();
        b.add(&Features { speech_secs: 0.4, ..hold(5.0, -25.0, 140.0) });
        assert_eq!(b.holds, 0);
        for i in 0..6 {
            b.add(&hold(5.0 + i as f32 * 0.1, -25.0, 140.0));
        }
        assert_eq!(b.holds, 6);
        assert!((b.f0_st.mean - semitones(140.0)).abs() < 1e-4);
        let dir = std::env::temp_dir().join(format!("snth-tone-baseline-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("island-tone.json");
        b.save(&p).unwrap();
        assert_eq!(Baseline::load(&p), b);
        std::fs::write(&p, b"not json").unwrap();
        assert_eq!(Baseline::load(&p), Baseline::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
