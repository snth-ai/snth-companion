//! "Check the helper": the driver's own health report, for the owner.
//!
//! Three diagnostics exist in Cua Driver 0.34.1. They were read in its
//! source at the pinned tag (`cua-driver/src/doctor.rs`, `cli.rs`,
//! `cua-driver-core/src/health_report.rs`, the three platform providers):
//!
//!  - `health_report`, a tool. It runs inside the daemon it is called on,
//!    is read-only on every system (on macOS it does not even probe screen
//!    capture, because that probe can raise a system prompt) and answers a
//!    fixed list of checks: `binary_version`, `platform_supported`,
//!    `session_active`, `bundle_identity`, `tcc_accessibility`,
//!    `tcc_screen_recording`, `ax_capability`, `screen_capture_capability`
//!    (Linux adds `wayland_backend`), each `pass`, `fail` or `skip`.
//!    THIS is what the app uses, on OUR daemon, over a connection of its
//!    own that the synth never sees.
//!  - `cua-driver doctor`, a command. On macOS it is not read-only: it
//!    unloads and deletes `~/Library/LaunchAgents/com.trycua.cua_driver_updater.plist`
//!    and `/usr/local/bin/cua-driver-update`, which belong to a driver the
//!    owner may have installed for themselves. Not used.
//!  - `cua-driver diagnose`, a command. It asks the daemon on the DEFAULT
//!    socket (the owner's own driver, never ours), reads the system's
//!    permission database for `com.trycua.driver`, and prints paths in the
//!    home folder. Not used.
//!
//! The driver's hints are written for its own app ("Grant Accessibility to
//! CuaDriver.app"; since 0.34 they add "for an embedded or linked driver,
//! follow the host's permission setup"), which is no help to the owner of
//! a driver that runs under SNTH Connect. So the app keeps only each check's name and verdict for the
//! owner (the host has its own words for them, in both languages) and puts
//! the driver's message into the text for "copy details", after `scrub`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};

use crate::core::Shared;
use crate::cua::PINNED_VERSION;
use crate::settings::AppsPolicy;

/// One answer of the daemon may take this long (the first wakes it up).
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

/// How one item stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Ok,
    Fail,
    /// Not checked, and why is in `code`.
    Skip,
}

/// One line of the check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Item {
    /// A fixed name the host has words for: `installed`, `daemon`,
    /// `pointer`, `apps`, or the name of a check of the driver's report
    /// (`tcc_accessibility`, `ax_capability`, ...). A check the app does
    /// not know keeps the driver's name.
    pub id: String,
    pub verdict: Verdict,
    /// Why, as a fixed word the host has a sentence for (`not_installed`,
    /// `wrong_version`, `off`, `paused`, `no_start`, `no_answer`,
    /// `switched_off`, `not_probed`, `bounded`, `not_bounded`, ...). Empty
    /// when the verdict says it all.
    pub code: String,
    /// A fact to show next to it: a version, a number of apps. Never a
    /// path, a window title or anything the synth wrote.
    pub value: String,
}

impl Item {
    fn new(id: &str, verdict: Verdict, code: &str, value: impl Into<String>) -> Item {
        Item { id: id.into(), verdict, code: code.into(), value: value.into() }
    }
}

/// The whole check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    /// `ok`: nothing failed. `problems`: something did. `not_run`: the
    /// daemon was not asked (the helper is not installed, computer control
    /// is off, or the app is paused).
    pub overall: &'static str,
    pub items: Vec<Item>,
    /// The text for "copy details": plain English lines with the driver's
    /// own words. No secrets, no window titles, and no path outside the
    /// app's own data folder (`scrub`).
    pub details: String,
}

/// The checks of the driver's report the app shows the owner, in this
/// order. `session_active` is the daemon answering at all (the `daemon`
/// item); `binary_version` is the `installed` item.
const SHOWN: [&str; 7] = [
    "platform_supported",
    "bundle_identity",
    "tcc_accessibility",
    "tcc_screen_recording",
    "ax_capability",
    "screen_capture_capability",
    "wayland_backend",
];

/// Turn the driver's `health_report` answer (its `structuredContent`) into
/// items. A check that does not apply on this system (`skip`, "not
/// applicable") is left out; one the driver did not probe is shown as not
/// checked; one the app has no name for is shown under the driver's.
pub fn items_of(report: &Value) -> Vec<Item> {
    let checks: Vec<&Value> = report.get("checks").and_then(Value::as_array).map(|c| c.iter().collect()).unwrap_or_default();
    let name = |c: &Value| c.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    let mut ordered: Vec<&Value> = Vec::new();
    for shown in SHOWN {
        ordered.extend(checks.iter().copied().filter(|c| name(c) == shown));
    }
    ordered.extend(
        checks
            .iter()
            .copied()
            .filter(|c| !SHOWN.contains(&name(c).as_str()) && !["binary_version", "session_active", ""].contains(&name(c).as_str())),
    );
    let mut out = Vec::new();
    for check in ordered {
        let id: String = name(check).chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(48).collect();
        let message = check.get("message").and_then(Value::as_str).unwrap_or("");
        match check.get("status").and_then(Value::as_str).unwrap_or("") {
            "pass" => out.push(Item::new(&id, Verdict::Ok, "", "")),
            "fail" => out.push(Item::new(&id, Verdict::Fail, "", "")),
            "skip" if message.to_lowercase().contains("not applicable") => {}
            _ => out.push(Item::new(&id, Verdict::Skip, "not_probed", "")),
        }
    }
    out
}

/// Take out of a text everything that says where things are on this
/// computer: the app's own data folder becomes `<app-data>`, and every
/// other path becomes `<path>`.
pub fn scrub(text: &str, app_data: Option<&Path>) -> String {
    let mut text = text.to_string();
    if let Some(dir) = app_data {
        let mut spellings = vec![dir.to_path_buf()];
        if let Ok(real) = std::fs::canonicalize(dir) {
            spellings.push(crate::files::plain(&real));
            spellings.push(real);
        }
        // The longest spelling first: `/private/tmp/x` before `/tmp/x`,
        // or the shorter one would be found inside the longer.
        let mut spellings: Vec<String> = spellings.iter().map(|p| p.to_string_lossy().to_string()).collect();
        spellings.sort_by_key(|s| std::cmp::Reverse(s.len()));
        for spelling in spellings {
            if spelling.len() > 3 {
                text = text.replace(&spelling, "<app-data>");
            }
        }
    }
    let is_path = |word: &str| {
        let w = word.trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | ':' | '.' | '=' | '<' | '>'));
        let bytes = w.as_bytes();
        (w.starts_with('/') && w.len() > 1 && w[1..].contains('/'))
            || w.starts_with("~/")
            || w.starts_with("\\\\")
            || (bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/'))
    };
    // A path may hold spaces, so nobody can tell where one ends: the line
    // is cut where a path outside the app's folder begins.
    text.lines()
        .map(|line| {
            let mut kept: Vec<&str> = Vec::new();
            for word in line.split(' ') {
                // Inside our own folder: what follows the marker is a
                // file of the app's own.
                let ours = word.contains("<app-data>");
                let foreign = !ours && (is_path(word) || word.contains(":\\") || (word.matches('/').count() >= 2 && !word.contains("://")));
                if foreign {
                    kept.push("<path>");
                    break;
                }
                kept.push(word);
            }
            kept.join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn one_line(text: &str, max: usize) -> String {
    let flat: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = flat.chars().take(max).collect();
    if flat.chars().count() > max {
        out.push_str("...");
    }
    out
}

/// A connection of the app's own to a daemon: `cua-driver mcp`, spoken
/// line by line. The synth's lines never go through it.
struct Client {
    child: tokio::process::Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Client {
    async fn open(shared: &Arc<Shared>, socket: &str, embedded: bool) -> Result<Client, String> {
        let mut child = crate::mcp::spawn_proxy(shared, socket, embedded).map_err(|e| format!("the helper's connection could not be started: {e}"))?;
        let stdin = child.stdin.take().ok_or("the helper's connection has no input")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("the helper's connection has no output")?);
        // Nobody reads its complaints: do not let them fill a pipe.
        drop(child.stderr.take());
        let mut client = Client { child, stdin, stdout, next_id: 0 };
        client
            .request(
                "initialize",
                json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "snth-connect-check", "version": env!("CARGO_PKG_VERSION")}}),
            )
            .await?;
        client.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await?;
        Ok(client)
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
        let wait = async {
            loop {
                let mut line = String::new();
                let n = self.stdout.read_line(&mut line).await.map_err(|e| format!("the helper stopped answering: {e}"))?;
                if n == 0 {
                    return Err("the helper stopped answering".to_string());
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
        };
        tokio::time::timeout(ANSWER_TIMEOUT, wait).await.map_err(|_| format!("the helper did not answer {method} in {} s", ANSWER_TIMEOUT.as_secs()))?
    }

    /// One tool; its whole result (the driver's refusals are results too).
    async fn tool(&mut self, tool: &str, arguments: Value) -> Result<Value, String> {
        self.request("tools/call", json!({"name": tool, "arguments": arguments})).await
    }

    async fn close(mut self) {
        drop(self.stdin);
        if tokio::time::timeout(Duration::from_secs(3), self.child.wait()).await.is_err() {
            let _ = self.child.start_kill();
        }
    }
}

fn refusal_code(result: &Value) -> Option<&str> {
    result.pointer("/structuredContent/refusal/code").or_else(|| result.pointer("/structuredContent/code")).and_then(Value::as_str)
}

/// Run the check. `app_data`: the app's own data folder, for the text of
/// "copy details" (None: every path is taken out).
///
/// Only OUR daemon is asked. With computer control switched off, or the
/// app paused, the daemon is not started for the sake of a check: the
/// report says so.
pub async fn check(shared: &Arc<Shared>, app_data: Option<&Path>) -> Report {
    let mut items: Vec<Item> = Vec::new();
    let mut lines: Vec<String> = vec![
        format!("SNTH Connect {}: helper check", shared.app_version),
        format!("system: {} {}", std::env::consts::OS, std::env::consts::ARCH),
    ];
    let settings = shared.settings();

    // 1. Installed, and the version the app was tested with.
    let found = shared.cua_detect(true).await;
    let version = found.version.clone().unwrap_or_default();
    if !found.installed {
        items.push(Item::new("installed", Verdict::Fail, "not_installed", PINNED_VERSION));
        lines.push(format!("[fail] installed: no helper found (expected {PINNED_VERSION})"));
        return finish("not_run", items, lines, app_data);
    }
    if version == PINNED_VERSION {
        items.push(Item::new("installed", Verdict::Ok, "", version.clone()));
        lines.push(format!("[ok] installed: cua-driver {version} (expected {PINNED_VERSION})"));
    } else {
        items.push(Item::new("installed", Verdict::Fail, "wrong_version", format!("{version} / {PINNED_VERSION}")));
        lines.push(format!("[fail] installed: cua-driver {version}, expected {PINNED_VERSION}"));
    }

    // 2. Our daemon. It is not started against the owner's switches.
    if shared.paused() || !settings.computer.enabled {
        let code = if shared.paused() { "paused" } else { "off" };
        items.push(Item::new("daemon", Verdict::Skip, code, ""));
        lines.push(format!("[skip] daemon: not started ({})", if shared.paused() { "the app is paused" } else { "computer control is switched off" }));
        return finish("not_run", items, lines, app_data);
    }
    let session = match shared.cua_session().await {
        Ok(session) => session,
        Err(why) => {
            items.push(Item::new("daemon", Verdict::Fail, "no_start", ""));
            lines.push(format!("[fail] daemon: {}", one_line(&why.to_string(), 600)));
            return finish("problems", items, lines, app_data);
        }
    };
    let mut client = match Client::open(shared, &session.socket, session.embedded).await {
        Ok(client) => client,
        Err(why) => {
            items.push(Item::new("daemon", Verdict::Fail, "no_answer", ""));
            lines.push(format!("[fail] daemon: {}", one_line(&why, 600)));
            return finish("problems", items, lines, app_data);
        }
    };
    lines.push(format!("daemon: {}", if session.embedded { "the app's own, on a private endpoint" } else { "the user's own (attached)" }));

    // 3. The driver's own report.
    match client.tool("health_report", json!({})).await {
        Ok(result) => {
            let report = result.get("structuredContent").cloned().unwrap_or(Value::Null);
            let answered = report.get("checks").and_then(Value::as_array).is_some_and(|c| !c.is_empty());
            if answered {
                items.push(Item::new("daemon", Verdict::Ok, "", ""));
                lines.push(format!(
                    "[ok] daemon: answers (driver {}, report {}, schema {})",
                    report.get("driver_version").and_then(Value::as_str).unwrap_or("?"),
                    report.get("overall").and_then(Value::as_str).unwrap_or("?"),
                    report.get("schema_version").and_then(Value::as_str).unwrap_or("?"),
                ));
                if let Some(build) = report.get("build") {
                    lines.push(format!(
                        "build: git {}, sha256 {}",
                        build.get("git_sha").and_then(Value::as_str).unwrap_or("?"),
                        build.get("exe_sha256").and_then(Value::as_str).unwrap_or("?"),
                    ));
                }
                for check in report.get("checks").and_then(Value::as_array).into_iter().flatten() {
                    let text = |key: &str| check.get(key).and_then(Value::as_str).unwrap_or("");
                    lines.push(format!("[{}] {}: {}", text("status"), text("name"), one_line(text("message"), 400)));
                    // One fact a line, so that a path in one of them takes
                    // nothing else with it when it is cut.
                    for (key, value) in check.get("data").and_then(Value::as_object).into_iter().flatten() {
                        let value = value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string());
                        lines.push(format!("       {key}: {}", one_line(&value, 300)));
                    }
                }
                items.extend(items_of(&report));
            } else {
                items.push(Item::new("daemon", Verdict::Fail, "no_answer", ""));
                lines.push(format!("[fail] daemon: health_report answered without checks: {}", one_line(&result.to_string(), 400)));
            }
        }
        Err(why) => {
            items.push(Item::new("daemon", Verdict::Fail, "no_answer", ""));
            lines.push(format!("[fail] daemon: {}", one_line(&why, 600)));
        }
    }

    // 4. The synth's pointer (the driver's overlay).
    if !settings.computer.look.cursor {
        items.push(Item::new("pointer", Verdict::Skip, "switched_off", ""));
        lines.push("[skip] pointer: switched off by the owner".into());
    } else {
        match client.tool("get_agent_cursor_state", json!({})).await {
            Ok(result) if result.get("isError").and_then(Value::as_bool) != Some(true) => {
                let enabled = result.pointer("/structuredContent/enabled").and_then(Value::as_bool) == Some(true);
                let theme = result.pointer("/structuredContent/theme/id").and_then(Value::as_str).unwrap_or("?");
                items.push(Item::new("pointer", if enabled { Verdict::Ok } else { Verdict::Fail }, "", ""));
                lines.push(format!("[{}] pointer: enabled {enabled}, theme {theme}", if enabled { "ok" } else { "fail" }));
            }
            Ok(result) => {
                items.push(Item::new("pointer", Verdict::Fail, "", ""));
                lines.push(format!(
                    "[fail] pointer: {} ({})",
                    one_line(result.pointer("/content/0/text").and_then(Value::as_str).unwrap_or("?"), 400),
                    refusal_code(&result).unwrap_or("no code")
                ));
            }
            Err(why) => {
                items.push(Item::new("pointer", Verdict::Fail, "", ""));
                lines.push(format!("[fail] pointer: {}", one_line(&why, 400)));
            }
        }
    }

    // 5. The owner's choice of apps, and for "only these" whether the
    //    driver itself holds it: a bounded daemon refuses the list of every
    //    window, a daemon that is not bounded answers it (nothing of the
    //    answer is kept).
    let scope = session.scope.clone();
    let count = scope.apps.len().to_string();
    match scope.policy {
        AppsPolicy::All => {
            items.push(Item::new("apps", Verdict::Ok, "all", ""));
            lines.push("[ok] apps: every app".into());
        }
        AppsPolicy::Except => {
            items.push(Item::new("apps", Verdict::Ok, "except", count.clone()));
            lines.push(format!("[ok] apps: every app except {count} (held by the app, call by call)"));
        }
        AppsPolicy::Only if !session.embedded => {
            items.push(Item::new("apps", Verdict::Ok, "only_pipe", count.clone()));
            lines.push(format!("[ok] apps: only {count} (held by the app; the daemon is not the app's own)"));
        }
        AppsPolicy::Only => match client.tool("list_windows", json!({})).await {
            Ok(result) if refusal_code(&result) == Some("bounded_resource_outside_manifest") => {
                items.push(Item::new("apps", Verdict::Ok, "bounded", count.clone()));
                lines.push(format!("[ok] apps: only {count} (the helper runs in bounded mode and refuses the rest itself)"));
            }
            Ok(result) => {
                items.push(Item::new("apps", Verdict::Fail, "not_bounded", count.clone()));
                lines.push(format!(
                    "[fail] apps: only {count}, but the helper did not refuse the list of every window ({})",
                    refusal_code(&result).unwrap_or(if result.get("isError").and_then(Value::as_bool) == Some(true) { "another refusal" } else { "it answered" })
                ));
            }
            Err(why) => {
                items.push(Item::new("apps", Verdict::Fail, "not_bounded", count.clone()));
                lines.push(format!("[fail] apps: only {count}: {}", one_line(&why, 400)));
            }
        },
    }

    client.close().await;
    drop(session);
    let overall = if items.iter().any(|i| i.verdict == Verdict::Fail) { "problems" } else { "ok" };
    finish(overall, items, lines, app_data)
}

fn finish(overall: &'static str, items: Vec<Item>, mut lines: Vec<String>, app_data: Option<&Path>) -> Report {
    lines.insert(2, format!("overall: {overall}"));
    Report { overall, items, details: scrub(&lines.join("\n"), app_data) }
}

/// The app's own data folder, as far as the core can tell: where its
/// private copy of the driver lives (`<data>/cua/home`).
pub fn app_data_of(env: &crate::cua::DriverEnv) -> Option<PathBuf> {
    env.home.as_ref()?.parent()?.parent().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the pinned driver answered on a Mac where the app has neither
    /// permission (the paths are the test's own).
    fn mac_report() -> Value {
        json!({
            "build": {"exe_sha256": "902b65ae", "git_sha": "a9baa8d1", "version": "0.33.1"},
            "checks": [
                {"message": "cua-driver 0.33.1", "name": "binary_version", "status": "pass"},
                {"data": {"architecture": "arm64", "os_version": "26.6.2"}, "message": "macOS 26.6.2 (arm64)", "name": "platform_supported", "status": "pass"},
                {"message": "MCP session is active.", "name": "session_active", "status": "pass"},
                {"data": {"configured_bundle_identifier": "ai.snth.connect", "executable_path": "/Users/owner/Library/Application Support/ai.snth.connect/cua/releases/0.33.1/cua-driver", "identity_source": "parent_application", "parent_process_id": 15863},
                 "hint": "Spawn `cua-driver serve --embedded` directly from the signed host app process.", "message": "Embedded mode is set, but the parent process is not an identifiable macOS application.", "name": "bundle_identity", "status": "fail"},
                {"hint": "Grant Accessibility to CuaDriver.app in System Settings.", "message": "Accessibility is NOT granted for this process.", "name": "tcc_accessibility", "status": "fail"},
                {"message": "Screen Recording is granted.", "name": "tcc_screen_recording", "status": "pass"},
                {"message": "AX is not trusted; UI inspection and event posting will fail.", "name": "ax_capability", "status": "fail"},
                {"message": "Direct ScreenCaptureKit readiness was not probed because health_report is read-only; run `cua-driver permissions grant` to request and verify it explicitly.", "name": "screen_capture_capability", "status": "skip"}
            ],
            "driver_version": "0.33.1", "overall": "degraded", "platform": "darwin", "schema_version": "1"
        })
    }

    #[test]
    fn the_drivers_report_becomes_items_the_owner_can_read() {
        let items = items_of(&mac_report());
        let got: Vec<(&str, Verdict, &str)> = items.iter().map(|i| (i.id.as_str(), i.verdict, i.code.as_str())).collect();
        assert_eq!(
            got,
            [
                ("platform_supported", Verdict::Ok, ""),
                ("bundle_identity", Verdict::Fail, ""),
                ("tcc_accessibility", Verdict::Fail, ""),
                ("tcc_screen_recording", Verdict::Ok, ""),
                ("ax_capability", Verdict::Fail, ""),
                // Not probed on macOS: the driver says so, and so does the app.
                ("screen_capture_capability", Verdict::Skip, "not_probed"),
            ]
        );
        // Nothing of the driver's own words reaches an item: its hints
        // name CuaDriver.app, which is not what the owner has.
        let shown = serde_json::to_string(&items).unwrap();
        assert!(!shown.contains("CuaDriver") && !shown.contains("/Users/"), "{shown}");
    }

    #[test]
    fn a_check_that_does_not_apply_is_left_out_and_an_unknown_one_is_kept() {
        let windows = json!({"checks": [
            {"name": "binary_version", "status": "pass", "message": "cua-driver 0.33.1"},
            {"name": "platform_supported", "status": "pass", "message": "Windows 11 (x86_64)"},
            {"name": "session_active", "status": "pass", "message": "MCP session is active."},
            {"name": "bundle_identity", "status": "skip", "message": "not applicable on Windows"},
            {"name": "tcc_accessibility", "status": "skip", "message": "not applicable on Windows"},
            {"name": "tcc_screen_recording", "status": "skip", "message": "not applicable on Windows"},
            {"name": "ax_capability", "status": "pass", "message": "UI Automation is available."},
            {"name": "screen_capture_capability", "status": "fail", "message": "capture failed"},
            {"name": "a new <check> of tomorrow", "status": "fail", "message": "something"},
            {"status": "fail", "message": "a check without a name"}
        ]});
        let got: Vec<(String, Verdict)> = items_of(&windows).into_iter().map(|i| (i.id, i.verdict)).collect();
        assert_eq!(
            got,
            [
                ("platform_supported".to_string(), Verdict::Ok),
                ("ax_capability".to_string(), Verdict::Ok),
                ("screen_capture_capability".to_string(), Verdict::Fail),
                ("anewcheckoftomorrow".to_string(), Verdict::Fail),
            ]
        );
        assert!(items_of(&json!({})).is_empty());
        assert!(items_of(&json!({"checks": "no"})).is_empty());
    }

    #[test]
    fn the_copied_text_says_nothing_of_where_things_are() {
        let data = Path::new("/Users/owner/Library/Application Support/ai.snth.connect");
        let text = "executable_path: /Users/owner/Library/Application Support/ai.snth.connect/cua/releases/0.33.1/cua-driver\nconfig at /Users/owner/.cua-driver/config.json, also ~/Documents/secret.txt and C:\\Users\\owner\\AppData\\Local\\x.exe\n{\"executable_path\":\"/private/tmp/x/cua-driver\",\"pid\":7}\nplain words, a version 0.33.1, a ratio 3/4 and https://example.com/a/b stay";
        let out = scrub(text, Some(data));
        assert_eq!(
            out,
            "executable_path: <app-data>/cua/releases/0.33.1/cua-driver\nconfig at <path>\n<path>\nplain words, a version 0.33.1, a ratio 3/4 and https://example.com/a/b stay"
        );
        for gone in ["/Users/owner", "owner", ".cua-driver", "secret.txt", "AppData", "/private/tmp"] {
            assert!(!out.contains(gone), "{gone} is still there: {out}");
        }
        // A folder with spaces in its name gives nothing away either: the
        // line ends where the path begins.
        assert_eq!(scrub("log: /Users/owner/My Secret Project/notes.txt (open)", Some(data)), "log: <path>");
        assert_eq!(scrub("at \\\\server\\share\\x and more", None), "at <path>");
        // A folder the system knows under a longer name too (macOS: /tmp
        // is /private/tmp) is taken out whole.
        let tmp = std::env::temp_dir().join(format!("connect-scrub-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let real = std::fs::canonicalize(&tmp).unwrap();
        for spelling in [tmp.clone(), real] {
            let line = format!("executable_path: {}", spelling.join("cua").join("cua-driver").to_string_lossy());
            let out = scrub(&line, Some(&tmp));
            assert!(out.starts_with("executable_path: <app-data>") && out.ends_with("cua-driver"), "{out}");
        }
        let _ = std::fs::remove_dir_all(&tmp);
        // Without a data folder every path goes.
        assert_eq!(scrub("at /Users/owner/Library/x/cua-driver now", None), "at <path>");
        assert_eq!(scrub("nothing to hide here", None), "nothing to hide here");
    }

    #[test]
    fn a_long_or_broken_message_is_one_bounded_line() {
        assert_eq!(one_line("a\nb\t c  \u{7}d", 100), "a b c d");
        let long = one_line(&"x".repeat(1000), 50);
        assert_eq!(long.len(), 53);
        assert!(long.ends_with("..."));
    }

    #[test]
    fn the_apps_data_folder_is_two_above_the_drivers_home() {
        let env = crate::cua::DriverEnv { home: Some(PathBuf::from("/data/ai.snth.connect/cua/home")) };
        assert_eq!(app_data_of(&env), Some(PathBuf::from("/data/ai.snth.connect")));
        assert_eq!(app_data_of(&crate::cua::DriverEnv::default()), None);
    }
}
