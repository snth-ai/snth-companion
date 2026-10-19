//! Where the island goes: pure arithmetic over what the system says about
//! the screen, so it can be tested without one.
//!
//! Coordinates are logical points with the origin at the top-left of the
//! primary screen and y growing downwards (the webview's own sense). The
//! macOS panel code turns AppKit's bottom-left rectangles into this and
//! back (`panel.rs`).
//!
//! The island is one transparent window at the top centre of the screen,
//! big enough for the expanded island; the webview draws the black shape
//! inside it and tells the shell which part of it takes clicks. With a
//! notch the window is centred on the notch and the collapsed shape is the
//! notch plus two "ears" (the face lives in the left one, the status in the
//! right one), so the black merges with the notch. Without a notch the
//! same shape is drawn as if there were one: a notch-sized pill flush with
//! the top edge, as tall as the menu bar.

use serde::{Deserialize, Serialize};

/// The island's window, in logical points. Wide and tall enough for the
/// expanded island (the face, a line of text, two buttons, a summary).
pub const WINDOW_W: f64 = 480.0;
pub const WINDOW_H: f64 = 300.0;

/// Each ear beside the notch: room for the face on one side and the
/// status on the other.
pub const EAR_W: f64 = 64.0;

/// The notch a Mac without one is drawn as if it had: about the notch of
/// a 14" MacBook Pro.
pub const VIRTUAL_NOTCH_W: f64 = 184.0;
/// The shortest island: below this a face cannot be drawn.
pub const MIN_BAR_H: f64 = 24.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// What the system says about the screen the island goes on.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenInfo {
    /// The whole screen (top-left global points).
    pub frame: Rect,
    /// macOS: the notch as the two areas beside it leave it, as x
    /// coordinates (the right edge of the left area, the left edge of the
    /// right area) and its height (`safeAreaInsets.top`). None: no notch.
    pub notch: Option<(f64, f64, f64)>,
    /// The menu bar's height (macOS), or 0.
    pub bar_h: f64,
}

/// Where everything is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Placement {
    /// The window (top-left global points).
    pub window: Rect,
    /// The notch inside the window, whether real or drawn as if it were.
    pub notch: Rect,
    /// The collapsed island inside the window: the notch with its ears.
    pub pill: Rect,
    /// A real notch (the shape merges with it) or a drawn one.
    pub real_notch: bool,
}

pub fn place(screen: &ScreenInfo) -> Placement {
    let f = screen.frame;
    let bar_h = screen.bar_h.max(MIN_BAR_H);
    // A notch is believed only when its numbers make sense: inside the
    // screen, wider than tall, not most of the screen.
    let notch = screen.notch.filter(|(left, right, h)| {
        let w = right - left;
        *h >= MIN_BAR_H / 2.0 && *h <= 80.0 && w >= 60.0 && w <= f.w / 2.0 && *left >= f.x && *right <= f.x + f.w
    });
    let (center_x, notch_w, notch_h, real) = match notch {
        Some((left, right, h)) => ((left + right) / 2.0, right - left, h, true),
        None => (f.x + f.w / 2.0, VIRTUAL_NOTCH_W, bar_h, false),
    };
    // The window is centred on the notch, its top on the screen's top, and
    // kept on the screen.
    let w = WINDOW_W.min(f.w);
    let x = (center_x - w / 2.0).clamp(f.x, f.x + f.w - w);
    let window = Rect { x, y: f.y, w, h: WINDOW_H.min(f.h) };
    let local_center = center_x - x;
    let notch = Rect { x: local_center - notch_w / 2.0, y: 0.0, w: notch_w, h: notch_h };
    let pill_w = (notch_w + 2.0 * EAR_W).min(w);
    let pill = Rect { x: local_center - pill_w / 2.0, y: 0.0, w: pill_w, h: notch_h };
    Placement { window, notch, pill, real_notch: real }
}

/// The pointer as the face sees it: a vector from the face's centre to
/// the pointer, each axis in -1..1 (1 = `reach` points or further away),
/// y growing downwards.
pub fn gaze(face: (f64, f64), pointer: (f64, f64), reach: f64) -> (f64, f64) {
    let dx = (pointer.0 - face.0) / reach;
    let dy = (pointer.1 - face.1) / reach;
    let len = (dx * dx + dy * dy).sqrt();
    // Keep the direction, squash the distance: far away is "as far as the
    // eyes go", not a clipped corner.
    if len > 1.0 {
        (dx / len, dy / len)
    } else {
        (dx, dy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mbp14() -> ScreenInfo {
        // A 14" MacBook Pro at its default resolution: 1512 x 982 points,
        // a notch of about 185 x 32 points.
        ScreenInfo { frame: Rect { x: 0.0, y: 0.0, w: 1512.0, h: 982.0 }, notch: Some((663.5, 848.5, 32.0)), bar_h: 37.0 }
    }

    #[test]
    fn with_a_notch_the_island_is_the_notch_and_its_ears() {
        let p = place(&mbp14());
        assert!(p.real_notch);
        assert_eq!(p.window, Rect { x: 756.0 - WINDOW_W / 2.0, y: 0.0, w: WINDOW_W, h: WINDOW_H });
        assert_eq!(p.notch.w, 185.0);
        assert_eq!(p.notch.h, 32.0);
        // Centred in the window, the notch exactly where the screen has it.
        assert_eq!(p.window.x + p.notch.x, 663.5);
        assert_eq!(p.pill.w, 185.0 + 2.0 * EAR_W);
        assert_eq!(p.pill.x + p.pill.w / 2.0, WINDOW_W / 2.0);
        assert_eq!(p.pill.h, 32.0, "as tall as the notch, never taller");
    }

    #[test]
    fn without_a_notch_the_same_shape_is_drawn_as_if_there_were_one() {
        let screen = ScreenInfo { frame: Rect { x: 0.0, y: 0.0, w: 2560.0, h: 1440.0 }, notch: None, bar_h: 25.0 };
        let p = place(&screen);
        assert!(!p.real_notch);
        assert_eq!(p.notch.w, VIRTUAL_NOTCH_W);
        assert_eq!(p.notch.h, 25.0, "as tall as the menu bar");
        assert_eq!(p.window.x + p.window.w / 2.0, 1280.0);
        // No menu bar height known (Windows): the smallest island.
        let bare = ScreenInfo { bar_h: 0.0, ..screen };
        assert_eq!(place(&bare).pill.h, MIN_BAR_H);
    }

    #[test]
    fn a_screen_elsewhere_in_the_desktop_keeps_its_own_offsets() {
        let screen = ScreenInfo { frame: Rect { x: -1920.0, y: -300.0, w: 1920.0, h: 1080.0 }, notch: None, bar_h: 24.0 };
        let p = place(&screen);
        assert_eq!(p.window.y, -300.0);
        assert_eq!(p.window.x + p.window.w / 2.0, -960.0);
    }

    #[test]
    fn numbers_that_make_no_sense_are_not_a_notch() {
        for notch in [(10.0, 20.0, 32.0), (0.0, 1500.0, 32.0), (700.0, 900.0, 200.0), (700.0, 900.0, 2.0), (-50.0, 150.0, 32.0)] {
            let screen = ScreenInfo { notch: Some(notch), ..mbp14() };
            assert!(!place(&screen).real_notch, "{notch:?}");
        }
    }

    #[test]
    fn a_small_screen_keeps_the_window_on_it() {
        let screen = ScreenInfo { frame: Rect { x: 0.0, y: 0.0, w: 400.0, h: 250.0 }, notch: None, bar_h: 24.0 };
        let p = place(&screen);
        assert_eq!(p.window, Rect { x: 0.0, y: 0.0, w: 400.0, h: 250.0 });
        assert!(p.pill.w <= 400.0);
    }

    #[test]
    fn the_gaze_points_at_the_pointer_and_stops_at_the_edge() {
        assert_eq!(gaze((100.0, 10.0), (100.0, 10.0), 500.0), (0.0, 0.0));
        assert_eq!(gaze((100.0, 10.0), (350.0, 10.0), 500.0), (0.5, 0.0));
        let (x, y) = gaze((0.0, 0.0), (3000.0, 4000.0), 500.0);
        assert!((x - 0.6).abs() < 1e-9 && (y - 0.8).abs() < 1e-9, "{x} {y}");
        assert!(Rect { x: 1.0, y: 1.0, w: 2.0, h: 2.0 }.contains(2.0, 2.9));
        assert!(!Rect { x: 1.0, y: 1.0, w: 2.0, h: 2.0 }.contains(3.0, 2.0));
    }
}
