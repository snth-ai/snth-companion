//! `mcp` streams: computer use through Cua Driver.
//!
//! The stream is piped into `cua-driver mcp [--embedded] --socket <s>`,
//! attached to our private daemon (the product path) or, for read-only
//! testing, the user's running daemon. One stream = one proxy process =
//! one persistent MCP connection, so named Cua sessions live for the
//! stream's life.
//!
//! Two gates sit here, on top of Cua's own layers:
//!
//!  - at stream open (connect.md): `catalog` streams are served without
//!    asking; a working stream follows `computer.mode` (`ask` needs a
//!    grant for "task"; `away` passes only while the owner is idle;
//!    `auto` passes);
//!  - per `tools/call`: a catalog stream refuses every call; the tool
//!    denylist refuses; the owner's choice of apps holds (`cua_apps`:
//!    "every app except these" is decided here, call by call, and the two
//!    lists are answered by the app itself, cut down; "only these" is the
//!    driver's own bounded mode, and decided here once more); a
//!    foreground-delivery call
//!    follows `computer.foreground` (never / idle / ask / always); what
//!    the synth may never trigger is refused whatever the settings say
//!    (`cua_guard`: recording the screen, installing software, updating or
//!    reconfiguring the helper, replaying a recording), and the driver
//!    writes a file only into a folder shared for writing; the owner's
//!    look stays the owner's (`cua_look::gate`: the synth's calls that
//!    change the cursor are refused, and a screenshot is asked for within
//!    the owner's limit).
//!
//! A stream holds our daemon for as long as it lives (`CuaSession`), so a
//! new look or a new choice of apps, which need a new daemon, never cut a
//! task short. The synth keeps its stream open between tasks, though, so
//! "the next session" would never come: a call that arrives after a quiet
//! spell (`TASK_GAP`, the time after which the driver has ended every
//! named session anyway) on a daemon that is out of date is answered with
//! a labeled error and the stream is closed. The synth's next call opens a
//! new one, on a new daemon.

use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::Child;
use tokio::sync::Mutex;

use crate::core::Shared;
use crate::cua;
use crate::grant::ApprovalRequest;
use crate::journal::Entry;
use crate::presence;
use crate::proto::{read_line, reply_ok, reply_refusal, MuxStream, Refusal, StreamHeader};
use crate::settings::{ComputerMode, Foreground};
use crate::logln;

/// One JSON-RPC line. A screenshot result is a few MiB of base64.
const MAX_RPC_LINE: usize = 64 * 1024 * 1024;

/// A stream with no call for this long is between tasks: the driver ends
/// a named session after five minutes without one.
const TASK_GAP: Duration = Duration::from_secs(5 * 60);

/// What the synth is told when its stream is closed so that a change of
/// the owner's applies.
const REOPEN: &str = "unavailable: SNTH Connect is restarting its helper so that the owner's current settings for computer control apply; this connection is closed and nothing was done; call again, starting with a fresh list_windows";

/// Start the `cua-driver mcp` proxy for one session against a daemon's
/// endpoint. It speaks line-delimited JSON-RPC on its stdin / stdout and
/// ends when its stdin closes. The app's mcp streams and the self-test
/// both start it here.
pub fn spawn_proxy(shared: &Shared, socket: &str, embedded: bool) -> std::io::Result<Child> {
    // The proxy of the same version as the daemon it talks to.
    let argv = cua::mcp_argv(&shared.cua_proxy_bin(), socket, embedded);
    let mut cmd = crate::proc::command(&argv[0]);
    cmd.args(&argv[1..]);
    shared.cua_env.apply(&mut cmd);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
}

pub async fn handle(mut s: MuxStream, hdr: StreamHeader, shared: &Arc<Shared>) {
    if hdr.server != "desktop" {
        // v1 reserves "desktop"; the P0a spike app served "cua", so accept
        // that alias too for back-compat with the existing Go peer.
        if hdr.server != "cua" {
            let why = Refusal::Denied(format!("unknown mcp server {:?}", hdr.server));
            reply_refusal(&mut s, &why).await;
            return;
        }
    }
    let is_catalog = hdr.purpose == crate::proto::PURPOSE_CATALOG;

    // Open gate (working streams only; a catalog stream is served freely).
    if !is_catalog {
        if let Err(why) = open_gate(shared, &hdr).await {
            shared
                .journal()
                .record(Entry::new("computer", "mcp.open", "desktop", why.to_string()));
            logln!("mcp: refused: {why}");
            reply_refusal(&mut s, &why).await;
            return;
        }
    }

    // Held until this function returns, however it ends.
    let session = match shared.cua_session().await {
        Ok(v) => v,
        Err(why) => {
            shared
                .journal()
                .record(Entry::new("computer", "mcp.open", "desktop", why.to_string()));
            logln!("mcp: refused: {why}");
            reply_refusal(&mut s, &why).await;
            return;
        }
    };

    let mut child = match spawn_proxy(shared, &session.socket, session.embedded) {
        Ok(c) => c,
        Err(e) => {
            let why = Refusal::Failed(format!("unavailable: spawn cua-driver mcp: {e}"));
            logln!("mcp: refused: {why}");
            reply_refusal(&mut s, &why).await;
            return;
        }
    };
    let mut child_in = child.stdin.take().expect("piped stdin");
    let child_out = child.stdout.take().expect("piped stdout");
    let child_err = child.stderr.take().expect("piped stderr");
    if reply_ok(&mut s).await.is_err() {
        return;
    }
    shared
        .journal()
        .record(Entry::new("computer", "mcp.open", "desktop", "ok"));
    logln!("mcp: session open (pid {:?}, catalog={is_catalog})", child.id());

    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let mut lines = BufReader::new(child_err).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            logln!("mcp cua stderr: {line}");
        }
    });

    let (sr, sw) = tokio::io::split(s);
    let mut sr = BufReader::with_capacity(64 * 1024, sr);
    let sw = Arc::new(Mutex::new(sw));

    let shared_for_gate = shared.clone();
    let session_ref = &session;
    let to_driver = async {
        let mut forwarded = 0u64;
        let mut refused = 0u64;
        // When this stream last carried a call.
        let mut last_call: Option<Instant> = None;
        loop {
            let line = match read_line(&mut sr, MAX_RPC_LINE).await {
                Ok(Some(l)) => l,
                Ok(None) => break,
                Err(e) => {
                    logln!("mcp: read from synth: {e}");
                    break;
                }
            };
            if !is_catalog {
                if let Some(id) = call_id(&line) {
                    // Between tasks, on a daemon the owner's settings have
                    // moved on from, and nobody else holds it: end here so
                    // the next call starts on a new one.
                    if between_tasks(last_call.map(|at| at.elapsed()), shared_for_gate.cua_sessions_open())
                        && session_ref.outdated()
                    {
                        logln!("mcp: the settings changed since this session began; closing it between tasks");
                        let mut b = serde_json::to_vec(&jsonrpc_error(id, REOPEN.into())).unwrap_or_default();
                        b.push(b'\n');
                        let mut w = sw.lock().await;
                        let _ = w.write_all(&b).await;
                        let _ = w.flush().await;
                        refused += 1;
                        break;
                    }
                    last_call = Some(Instant::now());
                }
            }
            match filter_scoped(&shared_for_gate, &session_ref.scope, is_catalog, &line).await {
                Gate::Forward(bytes) => {
                    forwarded += 1;
                    if child_in.write_all(&bytes).await.is_err()
                        || child_in.write_all(b"\n").await.is_err()
                        || child_in.flush().await.is_err()
                    {
                        break;
                    }
                }
                Gate::Answer(answer) => {
                    // The app's own answer (a list cut down to the apps
                    // the synth may use): the driver is not asked.
                    forwarded += 1;
                    let mut b = serde_json::to_vec(&answer).unwrap_or_default();
                    b.push(b'\n');
                    let mut w = sw.lock().await;
                    if w.write_all(&b).await.is_err() || w.flush().await.is_err() {
                        break;
                    }
                }
                Gate::Refuse(err) => {
                    refused += 1;
                    let mut b = serde_json::to_vec(&err).unwrap_or_default();
                    b.push(b'\n');
                    let mut w = sw.lock().await;
                    if w.write_all(&b).await.is_err() || w.flush().await.is_err() {
                        break;
                    }
                }
                Gate::Drop(why) => {
                    refused += 1;
                    logln!("mcp: dropped a message: {why}");
                }
            }
        }
        (forwarded, refused)
    };

    let from_driver = async {
        let mut out = BufReader::with_capacity(64 * 1024, child_out);
        loop {
            match read_line(&mut out, MAX_RPC_LINE).await {
                Ok(Some(mut line)) => {
                    line.push(b'\n');
                    let mut w = sw.lock().await;
                    if w.write_all(&line).await.is_err() || w.flush().await.is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    logln!("mcp: read from driver: {e}");
                    break;
                }
            }
        }
    };

    let (forwarded, refused) = tokio::select! {
        counts = to_driver => counts,
        _ = from_driver => (0, 0),
    };
    let _ = sw.lock().await.shutdown().await;
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    logln!("mcp: session closed (forwarded {forwarded}, refused {refused})");
    drop(session);
    if !is_catalog {
        shared.clear_task();
    }
    if shared.cua_sessions_open() == 0 {
        // Nobody needs the app's own reading of the apps any more.
        shared.cua_directory().stop().await;
    }
}

/// Is a stream between tasks, so that ending it cuts nothing short? It
/// has carried a call before, the last one was `TASK_GAP` ago or more
/// (`since_last_call`), and no other session holds the daemon (ending this
/// one would not free it otherwise).
fn between_tasks(since_last_call: Option<Duration>, sessions_open: usize) -> bool {
    since_last_call.is_some_and(|gap| gap >= TASK_GAP) && sessions_open <= 1
}

/// The id of a `tools/call`, when this line is one.
fn call_id(line: &[u8]) -> Option<Value> {
    let msg = serde_json::from_slice::<Value>(line).ok()?;
    cua::tool_name(&msg)?;
    Some(msg.get("id").cloned().unwrap_or(Value::Null))
}

/// The open gate for a working stream.
async fn open_gate(shared: &Arc<Shared>, hdr: &StreamHeader) -> Result<(), Refusal> {
    if shared.paused() {
        return Err(Refusal::Denied("computer use is paused by the owner".into()));
    }
    let s = shared.settings();
    if !s.computer.enabled {
        return Err(Refusal::Denied("computer use is turned off on this computer".into()));
    }
    match s.computer.mode {
        ComputerMode::Auto => Ok(()),
        ComputerMode::Away => {
            let idle = presence::read().idle_seconds;
            if cua::away_allows(ComputerMode::Away, s.computer.away_idle_minutes, idle) {
                Ok(())
            } else {
                Err(Refusal::Denied(format!(
                    "computer use is allowed only while you are away ({} min idle); you are active",
                    s.computer.away_idle_minutes
                )))
            }
        }
        ComputerMode::Ask => {
            if shared.grants().allows("computer", "task") {
                return Ok(());
            }
            let req = ApprovalRequest::new(
                "computer",
                "task",
                &hdr.reason,
                "let the synth use this computer for a task",
            );
            let appr = shared.ask_scoped("computer", "task", req).await;
            if appr.allow {
                Ok(())
            } else {
                Err(Refusal::Denied("the owner did not allow computer use for this task".into()))
            }
        }
    }
}

/// What the pipe does with one line from the synth.
pub enum Gate {
    /// Hand these bytes (one JSON-RPC message, no newline) to the driver.
    Forward(Vec<u8>),
    /// Do not hand it over; answer the synth with this JSON-RPC result,
    /// which the app made itself.
    Answer(Value),
    /// Do not hand it over; answer the synth with this JSON-RPC error.
    Refuse(Value),
    /// Not a message at all: say nothing.
    Drop(&'static str),
}

fn jsonrpc_error(id: Value, message: String) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32001, "message": message}})
}

/// Decide one line from the synth by the owner's settings as they are now.
/// A session decides by the choice of apps its daemon was started with
/// (`filter_scoped`); this is for a caller that holds no session.
///
/// Public for the self-test, which sends a synth's lines through the very
/// filter the app uses and hands the driver only what comes out.
pub async fn filter_line(shared: &Arc<Shared>, is_catalog: bool, line: &[u8]) -> Gate {
    let scope = crate::cua_apps::Scope::of(&shared.settings());
    filter_scoped(shared, &scope, is_catalog, line).await
}

/// Decide one line from the synth. Only `tools/call` is gated; everything
/// else (initialize, tools/list, ...) is forwarded, re-serialized so the
/// driver reads exactly what was checked. `scope`: which apps the synth
/// may use in this session.
pub async fn filter_scoped(
    shared: &Arc<Shared>,
    scope: &crate::cua_apps::Scope,
    is_catalog: bool,
    line: &[u8],
) -> Gate {
    let Ok(mut msg) = serde_json::from_slice::<Value>(line) else {
        return Gate::Drop("not valid JSON");
    };
    if !msg.is_object() {
        return Gate::Drop("not a JSON object");
    }
    let Some(tool) = cua::tool_name(&msg).map(str::to_string) else {
        // Not a tools/call: forward the canonical re-serialization.
        return match serde_json::to_vec(&msg) {
            Ok(b) => Gate::Forward(b),
            Err(_) => Gate::Drop("cannot re-serialize"),
        };
    };
    let tool = tool.as_str();
    let id = msg.get("id").cloned().unwrap_or(Value::Null);

    if is_catalog {
        shared
            .journal()
            .record(Entry::new("computer", "tools/call", tool, "denied: catalog stream"));
        return Gate::Refuse(jsonrpc_error(
            id,
            "denied: a catalog stream may not call tools".into(),
        ));
    }

    let s = shared.settings();

    // Never, whatever the owner's settings say: recording the screen,
    // installing software, updating or reconfiguring the helper, replaying
    // a recording (the list and the reasons: `cua_guard::NEVER`).
    if let Some(why) = crate::cua_guard::refusal(tool) {
        shared.journal().record(Entry::new(
            "computer",
            "tools/call",
            tool,
            "denied: never allowed for the synth",
        ));
        return Gate::Refuse(jsonrpc_error(id, why));
    }

    // Tool denylist.
    if s.computer.tool_denylist.iter().any(|t| t == tool) {
        shared
            .journal()
            .record(Entry::new("computer", "tools/call", tool, "denied: tool on the denylist"));
        return Gate::Refuse(jsonrpc_error(
            id,
            format!("denied: tool {tool:?} is on this computer's denylist"),
        ));
    }

    // The label the synth gave this task, for the owner's status line.
    // Kept in memory in the bounded form the driver itself shows; the
    // journal never gets it.
    shared.note_task(msg.pointer("/params/arguments/session").and_then(Value::as_str));

    // The owner's choice of apps: which app is this call about, and may
    // the synth use it? (Nothing is decided here for "every app".)
    match crate::cua_apps::decide(scope, tool, &msg, shared.cua_directory()).await {
        crate::cua_apps::Verdict::Pass => {}
        crate::cua_apps::Verdict::Refuse { tell, journal } => {
            shared.journal().record(Entry::new("computer", "tools/call", tool, journal));
            return Gate::Refuse(jsonrpc_error(id, tell));
        }
        crate::cua_apps::Verdict::Answer(result) => {
            shared.journal().record(Entry::new("computer", "tools/call", tool, "ok"));
            return Gate::Answer(json!({"jsonrpc": "2.0", "id": id, "result": result}));
        }
    }

    // Foreground-delivery gate.
    if cua::wants_foreground(&msg) {
        let idle = presence::read().idle_seconds;
        let allowed = match cua::foreground_decision(
            s.computer.foreground,
            s.computer.away_idle_minutes,
            idle,
        ) {
            Some(b) => b,
            None => {
                // Foreground = ask: consult the approver per call.
                let req = ApprovalRequest::new(
                    "computer",
                    format!("foreground:{tool}"),
                    "",
                    format!("let the synth bring a window to the foreground ({tool})"),
                );
                shared
                    .ask_scoped("computer", &format!("foreground:{tool}"), req)
                    .await
                    .allow
            }
        };
        if !allowed {
            let reason = match s.computer.foreground {
                Foreground::Never => "foreground delivery is turned off",
                Foreground::Idle => "foreground delivery is allowed only while you are away",
                _ => "the owner did not allow foreground delivery",
            };
            shared.journal().record(Entry::new(
                "computer",
                "tools/call",
                tool,
                format!("denied: {reason}"),
            ));
            return Gate::Refuse(jsonrpc_error(id, format!("denied: {reason}")));
        }
    }

    // A file the driver writes (a screenshot, a download) goes into a
    // folder the owner shared for writing, or nowhere.
    if let Err(why) = crate::cua_guard::confine_paths(&mut msg, &s.files).await {
        shared.journal().record(Entry::new(
            "computer",
            "tools/call",
            tool,
            "denied: outside the shared folders",
        ));
        return Gate::Refuse(jsonrpc_error(id, why));
    }

    // The owner's look: the synth does not change the cursor, and gets a
    // screenshot within the owner's limit.
    match crate::cua_look::gate(tool, &mut msg, &s.computer.look) {
        crate::cua_look::Gate::Refuse(why) => {
            shared.journal().record(Entry::new(
                "computer",
                "tools/call",
                tool,
                "denied: the look is the owner's to set",
            ));
            return Gate::Refuse(jsonrpc_error(id, format!("denied: {why}")));
        }
        // Forwarded with its arguments changed (a screenshot within the
        // owner's limit, a session without the synth's cursor theme).
        crate::cua_look::Gate::Changed(_) | crate::cua_look::Gate::Pass => {}
    }

    shared
        .journal()
        .record(Entry::new("computer", "tools/call", tool, "ok"));
    match serde_json::to_vec(&msg) {
        Ok(b) => Gate::Forward(b),
        Err(_) => Gate::Drop("cannot re-serialize"),
    }
}

#[cfg(test)]
mod tests {
    // The per-line decision is exercised through the pure predicates in
    // `cua` (cua::wants_foreground, tool_name, gates), through `cua_apps`
    // (the owner's choice of apps) and through the core's net/computer
    // policy tests. A full pipe test needs a live driver and is
    // `connectd selftest cua`.
    use super::*;

    #[test]
    fn a_stream_is_ended_for_a_change_only_between_tasks() {
        // The first call of a stream is never the end of it.
        assert!(!between_tasks(None, 1));
        // A task in progress: calls a few seconds or minutes apart.
        assert!(!between_tasks(Some(Duration::from_secs(3)), 1));
        assert!(!between_tasks(Some(TASK_GAP - Duration::from_secs(1)), 1));
        // Quiet for as long as the driver keeps a named session alive.
        assert!(between_tasks(Some(TASK_GAP), 1));
        assert!(between_tasks(Some(Duration::from_secs(3600)), 1));
        // Another session holds the daemon: ending this one frees nothing.
        assert!(!between_tasks(Some(Duration::from_secs(3600)), 2));
    }

    #[test]
    fn the_id_of_a_call_is_found_and_nothing_else_is_a_call() {
        let call = br#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"click","arguments":{}}}"#;
        assert_eq!(call_id(call), Some(Value::from(7)));
        assert_eq!(call_id(br#"{"jsonrpc":"2.0","id":8,"method":"tools/list"}"#), None);
        assert_eq!(call_id(b"not json"), None);
        // The synth is told to call again, in the label it knows.
        assert!(REOPEN.starts_with("unavailable: ") && REOPEN.contains("call again"));
    }
}
