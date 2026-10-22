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
//! down. The window always stays inside the work area.
//!
//! The owner may choose another spot (`Spot`, from the island's settings):
//! top, bottom or the middle of a side, at the left, the centre or the
//! right; eight spots, the middle of the screen is not one (middle with
//! the centre is bad data and means the default). None on an axis is
//! the system's default: macOS top centre (the notch placement above,
//! `edge: Top`, `floating: false`, exactly as it always was), Windows
//! bottom centre with the top-taskbar rule above. Every other spot, on
//! both systems, is the floating pill inside the screen's usable area
//! (`work`: Windows the work area, macOS `NSScreen.visibleFrame`, the
//! screen less the menu bar and a Dock that is not hidden, wherever the
//! Dock is; on a screen with a menu bar the area also starts no higher
//! than the bar and the notch, `menu_bar`). Top spots: `FLOAT_GAP` under
//! the top of the area (under the menu bar, a top taskbar), or
//! `FLOAT_GAP_EDGE` under the screen's edge with nothing above. Bottom
//! spots: the same above the bottom (the Dock, the taskbar, the edge).
//! Middle spots: the pill vertically centred on the area (`edge:
//! Middle`). Left and right spots keep `FLOAT_SIDE` from the area's side
//! edge (a Dock or a taskbar on that side included) and the window is
//! flush with that side (`align`), so the webview can anchor the open
//! island to the pill's side edge and grow it inwards.

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
/// Between a floating pill at the left or right and that side of the
/// usable area.
pub const FLOAT_SIDE: f64 = 12.0;

/// Which screen edge the island keeps to: the open island grows away
/// from it. `Middle`: the middle of a side edge (with `Align::Left` or
/// `Align::Right` only); the open island is centred on the pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Top,
    Bottom,
    Middle,
}

/// Where along the edge: the open island is anchored to the pill's own
/// side at the left and the right, centred on it in the centre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    Left,
    Center,
    Right,
}

/// The owner's choice of spot (the island's settings). None on an axis:
/// the system's default (see the module's notes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Spot {
    pub vertical: Option<Edge>,
    pub horizontal: Option<Align>,
}

impl Spot {
    pub const DEFAULT: Spot = Spot { vertical: None, horizontal: None };

    pub const fn new(vertical: Edge, horizontal: Align) -> Spot {
        Spot { vertical: Some(vertical), horizontal: Some(horizontal) }
    }

    /// The middle of the screen is not a spot: bad data means the
    /// default.
    fn sane(self) -> Spot {
        if self.vertical == Some(Edge::Middle) && self.horizontal.unwrap_or(Align::Center) == Align::Center {
            Spot::DEFAULT
        } else {
            self
        }
    }
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
    /// The usable area (top-left global points): Windows the work area,
    /// the screen less the taskbar; macOS the visible frame, the screen
    /// less the menu bar and the Dock. The floating spots go inside it.
    /// None: the whole screen.
    pub work: Option<Rect>,
    /// The screen has a menu bar the island merges with at the top centre
    /// (macOS): the default spot is the notch placement and the floating
    /// spots stay below the bar and the notch. False (Windows): the
    /// floating pill everywhere, and with a `work` area the default spot
    /// is the bottom centre.
    pub menu_bar: bool,
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
    /// A free-standing pill with every corner rounded (Windows, and every
    /// spot but the notch on macOS), not a shape flush with the screen's
    /// top edge.
    pub floating: bool,
    /// The side the open island is anchored to (`Center` for the notch).
    pub align: Align,
}

/// Where the island goes on this screen at this spot.
pub fn place(screen: &ScreenInfo, spot: Spot) -> Placement {
    let spot = spot.sane();
    // A screen with a menu bar (macOS), or without a usable area (Linux):
    // the default is the notch at the top centre.
    let notch_os = screen.menu_bar || screen.work.is_none();
    let align = spot.horizontal.unwrap_or(Align::Center);
    let vertical = spot.vertical.or(notch_os.then_some(Edge::Top));
    if notch_os && vertical == Some(Edge::Top) && align == Align::Center {
        return place_notch(screen);
    }
    place_floating(screen, vertical, align)
}

/// The notch placement (macOS's default): see the module's notes.
fn place_notch(screen: &ScreenInfo) -> Placement {
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
    Placement { window, notch, pill, real_notch: real, edge: Edge::Top, floating: false, align: Align::Center }
}

/// The floating island: see the module's notes. `vertical` None: the
/// Windows default (the bottom, the top under a taskbar on the top edge).
fn place_floating(screen: &ScreenInfo, vertical: Option<Edge>, align: Align) -> Placement {
    const SLACK: f64 = 1.0;
    let frame = screen.frame;
    let work = screen.work.unwrap_or(frame);
    // A work area that makes no sense (empty, or off the screen) is the
    // screen itself.
    let sane = work.w >= FLOAT_PILL_W && work.h >= FLOAT_PILL_H * 2.0 && work.x >= frame.x - SLACK && work.y >= frame.y - SLACK
        && work.x + work.w <= frame.x + frame.w + SLACK
        && work.y + work.h <= frame.y + frame.h + SLACK;
    let mut a = if sane { work } else { frame };
    // Under the menu bar and the notch, even when the bar hides itself
    // (the visible frame then reaches the top of the screen).
    if screen.menu_bar {
        let band = screen.bar_h.max(screen.notch.map_or(0.0, |n| n.2));
        let top = frame.y + band;
        if a.y < top {
            a.h = (a.h - (top - a.y)).max(0.0);
            a.y = top;
        }
    }
    let below = (frame.y + frame.h) - (a.y + a.h);
    let above = a.y - frame.y;
    // The default: only a taskbar on the top edge (and none at the
    // bottom) moves the island up.
    let edge = vertical.unwrap_or(if above > SLACK && below <= SLACK { Edge::Top } else { Edge::Bottom });
    let w = WINDOW_W.min(a.w);
    let h = WINDOW_H.min(a.h);
    let pill_w = FLOAT_PILL_W.min(w);
    let pill_h = FLOAT_PILL_H.min(h);
    // The window flush with the side the pill keeps to, the pill
    // `FLOAT_SIDE` in from it; or both centred on the area.
    let (x, pill_x) = match align {
        Align::Center => {
            let center_x = a.x + a.w / 2.0;
            let x = (center_x - w / 2.0).clamp(a.x, a.x + a.w - w);
            (x, center_x - x - pill_w / 2.0)
        }
        Align::Left => (a.x, FLOAT_SIDE.min(w - pill_w).max(0.0)),
        Align::Right => (a.x + a.w - w, (w - pill_w - FLOAT_SIDE).max(0.0)),
    };
    let (y, pill_y) = match edge {
        // The window's bottom on the area's; the pill a gap above it.
        Edge::Bottom => {
            let gap = if below > SLACK { FLOAT_GAP } else { FLOAT_GAP_EDGE };
            (a.y + a.h - h, (h - pill_h - gap).max(0.0))
        }
        // Just under the taskbar or the menu bar, or off the screen's top
        // edge.
        Edge::Top => {
            let gap = if above > SLACK { FLOAT_GAP } else { FLOAT_GAP_EDGE };
            (a.y, gap.min(h - pill_h).max(0.0))
        }
        // The window and the pill centred on the area.
        Edge::Middle => (a.y + (a.h - h) / 2.0, (h - pill_h) / 2.0),
    };
    let window = Rect { x, y, w, h };
    let pill = Rect { x: pill_x, y: pill_y, w: pill_w, h: pill_h };
    Placement { window, notch: pill, pill, real_notch: false, edge, floating: true, align }
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
        ScreenInfo { frame: Rect { x: 0.0, y: 0.0, w: 1512.0, h: 982.0 }, notch: Some((663.5, 848.5, 32.0)), bar_h: 37.0, work: None, menu_bar: true }
    }

    #[test]
    fn with_a_notch_the_island_is_the_notch_and_its_ears() {
        let p = place(&mbp14(), Spot::DEFAULT);
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
        let screen = ScreenInfo { frame: Rect { x: 0.0, y: 0.0, w: 2560.0, h: 1440.0 }, notch: None, bar_h: 25.0, work: None, menu_bar: true };
        let p = place(&screen, Spot::DEFAULT);
        assert!(!p.real_notch);
        assert_eq!(p.notch.w, VIRTUAL_NOTCH_W);
        assert_eq!(p.notch.h, 25.0, "as tall as the menu bar");
        assert_eq!(p.window.x + p.window.w / 2.0, 1280.0);
        // No menu bar height known (Windows): the smallest island.
        let bare = ScreenInfo { bar_h: 0.0, ..screen };
        assert_eq!(place(&bare, Spot::DEFAULT).pill.h, MIN_BAR_H);
    }

    #[test]
    fn a_screen_elsewhere_in_the_desktop_keeps_its_own_offsets() {
        let screen = ScreenInfo { frame: Rect { x: -1920.0, y: -300.0, w: 1920.0, h: 1080.0 }, notch: None, bar_h: 24.0, work: None, menu_bar: true };
        let p = place(&screen, Spot::DEFAULT);
        assert_eq!(p.window.y, -300.0);
        assert_eq!(p.window.x + p.window.w / 2.0, -960.0);
    }

    #[test]
    fn numbers_that_make_no_sense_are_not_a_notch() {
        for notch in [(10.0, 20.0, 32.0), (0.0, 1500.0, 32.0), (700.0, 900.0, 200.0), (700.0, 900.0, 2.0), (-50.0, 150.0, 32.0)] {
            let screen = ScreenInfo { notch: Some(notch), ..mbp14() };
            assert!(!place(&screen, Spot::DEFAULT).real_notch, "{notch:?}");
        }
    }

    #[test]
    fn a_small_screen_keeps_the_window_on_it() {
        let screen = ScreenInfo { frame: Rect { x: 0.0, y: 0.0, w: 400.0, h: 250.0 }, notch: None, bar_h: 24.0, work: None, menu_bar: true };
        let p = place(&screen, Spot::DEFAULT);
        assert_eq!(p.window, Rect { x: 0.0, y: 0.0, w: 400.0, h: 250.0 });
        assert!(p.pill.w <= 400.0);
    }

    fn windows(frame: Rect, work: Rect) -> ScreenInfo {
        ScreenInfo { frame, notch: None, bar_h: 0.0, work: Some(work), menu_bar: false }
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
        let p = place(&mbp14(), Spot::DEFAULT);
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
        let p = place(&windows(FHD, work), Spot::DEFAULT);
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
        let p = place(&windows(FHD, work), Spot::DEFAULT);
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
        let p = place(&windows(FHD, work), Spot::DEFAULT);
        assert_eq!(p.edge, Edge::Bottom);
        let pill = pill_on_screen(&p);
        assert_eq!(pill.x + pill.w / 2.0, 62.0 + 1858.0 / 2.0);
        assert_eq!(pill.y + pill.h, 1080.0 - FLOAT_GAP_EDGE, "nothing below: the edge gap");
        assert!(inside(work, p.window));
        // On the right.
        let work = Rect { x: 0.0, y: 0.0, w: 1858.0, h: 1080.0 };
        let p = place(&windows(FHD, work), Spot::DEFAULT);
        assert_eq!(pill_on_screen(&p).x + FLOAT_PILL_W / 2.0, 929.0);
        assert!(inside(work, p.window));
    }

    #[test]
    fn windows_with_an_auto_hidden_taskbar_keeps_off_the_edge() {
        let p = place(&windows(FHD, FHD), Spot::DEFAULT);
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
        let p = place(&windows(frame, work), Spot::DEFAULT);
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
        let p = place(&windows(frame, work), Spot::DEFAULT);
        assert_eq!(pill_on_screen(&p).y + FLOAT_PILL_H, 1392.0 - FLOAT_GAP);
        // A second monitor at 125 % left of the primary keeps its sign.
        assert_eq!(from_physical(-2400, -120, 2400, 1350, 1.25), Rect { x: -1920.0, y: -96.0, w: 1920.0, h: 1080.0 });
        // A scale the system never gives is 1.
        assert_eq!(from_physical(10, 20, 30, 40, 0.0), Rect { x: 10.0, y: 20.0, w: 30.0, h: 40.0 });
    }

    #[test]
    fn windows_with_a_work_area_that_makes_no_sense_uses_the_screen() {
        for work in [Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 }, Rect { x: 5000.0, y: 0.0, w: 1920.0, h: 1040.0 }] {
            let p = place(&windows(FHD, work), Spot::DEFAULT);
            assert_eq!(p.edge, Edge::Bottom, "{work:?}");
            assert!(inside(FHD, p.window), "{work:?}");
            assert_eq!(p.window.y + p.window.h, 1080.0);
        }
    }

    #[test]
    fn windows_on_a_small_screen_keeps_the_window_in_the_work_area() {
        let frame = Rect { x: 0.0, y: 0.0, w: 400.0, h: 290.0 };
        let work = Rect { x: 0.0, y: 0.0, w: 400.0, h: 250.0 };
        let p = place(&windows(frame, work), Spot::DEFAULT);
        assert_eq!(p.window, Rect { x: 0.0, y: 0.0, w: 400.0, h: 250.0 });
        assert!(inside(work, pill_on_screen(&p)));
    }

    // --- the owner's spot ---

    const SPOTS: [(Edge, Align); 8] = [
        (Edge::Top, Align::Left),
        (Edge::Top, Align::Center),
        (Edge::Top, Align::Right),
        (Edge::Middle, Align::Left),
        (Edge::Middle, Align::Right),
        (Edge::Bottom, Align::Left),
        (Edge::Bottom, Align::Center),
        (Edge::Bottom, Align::Right),
    ];

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    }

    /// One floating spot on a screen whose usable area (after the menu
    /// bar) is `area`: inside it, at its side, its edge or its middle,
    /// with these gaps above and below.
    fn check_spot(screen: &ScreenInfo, (v, h): (Edge, Align), area: Rect, gap_top: f64, gap_bottom: f64) -> Placement {
        let what = format!("{v:?} {h:?} on {:?}", screen.work);
        let p = place(screen, Spot::new(v, h));
        assert!(p.floating && !p.real_notch, "{what}");
        assert_eq!((p.edge, p.align), (v, h), "{what}");
        assert!(inside(area, p.window), "{what}: window {:?} outside {area:?}", p.window);
        let pill = pill_on_screen(&p);
        assert!(inside(p.window, pill), "{what}");
        assert_eq!((pill.w, pill.h), (FLOAT_PILL_W, FLOAT_PILL_H), "{what}");
        match h {
            Align::Left => {
                assert_eq!(pill.x, area.x + FLOAT_SIDE, "{what}");
                assert_eq!(p.window.x, area.x, "{what}: the window flush with the side");
            }
            Align::Right => {
                assert_eq!(pill.x + pill.w, area.x + area.w - FLOAT_SIDE, "{what}");
                assert_eq!(p.window.x + p.window.w, area.x + area.w, "{what}");
            }
            Align::Center => assert_eq!(pill.x + pill.w / 2.0, area.x + area.w / 2.0, "{what}"),
        }
        match v {
            Edge::Top => assert_eq!(pill.y, area.y + gap_top, "{what}"),
            Edge::Bottom => assert_eq!(pill.y + pill.h, area.y + area.h - gap_bottom, "{what}"),
            Edge::Middle => assert_eq!(pill.y + pill.h / 2.0, area.y + area.h / 2.0, "{what}"),
        }
        // The open island is anchored to the pill's side and grows
        // inwards: the window has room for the widest card that way.
        match h {
            Align::Left => assert!(p.window.w - p.pill.x >= 360.0, "{what}"),
            Align::Right => assert!(p.pill.x + p.pill.w >= 360.0, "{what}"),
            Align::Center => {}
        }
        p
    }

    fn mac(frame: Rect, notch: Option<(f64, f64, f64)>, bar_h: f64, visible: Rect) -> ScreenInfo {
        ScreenInfo { frame, notch, bar_h, work: Some(visible), menu_bar: true }
    }

    const MBP: Rect = Rect { x: 0.0, y: 0.0, w: 1512.0, h: 982.0 };

    #[test]
    fn macos_default_spot_is_the_notch_whatever_the_visible_frame() {
        // The visible frame is now read on macOS too; the default ignores it.
        let with_dock = mac(MBP, Some((663.5, 848.5, 32.0)), 37.0, Rect { x: 0.0, y: 37.0, w: 1512.0, h: 875.0 });
        let p = place(&with_dock, Spot::DEFAULT);
        assert_eq!(p, place(&mbp14(), Spot::DEFAULT));
        assert!(!p.floating && p.real_notch);
        assert_eq!((p.edge, p.align), (Edge::Top, Align::Center));
        // Top centre chosen explicitly, or one axis alone: the same.
        assert_eq!(place(&with_dock, Spot::new(Edge::Top, Align::Center)), p);
        assert_eq!(place(&with_dock, Spot { vertical: Some(Edge::Top), horizontal: None }), p);
        assert_eq!(place(&with_dock, Spot { vertical: None, horizontal: Some(Align::Center) }), p);
        // The middle of the screen is bad data: the default.
        assert_eq!(place(&with_dock, Spot::new(Edge::Middle, Align::Center)), p);
        assert_eq!(place(&with_dock, Spot { vertical: Some(Edge::Middle), horizontal: None }), p);
        // One axis alone elsewhere: the other is the default's.
        let left = place(&with_dock, Spot { vertical: None, horizontal: Some(Align::Left) });
        assert_eq!((left.edge, left.align, left.floating), (Edge::Top, Align::Left, true));
        let bottom = place(&with_dock, Spot { vertical: Some(Edge::Bottom), horizontal: None });
        assert_eq!((bottom.edge, bottom.align, bottom.floating), (Edge::Bottom, Align::Center, true));
        let json = serde_json::to_value(p).unwrap();
        assert_eq!(json["align"], "center");
        assert_eq!(serde_json::to_value(left).unwrap()["align"], "left");
        assert_eq!(serde_json::to_value(place(&with_dock, Spot::new(Edge::Middle, Align::Right))).unwrap()["edge"], "middle");
    }

    #[test]
    fn macos_spots_with_the_dock_at_the_bottom() {
        // 14" MacBook Pro: menu bar 37, Dock 70 at the bottom.
        let visible = Rect { x: 0.0, y: 37.0, w: 1512.0, h: 875.0 };
        let screen = mac(MBP, Some((663.5, 848.5, 32.0)), 37.0, visible);
        for spot in SPOTS {
            if spot == (Edge::Top, Align::Center) {
                assert!(!place(&screen, Spot::new(spot.0, spot.1)).floating);
                continue;
            }
            let p = check_spot(&screen, spot, visible, FLOAT_GAP, FLOAT_GAP);
            let pill = pill_on_screen(&p);
            // Never over the menu bar or the notch.
            assert!(!overlaps(p.window, Rect { x: 0.0, y: 0.0, w: 1512.0, h: 37.0 }), "{spot:?}");
            assert!(!overlaps(pill, Rect { x: 663.5, y: 0.0, w: 185.0, h: 32.0 }), "{spot:?}");
        }
        // Concretely: bottom right sits 8 above the Dock, 12 from the right.
        let p = place(&screen, Spot::new(Edge::Bottom, Align::Right));
        assert_eq!(pill_on_screen(&p), Rect { x: 1512.0 - 12.0 - 84.0, y: 912.0 - 8.0 - 28.0, w: 84.0, h: 28.0 });
        // Top left: 8 under the menu bar, 12 from the left edge.
        let p = place(&screen, Spot::new(Edge::Top, Align::Left));
        assert_eq!(pill_on_screen(&p), Rect { x: 12.0, y: 45.0, w: 84.0, h: 28.0 });
        assert_eq!(p.window, Rect { x: 0.0, y: 37.0, w: WINDOW_W, h: WINDOW_H });
        // Middle left: centred between the menu bar and the Dock.
        let p = place(&screen, Spot::new(Edge::Middle, Align::Left));
        assert_eq!(pill_on_screen(&p).y + 14.0, 37.0 + 875.0 / 2.0);
    }

    #[test]
    fn macos_spots_with_the_dock_on_a_side() {
        // A 2560 x 1440 display without a notch, menu bar 25, Dock 64 on
        // the left, then on the right: the side spots keep off it.
        let frame = Rect { x: 0.0, y: 0.0, w: 2560.0, h: 1440.0 };
        let dock_left = Rect { x: 64.0, y: 25.0, w: 2496.0, h: 1415.0 };
        let screen = mac(frame, None, 25.0, dock_left);
        for spot in SPOTS {
            if spot == (Edge::Top, Align::Center) {
                continue;
            }
            // Nothing below: the edge gap at the bottom.
            let p = check_spot(&screen, spot, dock_left, FLOAT_GAP, FLOAT_GAP_EDGE);
            assert!(!overlaps(p.window, Rect { x: 0.0, y: 0.0, w: 64.0, h: 1440.0 }), "{spot:?} over the Dock");
        }
        assert_eq!(pill_on_screen(&place(&screen, Spot::new(Edge::Middle, Align::Left))).x, 76.0);
        let dock_right = Rect { x: 0.0, y: 25.0, w: 2496.0, h: 1415.0 };
        let screen = mac(frame, None, 25.0, dock_right);
        for spot in SPOTS {
            if spot == (Edge::Top, Align::Center) {
                continue;
            }
            let p = check_spot(&screen, spot, dock_right, FLOAT_GAP, FLOAT_GAP_EDGE);
            assert!(!overlaps(p.window, Rect { x: 2496.0, y: 0.0, w: 64.0, h: 1440.0 }), "{spot:?} over the Dock");
        }
        let p = pill_on_screen(&place(&screen, Spot::new(Edge::Middle, Align::Right)));
        assert_eq!(p.x + p.w, 2496.0 - 12.0);
    }

    #[test]
    fn macos_spots_with_the_dock_and_the_menu_bar_hidden() {
        // The Dock hides: the visible frame reaches the bottom; the menu
        // bar hides: it reaches the top, but the spots stay under where
        // the bar and the notch come in.
        let visible = Rect { x: 0.0, y: 37.0, w: 1512.0, h: 945.0 };
        let screen = mac(MBP, Some((663.5, 848.5, 32.0)), 37.0, visible);
        for spot in SPOTS {
            if spot != (Edge::Top, Align::Center) {
                check_spot(&screen, spot, visible, FLOAT_GAP, FLOAT_GAP_EDGE);
            }
        }
        let bar_hidden = mac(MBP, Some((663.5, 848.5, 32.0)), 37.0, MBP);
        for spot in SPOTS {
            if spot == (Edge::Top, Align::Center) {
                continue;
            }
            let p = check_spot(&bar_hidden, spot, visible, FLOAT_GAP, FLOAT_GAP_EDGE);
            assert!(p.window.y >= 37.0, "{spot:?}");
        }
    }

    #[test]
    fn macos_spots_on_a_screen_away_from_the_origin() {
        let frame = Rect { x: -1920.0, y: -300.0, w: 1920.0, h: 1080.0 };
        let visible = Rect { x: -1920.0, y: -276.0, w: 1920.0, h: 990.0 };
        let screen = mac(frame, None, 24.0, visible);
        for spot in SPOTS {
            if spot != (Edge::Top, Align::Center) {
                check_spot(&screen, spot, visible, FLOAT_GAP, FLOAT_GAP);
            }
        }
        let p = pill_on_screen(&place(&screen, Spot::new(Edge::Bottom, Align::Left)));
        assert_eq!((p.x, p.y + p.h), (-1908.0, 714.0 - 8.0));
    }

    #[test]
    fn windows_spots_with_the_taskbar_at_the_bottom_top_or_left() {
        // Bottom (the default 48 px).
        let work = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1032.0 };
        let screen = windows(FHD, work);
        for spot in SPOTS {
            check_spot(&screen, spot, work, FLOAT_GAP_EDGE, FLOAT_GAP);
        }
        // Bottom centre chosen is the default.
        assert_eq!(place(&screen, Spot::new(Edge::Bottom, Align::Center)), place(&screen, Spot::DEFAULT));
        assert_eq!(place(&screen, Spot::DEFAULT).align, Align::Center);
        // The middle of the screen: the default.
        assert_eq!(place(&screen, Spot::new(Edge::Middle, Align::Center)), place(&screen, Spot::DEFAULT));
        // Top (a taskbar on the top edge): an explicit bottom spot goes to
        // the bottom anyway, the edge gap under it.
        let work = Rect { x: 0.0, y: 48.0, w: 1920.0, h: 1032.0 };
        let screen = windows(FHD, work);
        for spot in SPOTS {
            check_spot(&screen, spot, work, FLOAT_GAP, FLOAT_GAP_EDGE);
        }
        assert_eq!(place(&screen, Spot::DEFAULT).edge, Edge::Top, "the default follows a top taskbar");
        assert_eq!(place(&screen, Spot { vertical: None, horizontal: Some(Align::Right) }).edge, Edge::Top);
        // Left (62 px): the left spots keep off it.
        let work = Rect { x: 62.0, y: 0.0, w: 1858.0, h: 1080.0 };
        let screen = windows(FHD, work);
        for spot in SPOTS {
            let p = check_spot(&screen, spot, work, FLOAT_GAP_EDGE, FLOAT_GAP_EDGE);
            assert!(!overlaps(p.window, Rect { x: 0.0, y: 0.0, w: 62.0, h: 1080.0 }), "{spot:?} over the taskbar");
        }
        assert_eq!(pill_on_screen(&place(&screen, Spot::new(Edge::Middle, Align::Left))), Rect { x: 74.0, y: 526.0, w: 84.0, h: 28.0 });
    }

    #[test]
    fn windows_spots_on_a_monitor_away_from_the_origin_and_a_small_one() {
        let frame = Rect { x: -1920.0, y: -300.0, w: 1920.0, h: 1080.0 };
        let work = Rect { x: -1920.0, y: -300.0, w: 1920.0, h: 1040.0 };
        let screen = windows(frame, work);
        for spot in SPOTS {
            check_spot(&screen, spot, work, FLOAT_GAP_EDGE, FLOAT_GAP);
        }
        // A screen narrower than the window: still inside, the pill at its side.
        let small = windows(Rect { x: 0.0, y: 0.0, w: 400.0, h: 290.0 }, Rect { x: 0.0, y: 0.0, w: 400.0, h: 250.0 });
        for (v, h) in SPOTS {
            let p = place(&small, Spot::new(v, h));
            assert!(inside(Rect { x: 0.0, y: 0.0, w: 400.0, h: 250.0 }, p.window), "{v:?} {h:?}");
            assert!(inside(p.window, pill_on_screen(&p)), "{v:?} {h:?}");
        }
    }

    #[test]
    fn a_full_screen_window_fills_the_screen_and_a_zoomed_one_does_not() {
        // Measured on a 1920 x 1080 screen with a 30 pt menu bar: the
        // full-screen window is the whole screen.
        let plain = ScreenInfo { frame: Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 }, notch: None, bar_h: 30.0, work: None, menu_bar: true };
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
        let side = ScreenInfo { frame: Rect { x: -1920.0, y: -300.0, w: 1920.0, h: 1080.0 }, notch: None, bar_h: 24.0, work: None, menu_bar: true };
        assert!(fills_screen(&side, Rect { x: -1920.0, y: -300.0, w: 1920.0, h: 1080.0 }));
        assert!(!fills_screen(&side, Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 }));
    }

    #[test]
    fn the_menu_bar_counts_while_it_slides_in() {
        let plain = ScreenInfo { frame: Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 }, notch: None, bar_h: 30.0, work: None, menu_bar: true };
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
