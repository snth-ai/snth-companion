//! Did the screen change? Two small gray frames compared pixel by pixel
//! (the prototype's `changed()` and `wait_stable`, host/phone_task.py and
//! host/phone.py).
//!
//! The frames are the phone's screen scaled so the long edge is
//! `GRAY_EDGE` pixels (207x448 on an iPhone XR), one byte of brightness per
//! pixel, rows top to bottom.

/// The long edge of the frames compared here.
pub const GRAY_EDGE: u32 = 448;
/// A pixel counts as changed when its brightness moved by more than this.
pub const PIXEL_LEVEL: u8 = 25;
/// Around the click, this many pixels (of the small frame) are not
/// counted: the pointer itself reacts to a click even when the app does
/// not.
pub const POINTER_RADIUS: f64 = 14.0;
/// Fewer changed pixels than this = the tap did nothing.
pub const MIN_CHANGED: usize = 12;
/// Two frames whose mean difference is below this many brightness levels
/// show a screen at rest.
pub const STILL_LEVEL: f64 = 0.6;

/// A gray frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gray {
    pub w: u32,
    pub h: u32,
    /// `w * h` bytes, rows top to bottom.
    pub px: Vec<u8>,
}

impl Gray {
    pub fn new(w: u32, h: u32, px: Vec<u8>) -> Result<Gray, String> {
        if px.len() != (w as usize) * (h as usize) || w == 0 || h == 0 {
            return Err(format!("a gray frame of {w}x{h} needs {} bytes, got {}", w as usize * h as usize, px.len()));
        }
        Ok(Gray { w, h, px })
    }

    /// One brightness everywhere (tests).
    pub fn flat(w: u32, h: u32, level: u8) -> Gray {
        Gray { w, h, px: vec![level; (w * h) as usize] }
    }
}

/// The mean absolute difference of two frames, in brightness levels.
/// Frames of different sizes (the phone turned) differ completely.
pub fn mean_diff(a: &Gray, b: &Gray) -> f64 {
    if a.w != b.w || a.h != b.h || a.px.is_empty() {
        return f64::INFINITY;
    }
    let sum: u64 = a.px.iter().zip(&b.px).map(|(x, y)| x.abs_diff(*y) as u64).sum();
    sum as f64 / a.px.len() as f64
}

/// How many pixels moved by more than `PIXEL_LEVEL`, leaving out a circle
/// of `POINTER_RADIUS` around `at` (a point in frame pixels of the
/// native size `native`). Frames of different sizes count as all changed.
pub fn changed(a: &Gray, b: &Gray, at: Option<(f64, f64)>, native: (u32, u32)) -> usize {
    if a.w != b.w || a.h != b.h {
        return usize::MAX;
    }
    let skip = at.filter(|_| native.0 > 0 && native.1 > 0).map(|(x, y)| {
        (x * a.w as f64 / native.0 as f64, y * a.h as f64 / native.1 as f64)
    });
    let r2 = POINTER_RADIUS * POINTER_RADIUS;
    let mut n = 0;
    for row in 0..a.h {
        for col in 0..a.w {
            let i = (row * a.w + col) as usize;
            if a.px[i].abs_diff(b.px[i]) <= PIXEL_LEVEL {
                continue;
            }
            if let Some((sx, sy)) = skip {
                let (dx, dy) = (col as f64 + 0.5 - sx, row as f64 + 0.5 - sy);
                if dx * dx + dy * dy <= r2 {
                    continue;
                }
            }
            n += 1;
        }
    }
    n
}

/// The size of the gray frame for a screen of `w`x`h`: the long edge
/// `GRAY_EDGE` (never larger than the screen).
pub fn gray_size(w: u32, h: u32) -> (u32, u32) {
    let long = w.max(h).max(1);
    if long <= GRAY_EDGE {
        return (w.max(1), h.max(1));
    }
    let s = GRAY_EDGE as f64 / long as f64;
    (((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame with a block of `level` at (x, y, w, h) on black.
    fn with_block(x: u32, y: u32, bw: u32, bh: u32, level: u8) -> Gray {
        let mut g = Gray::flat(207, 448, 0);
        for row in y..y + bh {
            for col in x..x + bw {
                g.px[(row * 207 + col) as usize] = level;
            }
        }
        g
    }

    #[test]
    fn the_xr_screen_compares_at_207_by_448() {
        assert_eq!(gray_size(828, 1792), (207, 448));
        assert_eq!(gray_size(1792, 828), (448, 207));
        assert_eq!(gray_size(300, 200), (300, 200));
    }

    #[test]
    fn small_brightness_changes_are_not_a_change() {
        let a = Gray::flat(207, 448, 100);
        let b = Gray::flat(207, 448, 125);
        assert_eq!(changed(&a, &b, None, (828, 1792)), 0, "25 levels is not more than 25");
        let c = Gray::flat(207, 448, 126);
        assert_eq!(changed(&a, &c, None, (828, 1792)), 207 * 448);
    }

    #[test]
    fn the_pointer_spot_is_not_counted() {
        let a = Gray::flat(207, 448, 0);
        // A 10x10 change right under a click at native (400, 800) =
        // small (100, 200).
        let b = with_block(95, 195, 10, 10, 255);
        assert_eq!(changed(&a, &b, Some((400.0, 800.0)), (828, 1792)), 0);
        assert_eq!(changed(&a, &b, None, (828, 1792)), 100);
        // Elsewhere it counts.
        assert_eq!(changed(&a, &b, Some((40.0, 80.0)), (828, 1792)), 100);
        // At the edge of the circle: a block 20 px away is outside it.
        let c = with_block(120, 200, 3, 3, 255);
        assert_eq!(changed(&a, &c, Some((400.0, 800.0)), (828, 1792)), 9);
    }

    #[test]
    fn a_tap_that_did_nothing_is_under_the_threshold() {
        let a = Gray::flat(207, 448, 0);
        let b = with_block(10, 10, 3, 3, 255);
        assert!(changed(&a, &b, None, (828, 1792)) < MIN_CHANGED);
        let c = with_block(10, 10, 4, 4, 255);
        assert!(changed(&a, &c, None, (828, 1792)) >= MIN_CHANGED);
    }

    #[test]
    fn mean_difference_tells_a_screen_at_rest() {
        let a = Gray::flat(207, 448, 50);
        assert_eq!(mean_diff(&a, &a), 0.0);
        let b = with_block(0, 0, 10, 10, 255);
        let still = Gray::flat(207, 448, 0);
        assert!(mean_diff(&still, &b) < STILL_LEVEL, "a 10x10 blink is rest");
        let moving = with_block(0, 0, 100, 100, 255);
        assert!(mean_diff(&still, &moving) > STILL_LEVEL);
        let turned = Gray::flat(448, 207, 0);
        assert!(mean_diff(&still, &turned).is_infinite());
        assert_eq!(changed(&still, &turned, None, (828, 1792)), usize::MAX);
    }

    #[test]
    fn a_frame_must_have_its_bytes() {
        assert!(Gray::new(2, 2, vec![0; 4]).is_ok());
        assert!(Gray::new(2, 2, vec![0; 3]).is_err());
        assert!(Gray::new(0, 2, vec![]).is_err());
    }
}
