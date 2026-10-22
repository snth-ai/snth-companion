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
//!
//! Windows (a `work` area in `ScreenInfo`): there is no menu bar, and the
//! top of the screen is where browser tabs and title bars are, so the
//! island floats instead: a small free-standing pill (`FLOAT_PILL_W` x
//! `FLOAT_PILL_H`, every corner rounded, no notch, no ears) at the bottom
//! centre of the primary monitor's work area (the screen less the
//! taskbar), its bottom edge `FLOAT_GAP` above the taskbar
//! (`FLOAT_GAP_EDGE` above the screen's edge when nothing is below it: an
//! auto-hidden taskbar, or one on the left or right). That is
//! `edge: Bottom`: the webview grows the open island upwards from the
//! face, which stays at the bottom of the shape. A taskbar on the top
//! edge puts the island at the top of the work area instead (`edge:
//! Top`, still a floating pill, `FLOAT_GAP` under the taskbar), growing
//! down. The window always stays inside the work area. macOS never has a
//! work area here and keeps the notch placement above (`edge: Top`,
//! `floating: false`).

use serde::{Deserialize, Serialize};

/// The island's window, in logical points. Wide and tall enough for the
/// expanded island (the face, a line of text, two buttons, a summary).
pub const WINDOW_W: f64 = 480.0;
pub const WINDOW_H: f64 = 300.0;

/// Each ear beside the notch: room for the face (about 70 px wide at the
/// closed height, its state symbol included) on one side and the
/// status on the other.
pub const EAR_W: f64 = 84.0;

/// The notch a Mac without one is drawn as if it had: about the notch of
/// a 14" MacBook Pro.
pub const VIRTUAL_NOTCH_W: f64 = 184.0;
/// The shortest island: below this a face cannot be drawn.
pub const MIN_BAR_H: f64 = 24.0;

/// The floating island (Windows), closed: a pill just big enough for
/// the face.
pub const FLOAT_PILL_W: f64 = 84.0;
pub const FLOAT_PILL_H: f64 = 28.0;
/// Between the floating pill and the taskbar (or the top taskbar and
/// the pill).
pub const FLOAT_GAP: f64 = 8.0;
/// Between the floating pill and the screen's bottom edge when no
/// taskbar is under it (auto-hidden, or on a side): a little more, so
/// the pill does not sit on the pixel row that calls an auto-hidden
/// taskbar up.
pub const FLOAT_GAP_EDGE: f64 = 12.0;

/// Which screen edge the island keeps to: the open island grows away
/// from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Top,
    Bottom,
}

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
    /// Windows: the work area, the screen less the taskbar (top-left
    /// global points). Some: the island floats (see the module's notes).
    /// None (macOS): the notch placement.
    pub work: Option<Rect>,
}

/// A rectangle the system gave in physical pixels, in logical points at
/// this scale (Windows: the monitor's DPI scale, 1.5 at 144 dpi).
pub fn from_physical(x: i32, y: i32, w: u32, h: u32, scale: f64) -> Rect {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    Rect { x: f64::from(x) / scale, y: f64::from(y) / scale, w: f64::from(w) / scale, h: f64::from(h) / scale }
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
    /// The edge the island keeps to (it grows away from it).
    pub edge: Edge,
    /// A free-standing pill with every corner rounded (Windows), not a
    /// shape flush with the screen's top edge.
    pub floating: bool,
}

pub fn place(screen: &ScreenInfo) -> Placement {
    if let Some(work) = screen.work {
        return place_floating(screen.frame, work);
    }
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
    Placement { window, notch, pill, real_notch: real, edge: Edge::Top, floating: false }
}

/// The floating island (Windows): see the module's notes.
fn place_floating(frame: Rect, work: Rect) -> Placement {
    const SLACK: f64 = 1.0;
    // A work area that makes no sense (empty, or off the screen) is the
    // screen itself.
    let sane = work.w >= FLOAT_PILL_W && work.h >= FLOAT_PILL_H * 2.0 && work.x >= frame.x - SLACK && work.y >= frame.y - SLACK
        && work.x + work.w <= frame.x + frame.w + SLACK
        && work.y + work.h <= frame.y + frame.h + SLACK;
    let a = if sane { work } else { frame };
    let below = (frame.y + frame.h) - (a.y + a.h);
    let above = a.y - frame.y;
    // Only a taskbar on the top edge (and none at the bottom) moves the
    // island up.
    let edge = if above > SLACK && below <= SLACK { Edge::Top } else { Edge::Bottom };
    let w = WINDOW_W.min(a.w);
    let h = WINDOW_H.min(a.h);
    let center_x = a.x + a.w / 2.0;
    let x = (center_x - w / 2.0).clamp(a.x, a.x + a.w - w);
    let local_center = center_x - x;
    let pill_w = FLOAT_PILL_W.min(w);
    let pill_h = FLOAT_PILL_H.min(h);
    let (y, pill_y) = match edge {
        // The window's bottom on the work area's; the pill a gap above it.
        Edge::Bottom => {
            let gap = if below > SLACK { FLOAT_GAP } else { FLOAT_GAP_EDGE };
            (a.y + a.h - h, (h - pill_h - gap).max(0.0))
        }
        // Just under the taskbar.
        Edge::Top => (a.y, FLOAT_GAP.min(h - pill_h).max(0.0)),
    };
    let window = Rect { x, y, w, h };
    let pill = Rect { x: local_center - pill_w / 2.0, y: pill_y, w: pill_w, h: pill_h };
    Placement { window, notch: pill, pill, real_notch: false, edge, floating: true }
}

/// What the screen shows, for hiding the island in full screen the way
/// the menu bar hides.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Cover {
    /// A full-screen window fills the screen.
    pub covered: bool,
    /// The system's menu bar is on the screen (macOS), or coming in.
    pub bar: bool,
}

/// Does this window (top-left global points) fill the screen the way a
/// full-screen window does: the whole width, down to the bottom, from
/// the top or from just under the notch (on a Mac with one a full-screen
/// window starts below it unless the app asks otherwise). A window
/// zoomed in a normal Space stops at the menu bar and the Dock, and the
/// menu bar is on the screen then anyway.
pub fn fills_screen(screen: &ScreenInfo, w: Rect) -> bool {
    const SLACK: f64 = 1.0;
    let f = screen.frame;
    let band = screen.bar_h.max(screen.notch.map_or(0.0, |n| n.2));
    (w.x - f.x).abs() <= SLACK
        && (w.w - f.w).abs() <= SLACK
        && ((w.y + w.h) - (f.y + f.h)).abs() <= SLACK
        && w.y >= f.y - SLACK
        && w.y <= f.y + band + SLACK
}

/// Is this the menu bar of this screen: across its top, shown or sliding
/// in (it starts above the screen's edge and moves down). A screen above
/// this one has its own bar far higher up.
pub fn bar_on_screen(screen: &ScreenInfo, w: Rect) -> bool {
    const SLIDE: f64 = 100.0;
    let f = screen.frame;
    w.x < f.x + f.w && w.x + w.w > f.x && w.y < f.y + screen.bar_h.max(MIN_BAR_H) && w.y + w.h > f.y - SLIDE
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
        ScreenInfo { frame: Rect { x: 0.0, y: 0.0, w: 1512.0, h: 982.0 }, notch: Some((663.5, 848.5, 32.0)), bar_h: 37.0, work: None }
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
        let screen = ScreenInfo { frame: Rect { x: 0.0, y: 0.0, w: 2560.0, h: 1440.0 }, notch: None, bar_h: 25.0, work: None };
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
        let screen = ScreenInfo { frame: Rect { x: -1920.0, y: -300.0, w: 1920.0, h: 1080.0 }, notch: None, bar_h: 24.0, work: None };
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
        let screen = ScreenInfo { frame: Rect { x: 0.0, y: 0.0, w: 400.0, h: 250.0 }, notch: None, bar_h: 24.0, work: None };
        let p = place(&screen);
        assert_eq!(p.window, Rect { x: 0.0, y: 0.0, w: 400.0, h: 250.0 });
        assert!(p.pill.w <= 400.0);
    }

    fn windows(frame: Rect, work: Rect) -> ScreenInfo {
        ScreenInfo { frame, notch: None, bar_h: 0.0, work: Some(work) }
    }

    const FHD: Rect = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 };

    /// The pill's rectangle in global points.
    fn pill_on_screen(p: &Placement) -> Rect {
        Rect { x: p.window.x + p.pill.x, y: p.window.y + p.pill.y, w: p.pill.w, h: p.pill.h }
    }

    fn inside(outer: Rect, inner: Rect) -> bool {
        inner.x >= outer.x && inner.y >= outer.y && inner.x + inner.w <= outer.x + outer.w && inner.y + inner.h <= outer.y + outer.h
    }

    #[test]
    fn macos_keeps_the_notch_placement() {
        let p = place(&mbp14());
        assert_eq!(p.edge, Edge::Top);
        assert!(!p.floating);
        let json = serde_json::to_value(p).unwrap();
        assert_eq!(json["edge"], "top");
        assert_eq!(json["floating"], false);
    }

    #[test]
    fn windows_floats_the_pill_above_a_bottom_taskbar() {
        // 1920 x 1080 with the default 48 px taskbar at the bottom.
        let work = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1032.0 };
        let p = place(&windows(FHD, work));
        assert_eq!(p.edge, Edge::Bottom);
        assert!(p.floating);
        assert!(!p.real_notch);
        assert_eq!(p.window, Rect { x: 960.0 - WINDOW_W / 2.0, y: 1032.0 - WINDOW_H, w: WINDOW_W, h: WINDOW_H });
        let pill = pill_on_screen(&p);
        assert_eq!((pill.w, pill.h), (FLOAT_PILL_W, FLOAT_PILL_H));
        assert_eq!(pill.x + pill.w / 2.0, 960.0, "centred");
        assert_eq!(pill.y + pill.h, 1032.0 - FLOAT_GAP, "a gap above the taskbar");
        assert!(inside(work, p.window));
        assert_eq!(serde_json::to_value(p).unwrap()["edge"], "bottom");
    }

    #[test]
    fn windows_with_the_taskbar_on_top_floats_under_it() {
        let work = Rect { x: 0.0, y: 48.0, w: 1920.0, h: 1032.0 };
        let p = place(&windows(FHD, work));
        assert_eq!(p.edge, Edge::Top);
        assert!(p.floating, "a pill, not a notch");
        assert_eq!(p.window.y, 48.0);
        let pill = pill_on_screen(&p);
        assert_eq!(pill.y, 48.0 + FLOAT_GAP);
        assert_eq!(pill.x + pill.w / 2.0, 960.0);
        assert!(inside(work, p.window));
    }

    #[test]
    fn windows_with_the_taskbar_on_a_side_centres_on_the_work_area() {
        // Taskbar on the left, 62 px wide.
        let work = Rect { x: 62.0, y: 0.0, w: 1858.0, h: 1080.0 };
        let p = place(&windows(FHD, work));
        assert_eq!(p.edge, Edge::Bottom);
        let pill = pill_on_screen(&p);
        assert_eq!(pill.x + pill.w / 2.0, 62.0 + 1858.0 / 2.0);
        assert_eq!(pill.y + pill.h, 1080.0 - FLOAT_GAP_EDGE, "nothing below: the edge gap");
        assert!(inside(work, p.window));
        // On the right.
        let work = Rect { x: 0.0, y: 0.0, w: 1858.0, h: 1080.0 };
        let p = place(&windows(FHD, work));
        assert_eq!(pill_on_screen(&p).x + FLOAT_PILL_W / 2.0, 929.0);
        assert!(inside(work, p.window));
    }

    #[test]
    fn windows_with_an_auto_hidden_taskbar_keeps_off_the_edge() {
        let p = place(&windows(FHD, FHD));
        assert_eq!(p.edge, Edge::Bottom);
        assert_eq!(p.window.y + p.window.h, 1080.0);
        let pill = pill_on_screen(&p);
        assert_eq!(pill.y + pill.h, 1080.0 - FLOAT_GAP_EDGE);
        assert_eq!(pill.x + pill.w / 2.0, 960.0);
    }

    #[test]
    fn windows_on_a_monitor_away_from_the_origin_keeps_its_offsets() {
        // A primary monitor whose top-left is not 0,0 in the desktop.
        let frame = Rect { x: -1920.0, y: -300.0, w: 1920.0, h: 1080.0 };
        let work = Rect { x: -1920.0, y: -300.0, w: 1920.0, h: 1040.0 };
        let p = place(&windows(frame, work));
        assert_eq!(p.edge, Edge::Bottom);
        assert_eq!(p.window.y + p.window.h, 740.0);
        let pill = pill_on_screen(&p);
        assert_eq!(pill.x + pill.w / 2.0, -960.0);
        assert_eq!(pill.y + pill.h, 740.0 - FLOAT_GAP);
        assert!(inside(work, p.window));
    }

    #[test]
    fn windows_dpi_scale_turns_pixels_into_points() {
        // 3840 x 2160 at 150 %, taskbar 72 px (48 points) at the bottom:
        // what panel.rs gets from the monitor, in pixels.
        let frame = from_physical(0, 0, 3840, 2160, 1.5);
        let work = from_physical(0, 0, 3840, 2088, 1.5);
        assert_eq!(frame, Rect { x: 0.0, y: 0.0, w: 2560.0, h: 1440.0 });
        assert_eq!(work.h, 1392.0);
        let p = place(&windows(frame, work));
        assert_eq!(pill_on_screen(&p).y + FLOAT_PILL_H, 1392.0 - FLOAT_GAP);
        // A second monitor at 125 % left of the primary keeps its sign.
        assert_eq!(from_physical(-2400, -120, 2400, 1350, 1.25), Rect { x: -1920.0, y: -96.0, w: 1920.0, h: 1080.0 });
        // A scale the system never gives is 1.
        assert_eq!(from_physical(10, 20, 30, 40, 0.0), Rect { x: 10.0, y: 20.0, w: 30.0, h: 40.0 });
    }

    #[test]
    fn windows_with_a_work_area_that_makes_no_sense_uses_the_screen() {
        for work in [Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 }, Rect { x: 5000.0, y: 0.0, w: 1920.0, h: 1040.0 }] {
            let p = place(&windows(FHD, work));
            assert_eq!(p.edge, Edge::Bottom, "{work:?}");
            assert!(inside(FHD, p.window), "{work:?}");
            assert_eq!(p.window.y + p.window.h, 1080.0);
        }
    }

    #[test]
    fn windows_on_a_small_screen_keeps_the_window_in_the_work_area() {
        let frame = Rect { x: 0.0, y: 0.0, w: 400.0, h: 290.0 };
        let work = Rect { x: 0.0, y: 0.0, w: 400.0, h: 250.0 };
        let p = place(&windows(frame, work));
        assert_eq!(p.window, Rect { x: 0.0, y: 0.0, w: 400.0, h: 250.0 });
        assert!(inside(work, pill_on_screen(&p)));
    }

    #[test]
    fn a_full_screen_window_fills_the_screen_and_a_zoomed_one_does_not() {
        // Measured on a 1920 x 1080 screen with a 30 pt menu bar: the
        // full-screen window is the whole screen.
        let plain = ScreenInfo { frame: Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 }, notch: None, bar_h: 30.0, work: None };
        assert!(fills_screen(&plain, Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 }));
        // Zoomed in a normal Space: under the menu bar, above the Dock.
        assert!(!fills_screen(&plain, Rect { x: 0.0, y: 30.0, w: 1920.0, h: 990.0 }));
        assert!(!fills_screen(&plain, Rect { x: 42.0, y: 0.0, w: 1878.0, h: 1080.0 }));
        // With a notch the full-screen window starts below it.
        let notched = mbp14();
        assert!(fills_screen(&notched, Rect { x: 0.0, y: 32.0, w: 1512.0, h: 950.0 }));
        assert!(fills_screen(&notched, Rect { x: 0.0, y: 0.0, w: 1512.0, h: 982.0 }));
        assert!(!fills_screen(&notched, Rect { x: 0.0, y: 120.0, w: 1512.0, h: 862.0 }));
        // A screen elsewhere in the desktop.
        let side = ScreenInfo { frame: Rect { x: -1920.0, y: -300.0, w: 1920.0, h: 1080.0 }, notch: None, bar_h: 24.0, work: None };
        assert!(fills_screen(&side, Rect { x: -1920.0, y: -300.0, w: 1920.0, h: 1080.0 }));
        assert!(!fills_screen(&side, Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 }));
    }

    #[test]
    fn the_menu_bar_counts_while_it_slides_in() {
        let plain = ScreenInfo { frame: Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 }, notch: None, bar_h: 30.0, work: None };
        // Measured: shown at 0, sliding in from -50.
        assert!(bar_on_screen(&plain, Rect { x: 0.0, y: 0.0, w: 1920.0, h: 30.0 }));
        assert!(bar_on_screen(&plain, Rect { x: 0.0, y: -50.0, w: 1920.0, h: 30.0 }));
        // The bar of a screen above, or beside.
        assert!(!bar_on_screen(&plain, Rect { x: 0.0, y: -1080.0, w: 1920.0, h: 30.0 }));
        assert!(!bar_on_screen(&plain, Rect { x: 1920.0, y: 0.0, w: 1440.0, h: 30.0 }));
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
