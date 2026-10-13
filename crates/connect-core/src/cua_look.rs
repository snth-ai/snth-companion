//! How computer control looks on the owner's screen: the synth's cursor,
//! how it moves, when it hides, and how large a screenshot the synth gets.
//!
//! What Cua Driver 0.33.1 offers, read from its source at the pinned tag
//! (`libs/cua-driver/rust/crates/cursor-overlay`) and checked against the
//! real binary by `connectd selftest cua`:
//!
//!  - The daemon's start flags are the template of EVERY session's cursor:
//!    `--no-overlay`, `--cursor-theme <id>`, `--cursor-reduced-motion
//!    <auto|on|off>`, `--glide-ms`, `--dwell-ms`, `--idle-hide-ms`
//!    (`CursorConfig::parse`; the last three are missing from `--help`).
//!    So the owner's look is the flags of OUR daemon, and it holds for
//!    every session without the synth doing or knowing anything.
//!  - No colour setting. The built-in theme is recoloured by the driver,
//!    one of nine colours per session, picked from the session's name
//!    (`theme.rs`, `session_fill_rgba`); an installed theme keeps the
//!    colours it was made with. A fixed colour is therefore a theme: the
//!    app ships seven, compiled from the driver's own theme source with the
//!    driver's own compiler (`scripts/build-cursor-themes.py`).
//!  - Themes are looked up in `CUA_DRIVER_CURSOR_THEME_DIR`, else in a
//!    folder of the user's home that a driver the owner installed for
//!    themselves uses too (`theme_artifact.rs`, `theme_store_root`;
//!    `CUA_DRIVER_RS_HOME` does NOT move it). `DriverEnv` points every
//!    driver process of ours at `<our driver home>/cursor-themes`.
//!  - `--cursor-reduced-motion auto` does not look at the system: only `on`
//!    changes anything. "Follow the system" is resolved here.
//!  - The name under the cursor is the `session` label the synth passes on
//!    its calls (`cursor_events.rs`). There is no setting for its text or
//!    its colour, so the app has none either.
//!  - The driver's own settings file is `~/.cua-driver/config.json` on
//!    every system, for our daemon as well as for the owner's own driver
//!    (`CUA_DRIVER_RS_HOME` does not move it either). The app never writes
//!    it: the screenshot limit is put on each call instead, and the preview
//!    window is a start flag. For the same reason the synth's `set_config`
//!    is never allowed (`cua_guard::NEVER`): one of its keys is written to
//!    that shared file.
//!
//! A change of look needs a new daemon. The daemon is replaced only when no
//! computer-control session is open (`Shared::cua_session`), so a task in
//! progress keeps the look it started with.

use std::io;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::cua::DriverEnv;
use crate::settings::{CursorColor, CursorSpeed, LookSettings, ReducedMotion};

/// One fixed cursor colour: a compiled theme the app carries.
pub struct Theme {
    pub color: CursorColor,
    /// The id the driver knows it by (and its file name in the store).
    pub id: &'static str,
    /// The colour, for a swatch.
    pub hex: &'static str,
    bytes: &'static [u8],
}

macro_rules! theme {
    ($color:ident, $name:literal, $hex:literal) => {
        Theme {
            color: CursorColor::$color,
            id: concat!("ai.snth.connect.", $name),
            hex: $hex,
            bytes: include_bytes!(concat!(
                "../assets/cursor-themes/ai.snth.connect.",
                $name,
                ".cua-theme"
            )),
        }
    };
}

/// The themes the app ships. KEEP IN STEP with `LOOKS` in
/// `scripts/build-cursor-themes.py`, which builds the files.
pub const THEMES: &[Theme] = &[
    theme!(Sky, "sky", "#5EC0E8"),
    theme!(Violet, "violet", "#B284FF"),
    theme!(Rose, "rose", "#F0569A"),
    theme!(Green, "green", "#3FCF8E"),
    theme!(Amber, "amber", "#F4B242"),
    theme!(Red, "red", "#E85262"),
    theme!(Graphite, "graphite", "#4B5160"),
];

/// The theme of a fixed colour; None for the driver's own cursor.
pub fn theme_for(color: CursorColor) -> Option<&'static Theme> {
    THEMES.iter().find(|t| t.color == color)
}

/// The motion values of a speed, in the driver's units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Motion {
    /// Fixed length of one move in milliseconds; 0 = the driver's own
    /// timing (about 490 points a second on average, so a short hop takes
    /// a quarter of a second and a move across a large screen three).
    pub glide_ms: u32,
    /// The pause after a click.
    pub dwell_ms: u32,
}

/// The driver's defaults (`MotionConfig::default`).
pub const DRIVER_MOTION: Motion = Motion { glide_ms: 0, dwell_ms: 80 };
/// The driver's default idle hide, in seconds.
pub const DRIVER_IDLE_HIDE_SECONDS: u32 = 15;

pub fn motion(speed: CursorSpeed) -> Motion {
    match speed {
        CursorSpeed::Fast => Motion { glide_ms: 250, dwell_ms: 40 },
        CursorSpeed::Normal => DRIVER_MOTION,
        CursorSpeed::Relaxed => Motion { glide_ms: 1500, dwell_ms: 400 },
    }
}

/// What this computer's driver can show, so a host hides a control that
/// would do nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Support {
    /// The cursor can be switched on and off.
    pub cursor: bool,
    /// The cursor can have a fixed colour (an installed theme is drawn).
    pub color: bool,
    /// Speed, idle hide and reduced motion are followed.
    pub motion: bool,
    /// The experimental preview window exists.
    pub preview_window: bool,
}

/// macOS, Windows and Linux X11 draw the cursor with the driver's shared
/// renderer, which follows the theme and the motion values. On a Wayland
/// session the driver may hand the cursor to a helper inside the desktop
/// shell, which draws its own and takes only a position and the session's
/// colour (`platform-linux/src/overlay.rs`, `dispatch_shell_helper_command`),
/// so there only the on/off switch is offered. The preview window exists on
/// macOS only (elsewhere the driver prints "not yet implemented").
pub fn support() -> Support {
    let wayland = cfg!(target_os = "linux")
        && (std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v.eq_ignore_ascii_case("wayland"))
            || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()));
    Support {
        cursor: true,
        color: !wayland,
        motion: !wayland,
        preview_window: cfg!(target_os = "macos"),
    }
}

/// Put the app's themes into the driver's theme folder: the same file per
/// theme, under the same name, that `cua-driver cursor-theme install`
/// writes. A file that is already there and equal is left alone.
pub fn install_themes(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for theme in THEMES {
        let path = dir.join(format!("{}.cua-theme", theme.id));
        // The driver refuses anything but a plain file.
        let plain = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file());
        if plain && std::fs::read(&path).is_ok_and(|have| have == theme.bytes) {
            continue;
        }
        let staged = dir.join(format!(".{}.{}.tmp", theme.id, std::process::id()));
        std::fs::write(&staged, theme.bytes)?;
        if let Err(e) = std::fs::rename(&staged, &path) {
            let _ = std::fs::remove_file(&staged);
            return Err(e);
        }
    }
    Ok(())
}

/// The start flags of our daemon for this look. `themes`: the app's themes
/// are in the daemon's theme folder. `system_reduced`: the owner asked the
/// computer to reduce motion. `support`: what this computer's driver
/// follows (a flag for something it does not is not passed).
pub fn daemon_args(
    look: &LookSettings,
    themes: bool,
    system_reduced: bool,
    support: Support,
) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if !look.cursor {
        args.push("--no-overlay".into());
    } else {
        if themes && support.color {
            if let Some(theme) = theme_for(look.color) {
                args.push("--cursor-theme".into());
                args.push(theme.id.into());
            }
        }
        if support.motion {
            // Never `auto`: the driver does not resolve it.
            let reduced = match look.reduced_motion {
                ReducedMotion::System => system_reduced,
                ReducedMotion::Always => true,
                ReducedMotion::Never => false,
            };
            args.push("--cursor-reduced-motion".into());
            args.push(if reduced { "on" } else { "off" }.into());
            let m = motion(look.speed);
            if m != DRIVER_MOTION {
                args.push("--glide-ms".into());
                args.push(m.glide_ms.to_string());
                args.push("--dwell-ms".into());
                args.push(m.dwell_ms.to_string());
            }
            if look.idle_hide_seconds != DRIVER_IDLE_HIDE_SECONDS {
                args.push("--idle-hide-ms".into());
                // The driver takes at most a minute; 0 = never hide.
                args.push((u64::from(look.idle_hide_seconds.min(60)) * 1000).to_string());
            }
        }
    }
    if look.preview_window && support.preview_window {
        args.push("--experimental-pip".into());
    }
    args
}

/// The flags for a daemon that is about to start (or the ones a running
/// daemon should have): puts the themes in place, reads the system's
/// "reduce motion", and builds the flags. Without a private driver home
/// there is no theme folder of our own, and then no theme is selected: the
/// only other place is a folder a driver of the owner's uses.
pub fn resolve(look: &LookSettings, env: &DriverEnv) -> Vec<String> {
    let themes = match env.theme_dir() {
        Some(dir) => match install_themes(&dir) {
            Ok(()) => true,
            Err(e) => {
                crate::logln!("cua look: the cursor themes could not be written to {}: {e}", dir.display());
                false
            }
        },
        None => false,
    };
    daemon_args(look, themes, crate::presence::reduced_motion(), support())
}

// --- the mcp pipe: what the synth may not change ---

/// The tools that change how a session's cursor looks.
const CURSOR_SETTERS: [&str; 3] = [
    "set_agent_cursor_enabled",
    "set_agent_cursor_motion",
    "set_agent_cursor_theme",
];

/// The tools whose screenshot the owner's limit applies to: the ones with
/// a `max_image_dimension` argument in the pinned driver (the self-test
/// checks the catalog still says so).
pub const SCREENSHOT_TOOLS: [&str; 2] = ["get_window_state", "get_desktop_state"];

/// What the pipe does with one `tools/call` of the synth's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// Forward as it is.
    Pass,
    /// Forward; the arguments were changed (the journal says how).
    Changed(&'static str),
    /// Do not forward; answer the synth with this.
    Refuse(&'static str),
}

/// Keep the owner's look the owner's: decide one `tools/call` (already
/// parsed, `tool` is its name) and change its arguments where needed.
///
/// Refused, not silently dropped: the synth is told the truth and goes on.
///  - `set_agent_cursor_*`: the cursor's look. (`set_config`, which would
///    lift the screenshot limit and switch the preview window on, is never
///    allowed at all: `cua_guard::NEVER`.)
///
///  - `move_cursor`, while the owner has the cursor switched off: the tool
///    moves nothing but that cursor, and the driver's own answer then
///    ("overlay is unavailable ... use a GUI private worker") sends the
///    synth looking for a fault that is not there.
///
/// Changed:
///  - `start_session`: a `cursor_theme` is taken out (the session starts).
///  - a screenshot tool, when the owner set a limit: `max_image_dimension`
///    is put on the call, or brought down to the limit.
pub fn gate(tool: &str, msg: &mut Value, look: &LookSettings) -> Gate {
    if CURSOR_SETTERS.contains(&tool) {
        return Gate::Refuse(
            "the cursor's look on this computer is its owner's choice, in SNTH Connect under Computer control; go on without changing it, and tell the owner where the setting is if they asked for another look",
        );
    }
    if tool == "move_cursor" && !look.cursor {
        return Gate::Refuse(
            "the owner of this computer switched the agent cursor off, in SNTH Connect under Computer control; nothing is wrong, go on without it, and tell the owner where the switch is if they miss the cursor",
        );
    }
    let Some(params) = msg.get_mut("params").and_then(Value::as_object_mut) else {
        return Gate::Pass;
    };
    if tool == "start_session" {
        let removed = params
            .get_mut("arguments")
            .and_then(Value::as_object_mut)
            .is_some_and(|args| args.remove("cursor_theme").is_some());
        return if removed { Gate::Changed("cursor_theme removed") } else { Gate::Pass };
    }
    if look.screenshot_max > 0 && SCREENSHOT_TOOLS.contains(&tool) {
        let limit = u64::from(look.screenshot_max);
        // A call without arguments gets some: for the whole desktop the
        // driver's default is the full size.
        let args = params.entry("arguments").or_insert(Value::Null);
        if args.is_null() {
            *args = Value::Object(serde_json::Map::new());
        }
        if let Some(args) = args.as_object_mut() {
            // 0 asks for the full size.
            let asked = args.get("max_image_dimension").and_then(Value::as_u64);
            if !asked.is_some_and(|v| v > 0 && v <= limit) {
                args.insert("max_image_dimension".into(), Value::from(limit));
                return Gate::Changed("screenshot limited");
            }
        }
    }
    Gate::Pass
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ALL: Support = Support { cursor: true, color: true, motion: true, preview_window: true };

    fn call(tool: &str, args: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": tool, "arguments": args}})
    }

    #[test]
    fn the_default_look_changes_nothing_but_names_the_motion_choice() {
        let look = LookSettings::default();
        assert_eq!(daemon_args(&look, true, false, ALL), ["--cursor-reduced-motion", "off"]);
        // The system asks for less motion: the default follows it.
        assert_eq!(daemon_args(&look, true, true, ALL), ["--cursor-reduced-motion", "on"]);
    }

    #[test]
    fn every_choice_becomes_a_flag_of_the_daemon() {
        let look = LookSettings {
            cursor: true,
            color: CursorColor::Rose,
            speed: CursorSpeed::Fast,
            idle_hide_seconds: 5,
            reduced_motion: ReducedMotion::Always,
            screenshot_max: 1024,
            preview_window: true,
        };
        assert_eq!(
            daemon_args(&look, true, false, ALL),
            [
                "--cursor-theme",
                "ai.snth.connect.rose",
                "--cursor-reduced-motion",
                "on",
                "--glide-ms",
                "250",
                "--dwell-ms",
                "40",
                "--idle-hide-ms",
                "5000",
                "--experimental-pip",
            ]
        );
        let relaxed = LookSettings { speed: CursorSpeed::Relaxed, idle_hide_seconds: 0, reduced_motion: ReducedMotion::Never, ..LookSettings::default() };
        assert_eq!(
            daemon_args(&relaxed, true, true, ALL),
            ["--cursor-reduced-motion", "off", "--glide-ms", "1500", "--dwell-ms", "400", "--idle-hide-ms", "0"]
        );
    }

    #[test]
    fn a_hidden_cursor_is_one_flag() {
        let look = LookSettings { cursor: false, color: CursorColor::Red, speed: CursorSpeed::Fast, ..LookSettings::default() };
        assert_eq!(daemon_args(&look, true, false, ALL), ["--no-overlay"]);
    }

    #[test]
    fn no_flag_for_what_this_computer_does_not_follow() {
        let look = LookSettings {
            color: CursorColor::Green,
            speed: CursorSpeed::Fast,
            idle_hide_seconds: 60,
            preview_window: true,
            ..LookSettings::default()
        };
        // No theme folder of our own: no theme is selected.
        let no_themes = daemon_args(&look, false, false, ALL);
        assert!(!no_themes.contains(&"--cursor-theme".to_string()));
        // A Wayland shell helper, and a system without the preview window.
        let bare = Support { cursor: true, color: false, motion: false, preview_window: false };
        assert!(daemon_args(&look, true, false, bare).is_empty());
        assert_eq!(daemon_args(&LookSettings { cursor: false, ..look.clone() }, true, false, bare), ["--no-overlay"]);
    }

    #[test]
    fn the_shipped_themes_are_what_the_driver_reads() {
        assert_eq!(THEMES.len(), 7);
        for theme in THEMES {
            // The artifact header of the pinned driver (`theme_artifact.rs`):
            // magic, version 3.
            assert_eq!(&theme.bytes[..8], b"CUATHEM3", "{}", theme.id);
            assert_eq!(u16::from_le_bytes([theme.bytes[8], theme.bytes[9]]), 3, "{}", theme.id);
            assert!(theme.bytes.len() < 64 * 1024, "{}", theme.id);
            assert!(theme.id.starts_with("ai.snth.connect."));
            assert!(theme_for(theme.color).is_some_and(|t| t.id == theme.id));
            assert_eq!(theme.hex.len(), 7);
        }
        assert!(theme_for(CursorColor::Auto).is_none());
        // Every colour the settings know, except the driver's own, has one.
        for color in [
            CursorColor::Sky,
            CursorColor::Violet,
            CursorColor::Rose,
            CursorColor::Green,
            CursorColor::Amber,
            CursorColor::Red,
            CursorColor::Graphite,
        ] {
            assert!(theme_for(color).is_some(), "{color:?}");
        }
    }

    #[test]
    fn themes_are_installed_once_and_repaired() {
        let dir = std::env::temp_dir().join(format!("connect-look-themes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        install_themes(&dir).unwrap();
        let rose = dir.join("ai.snth.connect.rose.cua-theme");
        assert_eq!(std::fs::read(&rose).unwrap(), theme_for(CursorColor::Rose).unwrap().bytes);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), THEMES.len());
        // A damaged file is put right; nothing else is left behind.
        std::fs::write(&rose, b"broken").unwrap();
        install_themes(&dir).unwrap();
        assert_eq!(std::fs::read(&rose).unwrap(), theme_for(CursorColor::Rose).unwrap().bytes);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), THEMES.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_synth_cannot_change_the_cursor() {
        let look = LookSettings::default();
        for tool in ["set_agent_cursor_enabled", "set_agent_cursor_motion", "set_agent_cursor_theme"] {
            let mut msg = call(tool, json!({"session": "task-a", "enabled": false}));
            assert!(matches!(gate(tool, &mut msg, &look), Gate::Refuse(_)), "{tool}");
        }
        // Moving the cursor is the synth's own business while there is one.
        let mut moved = call("move_cursor", json!({"session": "task-a", "x": 10, "y": 10}));
        assert_eq!(gate("move_cursor", &mut moved, &look), Gate::Pass);
        let hidden = LookSettings { cursor: false, ..LookSettings::default() };
        assert!(matches!(gate("move_cursor", &mut moved, &hidden), Gate::Refuse(_)));
        let mut click = call("click", json!({"session": "task-a", "x": 10, "y": 10}));
        assert_eq!(gate("click", &mut click, &hidden), Gate::Pass);
        // Reading is free.
        for tool in ["get_agent_cursor_state", "get_config", "click", "end_session"] {
            let mut msg = call(tool, json!({"session": "task-a"}));
            assert_eq!(gate(tool, &mut msg, &look), Gate::Pass, "{tool}");
        }
    }

    #[test]
    fn a_session_starts_without_the_synths_theme() {
        let look = LookSettings::default();
        let mut msg = call("start_session", json!({"session": "task-a", "cursor_theme": {"theme_id": "cua.default"}}));
        assert_eq!(gate("start_session", &mut msg, &look), Gate::Changed("cursor_theme removed"));
        assert_eq!(msg["params"]["arguments"], json!({"session": "task-a"}));
        let mut plain = call("start_session", json!({"session": "task-a"}));
        assert_eq!(gate("start_session", &mut plain, &look), Gate::Pass);
    }

    #[test]
    fn the_screenshot_limit_is_put_on_the_call() {
        let limited = LookSettings { screenshot_max: 1024, ..LookSettings::default() };
        for (asked, want) in [
            (json!({"pid": 1}), 1024),                               // nothing asked
            (json!({"pid": 1, "max_image_dimension": 0}), 1024),     // full size asked
            (json!({"pid": 1, "max_image_dimension": 4000}), 1024),  // more than the limit
            (json!({"pid": 1, "max_image_dimension": "big"}), 1024), // not a number
        ] {
            let mut msg = call("get_window_state", asked);
            assert_eq!(gate("get_window_state", &mut msg, &limited), Gate::Changed("screenshot limited"));
            assert_eq!(msg["params"]["arguments"]["max_image_dimension"], want);
            assert_eq!(msg["params"]["arguments"]["pid"], 1);
        }
        // Less than the limit is the synth's own business.
        let mut small = call("get_desktop_state", json!({"max_image_dimension": 800}));
        assert_eq!(gate("get_desktop_state", &mut small, &limited), Gate::Pass);
        assert_eq!(small["params"]["arguments"]["max_image_dimension"], 800);
        // A call without arguments at all.
        let mut bare = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "get_desktop_state"}});
        assert_eq!(gate("get_desktop_state", &mut bare, &limited), Gate::Changed("screenshot limited"));
        assert_eq!(bare["params"]["arguments"], json!({"max_image_dimension": 1024}));
        // Other tools are not given an argument they do not have.
        let mut click = call("click", json!({"x": 1, "y": 2}));
        assert_eq!(gate("click", &mut click, &limited), Gate::Pass);
        assert!(click["params"]["arguments"].get("max_image_dimension").is_none());
        // No limit set: nothing is touched.
        let mut free = call("get_window_state", json!({"pid": 1, "max_image_dimension": 0}));
        assert_eq!(gate("get_window_state", &mut free, &LookSettings::default()), Gate::Pass);
        assert_eq!(free["params"]["arguments"]["max_image_dimension"], 0);
    }
}
