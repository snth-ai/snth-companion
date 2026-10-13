//! Which apps the synth may use through computer control.
//!
//! The owner has three choices (`settings::AppsPolicy`): every app, every
//! app except some, only some. What Cua Driver 0.34.1 can do about it was
//! read in its source at the pinned tag (`cua-driver-core`:
//! `session_manifest.rs`, `authorization.rs`, `tool.rs`; none of the three
//! changed between 0.33.1 and 0.34.1) and checked on our own daemon
//! (`connectd selftest cua`):
//!
//!  - **Only these** is the driver's own business. Our daemon is started in
//!    bounded permission mode with a capability manifest (`Scope::manifest`)
//!    that lists the chosen apps and sets `desktop.display: false`. The
//!    driver then refuses every observation and every input whose process
//!    or window is not of a listed app (`bounded_resource_outside_manifest`),
//!    and everything that has no process or window at all: the picture of
//!    the whole screen, a click by screen position, the unfiltered lists.
//!  - **Every app except these** does not exist in the driver: a manifest
//!    only allows. It is enforced here, in the pipe: every call is resolved
//!    to an app (from its pid and window id) and refused when that app is
//!    excluded, or when the call names no window, or when the app cannot be
//!    told.
//!
//! The pipe makes the same decision in both restricted choices (`decide`),
//! so "only these" holds twice, and three things the driver does not check
//! even in bounded mode are covered: `launch_app` (the manifest's `launch`
//! flag is read and never used), and the two lists the bounded daemon
//! refuses outright, `list_apps` and `list_windows`, which the app answers
//! itself from its own reading of the driver's lists, cut down to the apps
//! the synth may use.
//!
//! That reading is the `Directory`: a second runtime of the same driver
//! binary (`cua-driver mcp --direct`, no daemon, no socket) that only ever
//! runs `list_apps` and `list_windows`. The synth never reaches it. It also
//! feeds the app picker of the settings.
//!
//! A scope belongs to a daemon for its whole life (the manifest is read
//! once at start). A change of the owner's choice starts a new daemon when
//! no computer-control session holds the old one, exactly like the look.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

use crate::cua::DriverEnv;
use crate::settings::{AppRef, AppsPolicy, Settings};

/// How long a bounded daemon may live: the driver's maximum for
/// `expires_after`. `idle_timeout` is the same, so the daemon never lapses
/// for want of calls (an idle lapse is final for that daemon).
pub const LEASE: Duration = Duration::from_secs(24 * 60 * 60);
/// A bounded daemon older than this is replaced the next time no session
/// holds it, so a task never starts on a lease that is about to run out.
pub const RENEW_AFTER: Duration = Duration::from_secs(12 * 60 * 60);

/// How long the pid -> app answer of `list_apps` is trusted. Short, because
/// a pid can be given to another program after its owner quit.
const APPS_TTL: Duration = Duration::from_secs(60);
/// How long the window -> pid answer of `list_windows` is trusted.
const WINDOWS_TTL: Duration = Duration::from_secs(5);
/// An unknown pid or window makes the directory read the list again, but
/// not more often than this.
const REFRESH_GAP: Duration = Duration::from_secs(2);
/// One call of the directory's own runtime.
const LISTER_TIMEOUT: Duration = Duration::from_secs(45);

/// One app as the driver's `list_apps` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppRow {
    /// 0 when it is installed and not running.
    pub pid: i64,
    pub name: String,
    /// macOS.
    pub bundle_id: Option<String>,
    /// Where it is started from: the bundle on macOS, the program on
    /// Windows and Linux.
    pub path: Option<String>,
    pub running: bool,
}

impl AppRow {
    /// What the driver matches this app by in a capability manifest: the
    /// bundle id where there is one, else the path of the program.
    pub fn key(&self) -> Option<&str> {
        self.bundle_id.as_deref().filter(|b| !b.is_empty()).or(self.path.as_deref().filter(|p| !p.is_empty()))
    }
}

/// One window as `list_windows` reports it: the id and its owner, nothing
/// else (titles are never kept).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WinRow {
    pub window_id: u64,
    pub pid: i64,
}

fn same_path(a: &str, b: &str) -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// Is `app` the one the owner picked?
pub fn matches(pick: &AppRef, app: &AppRow) -> bool {
    if pick.loose {
        // A word typed by hand before the picker existed: any app whose
        // name, bundle id or path has it.
        let word = pick.id.to_lowercase();
        return !word.is_empty()
            && [Some(app.name.as_str()), app.bundle_id.as_deref(), app.path.as_deref()]
                .into_iter()
                .flatten()
                .any(|field| field.to_lowercase().contains(&word));
    }
    app.bundle_id.as_deref().is_some_and(|b| b.eq_ignore_ascii_case(&pick.id))
        || app.path.as_deref().is_some_and(|p| same_path(p, &pick.id))
}

/// The owner's choice as one daemon was started with it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Scope {
    pub policy: AppsPolicy,
    /// The list the choice goes by: the excluded apps, or the only ones.
    pub apps: Vec<AppRef>,
    /// `Only`: the folders shared for writing, which the manifest has to
    /// name too or the bounded driver refuses a screenshot file the app's
    /// own check let through. Empty otherwise.
    pub write_roots: Vec<PathBuf>,
}

impl Scope {
    /// Every app (and what a daemon of the user's own gets: nothing of
    /// ours limits it).
    pub fn open() -> Scope {
        Scope::default()
    }

    pub fn of(settings: &Settings) -> Scope {
        let scope = &settings.computer.apps;
        let files = &settings.files;
        let write_roots = if scope.policy == AppsPolicy::Only && files.enabled && !files.ask_on_write {
            files.roots.iter().filter(|r| r.write).map(|r| r.path.clone()).collect()
        } else {
            Vec::new()
        };
        Scope { policy: scope.policy, apps: scope.active().to_vec(), write_roots }
    }

    pub fn restricted(&self) -> bool {
        self.policy != AppsPolicy::All
    }

    /// May the synth use this app?
    pub fn allows(&self, app: &AppRow) -> bool {
        let listed = self.apps.iter().any(|pick| matches(pick, app));
        match self.policy {
            AppsPolicy::All => true,
            AppsPolicy::Except => !listed,
            AppsPolicy::Only => listed,
        }
    }

    /// The capability manifest of a daemon for "only these"; None for the
    /// other choices, whose daemon runs in the driver's standard mode.
    ///
    /// Deny by default: a tool must be listed and every window or process
    /// it touches must be of a listed app. No browser origins: a manifest
    /// with them may not allow the window tools at all, so the driver's
    /// page tools are left out instead (`MANIFEST_TOOLS`).
    /// `desktop.display: false`: no picture of the whole screen and no
    /// input by screen position, both of which would reach every app.
    pub fn manifest(&self) -> Option<String> {
        if self.policy != AppsPolicy::Only {
            return None;
        }
        let hours = LEASE.as_secs() / 3600;
        let mut y = format!("version: 3\nexpires_after: {hours}h\nidle_timeout: {hours}h\nallow:\n  tools:\n");
        for tool in MANIFEST_TOOLS {
            y.push_str(&format!("    - {tool}\n"));
        }
        y.push_str("resources:\n");
        let mut seen: Vec<String> = Vec::new();
        let mut apps = String::new();
        for pick in &self.apps {
            if pick.loose || pick.id.is_empty() {
                continue;
            }
            // The driver takes a path only in its resolved form, and
            // refuses a manifest that names one app twice.
            let (field, value) = if Path::new(&pick.id).is_absolute() {
                let resolved = std::fs::canonicalize(&pick.id)
                    .map(|p| crate::files::plain(&p).to_string_lossy().to_string())
                    .unwrap_or_else(|_| pick.id.clone());
                ("executable", resolved)
            } else {
                ("bundle_id", pick.id.clone())
            };
            let dedup = value.to_lowercase();
            if seen.contains(&dedup) {
                continue;
            }
            seen.push(dedup);
            apps.push_str(&format!(
                "    - {field}: {}\n      launch: true\n      windows: all\n      terminate: driver_launched\n",
                yaml_string(&value)
            ));
        }
        if apps.is_empty() {
            y.push_str("  apps: []\n");
        } else {
            y.push_str("  apps:\n");
            y.push_str(&apps);
        }
        let mut roots = String::new();
        let mut seen_roots: Vec<PathBuf> = Vec::new();
        for root in &self.write_roots {
            // Only a folder that is there, in its resolved form, and never
            // the top of a disk: anything else the driver refuses to start
            // with.
            let Ok(resolved) = std::fs::canonicalize(root) else {
                continue;
            };
            let resolved = crate::files::plain(&resolved);
            if !resolved.is_dir() || resolved.parent().is_none() || seen_roots.contains(&resolved) {
                continue;
            }
            roots.push_str(&format!(
                "      - dir: {}\n        recursive: true\n",
                yaml_string(&resolved.to_string_lossy())
            ));
            seen_roots.push(resolved);
        }
        if !roots.is_empty() {
            y.push_str("  files:\n    write:\n");
            y.push_str(&roots);
        }
        y.push_str("  desktop:\n    display: false\n");
        Some(y)
    }
}

/// A YAML double-quoted scalar. JSON's string form is one.
fn yaml_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into())
}

/// The tools a daemon for "only these" allows. Every name is one the
/// pinned driver has reviewed (`authorization.rs`, `advertised_risk_for`):
/// it refuses to start with a name it does not know.
///
/// Left out on purpose:
///  - what the synth may never call (`cua_guard::NEVER`) and the cursor
///    setters (`cua_look`);
///  - `list_apps`, `get_desktop_state`, `get_accessibility_tree`,
///    `escalate_session`: they are about the whole screen, which
///    `desktop.display: false` refuses anyway (the app answers `list_apps`
///    and a `list_windows` without a pid itself);
///  - `clipboard_read`, `clipboard_write`: the driver refuses the system
///    clipboard under any manifest;
///  - the page tools (`get_browser_state`, `browser_*`): they need browser
///    origins in the manifest, and a manifest with origins may not allow
///    the window tools.
pub const MANIFEST_TOOLS: &[&str] = &[
    "start_session",
    "end_session",
    "get_session",
    "list_sessions",
    "get_session_state",
    "list_windows",
    "get_window_state",
    "verify_state",
    "zoom",
    "parse_visual_regions",
    "get_screen_size",
    "get_cursor_position",
    "get_agent_cursor_state",
    "move_cursor",
    "click",
    "double_click",
    "right_click",
    "drag",
    "scroll",
    "type_text",
    "press_key",
    "hotkey",
    "set_value",
    "invoke_menu",
    "launch_app",
    "kill_app",
    "bring_to_front",
    "set_window_frame",
    "page",
    "check_permissions",
    "health_report",
    "get_config",
    "get_recording_state",
    "stop_recording",
];

/// What a tool is about, for the decision below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// No app is touched: sessions, sizes, the driver's own state.
    Free,
    /// `list_apps`: answered by the app, cut down.
    ListApps,
    /// `list_windows`: answered by the app, cut down.
    ListWindows,
    /// The whole screen, whichever apps are on it.
    Screen,
    /// Starts an app the call names.
    Launch,
    /// Reads or acts on a window or a process the call has to name.
    Window,
    /// The agent's own pointer: a window when the call names one, the
    /// real pointer and the whole screen with `scope: "desktop"`.
    Pointer,
    /// The driver's page tools. A call that binds a browser window names
    /// it (pid, window id); the rest go by a target id the driver minted
    /// for such a bind on the same connection.
    Browser,
    /// The system clipboard: no app's, and refused by a bounded driver.
    Clipboard,
}

/// Every tool of the pinned driver (58 on macOS, plus the ones only the
/// Windows and Linux drivers have). A tool that is not here is refused
/// while the owner limits the apps, and fails `connectd selftest cua`.
pub const TOOLS: &[(&str, Kind)] = &[
    ("list_apps", Kind::ListApps),
    ("list_windows", Kind::ListWindows),
    ("get_desktop_state", Kind::Screen),
    ("get_accessibility_tree", Kind::Screen),
    ("escalate_session", Kind::Screen),
    ("launch_app", Kind::Launch),
    ("get_window_state", Kind::Window),
    ("verify_state", Kind::Window),
    ("zoom", Kind::Window),
    ("debug_window_info", Kind::Window),
    ("click", Kind::Window),
    ("double_click", Kind::Window),
    ("right_click", Kind::Window),
    ("drag", Kind::Window),
    ("scroll", Kind::Window),
    ("type_text", Kind::Window),
    ("type_text_chars", Kind::Window),
    ("press_key", Kind::Window),
    ("hotkey", Kind::Window),
    ("set_value", Kind::Window),
    ("mouse_button_down", Kind::Window),
    ("mouse_button_up", Kind::Window),
    ("mouse_drag", Kind::Window),
    ("parallel_mouse_drag", Kind::Window),
    ("invoke_menu", Kind::Window),
    ("bring_to_front", Kind::Window),
    ("set_window_frame", Kind::Window),
    ("kill_app", Kind::Window),
    ("page", Kind::Window),
    ("move_cursor", Kind::Pointer),
    ("get_browser_state", Kind::Browser),
    ("browser_prepare", Kind::Browser),
    ("browser_navigate", Kind::Browser),
    ("browser_click", Kind::Browser),
    ("browser_type", Kind::Browser),
    ("browser_dialog", Kind::Browser),
    ("browser_set_input_files", Kind::Browser),
    ("browser_download", Kind::Browser),
    ("browser_pointer", Kind::Browser),
    ("clipboard_read", Kind::Clipboard),
    ("clipboard_write", Kind::Clipboard),
    ("get_screen_size", Kind::Free),
    ("get_cursor_position", Kind::Free),
    ("get_agent_cursor_state", Kind::Free),
    ("set_agent_cursor_enabled", Kind::Free),
    ("set_agent_cursor_motion", Kind::Free),
    ("set_agent_cursor_theme", Kind::Free),
    ("check_permissions", Kind::Free),
    ("health_report", Kind::Free),
    ("get_config", Kind::Free),
    ("set_config", Kind::Free),
    ("start_session", Kind::Free),
    ("end_session", Kind::Free),
    ("get_session", Kind::Free),
    ("list_sessions", Kind::Free),
    ("get_session_state", Kind::Free),
    ("start_recording", Kind::Free),
    ("stop_recording", Kind::Free),
    ("get_recording_state", Kind::Free),
    ("replay_trajectory", Kind::Free),
    ("install_ffmpeg", Kind::Free),
    ("install_extension", Kind::Free),
    ("check_for_update", Kind::Free),
    ("parse_visual_regions", Kind::Free),
    ("history_status", Kind::Free),
    ("history_query", Kind::Free),
    ("probe", Kind::Free),
];

pub fn kind_of(tool: &str) -> Option<Kind> {
    TOOLS.iter().find(|(name, _)| *name == tool).map(|(_, kind)| *kind)
}

/// The tools of a driver catalog (the `tools` array of `tools/list`) that
/// nobody has placed in `TOOLS`. Empty = the decision below knows every
/// tool this driver has. The self-test fails on anything else.
pub fn unplaced(tools: &[Value]) -> Vec<String> {
    tools
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .filter(|name| kind_of(name).is_none())
        .map(str::to_string)
        .collect()
}

/// What the pipe does with one `tools/call` while the apps are limited.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Hand it to the driver.
    Pass,
    /// Do not hand it over: answer the synth with this result (a list the
    /// app made itself).
    Answer(Value),
    /// Do not hand it over. `tell` is for the synth (it starts with
    /// `denied: ` or `unavailable: `), `journal` for the owner's activity
    /// list (never an app's or a window's name).
    Refuse { tell: String, journal: &'static str },
}

const WHERE: &str = "in SNTH Connect under Computer control";

fn refuse(tell: String, journal: &'static str) -> Verdict {
    Verdict::Refuse { tell, journal }
}

fn not_allowed(scope: &Scope) -> Verdict {
    match scope.policy {
        AppsPolicy::Only => refuse(
            format!("denied: the owner of this computer lets you use only the apps they chose {WHERE}, and this one is not among them; list_apps shows the ones you may use; if the task needs another app, tell the owner they can add it there"),
            "denied: not one of the apps the owner chose",
        ),
        _ => refuse(
            format!("denied: the owner of this computer does not let you use this app through SNTH Connect; leave it alone, and if the task needs it tell the owner they can change that {WHERE}"),
            "denied: an app the owner excluded",
        ),
    }
}

fn whole_screen() -> Verdict {
    refuse(
        format!("denied: while the owner limits which apps you may use, you work window by window: there is no picture of the whole screen and no action by screen position, because they would reach every app; use list_windows, then get_window_state and the window actions with pid and window_id; if the task cannot be done that way, tell the owner they can change the app setting {WHERE}"),
        "denied: the whole screen, while apps are limited",
    )
}

fn no_window() -> Verdict {
    refuse(
        format!("denied: this call does not say which window it is for; while the owner limits which apps you may use, pass pid and window_id (from list_windows) on every call, also next to an element_token; the owner's setting is {WHERE}"),
        "denied: no window named, while apps are limited",
    )
}

fn unknown_owner() -> Verdict {
    refuse(
        format!("denied: SNTH Connect could not tell which app this window or process belongs to, so it is not allowed while the owner limits which apps you may use; take pid and window_id from a fresh list_windows, and tell the owner if the window you need is not there (the setting is {WHERE})"),
        "denied: a window of no known app, while apps are limited",
    )
}

fn unavailable(why: &str) -> Verdict {
    refuse(
        format!("unavailable: SNTH Connect could not read the list of apps on this computer ({why}), and without it no app may be used while the owner limits them; try again in a moment, and tell the owner if it keeps failing"),
        "failed: the list of apps could not be read",
    )
}

/// The arguments of a `tools/call`.
fn arguments(msg: &Value) -> Option<&serde_json::Map<String, Value>> {
    msg.get("params")?.get("arguments")?.as_object()
}

/// What a call says about its target.
#[derive(Debug, Default, PartialEq, Eq)]
struct Target {
    pids: Vec<i64>,
    windows: Vec<u64>,
    /// `scope: "desktop"` or `target.kind: "desktop"`.
    desktop: bool,
}

fn target_of(args: Option<&serde_json::Map<String, Value>>) -> Target {
    let mut t = Target::default();
    let Some(args) = args else {
        return t;
    };
    let mut take = |holder: &serde_json::Map<String, Value>| {
        if let Some(pid) = holder.get("pid").and_then(Value::as_i64) {
            if !t.pids.contains(&pid) {
                t.pids.push(pid);
            }
        }
        if let Some(window) = holder.get("window_id").and_then(Value::as_u64) {
            if !t.windows.contains(&window) {
                t.windows.push(window);
            }
        }
    };
    take(args);
    if let Some(target) = args.get("target").and_then(Value::as_object) {
        take(target);
        if target.get("kind").and_then(Value::as_str) == Some("desktop") {
            t.desktop = true;
        }
    }
    if args.get("scope").and_then(Value::as_str) == Some("desktop") {
        t.desktop = true;
    }
    t
}

/// Decide one `tools/call` (already parsed, `tool` is its name) for a
/// scope that limits the apps. With `AppsPolicy::All` nothing is decided
/// here and the call passes.
pub async fn decide(scope: &Scope, tool: &str, msg: &Value, dir: &Directory) -> Verdict {
    if !scope.restricted() {
        return Verdict::Pass;
    }
    let Some(kind) = kind_of(tool) else {
        return refuse(
            format!("denied: SNTH Connect does not know the tool {tool:?} well enough to tell which app it touches, so it is not available while the owner limits which apps you may use; tell the owner, the setting is {WHERE}"),
            "denied: a tool the app cannot place, while apps are limited",
        );
    };
    let args = arguments(msg);
    let target = target_of(args);
    match kind {
        Kind::Free => Verdict::Pass,
        Kind::Screen => whole_screen(),
        Kind::Clipboard => match scope.policy {
            // The bounded driver refuses the clipboard under any manifest.
            AppsPolicy::Only => refuse(
                format!("denied: the clipboard is not one of the apps the owner chose for computer control; if the owner switched the clipboard on for you, device_apps has it; otherwise tell the owner (the app setting is {WHERE})"),
                "denied: the clipboard, while only chosen apps are allowed",
            ),
            _ => Verdict::Pass,
        },
        Kind::ListApps => match dir.apps().await {
            Ok(rows) => Verdict::Answer(apps_answer(scope, &rows)),
            Err(why) => unavailable(&why),
        },
        Kind::ListWindows => {
            // A list for one process is that process's business first.
            if let Err(verdict) = check_targets(scope, &target, dir).await {
                return verdict;
            }
            let on_screen_only = args.and_then(|a| a.get("on_screen_only")).and_then(Value::as_bool) == Some(true);
            let (listed, rows) = match (dir.windows_listing().await, dir.apps().await) {
                (Ok(listed), Ok(rows)) => (listed, rows),
                (Err(why), _) | (_, Err(why)) => return unavailable(&why),
            };
            Verdict::Answer(windows_answer(scope, &listed, &rows, target.pids.first().copied(), on_screen_only))
        }
        Kind::Launch => launch(scope, args, dir).await,
        Kind::Pointer => {
            if target.desktop {
                return whole_screen();
            }
            if target.pids.is_empty() && target.windows.is_empty() {
                // Without a window it moves the agent's own pointer and
                // nothing else. The bounded driver wants a window even so.
                return match scope.policy {
                    AppsPolicy::Only => no_window(),
                    _ => Verdict::Pass,
                };
            }
            match check_targets(scope, &target, dir).await {
                Ok(()) => Verdict::Pass,
                Err(verdict) => verdict,
            }
        }
        Kind::Window => {
            if target.desktop {
                return whole_screen();
            }
            if target.pids.is_empty() && target.windows.is_empty() {
                return no_window();
            }
            match check_targets(scope, &target, dir).await {
                Ok(()) => Verdict::Pass,
                Err(verdict) => verdict,
            }
        }
        Kind::Browser => {
            if scope.policy == AppsPolicy::Only {
                return refuse(
                    format!("denied: the helper's page tools (get_browser_state, browser_*) do not work while the owner limits you to chosen apps; if the browser is one of them, work in its window with get_window_state and the window actions; otherwise tell the owner they can change the app setting {WHERE}"),
                    "denied: the page tools, while only chosen apps are allowed",
                );
            }
            if !target.pids.is_empty() || !target.windows.is_empty() {
                // Binding a browser window: the window is named.
                return match check_targets(scope, &target, dir).await {
                    Ok(()) => Verdict::Pass,
                    Err(verdict) => verdict,
                };
            }
            if args.is_some_and(|a| a.get("target_id").and_then(Value::as_str).is_some_and(|t| !t.is_empty())) {
                // A target id is minted by the driver for a bind on this
                // connection only, and every bind went through the check
                // above under this same scope.
                return Verdict::Pass;
            }
            if tool == "browser_prepare" {
                // No pid: the driver starts a browser of its own choosing
                // (Chrome or Edge) with a profile of its own.
                let browser = |pick: &AppRef| {
                    let text = format!("{} {}", pick.id, pick.name).to_lowercase();
                    ["chrome", "chromium", "edge"].iter().any(|word| text.contains(word))
                };
                if scope.apps.iter().any(browser) {
                    return refuse(
                        format!("denied: the helper would start Chrome or Edge for this, and SNTH Connect cannot tell which, while the owner excludes one of them; name the browser window you mean with pid and window_id, or tell the owner (the setting is {WHERE})"),
                        "denied: starting a browser, while one is excluded",
                    );
                }
                return Verdict::Pass;
            }
            no_window()
        }
    }
}

/// Every process the call names, and the owner of every window it names,
/// must be an app the synth may use.
async fn check_targets(scope: &Scope, target: &Target, dir: &Directory) -> Result<(), Verdict> {
    let mut pids = target.pids.clone();
    for window in &target.windows {
        match dir.window_owner(*window).await {
            Ok(Some(pid)) => {
                if !pids.contains(&pid) {
                    pids.push(pid);
                }
            }
            Ok(None) => return Err(unknown_owner()),
            Err(why) => return Err(unavailable(&why)),
        }
    }
    for pid in pids {
        match dir.app_of_pid(pid).await {
            Ok(Some(app)) => {
                if !scope.allows(&app) {
                    return Err(not_allowed(scope));
                }
            }
            Ok(None) => return Err(unknown_owner()),
            Err(why) => return Err(unavailable(&why)),
        }
    }
    Ok(())
}

/// `launch_app`: the app the call names must be one the synth may use.
/// The driver does not check this even in bounded mode.
async fn launch(scope: &Scope, args: Option<&serde_json::Map<String, Value>>, dir: &Directory) -> Verdict {
    let text = |key: &str| args.and_then(|a| a.get(key)).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty());
    let (bundle_id, name) = (text("bundle_id"), text("name"));
    let path = text("launch_path").or(text("path"));
    if bundle_id.is_none() && name.is_none() && path.is_none() {
        return no_window();
    }
    let rows = match dir.apps().await {
        Ok(rows) => rows,
        Err(why) => return unavailable(&why),
    };
    // As the driver reads the call: the bundle id wins, then the path,
    // then the name.
    let found: Vec<&AppRow> = if let Some(bundle_id) = bundle_id {
        rows.iter().filter(|r| r.bundle_id.as_deref().is_some_and(|b| b.eq_ignore_ascii_case(bundle_id))).collect()
    } else if let Some(path) = path {
        rows.iter().filter(|r| r.path.as_deref().is_some_and(|p| same_path(p, path))).collect()
    } else {
        let name = name.unwrap_or_default();
        rows.iter().filter(|r| r.name.eq_ignore_ascii_case(name)).collect()
    };
    if found.is_empty() {
        return refuse(
            format!("denied: SNTH Connect does not find this app in the helper's list of apps, so it cannot tell whether you may start it while the owner limits which apps you may use; take bundle_id or the path from list_apps, and tell the owner if the app you need is not there (the setting is {WHERE})"),
            "denied: starting an app the app cannot place",
        );
    }
    // A name can belong to more than one app: all of them must be allowed.
    if found.iter().all(|app| scope.allows(app)) {
        Verdict::Pass
    } else {
        not_allowed(scope)
    }
}

/// `list_apps`, made by the app: the driver's own rows, without the apps
/// the synth may not use. The text is the app's (the driver's names every
/// app it found).
fn apps_answer(scope: &Scope, rows: &[AppRow]) -> Value {
    let allowed: Vec<&AppRow> = rows.iter().filter(|r| scope.allows(r)).collect();
    let running = allowed.iter().filter(|r| r.running).count();
    let mut text = format!("Found {} app(s) you may use here: {running} running, {} installed and not running.", allowed.len(), allowed.len() - running);
    for app in allowed.iter().filter(|r| r.running) {
        text.push_str(&format!("\n- {} (pid {})", app.name, app.pid));
        if let Some(key) = app.key() {
            text.push_str(&format!(" [{key}]"));
        }
    }
    let apps: Vec<Value> = allowed
        .iter()
        .map(|r| json!({"name": r.name, "pid": r.pid, "running": r.running, "bundle_id": r.bundle_id, "launch_path": r.path}))
        .collect();
    json!({"content": [{"type": "text", "text": text}], "structuredContent": {"apps": apps}})
}

/// `list_windows`, made by the app: the driver's own records (every field
/// as the driver gave it), without the windows of apps the synth may not
/// use and of processes that are no app the driver lists.
fn windows_answer(scope: &Scope, listed: &Value, rows: &[AppRow], pid: Option<i64>, on_screen_only: bool) -> Value {
    let allowed_pid = |p: i64| rows.iter().find(|r| r.running && r.pid == p).is_some_and(|app| scope.allows(app));
    let windows: Vec<Value> = listed
        .get("windows")
        .and_then(Value::as_array)
        .map(|all| {
            all.iter()
                .filter(|w| {
                    let Some(owner) = w.get("pid").and_then(Value::as_i64) else {
                        return false;
                    };
                    pid.is_none_or(|p| p == owner)
                        && allowed_pid(owner)
                        && (!on_screen_only || w.get("on_current_space").and_then(Value::as_bool) != Some(false))
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let mut structured = listed.as_object().cloned().unwrap_or_default();
    let text = format!("Found {} window(s) of the apps you may use here.", windows.len());
    structured.insert("windows".into(), Value::Array(windows));
    json!({"content": [{"type": "text", "text": text}], "structuredContent": structured})
}

// --- the directory: the app's own reading of the driver's lists ---

/// Which driver binary to run, asked each time a runtime is started.
pub type BinProvider = std::sync::Arc<dyn Fn() -> String + Send + Sync>;

/// Where the rows come from.
#[derive(Clone)]
enum Source {
    /// The driver, in a runtime of the app's own. `bin` says which binary
    /// to run when the runtime is started (after an update of the app the
    /// copy of the version before may be the one that is there).
    Driver { bin: BinProvider, env: DriverEnv },
    /// Rows given by a test.
    Fixed,
}

struct Lister {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

#[derive(Default)]
struct State {
    lister: Option<Lister>,
    apps: Vec<AppRow>,
    apps_at: Option<Instant>,
    /// The whole `structuredContent` of the last `list_windows`.
    windows: Value,
    owners: HashMap<u64, i64>,
    windows_at: Option<Instant>,
    /// How many times the lists were read from the source (tests).
    reads: (u64, u64),
}

/// The apps and windows of this computer as the driver lists them, kept
/// for a short while. One per core. Nothing of it is written anywhere.
pub struct Directory {
    source: std::sync::Mutex<Source>,
    state: tokio::sync::Mutex<State>,
}

impl Directory {
    pub fn driver(bin: BinProvider, env: DriverEnv) -> Directory {
        Directory {
            source: std::sync::Mutex::new(Source::Driver { bin, env }),
            state: tokio::sync::Mutex::new(State::default()),
        }
    }

    /// A directory with rows that never change by themselves (tests).
    pub fn fixed(apps: Vec<AppRow>, windows: Vec<WinRow>) -> Directory {
        let dir = Directory { source: std::sync::Mutex::new(Source::Fixed), state: tokio::sync::Mutex::new(State::default()) };
        dir.fix(apps, windows);
        dir
    }

    /// Give this directory rows of a test's own; no driver is asked from
    /// here on.
    pub fn fix(&self, apps: Vec<AppRow>, windows: Vec<WinRow>) {
        *self.source.lock().unwrap() = Source::Fixed;
        let mut st = self.state.try_lock().expect("a directory nobody is reading");
        st.set_windows(json!({"windows": windows.iter().map(|w| json!({"window_id": w.window_id, "pid": w.pid, "title": "", "on_current_space": true})).collect::<Vec<_>>()}));
        st.apps = apps;
        st.apps_at = Some(Instant::now());
        st.windows_at = None;
    }

    fn source(&self) -> Source {
        self.source.lock().unwrap().clone()
    }

    /// Every app the driver lists, installed or running. Read again when
    /// the last reading is older than a minute.
    pub async fn apps(&self) -> Result<Vec<AppRow>, String> {
        let mut st = self.state.lock().await;
        if st.apps_at.is_none_or(|at| at.elapsed() >= APPS_TTL) {
            self.read_apps(&mut st).await?;
        }
        Ok(st.apps.clone())
    }

    /// The same, read now: for the owner's picker.
    pub async fn apps_fresh(&self) -> Result<Vec<AppRow>, String> {
        let mut st = self.state.lock().await;
        self.read_apps(&mut st).await?;
        Ok(st.apps.clone())
    }

    /// The running app with this pid. A pid the last reading does not have
    /// makes the directory read again (an app may have just started).
    pub async fn app_of_pid(&self, pid: i64) -> Result<Option<AppRow>, String> {
        let mut st = self.state.lock().await;
        let find = |st: &State| st.apps.iter().find(|a| a.running && a.pid == pid).cloned();
        let fresh = st.apps_at.is_some_and(|at| at.elapsed() < APPS_TTL);
        if fresh {
            if let Some(app) = find(&st) {
                return Ok(Some(app));
            }
            if st.apps_at.is_some_and(|at| at.elapsed() < REFRESH_GAP) {
                return Ok(None);
            }
        }
        self.read_apps(&mut st).await?;
        Ok(find(&st))
    }

    /// The pid that owns this window.
    pub async fn window_owner(&self, window: u64) -> Result<Option<i64>, String> {
        let mut st = self.state.lock().await;
        let fresh = st.windows_at.is_some_and(|at| at.elapsed() < WINDOWS_TTL);
        if fresh {
            if let Some(pid) = st.owners.get(&window) {
                return Ok(Some(*pid));
            }
            if st.windows_at.is_some_and(|at| at.elapsed() < REFRESH_GAP) {
                return Ok(None);
            }
        }
        self.read_windows(&mut st).await?;
        Ok(st.owners.get(&window).copied())
    }

    /// The driver's whole answer to `list_windows`, read now.
    pub async fn windows_listing(&self) -> Result<Value, String> {
        let mut st = self.state.lock().await;
        self.read_windows(&mut st).await?;
        Ok(st.windows.clone())
    }

    /// Stop the directory's own runtime and forget what it read.
    pub async fn stop(&self) {
        let mut st = self.state.lock().await;
        if let Some(lister) = st.lister.take() {
            lister.stop().await;
        }
        if matches!(self.source(), Source::Driver { .. }) {
            st.apps_at = None;
            st.windows_at = None;
        }
    }

    /// How many times the two lists were read from the source.
    pub async fn reads(&self) -> (u64, u64) {
        self.state.lock().await.reads
    }

    async fn read_apps(&self, st: &mut State) -> Result<(), String> {
        st.reads.0 += 1;
        if let Source::Driver { .. } = self.source() {
            let listed = self.call(st, "list_apps").await?;
            st.apps = parse_apps(&listed);
        }
        st.apps_at = Some(Instant::now());
        Ok(())
    }

    async fn read_windows(&self, st: &mut State) -> Result<(), String> {
        st.reads.1 += 1;
        if let Source::Driver { .. } = self.source() {
            let listed = self.call(st, "list_windows").await?;
            st.set_windows(listed);
        }
        st.windows_at = Some(Instant::now());
        Ok(())
    }

    /// One read-only call of the directory's runtime; its
    /// `structuredContent`. A runtime that fails is stopped and started
    /// once more.
    async fn call(&self, st: &mut State, tool: &str) -> Result<Value, String> {
        let Source::Driver { bin, env } = self.source() else {
            return Err("no driver".into());
        };
        let mut last = String::new();
        for _ in 0..2 {
            if st.lister.is_none() {
                match tokio::time::timeout(LISTER_TIMEOUT, Lister::start(&bin(), &env)).await {
                    Ok(Ok(lister)) => st.lister = Some(lister),
                    Ok(Err(why)) => return Err(why),
                    Err(_) => return Err("the helper did not start in time".into()),
                }
            }
            let lister = st.lister.as_mut().expect("just started");
            match tokio::time::timeout(LISTER_TIMEOUT, lister.tool(tool)).await {
                Ok(Ok(result)) => return Ok(result),
                Ok(Err(why)) => last = why,
                Err(_) => last = format!("{tool} did not answer in time"),
            }
            if let Some(dead) = st.lister.take() {
                dead.stop().await;
            }
        }
        Err(last)
    }
}

impl State {
    fn set_windows(&mut self, listed: Value) {
        self.owners = listed
            .get("windows")
            .and_then(Value::as_array)
            .map(|all| {
                all.iter()
                    .filter_map(|w| Some((w.get("window_id")?.as_u64()?, w.get("pid")?.as_i64()?)))
                    .collect()
            })
            .unwrap_or_default();
        self.windows = listed;
    }
}

fn parse_apps(listed: &Value) -> Vec<AppRow> {
    let text = |app: &Value, key: &str| app.get(key).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
    listed
        .get("apps")
        .and_then(Value::as_array)
        .map(|all| {
            all.iter()
                .map(|app| {
                    let pid = app.get("pid").and_then(Value::as_i64).unwrap_or(0);
                    AppRow {
                        pid,
                        name: text(app, "name").unwrap_or_default(),
                        bundle_id: text(app, "bundle_id"),
                        path: text(app, "launch_path"),
                        running: app.get("running").and_then(Value::as_bool).unwrap_or(pid > 0) && pid > 0,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

impl Lister {
    /// `cua-driver mcp --direct`: the runtime lives in that one process,
    /// has no daemon and no socket, and draws no cursor. Embedded, like
    /// our daemon, so it never asks the system for anything.
    async fn start(bin: &str, env: &DriverEnv) -> Result<Lister, String> {
        let mut cmd = crate::proc::command(bin);
        cmd.args(["mcp", "--direct", "--embedded", "--host-bundle-id", crate::cua::HOST_BUNDLE_ID, "--no-overlay"]);
        env.apply(&mut cmd);
        cmd.env("CUA_DRIVER_EMBEDDED", "1")
            .env("CUA_DRIVER_HOST_BUNDLE_ID", crate::cua::HOST_BUNDLE_ID)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if cfg!(windows) {
            cmd.env("CUA_DRIVER_EMBEDDED_HOST_PID", std::process::id().to_string());
        }
        let mut child = cmd.spawn().map_err(|e| format!("the helper could not be started: {e}"))?;
        let stdin = child.stdin.take().ok_or("the helper has no input")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("the helper has no output")?);
        let mut lister = Lister { child, stdin, stdout, next_id: 0 };
        lister
            .request(
                "initialize",
                json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "snth-connect-apps", "version": env!("CARGO_PKG_VERSION")}}),
            )
            .await?;
        lister.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await?;
        Ok(lister)
    }

    async fn send(&mut self, message: &Value) -> Result<(), String> {
        let mut line = serde_json::to_vec(message).map_err(|e| e.to_string())?;
        line.push(b'\n');
        self.stdin.write_all(&line).await.map_err(|e| format!("the helper stopped listening: {e}"))?;
        self.stdin.flush().await.map_err(|e| format!("the helper stopped listening: {e}"))
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).await?;
        loop {
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).await.map_err(|e| format!("the helper stopped answering: {e}"))?;
            if n == 0 {
                return Err("the helper stopped answering".into());
            }
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(format!("the helper answered {method} with an error: {}", error.get("message").and_then(Value::as_str).unwrap_or("?")));
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    async fn tool(&mut self, tool: &str) -> Result<Value, String> {
        let result = self.request("tools/call", json!({"name": tool, "arguments": {}})).await?;
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            let said: String = result.pointer("/content/0/text").and_then(Value::as_str).unwrap_or("?").chars().take(200).collect();
            return Err(format!("the helper refused {tool}: {said}"));
        }
        result.get("structuredContent").cloned().ok_or_else(|| format!("the helper answered {tool} without a list"))
    }

    async fn stop(mut self) {
        // Closing its input ends it.
        drop(self.stdin);
        if tokio::time::timeout(Duration::from_secs(2), self.child.wait()).await.is_err() {
            let _ = self.child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), self.child.wait()).await;
        }
    }
}

// --- for the owner's settings: the picker and the first exclusions ---

/// One app in the owner's picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppChoice {
    /// What `settings::AppRef::id` takes.
    pub id: String,
    pub name: String,
    pub running: bool,
    /// Where the app is, when that is not the id already (macOS).
    pub path: Option<String>,
    /// A password manager or a place where the system's own security is
    /// set: offered first when the owner starts a list of exceptions.
    pub sensitive: bool,
}

/// Password managers and the system's own security surfaces, by the names
/// their apps carry. Matched against the driver's list of apps, so the
/// owner is only ever offered what is on this computer.
///
/// The driver protects none of them by itself: its "protected host" is
/// its own app (it refuses to target its own process and to launch its own
/// host), nothing else.
pub const SENSITIVE_APPS: &[&str] = &[
    // Password managers, on every system.
    "1Password",
    "1Password 7",
    "Bitwarden",
    "KeePassXC",
    "KeePass",
    "KeePass 2",
    "Dashlane",
    "LastPass",
    "Proton Pass",
    "NordPass",
    "Enpass",
    "Keeper Password Manager",
    // macOS.
    "Keychain Access",
    "Passwords",
    "System Settings",
    "System Preferences",
    // Windows.
    "Windows Security",
    "Credential Manager",
    "Settings",
    // Linux.
    "Passwords and Keys",
    "Seahorse",
    "KWalletManager",
];

fn sensitive(app: &AppRow) -> bool {
    SENSITIVE_APPS.iter().any(|known| app.name.eq_ignore_ascii_case(known))
}

/// The picker's list: every app the driver lists that has an identity the
/// driver can match, once each, running ones first.
pub fn choices(rows: &[AppRow]) -> Vec<AppChoice> {
    let mut out: Vec<AppChoice> = Vec::new();
    for row in rows {
        let Some(key) = row.key() else {
            continue;
        };
        if row.name.is_empty() {
            continue;
        }
        if let Some(have) = out.iter_mut().find(|c| c.id.eq_ignore_ascii_case(key)) {
            have.running |= row.running;
            continue;
        }
        out.push(AppChoice {
            id: key.to_string(),
            name: row.name.clone(),
            running: row.running,
            path: row.path.clone().filter(|p| p != key),
            sensitive: sensitive(row),
        });
    }
    out.sort_by(|a, b| b.running.cmp(&a.running).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

/// What the list of exceptions starts with when the owner first picks
/// "every app except these": the sensitive apps this computer has.
pub fn default_exclusions(choices: &[AppChoice]) -> Vec<AppRef> {
    choices
        .iter()
        .filter(|c| c.sensitive)
        .map(|c| AppRef { id: c.id.clone(), name: c.name.clone(), loose: false })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{AppScope, Preset, Root};

    fn app(pid: i64, name: &str, bundle_id: &str) -> AppRow {
        AppRow { pid, name: name.into(), bundle_id: Some(bundle_id.into()), path: Some(format!("/Applications/{name}.app")), running: pid > 0 }
    }

    fn pick(id: &str) -> AppRef {
        AppRef { id: id.into(), name: id.into(), loose: false }
    }

    fn rows() -> Vec<AppRow> {
        vec![
            app(100, "Finder", "com.apple.finder"),
            app(200, "Safari", "com.apple.Safari"),
            app(300, "1Password", "com.1password.1password"),
            app(0, "Mail", "com.apple.mail"),
            app(0, "Google Chrome", "com.google.Chrome"),
        ]
    }

    fn wins() -> Vec<WinRow> {
        vec![
            WinRow { window_id: 11, pid: 100 },
            WinRow { window_id: 21, pid: 200 },
            WinRow { window_id: 31, pid: 300 },
            // A helper process that is no app the driver lists.
            WinRow { window_id: 91, pid: 900 },
        ]
    }

    fn dir() -> Directory {
        Directory::fixed(rows(), wins())
    }

    fn scope(policy: AppsPolicy, ids: &[&str]) -> Scope {
        Scope { policy, apps: ids.iter().map(|id| pick(id)).collect(), write_roots: vec![] }
    }

    fn call(tool: &str, args: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": tool, "arguments": args}})
    }

    async fn verdict(scope: &Scope, tool: &str, args: Value) -> Verdict {
        decide(scope, tool, &call(tool, args), &dir()).await
    }

    fn told(v: &Verdict) -> &str {
        match v {
            Verdict::Refuse { tell, .. } => tell,
            other => panic!("not refused: {other:?}"),
        }
    }

    #[test]
    fn an_app_is_matched_by_its_identity_or_by_an_old_word() {
        let safari = app(200, "Safari", "com.apple.Safari");
        assert!(matches(&pick("com.apple.safari"), &safari), "a bundle id whatever its case");
        assert!(!matches(&pick("com.apple.Safar"), &safari), "never by a part of it");
        assert!(matches(&pick("/Applications/Safari.app"), &safari));
        assert!(!matches(&pick("Safari"), &safari), "a name is not an identity");
        let word = |w: &str| AppRef { id: w.into(), name: w.into(), loose: true };
        assert!(matches(&word("safari"), &safari));
        assert!(matches(&word("apple.saf"), &safari));
        assert!(!matches(&word("mail"), &safari));
        // Windows and Linux: the program's path.
        let notepad = AppRow { pid: 7, name: "Notepad".into(), bundle_id: None, path: Some("C:\\Windows\\System32\\notepad.exe".into()), running: true };
        assert_eq!(notepad.key(), Some("C:\\Windows\\System32\\notepad.exe"));
        assert!(matches(&pick("C:\\Windows\\System32\\notepad.exe"), &notepad));
        assert!(!matches(&pick("C:\\Windows\\System32\\calc.exe"), &notepad));
    }

    #[test]
    fn the_scope_is_the_owners_choice() {
        let mut s = Settings::from_preset(Preset::Balanced, vec![]);
        assert_eq!(Scope::of(&s), Scope::open());
        assert!(!Scope::of(&s).restricted());
        s.computer.apps = AppScope { policy: AppsPolicy::Except, except: vec![pick("com.apple.mail")], only: vec![pick("com.apple.Safari")] };
        let except = Scope::of(&s);
        assert_eq!((except.policy, except.apps.len()), (AppsPolicy::Except, 1));
        assert!(except.allows(&app(1, "Safari", "com.apple.Safari")));
        assert!(!except.allows(&app(1, "Mail", "com.apple.mail")));
        assert!(except.manifest().is_none(), "the driver has no manifest for exceptions");
        s.computer.apps.policy = AppsPolicy::Only;
        let only = Scope::of(&s);
        assert!(only.allows(&app(1, "Safari", "com.apple.Safari")));
        assert!(!only.allows(&app(1, "Mail", "com.apple.mail")));
        assert!(!only.allows(&app(1, "Finder", "com.apple.finder")));
        // "Every app": the lists are kept and not used.
        s.computer.apps.policy = AppsPolicy::All;
        assert!(Scope::of(&s).allows(&app(1, "Mail", "com.apple.mail")));
    }

    #[test]
    fn the_manifest_for_only_these() {
        let only = scope(AppsPolicy::Only, &["com.apple.Safari", "com.apple.finder", "COM.APPLE.SAFARI"]);
        let y = only.manifest().unwrap();
        let want = format!(
            "version: 3\nexpires_after: 24h\nidle_timeout: 24h\nallow:\n  tools:\n{}resources:\n  apps:\n    - bundle_id: \"com.apple.Safari\"\n      launch: true\n      windows: all\n      terminate: driver_launched\n    - bundle_id: \"com.apple.finder\"\n      launch: true\n      windows: all\n      terminate: driver_launched\n  desktop:\n    display: false\n",
            MANIFEST_TOOLS.iter().fold(String::new(), |all, t| all + "    - " + t + "\n")
        );
        assert_eq!(y, want);
        // Nothing chosen yet: a manifest that allows no app at all.
        assert!(scope(AppsPolicy::Only, &[]).manifest().unwrap().contains("  apps: []\n"));
        // No browser origins, ever: they would shut the window tools out.
        assert!(!y.contains("browser") && !y.contains("origins"));
        // An odd name cannot break out of its line.
        let odd = scope(AppsPolicy::Only, &["com.x\"\n  desktop:\n    display: true"]);
        let y = odd.manifest().unwrap();
        assert_eq!(y.lines().filter(|l| l.trim_start().starts_with("display:")).count(), 1, "{y}");
        assert_eq!(y.lines().filter(|l| l.trim_start().starts_with("- bundle_id:")).count(), 1, "{y}");
        assert!(y.ends_with("  desktop:\n    display: false\n"));
    }

    #[test]
    fn the_manifest_tools_are_ones_the_driver_reviewed_and_the_app_allows() {
        // `advertised_risk_for` of the pinned driver (cua-driver-core,
        // authorization.rs): a name outside it stops the daemon at start.
        const REVIEWED: &[&str] = &[
            "get_screen_size", "get_cursor_position", "get_config", "get_session", "list_sessions", "get_session_state",
            "get_agent_cursor_state", "get_recording_state", "check_for_update", "health_report", "probe", "click",
            "double_click", "right_click", "drag", "scroll", "move_cursor", "mouse_button_down", "mouse_button_up",
            "mouse_drag", "parallel_mouse_drag", "type_text", "type_text_chars", "press_key", "hotkey", "set_value",
            "invoke_menu", "launch_app", "bring_to_front", "set_window_frame", "start_session", "end_session",
            "set_agent_cursor_enabled", "set_agent_cursor_motion", "set_agent_cursor_theme", "clipboard_write", "zoom",
            "clipboard_read", "list_apps", "list_windows", "debug_window_info", "check_permissions",
            "get_accessibility_tree", "verify_state", "set_config", "escalate_session", "start_recording",
            "get_browser_state", "browser_prepare", "browser_navigate", "browser_click", "browser_type",
            "browser_pointer", "history_status", "history_query", "parse_visual_regions", "get_desktop_state",
            "get_window_state", "kill_app", "stop_recording", "replay_trajectory", "install_ffmpeg",
            "install_extension", "page", "browser_dialog", "browser_set_input_files", "browser_download",
        ];
        let mut seen = std::collections::BTreeSet::new();
        for tool in MANIFEST_TOOLS {
            assert!(seen.insert(tool), "{tool} is listed twice: the driver refuses that");
            assert!(REVIEWED.contains(tool), "{tool} is not a tool the pinned driver reviewed");
            assert!(crate::cua_guard::refusal(tool).is_none(), "{tool} is never allowed for the synth");
            assert!(!tool.starts_with("set_agent_cursor"), "{tool}");
            assert!(!matches!(kind_of(tool), Some(Kind::Screen | Kind::Browser | Kind::Clipboard | Kind::ListApps)), "{tool} cannot work in bounded mode");
        }
        // And every tool the driver reviewed has a place in the decision.
        for tool in REVIEWED {
            assert!(kind_of(tool).is_some(), "{tool} has no kind");
        }
        let mut names = std::collections::BTreeSet::new();
        for (tool, _) in TOOLS {
            assert!(names.insert(tool), "{tool} is placed twice");
        }
    }

    #[test]
    fn the_manifest_names_the_folders_shared_for_writing() {
        let base = std::env::temp_dir().join(format!("connect-scope-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (rw, ro) = (base.join("rw"), base.join("ro"));
        std::fs::create_dir_all(&rw).unwrap();
        std::fs::create_dir_all(&ro).unwrap();
        let mut s = Settings::from_preset(Preset::Balanced, vec![
            Root { path: rw.clone(), write: true },
            Root { path: ro.clone(), write: false },
            Root { path: base.join("gone"), write: true },
        ]);
        s.computer.apps = AppScope { policy: AppsPolicy::Only, except: vec![], only: vec![pick("com.apple.Safari")] };
        let scope = Scope::of(&s);
        assert_eq!(scope.write_roots, vec![rw.clone(), base.join("gone")]);
        let y = scope.manifest().unwrap();
        let resolved = crate::files::plain(&std::fs::canonicalize(&rw).unwrap());
        assert!(y.contains(&format!("  files:\n    write:\n      - dir: {}\n        recursive: true\n  desktop:", yaml_string(&resolved.to_string_lossy()))), "{y}");
        assert!(!y.contains("ro\""), "a read-only folder is not named: {y}");
        assert!(!y.contains("gone"), "a folder that is not there would stop the daemon: {y}");
        // The owner is asked before every write, or files are off: the
        // driver may write nowhere.
        s.files.ask_on_write = true;
        assert!(Scope::of(&s).write_roots.is_empty());
        assert!(!Scope::of(&s).manifest().unwrap().contains("files:"));
        // The other choices do not look at the folders at all: a change of
        // them must not start a new daemon.
        s.files.ask_on_write = false;
        s.computer.apps.policy = AppsPolicy::Except;
        assert!(Scope::of(&s).write_roots.is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn every_app_decides_nothing() {
        let all = Scope::open();
        for (tool, args) in [
            ("click", json!({"x": 1, "y": 2, "scope": "desktop"})),
            ("get_desktop_state", json!({})),
            ("get_window_state", json!({"pid": 300, "window_id": 31})),
            ("a_tool_of_tomorrow", json!({})),
        ] {
            assert_eq!(verdict(&all, tool, args).await, Verdict::Pass, "{tool}");
        }
    }

    #[tokio::test]
    async fn an_excluded_app_is_refused_however_the_call_reaches_it() {
        let except = scope(AppsPolicy::Except, &["com.1password.1password"]);
        for (what, tool, args) in [
            ("by pid and window", "get_window_state", json!({"pid": 300, "window_id": 31})),
            ("by window alone", "zoom", json!({"window_id": 31, "x1": 0, "y1": 0, "x2": 5, "y2": 5})),
            ("by pid alone", "invoke_menu", json!({"pid": 300, "path": ["File"]})),
            ("by the target object", "click", json!({"x": 3, "y": 3, "target": {"kind": "window", "pid": 300, "window_id": 31}})),
            ("an allowed pid with the excluded app's window", "click", json!({"pid": 200, "window_id": 31, "x": 3, "y": 3})),
            ("the excluded pid with an allowed window", "type_text", json!({"pid": 300, "window_id": 21, "text": "x"})),
            ("an element token next to its window", "click", json!({"pid": 300, "window_id": 31, "element_token": "s0123abcd:4"})),
            ("the agent's pointer over its window", "move_cursor", json!({"x": 1, "y": 1, "target": {"kind": "window", "pid": 300, "window_id": 31}})),
            ("its windows listed by pid", "list_windows", json!({"pid": 300})),
            ("binding its window for the page tools", "get_browser_state", json!({"pid": 300, "window_id": 31})),
            ("killing it", "kill_app", json!({"pid": 300})),
        ] {
            let v = verdict(&except, tool, args).await;
            let said = told(&v);
            assert!(said.starts_with("denied: ") && said.contains("owner"), "{what}: {said}");
            assert!(said.contains("does not let you use this app"), "{what}: {said}");
            assert!(!said.contains("1Password") && !said.contains("1password"), "{what}: the refusal names the app: {said}");
            assert!(matches!(v, Verdict::Refuse { journal: "denied: an app the owner excluded", .. }), "{what}");
        }
        // The other apps work as before.
        for (tool, args) in [
            ("get_window_state", json!({"pid": 200, "window_id": 21})),
            ("click", json!({"pid": 100, "window_id": 11, "x": 4, "y": 4})),
            ("click", json!({"pid": 100, "window_id": 11, "element_token": "s0123abcd:4"})),
            ("type_text", json!({"pid": 200, "text": "hello"})),
            ("get_screen_size", json!({})),
            ("start_session", json!({"session": "Mia: flights"})),
            ("clipboard_read", json!({})),
            ("move_cursor", json!({"x": 10, "y": 10})),
            ("browser_click", json!({"target_id": "t1", "tab_id": "tab1", "ref": "e3"})),
            ("get_browser_state", json!({"pid": 200, "window_id": 21})),
            ("browser_prepare", json!({"allow_launch": true, "profile": {"mode": "isolated_new"}})),
        ] {
            assert_eq!(verdict(&except, tool, args.clone()).await, Verdict::Pass, "{tool} {args}");
        }
    }

    #[tokio::test]
    async fn a_call_that_names_no_window_or_the_whole_screen_is_refused_while_apps_are_limited() {
        for limited in [scope(AppsPolicy::Except, &["com.1password.1password"]), scope(AppsPolicy::Only, &["com.apple.Safari"])] {
            // By screen position, and the picture of the whole screen.
            for (tool, args) in [
                ("click", json!({"x": 100, "y": 100, "scope": "desktop"})),
                ("click", json!({"x": 100, "y": 100, "target": {"kind": "desktop", "display_id": "primary"}})),
                ("drag", json!({"from_x": 1, "from_y": 1, "to_x": 9, "to_y": 9, "scope": "desktop"})),
                ("move_cursor", json!({"x": 5, "y": 5, "scope": "desktop"})),
                ("get_desktop_state", json!({})),
                ("get_accessibility_tree", json!({})),
                ("escalate_session", json!({"session": "Mia: flights", "reason": "x"})),
            ] {
                let v = verdict(&limited, tool, args).await;
                assert!(told(&v).contains("window by window"), "{tool}: {}", told(&v));
                assert!(told(&v).starts_with("denied: ") && told(&v).contains("owner"), "{tool}");
            }
            // No window at all: it would go wherever the keyboard is.
            for (tool, args) in [
                ("type_text", json!({"text": "secret"})),
                ("press_key", json!({"key": "return"})),
                ("hotkey", json!({"keys": ["cmd", "c"]})),
                ("click", json!({"x": 1, "y": 1})),
                ("click", json!({"element_token": "s0123abcd:4"})),
                ("scroll", json!({"direction": "down"})),
            ] {
                let v = verdict(&limited, tool, args).await;
                assert!(told(&v).contains("does not say which window"), "{tool}: {}", told(&v));
            }
            // A window of a process that is no app the driver lists, and a
            // window nobody has.
            for args in [json!({"pid": 900, "window_id": 91}), json!({"window_id": 91}), json!({"pid": 100, "window_id": 4242}), json!({"pid": 4242})] {
                let v = verdict(&limited, "get_window_state", args.clone()).await;
                assert!(told(&v).contains("could not tell which app"), "{args}: {}", told(&v));
            }
            // A tool the app has never seen.
            let v = verdict(&limited, "a_tool_of_tomorrow", json!({"pid": 200})).await;
            assert!(told(&v).starts_with("denied: ") && told(&v).contains("owner"));
        }
    }

    #[tokio::test]
    async fn only_these_refuses_every_other_app_and_what_the_bounded_driver_cannot_do() {
        let only = scope(AppsPolicy::Only, &["com.apple.Safari"]);
        assert_eq!(verdict(&only, "get_window_state", json!({"pid": 200, "window_id": 21})).await, Verdict::Pass);
        assert_eq!(verdict(&only, "click", json!({"pid": 200, "window_id": 21, "x": 1, "y": 1})).await, Verdict::Pass);
        for args in [json!({"pid": 100, "window_id": 11}), json!({"pid": 300, "window_id": 31}), json!({"pid": 200, "window_id": 11})] {
            let v = verdict(&only, "get_window_state", args).await;
            assert!(told(&v).contains("only the apps they chose"), "{}", told(&v));
            assert!(matches!(v, Verdict::Refuse { journal: "denied: not one of the apps the owner chose", .. }));
        }
        // The clipboard, the page tools and a pointer move without a
        // window: the bounded driver refuses them, the pipe says why.
        assert!(told(&verdict(&only, "clipboard_read", json!({})).await).contains("clipboard"));
        assert!(told(&verdict(&only, "browser_click", json!({"target_id": "t1", "tab_id": "tab1"})).await).contains("page tools"));
        assert!(told(&verdict(&only, "get_browser_state", json!({"pid": 200, "window_id": 21})).await).contains("page tools"));
        assert!(told(&verdict(&only, "move_cursor", json!({"x": 1, "y": 1})).await).contains("does not say which window"));
        assert_eq!(verdict(&only, "move_cursor", json!({"x": 1, "y": 1, "target": {"kind": "window", "pid": 200, "window_id": 21}})).await, Verdict::Pass);
        assert_eq!(verdict(&only, "start_session", json!({"session": "Mia: flights"})).await, Verdict::Pass);
    }

    #[tokio::test]
    async fn starting_an_app_goes_by_the_same_choice() {
        let except = scope(AppsPolicy::Except, &["com.apple.mail"]);
        let only = scope(AppsPolicy::Only, &["com.apple.Safari"]);
        assert_eq!(verdict(&except, "launch_app", json!({"bundle_id": "com.apple.Safari"})).await, Verdict::Pass);
        assert_eq!(verdict(&except, "launch_app", json!({"name": "safari", "urls": ["https://example.com"]})).await, Verdict::Pass);
        assert_eq!(verdict(&except, "launch_app", json!({"launch_path": "/Applications/Finder.app"})).await, Verdict::Pass);
        for args in [json!({"bundle_id": "com.apple.Mail"}), json!({"name": "Mail"}), json!({"path": "/Applications/Mail.app"})] {
            assert!(told(&verdict(&except, "launch_app", args).await).contains("does not let you use this app"));
        }
        assert_eq!(verdict(&only, "launch_app", json!({"bundle_id": "com.apple.safari"})).await, Verdict::Pass);
        assert!(told(&verdict(&only, "launch_app", json!({"name": "Mail"})).await).contains("only the apps they chose"));
        // The bundle id wins over the name, as in the driver.
        assert!(told(&verdict(&only, "launch_app", json!({"bundle_id": "com.apple.mail", "name": "Safari"})).await).contains("only the apps they chose"));
        // An app the driver's list does not have, and a call that names none.
        for limited in [&except, &only] {
            assert!(told(&verdict(limited, "launch_app", json!({"name": "Nonesuch"})).await).contains("does not find this app"));
            assert!(told(&verdict(limited, "launch_app", json!({"urls": ["https://example.com"]})).await).starts_with("denied: "));
        }
        // The driver would start Chrome or Edge by its own choice.
        let no_chrome = scope(AppsPolicy::Except, &["com.google.Chrome"]);
        assert!(told(&verdict(&no_chrome, "browser_prepare", json!({"allow_launch": true})).await).contains("Chrome or Edge"));
    }

    #[tokio::test]
    async fn the_lists_are_answered_by_the_app_without_the_other_apps() {
        let except = scope(AppsPolicy::Except, &["com.1password.1password", "com.apple.mail"]);
        let Verdict::Answer(apps) = verdict(&except, "list_apps", json!({"session": "Mia: flights"})).await else {
            panic!("list_apps was not answered by the app");
        };
        let names: Vec<&str> = apps["structuredContent"]["apps"].as_array().unwrap().iter().map(|a| a["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["Finder", "Safari", "Google Chrome"]);
        let text = apps["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("Found 3 app(s) you may use here: 2 running, 1 installed and not running."), "{text}");
        assert!(text.contains("- Safari (pid 200) [com.apple.Safari]") && !text.contains("1Password") && !text.contains("Mail"), "{text}");

        let Verdict::Answer(windows) = verdict(&except, "list_windows", json!({})).await else {
            panic!("list_windows was not answered by the app");
        };
        let ids: Vec<u64> = windows["structuredContent"]["windows"].as_array().unwrap().iter().map(|w| w["window_id"].as_u64().unwrap()).collect();
        assert_eq!(ids, [11, 21], "no window of the excluded app, none of a process that is no app");
        let Verdict::Answer(of_safari) = verdict(&except, "list_windows", json!({"pid": 200})).await else {
            panic!();
        };
        assert_eq!(of_safari["structuredContent"]["windows"].as_array().unwrap().len(), 1);
        assert_eq!(of_safari["content"][0]["text"], "Found 1 window(s) of the apps you may use here.");

        let only = scope(AppsPolicy::Only, &["com.apple.Safari"]);
        let Verdict::Answer(apps) = verdict(&only, "list_apps", json!({})).await else {
            panic!();
        };
        assert_eq!(apps["structuredContent"]["apps"].as_array().unwrap().len(), 1);
        let Verdict::Answer(windows) = verdict(&only, "list_windows", json!({"on_screen_only": true})).await else {
            panic!();
        };
        assert_eq!(windows["structuredContent"]["windows"].as_array().unwrap().len(), 1);
        assert!(told(&verdict(&only, "list_windows", json!({"pid": 100})).await).contains("only the apps they chose"));
    }

    #[tokio::test]
    async fn the_directory_reads_again_only_for_what_it_does_not_know() {
        let dir = dir();
        assert_eq!(dir.reads().await, (0, 0));
        assert_eq!(dir.app_of_pid(200).await.unwrap().unwrap().name, "Safari");
        assert_eq!(dir.window_owner(21).await.unwrap(), Some(200));
        // Both known and fresh: nothing was read. (The windows of a fixed
        // directory have no reading yet, so the first question reads once.)
        assert_eq!(dir.reads().await, (0, 1));
        assert_eq!(dir.window_owner(11).await.unwrap(), Some(100));
        assert_eq!(dir.reads().await, (0, 1));
        // An unknown window or pid right after a reading: not read again.
        assert_eq!(dir.window_owner(4242).await.unwrap(), None);
        assert_eq!(dir.app_of_pid(4242).await.unwrap(), None);
        assert_eq!(dir.reads().await, (0, 1));
        // An app that is installed and not running has no pid to be found by.
        assert_eq!(dir.app_of_pid(0).await.unwrap(), None);
    }

    #[test]
    fn the_drivers_lists_are_read_as_they_are() {
        let listed = json!({"apps": [
            {"active": false, "bundle_id": "com.apple.finder", "kind": "desktop", "last_used": null, "launch_path": "/System/Library/CoreServices/Finder.app", "name": "Finder", "pid": 1021, "running": true, "windows": []},
            {"bundle_id": "com.apple.Safari", "launch_path": "/Applications/Safari.app", "name": "Safari", "pid": 0, "running": false},
            {"bundle_id": null, "launch_path": "C:\\Program Files\\App\\app.exe", "name": "App", "pid": 44},
            {"name": "Nameless thing", "pid": 0}
        ]});
        let rows = parse_apps(&listed);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0], AppRow { pid: 1021, name: "Finder".into(), bundle_id: Some("com.apple.finder".into()), path: Some("/System/Library/CoreServices/Finder.app".into()), running: true });
        assert!(!rows[1].running);
        assert!(rows[2].running && rows[2].bundle_id.is_none());
        assert_eq!(rows[2].key(), Some("C:\\Program Files\\App\\app.exe"));
        assert_eq!(rows[3].key(), None);

        let picker = choices(&rows);
        assert_eq!(picker.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["App", "Finder", "Safari"], "running first, no app without an identity");
        assert_eq!(picker[1].id, "com.apple.finder");
        assert_eq!(picker[1].path.as_deref(), Some("/System/Library/CoreServices/Finder.app"));
        assert_eq!(picker[0].path, None, "the id is the path already");

        let mut st = State::default();
        st.set_windows(json!({"current_space_id": 1, "windows": [{"window_id": 7, "pid": 1021, "title": "Documents"}, {"window_id": "x", "pid": 1}]}));
        assert_eq!(st.owners, HashMap::from([(7, 1021)]));
    }

    #[test]
    fn the_first_exceptions_are_the_sensitive_apps_this_computer_has() {
        let rows = vec![
            app(1, "1Password", "com.1password.1password"),
            app(0, "Keychain Access", "com.apple.keychainaccess"),
            app(0, "System Settings", "com.apple.systempreferences"),
            app(2, "Safari", "com.apple.Safari"),
            app(0, "passwords", "com.apple.Passwords"),
        ];
        let picker = choices(&rows);
        let first = default_exclusions(&picker);
        let ids: Vec<&str> = first.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["com.1password.1password", "com.apple.keychainaccess", "com.apple.Passwords", "com.apple.systempreferences"]);
        assert!(first.iter().all(|a| !a.loose && !a.name.is_empty()));
        // Bitwarden is not on this computer: it is not offered.
        assert!(!first.iter().any(|a| a.name == "Bitwarden"));
        assert!(default_exclusions(&choices(&[app(2, "Safari", "com.apple.Safari")])).is_empty());
    }

    #[test]
    fn a_catalog_with_a_tool_nobody_placed_is_noticed() {
        let catalog = vec![json!({"name": "click"}), json!({"name": "teleport_window"}), json!({"name": "list_apps"})];
        assert_eq!(unplaced(&catalog), ["teleport_window"]);
    }
}
