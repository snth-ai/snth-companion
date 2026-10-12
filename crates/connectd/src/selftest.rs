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
//!  5. stop the daemon and check its endpoint is gone
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
use connect_core::settings::ComputerMode;
use connect_core::{cookies, Core, CoreConfig, CuaAttach, InstallState, MemSecretStore, MultiReader};
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
        // The child half of the console check: says whether THIS process
        // was given a console window.
        Some("console-child") => {
            println!("console={}", connect_core::proc::has_console_window());
            0
        }
        _ => {
            eprintln!("usage: connectd selftest cua [--root <dir>] [--keep]");
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
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--root" => {
                root = args.get(i + 1).map(PathBuf::from);
                i += 1;
            }
            "--keep" => keep = true,
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

    let core = Core::new(CoreConfig {
        settings_path: None,
        journal_path: None,
        approver: Arc::new(DenyAll),
        cookies: Arc::new(MultiReader {
            chromium: vec![],
            firefox: vec![],
            keys: Arc::new(cookies::fixtures::NoKey),
        }),
        secret: Arc::new(MemSecretStore::new()),
        hub: Arc::new(connect_core::UreqHubClient),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        cua_bin: String::new(),
        cua_private_root: Some(root.join("cua")),
        cua_attach: CuaAttach::Private,
    });
    // Computer control on, no questions: the daemon gate must let us in.
    core.mutate_settings(|s| {
        s.computer.enabled = true;
        s.computer.mode = ComputerMode::Auto;
    });

    let result = exercise(&core).await;

    step("stop the daemon");
    core.cua_stop_daemon().await;
    if !keep {
        let _ = std::fs::remove_dir_all(&root);
    }
    result
}

async fn exercise(core: &Core) -> Result<(), String> {
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
