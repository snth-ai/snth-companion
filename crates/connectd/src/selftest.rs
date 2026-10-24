//! Two self-tests. `connectd selftest apps` is at the end of this file.
//!
//! `connectd selftest cua`: the computer-control path, end to end, the way
//! the app runs it, on the machine it is run on.
//!
//!  1. install the pinned driver into a fresh private dir through the
//!     core's installer (download, hash check, unpack, `--version`)
//!  2. start OUR private daemon exactly as the app does
//!     (`Shared::cua_socket`): a private Unix socket, or on Windows a
//!     private named pipe
//!  3. open one MCP session through the core's proxy spawn
//!     (`mcp::spawn_proxy`): `initialize`, `tools/list`, then read-only
//!     tools only (`get_screen_size`, `list_windows`)
//!  4. Windows: prove that a child made by the core's spawn helper has no
//!     console window (nothing can flash on the screen)
//!  5. what the synth is never allowed to trigger, against the real
//!     driver's catalog: every tool and argument of this driver that
//!     installs, records, updates, configures or writes a file is
//!     classified in `cua_guard` (an unclassified one FAILS the test, so a
//!     new driver version cannot bring one in unseen); each tool on the
//!     never list is refused by the app's filter with words for the owner
//!     and never reaches the driver; a file the driver writes is held to
//!     the folders shared for writing
//!  6. how it looks, against the real driver, by reading it back (nothing
//!     is clicked, typed or moved, so no cursor is ever drawn):
//!     the cursor themes the app ships are accepted by the pinned driver
//!     and land in OUR driver home; the owner's look is what every session
//!     gets; the synth's attempts to change it are refused by the app's own
//!     filter and change nothing; a screenshot is asked for within the
//!     owner's limit; a new look waits for the open session to end and
//!     then holds; the look survives a restart of the app; the cursor can
//!     be switched off; a driver the user runs themselves keeps its files
//!  7. which apps the synth may use, against the real driver and windows
//!     that are already open (nothing is launched, closed, clicked or typed
//!     through the driver; where no two apps have a window, the test starts
//!     two harmless ones itself and ends them): every tool of this driver
//!     has a place in the app's decision; "only these" starts OUR daemon in
//!     the driver's bounded mode, where the driver ITSELF lets the allowed
//!     app be observed and refuses another running app, the list of every
//!     app and the picture of the whole screen with its own refusal code,
//!     while the tools that touch no app still work; "every app except
//!     these" is held by the app's pipe, which resolves a call to an app by
//!     pid, by window alone and by a window under another app's pid, and
//!     refuses the whole screen; the two lists are answered by the app
//!     without the other apps; a session keeps the choice it began with
//!  8. "check the helper": the driver's own health report from OUR daemon,
//!     and a text for "copy details" with no path outside the test's folder
//!  9. an update of the app with a new driver version, with the real
//!     driver: the copy of the version before (the installed copy, moved
//!     to another version's folder) is what runs while the new one is not
//!     there; the new one is installed next to it while a session is open;
//!     that session goes on, on the old copy, which stays on disk; the next
//!     session starts on the new copy and the old one is removed
//! 10. stop the daemon and check its endpoint is gone
//!
//! `--preview-window` (macOS, by hand only) also starts the daemon once
//! with the experimental preview window, which shows that window on the
//! screen for a moment.
//!
//! Everything lives under one temp dir and a private endpoint. A driver or
//! daemon the user runs themselves is never used or touched. Exit code 0 =
//! every step passed. Built for CI (the Windows job runs it); it can be
//! run by hand, but note that it starts a real driver daemon on the
//! machine.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use connect_core::grant::{Approval, ApprovalFuture, ApprovalRequest, Approver};
use connect_core::settings::{ComputerMode, CursorColor, CursorSpeed, LookSettings, ReducedMotion};
use connect_core::{cookies, Core, CoreConfig, CuaAttach, CuaSession, InstallState, MemSecretStore, MultiReader, Shared};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};

/// How long one MCP answer may take. The first call wakes the runtime.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(60);
/// A driver with fewer tools than this is not the driver we expect.
const MIN_TOOLS: usize = 20;

struct DenyAll;
impl Approver for DenyAll {
    fn ask(&self, _req: ApprovalRequest) -> ApprovalFuture {
        Box::pin(async { Approval::deny() })
    }
}

pub async fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("cua") => match cua(&args[1..]).await {
            Ok(()) => {
                println!("SELFTEST OK");
                0
            }
            Err(why) => {
                eprintln!("SELFTEST FAILED: {why}");
                1
            }
        },
        Some("apps") => match apps(&args[1..]).await {
            Ok(()) => {
                println!("SELFTEST OK");
                0
            }
            Err(why) => {
                eprintln!("SELFTEST FAILED: {why}");
                1
            }
        },
        Some("host") => match crate::selftest_host::host(&args[1..]).await {
            Ok(()) => {
                println!("SELFTEST OK");
                0
            }
            Err(why) => {
                eprintln!("SELFTEST FAILED: {why}");
                1
            }
        },
        #[cfg(target_os = "macos")]
        Some("safari") => match crate::selftest_safari::safari(&args[1..]).await {
            Ok(()) => {
                println!("SELFTEST OK");
                0
            }
            Err(why) => {
                eprintln!("SELFTEST FAILED: {why}");
                1
            }
        },
        #[cfg(not(target_os = "macos"))]
        Some("safari") => {
            eprintln!("selftest safari: Safari is a macOS browser; nothing to test on this system");
            2
        }
        // The child half of the console check: says whether THIS process
        // was given a console window.
        Some("console-child") => {
            println!("console={}", connect_core::proc::has_console_window());
            0
        }
        _ => {
            eprintln!("usage: connectd selftest cua [--root <dir>] [--keep] [--preview-window]\n       connectd selftest apps [--touch]\n       connectd selftest host [--helper <path>] [--system]\n       connectd selftest safari --relay <path> [--group <id>] (macOS)");
            2
        }
    }
}

fn step(text: &str) {
    println!("==> {text}");
}

async fn cua(args: &[String]) -> Result<(), String> {
    let mut root: Option<PathBuf> = None;
    let mut keep = false;
    let mut preview_window = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--root" => {
                root = args.get(i + 1).map(PathBuf::from);
                i += 1;
            }
            "--keep" => keep = true,
            "--preview-window" => preview_window = true,
            other => return Err(format!("unknown option {other}")),
        }
        i += 1;
    }
    let root = root.unwrap_or_else(|| {
        // A space and a non-ASCII letter on purpose: real user folders
        // have both.
        std::env::temp_dir().join(format!("connect selftest é {}", std::process::id()))
    });
    std::fs::create_dir_all(&root).map_err(|e| format!("create {}: {e}", root.display()))?;
    println!("root: {}", root.display());

    let users_driver = UsersDriver::read();
    let core = selftest_core(&root);
    // Computer control on, no questions: the daemon gate must let us in.
    core.mutate_settings(|s| {
        s.computer.enabled = true;
        s.computer.mode = ComputerMode::Auto;
        s.computer.look = LookSettings::default();
    });

    let mut result = exercise(&core, &root).await;
    if result.is_ok() && preview_window {
        result = preview(&core).await;
    }

    step("stop the daemon");
    core.cua_stop_daemon().await;
    if result.is_ok() {
        step("a driver the user runs themselves kept its files");
        result = users_driver.unchanged();
    }
    if !keep {
        let _ = std::fs::remove_dir_all(&root);
    }
    result
}

/// The core the self-test drives: the app's own driver copy under `root`,
/// settings in a file there (so a second core can prove what survives a
/// restart), nothing asked of anybody.
fn selftest_core(root: &std::path::Path) -> Core {
    Core::new(CoreConfig {
        settings_path: Some(root.join("settings.json")),
        journal_path: None,
        approver: Arc::new(DenyAll),
        cookies: Arc::new(MultiReader {
            chromium: vec![],
            firefox: vec![],
            keys: Arc::new(cookies::fixtures::NoKey),
        }),
        secret: Arc::new(MemSecretStore::new()),
        apps: Arc::new(connect_core::apps::stub::StubBackend::offering(&[])),
        hub: Arc::new(connect_core::UreqHubClient),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        cua_bin: String::new(),
        cua_private_root: Some(root.join("cua")),
        cua_attach: CuaAttach::Private,
    })
}

async fn exercise(core: &Core, root: &std::path::Path) -> Result<(), String> {
    step("install the pinned driver through the core's installer");
    match core.cua_ensure().await {
        InstallState::Ready { version } => println!("driver {version} at {}", core.cua_bin()),
        other => return Err(format!("the driver was not installed: {other:?}")),
    }
    // A second call must find it in place.
    match core.cua_ensure().await {
        InstallState::Ready { .. } => {}
        other => return Err(format!("the installed driver did not pass a second check: {other:?}")),
    }

    step("start our private daemon the way the app does");
    let shared = core.shared();
    let (endpoint, embedded) = shared
        .cua_socket()
        .await
        .map_err(|e| format!("the daemon did not start: {e}"))?;
    println!("endpoint: {endpoint} (embedded: {embedded})");
    if cfg!(windows) && !endpoint.starts_with(r"\\.\pipe\") {
        return Err(format!("on Windows the endpoint must be a named pipe, got {endpoint}"));
    }
    if !connect_core::cua::endpoint_ready(&endpoint) {
        return Err("the daemon reported ready but its endpoint is not there".into());
    }
    // Asking again must hand back the same daemon, not start another.
    let (again, _) = shared.cua_socket().await.map_err(|e| e.to_string())?;
    if again != endpoint {
        return Err(format!("a second request started another daemon: {again}"));
    }

    step("open an MCP session through the core's proxy");
    let mut child = connect_core::mcp::spawn_proxy(&shared, &endpoint, embedded)
        .map_err(|e| format!("spawn the mcp proxy: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("the proxy has no stdin")?;
    let mut stdout = BufReader::new(child.stdout.take().ok_or("the proxy has no stdout")?);
    let stderr = child.stderr.take();
    let errors = tokio::spawn(async move {
        let mut text = String::new();
        if let Some(stderr) = stderr {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                text.push_str(&line);
                text.push('\n');
            }
        }
        text
    });

    let session = async {
        let init = call(
            &mut stdin,
            &mut stdout,
            1,
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "connect-selftest", "version": env!("CARGO_PKG_VERSION")}
            }),
        )
        .await?;
        let server = init.pointer("/serverInfo/name").and_then(Value::as_str).unwrap_or("?");
        let server_version = init.pointer("/serverInfo/version").and_then(Value::as_str).unwrap_or("?");
        println!("initialize: server {server} {server_version}");
        send(&mut stdin, &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await?;

        let listed = call(&mut stdin, &mut stdout, 2, "tools/list", json!({})).await?;
        let tools: Vec<&str> = listed
            .get("tools")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|t| t.get("name").and_then(Value::as_str)).collect())
            .unwrap_or_default();
        println!("tools/list: {} tools", tools.len());
        if tools.len() < MIN_TOOLS {
            return Err(format!("only {} tools listed, expected at least {MIN_TOOLS}", tools.len()));
        }
        for needed in ["get_screen_size", "list_windows", "click"] {
            if !tools.contains(&needed) {
                return Err(format!("the tool {needed} is missing from the catalog"));
            }
        }

        // Read-only tools only: nothing on the machine is touched.
        for (id, tool) in [(3, "get_screen_size"), (4, "list_windows")] {
            let result = call(
                &mut stdin,
                &mut stdout,
                id,
                "tools/call",
                json!({"name": tool, "arguments": {}}),
            )
            .await?;
            if result.get("isError").and_then(Value::as_bool) == Some(true) {
                return Err(format!("{tool} answered with an error: {}", short(&result)));
            }
            println!("tools/call {tool}: ok ({})", short(&result));
        }
        Ok(())
    }
    .await;

    // Closing its stdin ends the proxy.
    drop(stdin);
    let _ = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
    let _ = child.start_kill();
    let stderr_text = tokio::time::timeout(Duration::from_secs(3), errors)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    if let Err(why) = session {
        return Err(format!("{why}\n--- mcp proxy stderr ---\n{stderr_text}"));
    }

    if cfg!(windows) {
        step("a child made by the core's spawn helper has no console window");
        let me = std::env::current_exe().map_err(|e| e.to_string())?;
        let out = connect_core::proc::command(&me)
            .args(["selftest", "console-child"])
            .output()
            .await
            .map_err(|e| format!("run the console check: {e}"))?;
        let said = String::from_utf8_lossy(&out.stdout).trim().to_string();
        println!("child says: {said}");
        if said != "console=false" {
            return Err(format!("a child process got a console window ({said})"));
        }
    }

    guard(core, root).await?;
    let endpoint = look(core, root, endpoint).await?;
    apps_scope(core, root).await?;
    update_path(core, root).await?;
    let endpoint = match core.shared().cua_socket().await {
        Ok((now, _)) => now,
        Err(_) => endpoint,
    };

    step("stopping the daemon removes its endpoint");
    core.cua_stop_daemon().await;
    for _ in 0..50 {
        if !connect_core::cua::endpoint_ready(&endpoint) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!("the endpoint {endpoint} is still served after the daemon was stopped"))
}

// ---------------------------------------------------------------------
// What the synth is never allowed to trigger (`connect_core::cua_guard`),
// held against the catalog of the driver that is really installed. Every
// call here stops at the app's filter or is read-only: nothing is
// recorded, installed, updated or written.
// ---------------------------------------------------------------------

async fn guard(core: &Core, root: &std::path::Path) -> Result<(), String> {
    use connect_core::cua_guard;
    let shared = core.shared();

    step("every tool of this driver that installs, records, updates, configures or writes a file is classified");
    let mut session = Session::open(&shared).await?;
    let listed = session
        .synth_raw("tools/list", json!({}))
        .await?
        .ok_or("tools/list was not forwarded")?;
    let tools: Vec<Value> = listed.get("tools").and_then(Value::as_array).cloned().unwrap_or_default();
    let names: Vec<&str> = tools.iter().filter_map(|t| t.get("name").and_then(Value::as_str)).collect();
    let unknown = cua_guard::unclassified(&tools);
    if !unknown.is_empty() {
        return Err(format!(
            "this driver has tools or arguments of these kinds that nobody has read and decided: {}. Read each one in the driver's source, then add it to NEVER, WRITE_ARGS, REVIEWED_TOOLS or REVIEWED_ARGS in crates/connect-core/src/cua_guard.rs",
            unknown.join(", ")
        ));
    }
    println!(
        "{} tools: {} never allowed, {} read and allowed, {} write arguments held to the shared folders, nothing unclassified",
        names.len(),
        cua_guard::NEVER.len(),
        cua_guard::REVIEWED_TOOLS.len(),
        cua_guard::WRITE_ARGS.len(),
    );

    step("the synth is refused what it is never allowed to trigger");
    let recording = root.join("recording");
    let recording_s = recording.to_string_lossy().to_string();
    for never in cua_guard::NEVER {
        if !names.contains(&never.tool) {
            println!("{}: not in this driver's catalog", never.tool);
            continue;
        }
        let arguments = match never.tool {
            "start_recording" => json!({"session": "task-guard", "output_dir": recording_s, "record_video": true}),
            "install_ffmpeg" => json!({"confirm": true}),
            "install_extension" => json!({"name": "perception", "confirm": true}),
            "set_config" => json!({"experimental_pip": true}),
            "replay_trajectory" => json!({"dir": recording_s}),
            _ => json!({}),
        };
        match session.synth(never.tool, arguments).await? {
            Synth::Refused(why) if why.starts_with("denied: ") && why.contains("owner") => {
                println!("{}: refused ({why})", never.tool);
            }
            Synth::Refused(why) => {
                return Err(format!("{}: refused without words for the owner: {why:?}", never.tool));
            }
            Synth::Answer(result) => return Err(format!("{} reached the driver: {}", never.tool, short(&result))),
        }
    }
    if recording.exists() {
        return Err(format!("{} was created", recording.display()));
    }
    let journal = serde_json::to_string(&core.recent_journal()).unwrap_or_default();
    if !journal.contains("denied: never allowed for the synth") {
        return Err("the refusals are not in the journal".into());
    }

    step("what was read and found safe still passes");
    for (tool, why) in cua_guard::REVIEWED_TOOLS {
        // The download is the write rule's business, below.
        if *tool == "browser_download" {
            continue;
        }
        let line = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": {"session": "task-guard"}}});
        match filtered(&shared, &line).await? {
            Filtered::Forwarded(_) => println!("{tool}: passes ({why})"),
            Filtered::Refused(said) => return Err(format!("{tool} is refused: {said}")),
        }
    }
    // The read-only one for real: nothing is being recorded.
    match session.synth("get_recording_state", json!({"session": "task-guard"})).await? {
        Synth::Answer(result) if result.get("isError").and_then(Value::as_bool) != Some(true) => {
            println!("get_recording_state: {}", short(&result));
            if result.pointer("/structuredContent/recording").and_then(Value::as_bool) == Some(true) {
                return Err("the driver says a recording is running".into());
            }
        }
        Synth::Answer(result) => return Err(format!("get_recording_state failed: {}", short(&result))),
        Synth::Refused(why) => return Err(format!("get_recording_state was refused: {why}")),
    }

    step("a file the driver writes goes into a folder shared for writing, or nowhere");
    for arg in cua_guard::WRITE_ARGS {
        let has = tools.iter().any(|t| t.pointer(&format!("/inputSchema/properties/{arg}")).is_some());
        println!("{arg}: {}", if has { "in this driver's catalog" } else { "not in this driver's catalog" });
    }
    let (rw, ro, outside) = (root.join("guard shared"), root.join("guard read only"), root.join("guard outside"));
    for dir in [&rw, &ro, &outside] {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    let files_before = core.settings().files;
    core.add_root(rw.clone(), true);
    core.add_root(ro.clone(), false);
    let inside = connect_core::files::plain(&std::fs::canonicalize(&rw).map_err(|e| e.to_string())?);
    let path = |dir: &std::path::Path, name: &str| dir.join(name).to_string_lossy().to_string();
    let result = async {
        for (what, tool, arg, arguments, allowed) in [
            ("a screenshot into the shared folder", "get_window_state", "screenshot_out_file", json!({"pid": 1, "window_id": 1, "screenshot_out_file": path(&rw, "shot.png")}), true),
            ("a screenshot into a read-only folder", "get_window_state", "screenshot_out_file", json!({"pid": 1, "window_id": 1, "screenshot_out_file": path(&ro, "shot.png")}), false),
            ("a screenshot outside the shared folders", "get_desktop_state", "screenshot_out_file", json!({"screenshot_out_file": path(&outside, "shot.png")}), false),
            ("a screenshot into the home folder by ~", "get_desktop_state", "screenshot_out_file", json!({"screenshot_out_file": "~/shot.png"}), false),
            ("a marked screenshot outside the shared folders", "click", "debug_image_out", json!({"pid": 1, "window_id": 1, "x": 1, "y": 1, "debug_image_out": path(&outside, "mark.png")}), false),
            ("a download outside the shared folders", "browser_download", "destination_root", json!({"session": "task-guard", "target_id": "t", "tab_id": "t", "ref": "p1:1", "destination_root": outside.to_string_lossy()}), false),
            ("a download into the shared folder", "browser_download", "destination_root", json!({"session": "task-guard", "target_id": "t", "tab_id": "t", "ref": "p1:1", "destination_root": rw.to_string_lossy()}), true),
        ] {
            // Only the filter is asked: nothing is captured or downloaded.
            let line = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": arguments}});
            match (filtered(&shared, &line).await?, allowed) {
                (Filtered::Forwarded(message), true) => {
                    let got = message.pointer(&format!("/params/arguments/{arg}")).and_then(Value::as_str).unwrap_or("");
                    if !std::path::Path::new(got).starts_with(&inside) {
                        return Err(format!("{what}: forwarded as {got:?}, which is not inside {}", inside.display()));
                    }
                    println!("{what}: passes");
                }
                (Filtered::Refused(why), false) if why.starts_with("denied: ") && why.contains("owner") => {
                    println!("{what}: refused");
                }
                (Filtered::Refused(why), false) => return Err(format!("{what}: refused without words for the owner: {why:?}")),
                (Filtered::Refused(why), true) => return Err(format!("{what} was refused: {why}")),
                (Filtered::Forwarded(message), false) => return Err(format!("{what} was let through: {message}")),
            }
        }
        // Files switched off: nothing may be written at all.
        core.mutate_settings(|s| s.files.enabled = false);
        let line = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "get_desktop_state", "arguments": {"screenshot_out_file": path(&rw, "shot.png")}}});
        match filtered(&shared, &line).await? {
            Filtered::Refused(why) if why.contains("turned off") => println!("with files switched off: refused"),
            Filtered::Refused(why) => Err(format!("with files switched off: refused for another reason: {why}"))?,
            Filtered::Forwarded(message) => Err(format!("with files switched off a screenshot file was let through: {message}"))?,
        }
        Ok(())
    }
    .await;
    core.mutate_settings(|s| s.files = files_before.clone());
    session.close().await;
    result?;
    for dir in [&rw, &ro, &outside] {
        if std::fs::read_dir(dir).map(|d| d.count()).unwrap_or(0) != 0 {
            return Err(format!("something was written into {}", dir.display()));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------
// How it looks. Everything is proved by reading it back from the real
// driver (`get_agent_cursor_state`, `cursor-theme list`); no tool that
// acts is called, so the driver never draws a cursor.
// ---------------------------------------------------------------------

/// One computer-control session the way the app runs one: it holds our
/// daemon (`Shared::cua_session`), talks to it through the core's proxy
/// spawn, and every line "from the synth" goes through the app's own
/// filter (`mcp::filter_line`) first.
struct Session {
    shared: Arc<Shared>,
    held: CuaSession,
    child: tokio::process::Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

/// What became of one call "from the synth".
enum Synth {
    /// The filter let it through (maybe changed); the driver's result.
    Answer(Value),
    /// The filter refused it; the driver never saw it.
    Refused(String),
}

impl Session {
    async fn open(shared: &Arc<Shared>) -> Result<Session, String> {
        let held = shared.cua_session().await.map_err(|e| format!("no daemon for a session: {e}"))?;
        let mut child = connect_core::mcp::spawn_proxy(shared, &held.socket, held.embedded)
            .map_err(|e| format!("spawn the mcp proxy: {e}"))?;
        let mut stdin = child.stdin.take().ok_or("the proxy has no stdin")?;
        let mut stdout = BufReader::new(child.stdout.take().ok_or("the proxy has no stdout")?);
        call(
            &mut stdin,
            &mut stdout,
            1,
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "connect-selftest", "version": env!("CARGO_PKG_VERSION")}
            }),
        )
        .await?;
        send(&mut stdin, &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await?;
        Ok(Session { shared: shared.clone(), held, child, stdin, stdout, next_id: 10 })
    }

    fn endpoint(&self) -> &str {
        &self.held.socket
    }

    /// One `tools/call` as the synth would send it.
    async fn synth(&mut self, tool: &str, arguments: Value) -> Result<Synth, String> {
        self.next_id += 1;
        let id = self.next_id;
        let line = json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": tool, "arguments": arguments}});
        match filtered(&self.shared, &line).await? {
            Filtered::Refused(why) => Ok(Synth::Refused(why)),
            Filtered::Forwarded(message) => {
                send(&mut self.stdin, &message).await?;
                Ok(Synth::Answer(answer(&mut self.stdout, id, tool).await?))
            }
        }
    }

    /// One `tools/call` as the synth would send it in THIS session: decided
    /// by the choice of apps the session's daemon was started with, as the
    /// app's own pipe decides it.
    async fn ask(&mut self, tool: &str, arguments: Value) -> Result<Asked, String> {
        self.next_id += 1;
        let id = self.next_id;
        let line = json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": tool, "arguments": arguments}});
        match piped(&self.shared, Some(&self.held.scope), &line).await? {
            Piped::Refused(why) => Ok(Asked::Refused(why)),
            Piped::App(result) => Ok(Asked::App(result)),
            Piped::Driver(message) => {
                send(&mut self.stdin, &message).await?;
                Ok(Asked::Driver(answer(&mut self.stdout, id, tool).await?))
            }
        }
    }

    /// One `tools/call` handed to the driver as it is, past the app's
    /// filter: what the DRIVER itself does with it.
    async fn direct(&mut self, tool: &str, arguments: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        send(&mut self.stdin, &json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": tool, "arguments": arguments}})).await?;
        answer(&mut self.stdout, id, tool).await
    }

    /// Any other request as the synth would send it (`tools/list`).
    async fn synth_raw(&mut self, method: &str, params: Value) -> Result<Option<Value>, String> {
        self.next_id += 1;
        let id = self.next_id;
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        match filtered(&self.shared, &line).await? {
            Filtered::Refused(_) => Ok(None),
            Filtered::Forwarded(message) => {
                send(&mut self.stdin, &message).await?;
                Ok(Some(answer(&mut self.stdout, id, method).await?))
            }
        }
    }

    /// The cursor of one of this connection's sessions, as the driver
    /// reports it: Ok(state), or Err(the driver's refusal).
    async fn cursor(&mut self, arguments: Value) -> Result<Result<Value, String>, String> {
        match self.synth("get_agent_cursor_state", arguments).await? {
            Synth::Refused(why) => Err(format!("reading the cursor state was refused by the filter: {why}")),
            Synth::Answer(result) => {
                if result.get("isError").and_then(Value::as_bool) == Some(true) {
                    return Ok(Err(short(&result)));
                }
                result
                    .get("structuredContent")
                    .cloned()
                    .map(Ok)
                    .ok_or_else(|| format!("get_agent_cursor_state answered without a state: {}", short(&result)))
            }
        }
    }

    async fn close(mut self) {
        drop(self.stdin);
        let _ = tokio::time::timeout(Duration::from_secs(10), self.child.wait()).await;
        let _ = self.child.start_kill();
    }
}

enum Filtered {
    Forwarded(Value),
    Refused(String),
}

/// Send one line through the app's own filter, by the owner's settings as
/// they are now. For the checks that do not limit the apps: there the app
/// never answers a call itself.
async fn filtered(shared: &Arc<Shared>, line: &Value) -> Result<Filtered, String> {
    match piped(shared, None, line).await? {
        Piped::Driver(message) => Ok(Filtered::Forwarded(message)),
        Piped::Refused(why) => Ok(Filtered::Refused(why)),
        Piped::App(result) => Err(format!("the app answered a call itself where the apps are not limited: {}", short(&result))),
    }
}

/// What the app's pipe does with one call of the synth's.
enum Piped {
    /// Handed to the driver, as this message (maybe changed).
    Driver(Value),
    /// Answered by the app itself with this result (a list, cut down to
    /// the apps the synth may use); the driver is not asked.
    App(Value),
    /// Refused with this sentence; the driver never sees it.
    Refused(String),
}

/// What became of one call "from the synth" in a session (`Session::ask`).
enum Asked {
    /// The driver's result.
    Driver(Value),
    /// The app's own result.
    App(Value),
    Refused(String),
}

/// Send one line through the app's own filter. `scope`: the choice of apps
/// of a session that is open; None = the owner's settings as they are now.
async fn piped(shared: &Arc<Shared>, scope: Option<&connect_core::cua_apps::Scope>, line: &Value) -> Result<Piped, String> {
    let bytes = serde_json::to_vec(line).map_err(|e| e.to_string())?;
    let gate = match scope {
        Some(scope) => connect_core::mcp::filter_scoped(shared, scope, false, &bytes).await,
        None => connect_core::mcp::filter_line(shared, false, &bytes).await,
    };
    match gate {
        connect_core::mcp::Gate::Forward(out) => serde_json::from_slice(&out)
            .map(Piped::Driver)
            .map_err(|e| format!("the filter forwarded something that is not JSON: {e}")),
        connect_core::mcp::Gate::Answer(answer) => Ok(Piped::App(answer.get("result").cloned().unwrap_or(Value::Null))),
        connect_core::mcp::Gate::Refuse(error) => {
            // Every refusal says what it asks of the synth: the owner's
            // policy, or an argument it can correct; and which refusal.
            let class = error.pointer("/error/data/class").and_then(Value::as_str).unwrap_or("");
            let code = error.pointer("/error/data/code").and_then(Value::as_str).unwrap_or("");
            if !matches!(class, "owner_policy" | "bad_argument") || code.is_empty() {
                return Err(format!("a refusal without a class and a code in error.data: {error}"));
            }
            Ok(Piped::Refused(error.pointer("/error/message").and_then(Value::as_str).unwrap_or("").to_string()))
        }
        connect_core::mcp::Gate::Drop(why) => Err(format!("the filter dropped a well-formed call: {why}")),
    }
}

/// What a session's cursor must read back as.
struct Expect {
    theme: &'static str,
    reduced: &'static str,
    glide_ms: f64,
    dwell_ms: f64,
    idle_hide_ms: f64,
}

fn check_cursor(state: &Value, want: &Expect, whose: &str) -> Result<(), String> {
    let text = |path: &str| state.pointer(path).and_then(Value::as_str).unwrap_or("?").to_string();
    let number = |path: &str| state.pointer(path).and_then(Value::as_f64).unwrap_or(-1.0);
    let got = (
        text("/theme/id"),
        text("/theme/reduced_motion"),
        number("/motion/glide_duration_ms"),
        number("/motion/dwell_after_click_ms"),
        number("/motion/idle_hide_ms"),
    );
    println!(
        "{whose}: theme {} (fallback {}), reduced motion {}, glide {} ms, dwell {} ms, idle hide {} ms, enabled {}, style {} ({} timing)",
        got.0,
        state.pointer("/theme/fallback").unwrap_or(&Value::Null),
        got.1,
        got.2,
        got.3,
        got.4,
        state.get("enabled").unwrap_or(&Value::Null),
        text("/motion/style"),
        text("/motion/timing"),
    );
    if !state.pointer("/theme/fallback").is_none_or(Value::is_null) {
        return Err(format!("{whose}: the driver fell back from the theme: {state}"));
    }
    if state.get("enabled").and_then(Value::as_bool) != Some(true) {
        return Err(format!("{whose}: the cursor is not enabled: {state}"));
    }
    let want_tuple = (
        want.theme.to_string(),
        want.reduced.to_string(),
        want.glide_ms,
        want.dwell_ms,
        want.idle_hide_ms,
    );
    if got != want_tuple {
        return Err(format!("{whose}: read back {got:?}, expected {want_tuple:?}"));
    }
    Ok(())
}

/// Returns the endpoint of the daemon that runs at the end.
async fn look(core: &Core, root: &std::path::Path, first_endpoint: String) -> Result<String, String> {
    use connect_core::cua_look;
    let shared = core.shared();
    let support = core.look_support();
    println!("this computer's driver follows: {support:?}");

    step("the cursor themes the app ships are accepted by the pinned driver, in OUR driver home");
    let themes = core.cua_theme_dir().ok_or("the core has no theme folder of its own")?;
    if !themes.starts_with(root) {
        return Err(format!("the theme folder {} is outside the private root", themes.display()));
    }
    cua_look::install_themes(&themes).map_err(|e| format!("write the themes: {e}"))?;
    let listed = driver(core, &["cursor-theme", "list", "--json"]).await?;
    let listed: Vec<String> = serde_json::from_str(listed.trim())
        .map_err(|e| format!("cursor-theme list --json did not answer a list ({e}): {listed}"))?;
    println!("cursor-theme list: {listed:?}");
    for theme in cua_look::THEMES {
        if !listed.iter().any(|id| id == theme.id) {
            return Err(format!("the driver does not list the theme {}", theme.id));
        }
        let file = themes.join(format!("{}.cua-theme", theme.id));
        let seen = driver(core, &["cursor-theme", "inspect", &file.to_string_lossy()]).await?;
        if !seen.lines().any(|l| l.trim() == format!("id: {}", theme.id)) {
            return Err(format!("the driver does not accept {}: {seen}", file.display()));
        }
    }
    println!("inspect: all {} themes accepted", cua_look::THEMES.len());

    step("the owner's look is what every session gets");
    core.mutate_settings(|s| {
        s.computer.look = LookSettings {
            cursor: true,
            color: CursorColor::Rose,
            speed: CursorSpeed::Fast,
            idle_hide_seconds: 5,
            reduced_motion: ReducedMotion::Always,
            screenshot_max: 0,
            preview_window: false,
        };
    });
    // Nobody holds the daemon that ran with the default look: a session
    // that starts now must get a new one.
    let mut a = Session::open(&shared).await?;
    println!("daemon look: {:?}", shared.cua_daemon_look().await.unwrap_or_default());
    if a.endpoint() == first_endpoint {
        return Err("the look changed while no session was open, but the old daemon was kept".into());
    }
    if connect_core::cua::endpoint_ready(&first_endpoint) {
        return Err("the daemon with the old look still runs".into());
    }
    let rose = if support.color { "ai.snth.connect.rose" } else { "cua.default" };
    let fast = if support.motion {
        Expect { theme: rose, reduced: "on", glide_ms: 250.0, dwell_ms: 40.0, idle_hide_ms: 5000.0 }
    } else {
        Expect { theme: rose, reduced: "auto", glide_ms: 0.0, dwell_ms: 80.0, idle_hide_ms: 15000.0 }
    };
    for (whose, arguments) in [
        ("a named session", json!({"session": "task-look"})),
        ("another session", json!({"session": "task-other"})),
        ("the unnamed session", json!({})),
    ] {
        let state = a.cursor(arguments).await?.map_err(|e| format!("{whose}: no cursor state: {e}"))?;
        check_cursor(&state, &fast, whose)?;
    }

    step("the synth cannot change the look");
    for (tool, arguments) in [
        ("set_agent_cursor_theme", json!({"session": "task-look", "theme_id": "cua.default", "reduced_motion": "off"})),
        ("set_agent_cursor_motion", json!({"session": "task-look", "glide_duration_ms": 5000, "idle_hide_ms": 0})),
        ("set_agent_cursor_motion", json!({"session": "task-look", "style": "magnetic", "timing": "fixed", "effects": {"trail": true}})),
        ("set_config", json!({"cursor.motion.style": "classic"})),
        ("set_agent_cursor_enabled", json!({"session": "task-look", "enabled": false})),
        ("set_config", json!({"experimental_pip": true})),
        ("set_config", json!({"key": "max_image_dimension", "value": 4000})),
    ] {
        match a.synth(tool, arguments).await? {
            Synth::Refused(why) if why.starts_with("denied: ") => println!("{tool}: refused ({why})"),
            Synth::Refused(why) => return Err(format!("{tool}: refused without a reason the synth can read: {why:?}")),
            Synth::Answer(result) => return Err(format!("{tool} reached the driver: {}", short(&result))),
        }
    }
    // A session may start; neither the cursor theme nor (since 0.34) the
    // motion style the synth asked for is passed on.
    let wanted = json!({"session": "task-look", "cursor_theme": {"theme_id": "cua.default", "reduced_motion": "off"}, "cursor_motion": {"style": "comet_swoop", "timing": "fitts", "effects": {"trail": true}}});
    let start = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "start_session", "arguments": wanted}});
    match filtered(&shared, &start).await? {
        Filtered::Forwarded(message) if message["params"]["arguments"] == json!({"session": "task-look"}) => {
            println!("start_session: forwarded as {}", message["params"]["arguments"]);
        }
        Filtered::Forwarded(message) => return Err(format!("start_session kept the synth's theme or motion: {message}")),
        Filtered::Refused(why) => return Err(format!("start_session was refused: {why}")),
    }
    match a.synth("start_session", wanted.clone()).await?
    {
        Synth::Answer(result) if result.get("isError").and_then(Value::as_bool) != Some(true) => {
            println!("start_session: {}", short(&result));
            if result.pointer("/structuredContent/cursor_motion").is_some() {
                return Err(format!("the driver was given a motion style for the session: {}", short(&result)));
            }
        }
        Synth::Answer(result) => return Err(format!("start_session failed: {}", short(&result))),
        Synth::Refused(why) => return Err(format!("start_session was refused: {why}")),
    }
    let state = a.cursor(json!({"session": "task-look"})).await?.map_err(|e| format!("no cursor state: {e}"))?;
    check_cursor(&state, &fast, "after the synth's attempts")?;
    let journal = serde_json::to_string(&core.recent_journal()).unwrap_or_default();
    if !journal.contains("denied: the look is the owner's to set") {
        return Err("the refusals are not in the journal".into());
    }

    step("a screenshot is asked for within the owner's limit");
    let listed = match a.synth_raw("tools/list", json!({})).await? {
        Some(v) => v,
        None => return Err("tools/list was not forwarded".into()),
    };
    for tool in cua_look::SCREENSHOT_TOOLS {
        let has = listed
            .get("tools")
            .and_then(Value::as_array)
            .and_then(|tools| tools.iter().find(|t| t.get("name").and_then(Value::as_str) == Some(tool)))
            .is_some_and(|t| t.pointer("/inputSchema/properties/max_image_dimension").is_some());
        if !has {
            return Err(format!("the driver's {tool} no longer takes max_image_dimension"));
        }
    }
    core.mutate_settings(|s| s.computer.look.screenshot_max = 1024);
    for (tool, arguments, want) in [
        ("get_window_state", json!({"session": "task-look", "pid": 1, "window_id": 1}), 1024),
        ("get_window_state", json!({"session": "task-look", "pid": 1, "window_id": 1, "max_image_dimension": 0}), 1024),
        ("get_desktop_state", json!({"max_image_dimension": 4000}), 1024),
        ("get_desktop_state", json!({"max_image_dimension": 640}), 640),
    ] {
        // Only the filter is asked: no screenshot is taken.
        let line = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": arguments}});
        match filtered(&shared, &line).await? {
            Filtered::Forwarded(message) => {
                let got = message.pointer("/params/arguments/max_image_dimension").and_then(Value::as_u64);
                println!("{tool} {arguments} -> max_image_dimension {got:?}");
                if got != Some(want) {
                    return Err(format!("{tool}: forwarded with max_image_dimension {got:?}, expected {want}"));
                }
            }
            Filtered::Refused(why) => return Err(format!("{tool} was refused: {why}")),
        }
    }

    step("a new look waits for the open session to end, then holds");
    core.mutate_settings(|s| {
        s.computer.look.color = CursorColor::Green;
        s.computer.look.speed = CursorSpeed::Relaxed;
        s.computer.look.idle_hide_seconds = 0;
        s.computer.look.reduced_motion = ReducedMotion::Never;
    });
    let mut b = Session::open(&shared).await?;
    if b.endpoint() != a.endpoint() {
        return Err("the daemon was replaced while a session was open".into());
    }
    let state = a.cursor(json!({"session": "task-look"})).await?.map_err(|e| format!("the open session lost its cursor: {e}"))?;
    check_cursor(&state, &fast, "the session that was open")?;
    let state = b.cursor(json!({"session": "task-second"})).await?.map_err(|e| format!("no cursor state: {e}"))?;
    check_cursor(&state, &fast, "a session opened meanwhile")?;
    let held_endpoint = a.endpoint().to_string();
    b.close().await;
    a.close().await;
    if shared.cua_sessions_open() != 0 {
        return Err(format!("{} sessions still hold the daemon after they ended", shared.cua_sessions_open()));
    }
    let green = if support.color { "ai.snth.connect.green" } else { "cua.default" };
    let relaxed = if support.motion {
        Expect { theme: green, reduced: "off", glide_ms: 1500.0, dwell_ms: 400.0, idle_hide_ms: 0.0 }
    } else {
        Expect { theme: green, reduced: "auto", glide_ms: 0.0, dwell_ms: 80.0, idle_hide_ms: 15000.0 }
    };
    let mut c = Session::open(&shared).await?;
    println!("daemon look: {:?}", shared.cua_daemon_look().await.unwrap_or_default());
    if c.endpoint() == held_endpoint {
        return Err("the new look did not get a new daemon after the sessions ended".into());
    }
    let state = c.cursor(json!({"session": "task-look"})).await?.map_err(|e| format!("no cursor state: {e}"))?;
    check_cursor(&state, &relaxed, "the next session")?;
    c.close().await;

    step("the look survives a restart of the app");
    core.cua_stop_daemon().await;
    let chosen = core.settings().computer.look;
    let again = selftest_core(root);
    if again.settings().computer.look != chosen {
        return Err(format!("settings.json gave back {:?}, expected {chosen:?}", again.settings().computer.look));
    }
    let again_shared = again.shared();
    let mut d = Session::open(&again_shared).await?;
    let state = d.cursor(json!({"session": "task-look"})).await?.map_err(|e| format!("no cursor state: {e}"))?;
    check_cursor(&state, &relaxed, "after the restart")?;
    d.close().await;
    again.cua_stop_daemon().await;

    step("the cursor can be switched off");
    core.mutate_settings(|s| s.computer.look.cursor = false);
    let mut e = Session::open(&shared).await?;
    println!("daemon look: {:?}", shared.cua_daemon_look().await.unwrap_or_default());
    match e.cursor(json!({"session": "task-look"})).await? {
        // macOS: the driver has no overlay at all and says so.
        Err(refusal) => println!("no cursor: {refusal}"),
        Ok(state) if state.get("enabled").and_then(Value::as_bool) == Some(false) => println!("the cursor is disabled: {state}"),
        Ok(state) => return Err(format!("the cursor is still on with the overlay switched off: {state}")),
    }
    // The one tool that moves nothing but that cursor is answered by the
    // app, in words the synth can act on (and nothing is moved).
    match e.synth("move_cursor", json!({"session": "task-look", "x": 40, "y": 40})).await? {
        Synth::Refused(why) if why.contains("switched the agent cursor off") => println!("move_cursor: refused ({why})"),
        Synth::Refused(why) => return Err(format!("move_cursor: refused with {why:?}")),
        Synth::Answer(result) => return Err(format!("move_cursor reached the driver: {}", short(&result))),
    }
    // The rest of the driver works without it.
    match e.synth("get_screen_size", json!({})).await? {
        Synth::Answer(result) if result.get("isError").and_then(Value::as_bool) != Some(true) => {
            println!("get_screen_size without the cursor: ok ({})", short(&result));
        }
        Synth::Answer(result) => return Err(format!("get_screen_size failed without the cursor: {}", short(&result))),
        Synth::Refused(why) => return Err(format!("get_screen_size was refused: {why}")),
    }
    let endpoint = e.endpoint().to_string();
    e.close().await;
    core.mutate_settings(|s| s.computer.look = LookSettings::default());
    Ok(endpoint)
}

// ---------------------------------------------------------------------
// Which apps the synth may use (`connect_core::cua_apps`), against the
// real driver and windows that are open anyway. The driver is only ever
// asked to list and to read: nothing is launched, closed, clicked or typed
// through it. Calls that would act are shown to the app's filter alone.
// ---------------------------------------------------------------------

/// One app that has a window right now.
#[derive(Clone)]
struct Seen {
    /// What the driver matches it by: bundle id or program path.
    key: String,
    name: String,
    pid: i64,
    window: u64,
}

/// The running apps that have a window, as the app's own reading of the
/// driver's lists has them. Never the driver's own app (a driver the user
/// runs themselves is left alone, even by a look).
async fn apps_with_windows(shared: &Arc<Shared>) -> Result<Vec<Seen>, String> {
    let dir = shared.cua_directory();
    let apps = dir.apps_fresh().await.map_err(|e| format!("the app's own list of apps: {e}"))?;
    let listed = dir.windows_listing().await.map_err(|e| format!("the app's own list of windows: {e}"))?;
    let windows: Vec<(u64, i64)> = listed
        .get("windows")
        .and_then(Value::as_array)
        .map(|all| all.iter().filter_map(|w| Some((w.get("window_id")?.as_u64()?, w.get("pid")?.as_i64()?))).collect())
        .unwrap_or_default();
    let mut seen: Vec<Seen> = Vec::new();
    for app in apps.iter().filter(|a| a.running) {
        let Some(key) = app.key() else {
            continue;
        };
        let lower = format!("{key} {}", app.name).to_lowercase();
        if lower.contains("trycua") || lower.contains("cua driver") || lower.contains("cua-driver") {
            continue;
        }
        if seen.iter().any(|s| s.key.eq_ignore_ascii_case(key)) {
            continue;
        }
        if let Some((window, _)) = windows.iter().find(|(_, pid)| *pid == app.pid) {
            seen.push(Seen { key: key.to_string(), name: app.name.clone(), pid: app.pid, window: *window });
        }
    }
    Ok(seen)
}

/// The driver's own refusal code of a result, if it is a refusal.
fn refusal_code(result: &Value) -> Option<&str> {
    result.pointer("/structuredContent/refusal/code").and_then(Value::as_str)
}

fn is_error(result: &Value) -> bool {
    result.get("isError").and_then(Value::as_bool) == Some(true)
}

async fn apps_scope(core: &Core, root: &std::path::Path) -> Result<(), String> {
    use connect_core::cua_apps::{self, Scope};
    use connect_core::settings::{AppRef, AppScope, AppsPolicy};
    let shared = core.shared();

    step("every tool of this driver has a place in the app's decision about apps");
    let mut session = Session::open(&shared).await?;
    let listed = session.synth_raw("tools/list", json!({})).await?.ok_or("tools/list was not forwarded")?;
    let tools: Vec<Value> = listed.get("tools").and_then(Value::as_array).cloned().unwrap_or_default();
    let unplaced = cua_apps::unplaced(&tools);
    if !unplaced.is_empty() {
        return Err(format!(
            "this driver has tools the app cannot place: {}. Read each one in the driver's source and add it to TOOLS in crates/connect-core/src/cua_apps.rs (and to MANIFEST_TOOLS if it may be used when only chosen apps are allowed)",
            unplaced.join(", ")
        ));
    }
    println!("{} tools, all placed; {} of them allowed in a daemon for chosen apps only", tools.len(), cua_apps::MANIFEST_TOOLS.len());
    session.close().await;

    step("the app's own reading of the driver's lists (a runtime of its own, never the synth's daemon)");
    let picker = core.computer_apps().await.map_err(|e| format!("the list of apps for the owner's picker: {e}"))?;
    let by_path = picker.iter().filter(|c| std::path::Path::new(&c.id).is_absolute()).count();
    println!(
        "picker: {} apps ({} running), {} known by a bundle id, {} by the path of the program; {} sensitive ones would start a list of exceptions",
        picker.len(),
        picker.iter().filter(|c| c.running).count(),
        picker.len() - by_path,
        by_path,
        cua_apps::default_exclusions(&picker).len(),
    );
    if picker.is_empty() {
        return Err("the driver lists no app at all".into());
    }

    // Two apps that have a window. Where there are none (a bare CI
    // machine), the test starts harmless ones ITSELF, not through the
    // driver, and ends them when it is done.
    let mut started: Vec<tokio::process::Child> = Vec::new();
    let mut seen = apps_with_windows(&shared).await?;
    if seen.len() < 2 && cfg!(windows) {
        for program in ["charmap.exe", "notepad.exe", "winver.exe", "mspaint.exe"] {
            if seen.len() >= 2 {
                break;
            }
            match connect_core::proc::command(program).kill_on_drop(true).spawn() {
                Ok(child) => {
                    println!("started {program} for the test (pid {:?})", child.id());
                    started.push(child);
                }
                Err(e) => {
                    println!("{program} could not be started: {e}");
                    continue;
                }
            }
            for _ in 0..20 {
                tokio::time::sleep(Duration::from_millis(500)).await;
                seen = apps_with_windows(&shared).await?;
                if seen.len() >= started.len().min(2) {
                    break;
                }
            }
        }
    }
    let result = async {
        if seen.len() < 2 {
            return Err(format!(
                "this test needs two apps with a window open, and the driver lists {} ({}); open two ordinary apps and run it again",
                seen.len(),
                seen.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
        let (allowed, other) = (seen[0].clone(), seen[1].clone());
        println!("allowed for the test: {} [{}] pid {} window {}", allowed.name, allowed.key, allowed.pid, allowed.window);
        println!("the other app:        {} [{}] pid {} window {}", other.name, other.key, other.pid, other.window);
        let pick = |app: &Seen| AppRef { id: app.key.clone(), name: app.name.clone(), loose: false };

        // ---- only these: the driver itself ----
        step("only these: our daemon runs in the driver's bounded mode, and the DRIVER refuses the rest");
        core.mutate_settings(|s| s.computer.apps = AppScope { policy: AppsPolicy::Only, except: vec![], only: vec![pick(&allowed)] });
        let scope = Scope::of(&core.settings());
        println!("--- the manifest ---\n{}--------------------", scope.manifest().ok_or("no manifest for only these")?);
        let mut only = Session::open(&shared).await?;
        if shared.cua_daemon_scope().await.as_deref() != Some(&scope) {
            return Err("the daemon was not started with the owner's choice of apps".into());
        }
        // Past the app's filter, straight to the driver.
        let at = std::time::Instant::now();
        let mine = only.direct("list_windows", json!({"pid": allowed.pid})).await?;
        let took = at.elapsed();
        let count = mine.pointer("/structuredContent/windows").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
        if is_error(&mine) || count == 0 {
            return Err(format!("the driver did not list the allowed app's windows: {}", short(&mine)));
        }
        println!("driver, list_windows of the allowed app: {count} window(s), in {:.1} s", took.as_secs_f32());
        let state = only
            .direct("get_window_state", json!({"pid": allowed.pid, "window_id": allowed.window, "include_screenshot": false, "max_elements": 40}))
            .await?;
        if refusal_code(&state) == Some("bounded_resource_outside_manifest") {
            return Err(format!("the driver refused the ALLOWED app's window as outside the manifest: {}", short(&state)));
        }
        println!("driver, get_window_state of the allowed app: {} ({})", if is_error(&state) { "answered with an error of its own, not a refusal of the manifest" } else { "observed" }, short(&state));
        for (what, tool, arguments) in [
            ("the other app's windows", "list_windows", json!({"pid": other.pid})),
            ("the other app's window", "get_window_state", json!({"pid": other.pid, "window_id": other.window, "include_screenshot": false})),
            ("the other app's window by its id alone", "zoom", json!({"window_id": other.window, "x1": 0, "y1": 0, "x2": 4, "y2": 4})),
            ("the list of every window", "list_windows", json!({})),
        ] {
            let result = only.direct(tool, arguments).await?;
            match refusal_code(&result) {
                Some("bounded_resource_outside_manifest") => println!("driver, {what}: refused (bounded_resource_outside_manifest)"),
                other_code => return Err(format!("the driver did not refuse {what} as outside the manifest (code {other_code:?}): {}", short(&result))),
            }
        }
        // Tools the manifest does not list at all (the whole screen, the
        // clipboard): the driver refuses them by name.
        for (what, tool, arguments) in [
            ("the list of every app", "list_apps", json!({})),
            ("the picture of the whole screen", "get_desktop_state", json!({"max_image_dimension": 64})),
            ("the clipboard", "clipboard_read", json!({})),
        ] {
            let result = only.direct(tool, arguments).await?;
            let said = result.pointer("/content/0/text").and_then(Value::as_str).unwrap_or("");
            if !is_error(&result) || !said.contains("outside the capability manifest") {
                return Err(format!("the bounded driver did not refuse {what} ({tool}), which the manifest does not list: {}", short(&result)));
            }
            println!("driver, {what}: refused ({said})");
        }
        // What touches no app still works.
        for (tool, arguments) in [
            ("get_screen_size", json!({})),
            ("start_session", json!({"session": "Mia: self-test"})),
            ("get_agent_cursor_state", json!({"session": "Mia: self-test"})),
            ("health_report", json!({"include": ["binary_version", "session_active"]})),
            ("list_sessions", json!({})),
        ] {
            let result = only.direct(tool, arguments).await?;
            if is_error(&result) {
                return Err(format!("{tool} does not work in the bounded daemon: {}", short(&result)));
            }
            println!("driver, {tool}: ok ({})", short(&result));
        }
        // And as the synth: the same answers come from the app first, in
        // words, and the two lists are the app's own.
        match only.ask("get_window_state", json!({"pid": other.pid, "window_id": other.window})).await? {
            Asked::Refused(why) if why.starts_with("denied: ") && why.contains("only the apps they chose") && why.contains("owner") => println!("synth, the other app's window: refused ({why})"),
            Asked::Refused(why) => return Err(format!("the other app's window was refused in other words: {why}")),
            _ => return Err("the other app's window passed the app's filter".into()),
        }
        match only.ask("list_apps", json!({})).await? {
            Asked::App(result) => {
                let apps = result.pointer("/structuredContent/apps").and_then(Value::as_array).cloned().unwrap_or_default();
                let keys: Vec<String> = apps.iter().map(|a| a.to_string()).collect();
                if apps.is_empty() || !keys.iter().all(|a| a.contains(&serde_json::to_string(&allowed.key).unwrap_or_default())) {
                    return Err(format!("the app's list_apps is not the allowed app alone: {}", short(&result)));
                }
                println!("synth, list_apps: answered by the app, {} entr(ies), all of the allowed app", apps.len());
            }
            _ => return Err("list_apps was not answered by the app".into()),
        }
        match only.ask("list_windows", json!({})).await? {
            Asked::App(result) => {
                let windows = result.pointer("/structuredContent/windows").and_then(Value::as_array).cloned().unwrap_or_default();
                if windows.is_empty() || windows.iter().any(|w| w.get("pid").and_then(Value::as_i64) != Some(allowed.pid)) {
                    return Err(format!("the app's list_windows has windows of other apps or none: {} window(s)", windows.len()));
                }
                println!("synth, list_windows: answered by the app, {} window(s), all of the allowed app", windows.len());
            }
            _ => return Err("list_windows was not answered by the app".into()),
        }
        // Starting an app: the driver does not check which, the app does.
        // Only the filter is asked: nothing is started.
        let launch = |key: &str| {
            let field = if std::path::Path::new(key).is_absolute() { "launch_path" } else { "bundle_id" };
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "launch_app", "arguments": {field: key}}})
        };
        match piped(&shared, Some(&only.held.scope), &launch(&other.key)).await? {
            Piped::Refused(why) if why.starts_with("denied: ") => println!("synth, starting the other app: refused by the app"),
            _ => return Err("starting an app that is not allowed passed the app's filter".into()),
        }
        match piped(&shared, Some(&only.held.scope), &launch(&allowed.key)).await? {
            Piped::Driver(_) => println!("synth, starting the allowed app: would pass (not sent)"),
            _ => return Err("starting the allowed app was refused".into()),
        }
        // The helper check sees the bounded daemon for what it is.
        let health = core.cua_health().await;
        match health.items.iter().find(|i| i.id == "apps") {
            Some(item) if item.code == "bounded" => println!("check the helper: apps = only {} (the helper refuses the rest itself)", item.value),
            other_item => return Err(format!("the helper check does not see the bounded daemon: {other_item:?}")),
        }
        let only_endpoint = only.endpoint().to_string();
        only.close().await;
        let journal = serde_json::to_string(&core.recent_journal()).unwrap_or_default();
        if !journal.contains("denied: not one of the apps the owner chose") {
            return Err("the refusal is not in the journal".into());
        }

        // ---- every app except these: the app's pipe ----
        step("every app except these: the app resolves every call to an app and refuses the excluded one");
        core.mutate_settings(|s| s.computer.apps = AppScope { policy: AppsPolicy::Except, except: vec![pick(&other)], only: vec![] });
        let mut except = Session::open(&shared).await?;
        if except.endpoint() == only_endpoint {
            return Err("the owner's choice of apps changed while no session was open, but the bounded daemon was kept".into());
        }
        // The daemon is an ordinary one now: the driver itself answers
        // the list of every app (the driver has no "except").
        let every = except.direct("list_apps", json!({})).await?;
        if is_error(&every) {
            return Err(format!("the daemon for exceptions is not an ordinary one: {}", short(&every)));
        }
        println!("driver, list_apps: answered (the driver has no way to except an app; the app does it)");
        let refused = |what: &str, got: Piped| match got {
            Piped::Refused(why) if why.starts_with("denied: ") && why.contains("owner") => {
                println!("synth, {what}: refused ({})", why.chars().take(90).collect::<String>());
                Ok(())
            }
            Piped::Refused(why) => Err(format!("{what}: refused without words for the owner: {why}")),
            Piped::Driver(message) => Err(format!("{what} passed the app's filter: {message}")),
            Piped::App(_) => Err(format!("{what} was answered by the app")),
        };
        let call = |tool: &str, arguments: Value| json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": arguments}});
        // Only the filter is asked for everything that would act.
        for (what, tool, arguments) in [
            ("the excluded app's window by pid and window", "get_window_state", json!({"pid": other.pid, "window_id": other.window})),
            ("the excluded app's window by its id alone", "zoom", json!({"window_id": other.window, "x1": 0, "y1": 0, "x2": 4, "y2": 4})),
            ("the excluded app by pid alone", "list_windows", json!({"pid": other.pid})),
            ("a click with the allowed app's pid and the excluded app's window", "click", json!({"pid": allowed.pid, "window_id": other.window, "x": 1, "y": 1})),
            ("a click in a target object", "click", json!({"x": 1, "y": 1, "target": {"kind": "window", "pid": other.pid, "window_id": other.window}})),
            ("a click by screen position", "click", json!({"x": 10, "y": 10, "scope": "desktop"})),
            ("typing with no window named", "type_text", json!({"text": "x"})),
            ("the picture of the whole screen", "get_desktop_state", json!({})),
            ("the desktop's tree of every app", "get_accessibility_tree", json!({})),
            ("starting the excluded app", "launch_app", launch(&other.key)["params"]["arguments"].clone()),
        ] {
            refused(what, piped(&shared, Some(&except.held.scope), &call(tool, arguments)).await?)?;
        }
        match piped(&shared, Some(&except.held.scope), &call("click", json!({"pid": allowed.pid, "window_id": allowed.window, "x": 1, "y": 1}))).await? {
            Piped::Driver(_) => println!("synth, a click in the other app's own window: would pass (not sent)"),
            _ => return Err("a click in an app that is not excluded was refused".into()),
        }
        match except.ask("get_window_state", json!({"pid": allowed.pid, "window_id": allowed.window, "include_screenshot": false, "max_elements": 40})).await? {
            Asked::Driver(result) => println!("synth, a window of an app that is not excluded: reaches the driver ({})", short(&result)),
            Asked::Refused(why) => return Err(format!("a window of an app that is not excluded was refused: {why}")),
            Asked::App(_) => return Err("get_window_state was answered by the app".into()),
        }
        match except.ask("list_windows", json!({})).await? {
            Asked::App(result) => {
                let windows = result.pointer("/structuredContent/windows").and_then(Value::as_array).cloned().unwrap_or_default();
                let pids: Vec<i64> = windows.iter().filter_map(|w| w.get("pid").and_then(Value::as_i64)).collect();
                if pids.contains(&other.pid) || !pids.contains(&allowed.pid) {
                    return Err(format!("the app's list_windows is wrong: excluded app listed {}, the other app listed {}", pids.contains(&other.pid), pids.contains(&allowed.pid)));
                }
                println!("synth, list_windows: answered by the app, {} window(s), none of the excluded app", windows.len());
            }
            _ => return Err("list_windows was not answered by the app".into()),
        }
        match except.ask("list_apps", json!({})).await? {
            Asked::App(result) => {
                let text = result.pointer("/structuredContent/apps").map(Value::to_string).unwrap_or_default();
                if text.contains(&serde_json::to_string(&other.key).unwrap_or_default()) {
                    return Err("the app's list_apps names the excluded app".into());
                }
                println!("synth, list_apps: answered by the app, without the excluded app");
            }
            _ => return Err("list_apps was not answered by the app".into()),
        }

        step("a session keeps the choice of apps it began with; the next one gets the new choice");
        core.mutate_settings(|s| s.computer.apps = AppScope::default());
        refused("the excluded app, after the owner allowed every app, in the session that was open", piped(&shared, Some(&except.held.scope), &call("get_window_state", json!({"pid": other.pid, "window_id": other.window}))).await?)?;
        if !except.held.outdated() {
            return Err("the open session does not know the owner's choice moved on".into());
        }
        match filtered(&shared, &call("get_window_state", json!({"pid": other.pid, "window_id": other.window}))).await? {
            Filtered::Forwarded(_) => println!("by the settings as they are now: passes"),
            Filtered::Refused(why) => return Err(format!("with every app allowed the call is refused: {why}")),
        }
        except.close().await;

        step("a word the owner typed by hand in an older version still excludes the app, now by its window too");
        let word: String = other.name.to_lowercase().chars().take(6).collect();
        core.mutate_settings(|s| {
            s.computer.apps = AppScope::default();
            s.computer.app_blocklist = vec![word.clone()];
            s.migrate();
        });
        let migrated = core.settings().computer.apps;
        if migrated.policy != AppsPolicy::Except || migrated.except.len() != 1 || !migrated.except[0].loose {
            return Err(format!("the old blocked apps were not carried over: {migrated:?}"));
        }
        refused("the app named by an old word, reached by its window", piped(&shared, None, &call("get_window_state", json!({"window_id": other.window}))).await?)?;

        let journal = serde_json::to_string(&core.recent_journal()).unwrap_or_default();
        if !journal.contains("denied: an app the owner excluded") || !journal.contains("denied: the whole screen, while apps are limited") {
            return Err("the refusals are not in the journal".into());
        }
        if journal.contains(&other.key) || journal.contains("Mia") {
            return Err("the journal names an app or a task label".into());
        }
        Ok(())
    }
    .await;

    core.mutate_settings(|s| s.computer.apps = AppScope::default());
    shared.cua_directory().stop().await;
    for mut child in started {
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
    }
    result?;

    step("check the helper: the driver's own health report from OUR daemon");
    let health = core.cua_health().await;
    println!("overall: {}", health.overall);
    for item in &health.items {
        println!("  {:?} {}{}{}", item.verdict, item.id, if item.code.is_empty() { String::new() } else { format!(" ({})", item.code) }, if item.value.is_empty() { String::new() } else { format!(": {}", item.value) });
    }
    println!("--- copy details ---\n{}\n--------------------", health.details);
    let ok = |id: &str| health.items.iter().any(|i| i.id == id && i.verdict == connect_core::cua_health::Verdict::Ok);
    if !ok("installed") || !ok("daemon") {
        return Err("the helper check did not find the helper installed and its daemon answering".into());
    }
    if !health.items.iter().any(|i| i.id == "platform_supported") {
        return Err("the helper check has none of the driver's own checks".into());
    }
    let mut places = vec![root.to_string_lossy().to_string()];
    if let Ok(real) = std::fs::canonicalize(root) {
        places.push(real.to_string_lossy().to_string());
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        places.push(home.to_string_lossy().to_string());
    }
    for place in places.iter().filter(|p| p.len() > 3) {
        if health.details.contains(place.as_str()) {
            return Err(format!("the text for copy details names a place on this computer: {place}"));
        }
    }

    // For the record of what the driver's other diagnostic answers. Not on
    // macOS: there `doctor` deletes files of a driver the user may have
    // installed for themselves. `diagnose` is run nowhere: it asks the
    // daemon on the default socket, which is never ours.
    if !cfg!(target_os = "macos") {
        step("for the record: the driver's `doctor --json` (the app does not use it)");
        match driver(core, &["doctor", "--json"]).await {
            Ok(out) => println!("{}", out.chars().take(4000).collect::<String>()),
            Err(why) => println!("{}", why.chars().take(4000).collect::<String>()),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------
// An update of the app that brings a new driver version. The "version
// before" is the copy that is installed, moved into another version's
// folder: to the installer that is exactly what an older app version left.
// ---------------------------------------------------------------------

async fn update_path(core: &Core, root: &std::path::Path) -> Result<(), String> {
    step("an app update with a new driver: the old copy runs until the new one is in place and nobody holds it");
    core.cua_stop_daemon().await;
    let pinned = std::path::PathBuf::from(core.cua_bin());
    let pinned_dir = pinned.parent().ok_or("the driver has no folder")?.to_path_buf();
    let releases = pinned_dir.parent().ok_or("the driver has no releases folder")?.to_path_buf();
    let old_dir = releases.join("0.0.1-before");
    let _ = std::fs::remove_dir_all(&old_dir);
    std::fs::rename(&pinned_dir, &old_dir).map_err(|e| format!("move the installed copy aside: {e}"))?;
    let old_bin = old_dir.join(pinned.file_name().ok_or("the driver has no file name")?);

    // "The app after the update": a new core, whose pinned copy is missing.
    let next = selftest_core(root);
    let shared = next.shared();
    if std::path::Path::new(&shared.cua_bin_now()) != old_bin {
        return Err(format!("with the new version missing, the app would run {} and not the old copy", shared.cua_bin_now()));
    }
    let mut held = Session::open(&shared).await.map_err(|e| format!("computer control does not work on the old copy: {e}"))?;
    match held.synth("get_screen_size", json!({})).await? {
        Synth::Answer(result) if !is_error(&result) => println!("on the old copy, before the new one is there: get_screen_size ok ({})", short(&result)),
        Synth::Answer(result) => return Err(format!("the old copy does not answer: {}", short(&result))),
        Synth::Refused(why) => return Err(format!("get_screen_size was refused: {why}")),
    }
    let old_endpoint = held.endpoint().to_string();

    // The new version arrives while that session is open.
    match next.cua_ensure().await {
        InstallState::Ready { version } => println!("the new copy is installed: {version}"),
        other => return Err(format!("the new copy was not installed: {other:?}")),
    }
    if !pinned.is_file() {
        return Err("the new copy is not in place".into());
    }
    if !old_bin.is_file() {
        return Err("the old copy was removed while a session ran on it".into());
    }
    match held.synth("get_screen_size", json!({})).await? {
        Synth::Answer(result) if !is_error(&result) => println!("the open session goes on, on the old copy: ok"),
        Synth::Answer(result) => return Err(format!("the open session broke when the new copy was installed: {}", short(&result))),
        Synth::Refused(why) => return Err(format!("get_screen_size was refused: {why}")),
    }
    if !connect_core::cua::endpoint_ready(&old_endpoint) {
        return Err("the daemon of the open session was stopped by the install".into());
    }
    if !held.held.outdated() {
        return Err("the open session does not know a new version is in place".into());
    }
    held.close().await;

    // Nobody holds the old daemon now: the next session gets the new copy.
    let mut fresh = Session::open(&shared).await?;
    if fresh.endpoint() == old_endpoint {
        return Err("the next session is still on the daemon of the old copy".into());
    }
    if std::path::Path::new(&shared.cua_bin_now()) != pinned {
        return Err(format!("the app would still run {}", shared.cua_bin_now()));
    }
    match fresh.synth("get_screen_size", json!({})).await? {
        Synth::Answer(result) if !is_error(&result) => println!("the next session runs on the new copy: ok"),
        Synth::Answer(result) => return Err(format!("the new copy does not answer: {}", short(&result))),
        Synth::Refused(why) => return Err(format!("get_screen_size was refused: {why}")),
    }
    // The old copy goes once its daemon has stopped (on Windows a program
    // that still runs cannot be deleted, so give it a moment).
    let mut gone = false;
    for _ in 0..30 {
        if !old_dir.exists() {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        // What a later check of the install does: remove what is left over.
        let _ = next.cua_ensure().await;
    }
    fresh.close().await;
    next.cua_stop_daemon().await;
    if !gone {
        return Err(format!("the old copy is still on disk: {}", old_dir.display()));
    }
    println!("the old copy was removed");
    // The core of the rest of the test sees the same installed copy again.
    match core.cua_ensure().await {
        InstallState::Ready { .. } => Ok(()),
        other => Err(format!("after the update the first core does not find the driver: {other:?}")),
    }
}

/// Run one of the driver's own subcommands with our environment.
async fn driver(core: &Core, args: &[&str]) -> Result<String, String> {
    let out = core
        .cua_driver_command()
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| format!("run the driver ({}): {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!(
            "the driver's `{}` failed: {}{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// macOS, by hand: the experimental preview window is a start flag of our
/// daemon. The window shows on the screen for a moment.
async fn preview(core: &Core) -> Result<(), String> {
    if !core.look_support().preview_window {
        println!("the preview window does not exist on this system: nothing to start");
        return Ok(());
    }
    step("the experimental preview window (shows on the screen for a moment)");
    core.mutate_settings(|s| s.computer.look.preview_window = true);
    let shared = core.shared();
    let mut session = Session::open(&shared).await?;
    let look = shared.cua_daemon_look().await.unwrap_or_default();
    println!("daemon look: {look:?}");
    let listed = session.synth("get_config", json!({})).await;
    session.close().await;
    core.mutate_settings(|s| s.computer.look.preview_window = false);
    core.cua_stop_daemon().await;
    if !look.iter().any(|a| a == "--experimental-pip") {
        return Err("the daemon was not started with the preview window".into());
    }
    match listed? {
        Synth::Answer(result) => println!("the daemon runs with the preview window: {}", short(&result)),
        Synth::Refused(why) => return Err(format!("get_config was refused: {why}")),
    }
    Ok(())
}

/// The files of a driver the user may run themselves that our daemon could
/// reach if the app did not keep it away: the driver's settings file and
/// its cursor-theme folder. Read before and after; they must be the same.
struct UsersDriver {
    config: PathBuf,
    config_before: Option<Vec<u8>>,
    themes: Option<PathBuf>,
    themes_before: Option<Vec<String>>,
}

impl UsersDriver {
    fn read() -> UsersDriver {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_default();
        let config = home.join(".cua-driver").join("config.json");
        let themes = if cfg!(windows) {
            std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Cua Driver").join("cursor-themes"))
        } else if cfg!(target_os = "macos") {
            Some(home.join("Library").join("Application Support").join("Cua Driver").join("cursor-themes"))
        } else {
            Some(
                std::env::var_os("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".local").join("share"))
                    .join("cua-driver")
                    .join("cursor-themes"),
            )
        };
        UsersDriver {
            config_before: std::fs::read(&config).ok(),
            themes_before: themes.as_deref().and_then(Self::list),
            config,
            themes,
        }
    }

    fn list(dir: &std::path::Path) -> Option<Vec<String>> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| format!("{} {}", e.file_name().to_string_lossy(), e.metadata().map(|m| m.len()).unwrap_or(0)))
            .collect();
        names.sort();
        Some(names)
    }

    fn unchanged(&self) -> Result<(), String> {
        if std::fs::read(&self.config).ok() != self.config_before {
            return Err(format!("{} changed during the self-test", self.config.display()));
        }
        if self.themes.as_deref().and_then(Self::list) != self.themes_before {
            return Err(format!(
                "the user's own cursor-theme folder changed during the self-test: {:?}",
                self.themes
            ));
        }
        println!(
            "{}: {}; the user's cursor-theme folder: {}",
            self.config.display(),
            if self.config_before.is_some() { "unchanged" } else { "still absent" },
            if self.themes_before.is_some() { "unchanged" } else { "still absent" },
        );
        Ok(())
    }
}

async fn send(stdin: &mut ChildStdin, message: &Value) -> Result<(), String> {
    let mut line = serde_json::to_vec(message).map_err(|e| e.to_string())?;
    line.push(b'\n');
    stdin.write_all(&line).await.map_err(|e| format!("write to the proxy: {e}"))?;
    stdin.flush().await.map_err(|e| format!("write to the proxy: {e}"))
}

/// One JSON-RPC request; returns its `result`. Notifications and answers
/// to other ids that arrive in between are skipped.
async fn call(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    send(stdin, &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).await?;
    answer(stdout, id, method).await
}

/// Wait for the answer to request `id`; returns its `result`.
async fn answer(stdout: &mut BufReader<ChildStdout>, id: u64, method: &str) -> Result<Value, String> {
    let wait = async {
        loop {
            let mut line = String::new();
            let n = stdout
                .read_line(&mut line)
                .await
                .map_err(|e| format!("{method}: read from the proxy: {e}"))?;
            if n == 0 {
                return Err(format!("{method}: the proxy closed its output (transport closed)"));
            }
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(format!("{method}: the driver answered with an error: {error}"));
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    };
    tokio::time::timeout(ANSWER_TIMEOUT, wait)
        .await
        .map_err(|_| format!("{method}: no answer in {} s", ANSWER_TIMEOUT.as_secs()))?
}

/// The start of a result, for a log line (never a whole screenshot).
fn short(value: &Value) -> String {
    let text = value
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string());
    let mut out: String = text.chars().take(160).collect();
    if text.chars().count() > 160 {
        out.push_str("...");
    }
    out.replace('\n', " ")
}

// ---------------------------------------------------------------------
// `connectd selftest apps`: the owner's apps, through the same gate the
// synth's calls go through, against the REAL backend of this system.
//
// Always (nothing on the computer is touched):
//   1. what this system offers: never a Mac app outside macOS
//   2. the gate refuses what is not offered, what is not a document, and
//      a file outside the shared folder
//
// With `--touch` (the Windows CI job passes it; by hand it shows one
// notification, puts a test text on the clipboard and puts back what was
// there, and opens one small text file):
//   3. a notification is shown
//   4. the clipboard round trip: write a text with non-ASCII letters,
//      read the same text back
//   5. open: a text file inside a shared folder is handed to the system;
//      on Windows the Start menu can be listed and an app in it resolved
// ---------------------------------------------------------------------

struct AllowOnce;
impl Approver for AllowOnce {
    fn ask(&self, _req: ApprovalRequest) -> ApprovalFuture {
        Box::pin(async { Approval::once() })
    }
}

async fn apps(args: &[String]) -> Result<(), String> {
    use connect_core::apps;
    use connect_core::settings::AppsMode;

    let touch = args.iter().any(|a| a == "--touch");
    if let Some(other) = args.iter().find(|a| a.as_str() != "--touch") {
        return Err(format!("unknown option {other}"));
    }
    let root = std::env::temp_dir().join(format!("connect apps selftest é {}", std::process::id()));
    let shared_dir = root.join("shared");
    let outside_dir = root.join("outside");
    std::fs::create_dir_all(&shared_dir).map_err(|e| format!("create {}: {e}", shared_dir.display()))?;
    std::fs::create_dir_all(&outside_dir).map_err(|e| format!("create {}: {e}", outside_dir.display()))?;
    let note = shared_dir.join("snth-connect-selftest.txt");
    std::fs::write(&note, "SNTH Connect self-test. You can close this.\r\n").map_err(|e| e.to_string())?;
    let script = shared_dir.join(if cfg!(windows) { "run.bat" } else { "run.command" });
    std::fs::write(&script, "echo should never run\r\n").map_err(|e| e.to_string())?;
    let secret = outside_dir.join("outside.txt");
    std::fs::write(&secret, "outside").map_err(|e| e.to_string())?;

    let backend = apps::system_backend(None);
    let core = Core::new(CoreConfig {
        settings_path: None,
        journal_path: None,
        approver: Arc::new(AllowOnce),
        cookies: Arc::new(MultiReader {
            chromium: vec![],
            firefox: vec![],
            keys: Arc::new(cookies::fixtures::NoKey),
        }),
        apps: backend,
        secret: Arc::new(MemSecretStore::new()),
        hub: Arc::new(connect_core::UreqHubClient),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        cua_bin: "cua-driver".into(),
        cua_private_root: None,
        cua_attach: CuaAttach::User,
    });
    core.mutate_settings(|s| {
        s.apps.enabled = true;
        s.apps.mode = AppsMode::Auto;
    });
    core.add_root(shared_dir.clone(), false);
    let shared = core.shared();
    let call = |method: &'static str, params: Value| {
        let shared = shared.clone();
        async move { apps::handle(&shared, method, &params).await }
    };

    let result = async {
        step("what this system offers");
        let offered: Vec<&str> = core.apps_views().iter().map(|v| v.id).collect();
        println!("offered: {offered:?}");
        if !cfg!(target_os = "macos") {
            if let Some(mac_app) = offered.iter().find(|id| apps::MAC_ONLY.contains(id)) {
                return Err(format!("a Mac app is offered on this system: {mac_app}"));
            }
        }
        if cfg!(windows) && offered != ["notify", "clipboard", "open"] {
            return Err(format!("Windows must offer notify, clipboard and open, got {offered:?}"));
        }

        step("the gate refuses what does not belong here");
        if !cfg!(target_os = "macos") {
            let e = call("apps.calendar.list", json!({"start": "2026-10-13", "end": "2026-10-14"}))
                .await
                .err()
                .unwrap_or_default();
            if !e.starts_with("unavailable: ") {
                return Err(format!("Calendar outside macOS must be unavailable, got {e:?}"));
            }
            println!("calendar.list: {e}");
        }
        if offered.contains(&"open") {
            for (what, path, want) in [
                ("a script", &script, "denied: files of type"),
                ("a file outside the shared folder", &secret, "denied: "),
            ] {
                let e = call("apps.open.file", json!({"path": path.to_string_lossy()}))
                    .await
                    .err()
                    .unwrap_or_default();
                if !e.starts_with(want) {
                    return Err(format!("opening {what} must be refused, got {e:?}"));
                }
                println!("open.file ({what}): refused");
            }
            let e = call("apps.open.url", json!({"url": "file:///C:/Windows/win.ini"})).await.err().unwrap_or_default();
            if !e.starts_with("denied: ") {
                return Err(format!("a file: address must be refused, got {e:?}"));
            }
        }

        if !touch {
            println!("skipped: notification, clipboard, open (pass --touch to run them on this computer)");
            return Ok(());
        }

        if offered.contains(&"notify") {
            step("show a notification");
            call("apps.notify.show", json!({"title": "Self-test", "body": "SNTH Connect can show notifications."}))
                .await
                .map_err(|e| format!("notify.show: {e}"))?;
            println!("notify.show: shown");
        }

        if offered.contains(&"clipboard") {
            step("clipboard round trip");
            let before = call("apps.clipboard.read", json!({})).await.map_err(|e| format!("clipboard.read: {e}"))?;
            let marker = format!("SNTH Connect self-test \u{43f}\u{440}\u{438}\u{432}\u{435}\u{442} \u{2713} {}", std::process::id());
            call("apps.clipboard.write", json!({"text": marker})).await.map_err(|e| format!("clipboard.write: {e}"))?;
            let after = call("apps.clipboard.read", json!({})).await.map_err(|e| format!("clipboard.read: {e}"))?;
            let got = after.get("text").and_then(Value::as_str).unwrap_or("");
            // Put back what was there before judging.
            if let Some(old) = before.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                let _ = call("apps.clipboard.write", json!({"text": old})).await;
            }
            if got != marker {
                return Err(format!("the clipboard returned {got:?}, expected {marker:?}"));
            }
            println!("clipboard: wrote and read back {} characters, with non-ASCII letters", marker.chars().count());
        }

        if offered.contains(&"open") {
            step("open a text file inside the shared folder");
            let opened = call("apps.open.file", json!({"path": note.to_string_lossy()}))
                .await
                .map_err(|e| format!("open.file: {e}"))?;
            println!("open.file: {opened}");
            #[cfg(windows)]
            {
                step("the Start menu can be listed and an app in it resolved");
                let listed = apps::win::start_apps().await.map_err(|e| format!("Get-StartApps: {e}"))?;
                println!("Start menu: {} apps", listed.len());
                if listed.is_empty() {
                    return Err("the Start menu list is empty".into());
                }
                let known = ["Notepad", "Paint", "Calculator", "Command Prompt", "Windows PowerShell"];
                let present = known
                    .iter()
                    .copied()
                    .find(|k| listed.iter().any(|(name, _)| name.as_str() == *k));
                match present {
                    Some(name) => {
                        let opened = apps::handle(&shared, "apps.open.app", &json!({"name": name}))
                            .await
                            .map_err(|e| format!("open.app {name}: {e}"))?;
                        println!("open.app: {opened}");
                    }
                    None => println!("open.app: none of {known:?} is in this Start menu, not started"),
                }
                let e = apps::handle(&shared, "apps.open.app", &json!({"name": "No Such App 4f1c"}))
                    .await
                    .err()
                    .unwrap_or_default();
                if !e.starts_with("not found: ") {
                    return Err(format!("an app that does not exist must be not found, got {e:?}"));
                }
            }
        }
        Ok(())
    }
    .await;

    // One journal line per call, and none of them holds the test text.
    let journal = serde_json::to_string(&core.recent_journal()).unwrap_or_default();
    if journal.contains("self-test") || journal.contains("Self-test") {
        return Err("the journal holds text of a call".into());
    }
    let _ = std::fs::remove_dir_all(&root);
    result
}
