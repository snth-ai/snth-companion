//! `mcp` streams: computer use through Cua Driver.
//!
//! The stream is piped into `cua-driver mcp --socket <daemon socket>`, a
//! stdio MCP proxy attached to the Cua Driver daemon that already runs on
//! this desktop. One stream = one proxy process = one persistent MCP
//! connection, so named Cua sessions live as long as the stream.
//!
//! This app never starts, stops or reconfigures the daemon in P0. It asks
//! `cua-driver status` where the socket is and refuses with a label when
//! no daemon runs.
//!
//! A thin JSON-RPC filter sits in the pipe (TZ 4.3). In P0 it does one
//! thing: with `--mcp-tool` given, a `tools/call` for any other tool is
//! answered here with a JSON-RPC error and never reaches the driver.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::proto::{read_line, reply_ok, reply_refusal, MuxStream, Refusal, StreamHeader};
use crate::{logln, Config};

/// One JSON-RPC line. A screenshot result is a few MiB of base64.
const MAX_RPC_LINE: usize = 64 * 1024 * 1024;
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

/// Ask the running daemon for its socket. `Err` carries the label the
/// synth sees.
pub async fn daemon_socket(cua_bin: &str) -> Result<String, Refusal> {
    let run = Command::new(cua_bin)
        .arg("status")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = match tokio::time::timeout(STATUS_TIMEOUT, run).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            return Err(Refusal::Failed(format!(
                "computer use unavailable: cannot run {cua_bin}: {e}"
            )));
        }
        Err(_) => {
            return Err(Refusal::Failed(
                "computer use unavailable: cua-driver status did not answer".into(),
            ));
        }
    };
    let text = String::from_utf8_lossy(&out.stdout);
    // Output shape (0.33.1):
    //   Cua Driver daemon is running
    //     socket: /Users/x/Library/Caches/cua-driver/cua-driver.sock
    //     pid: 17087
    if !text.contains("daemon is running") {
        return Err(Refusal::Failed(
            "computer use unavailable: the Cua Driver daemon is not running".into(),
        ));
    }
    for line in text.lines() {
        if let Some(path) = line.trim().strip_prefix("socket:") {
            let path = path.trim();
            if !path.is_empty() {
                return Ok(path.to_string());
            }
        }
    }
    Err(Refusal::Failed(
        "computer use unavailable: cua-driver status reported no socket".into(),
    ))
}

pub async fn handle(mut s: MuxStream, hdr: StreamHeader, cfg: &Config) {
    if hdr.server != "cua" {
        let why = Refusal::Denied(format!("unknown mcp server {:?}", hdr.server));
        reply_refusal(&mut s, &why).await;
        return;
    }
    if cfg.no_computer {
        let why = Refusal::Denied("computer use is turned off on this computer".into());
        reply_refusal(&mut s, &why).await;
        return;
    }
    let socket = match daemon_socket(&cfg.cua_bin).await {
        Ok(p) => p,
        Err(why) => {
            logln!("mcp cua: refused: {why}");
            reply_refusal(&mut s, &why).await;
            return;
        }
    };

    let mut child = match Command::new(&cfg.cua_bin)
        .args(["mcp", "--socket", &socket])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let why = Refusal::Failed(format!("computer use unavailable: spawn cua-driver mcp: {e}"));
            logln!("mcp cua: refused: {why}");
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
    logln!("mcp cua: session open (pid {:?})", child.id());

    // Driver diagnostics go to our log, never to the synth.
    tokio::spawn(async move {
        let mut lines = BufReader::new(child_err).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            logln!("mcp cua stderr: {line}");
        }
    });

    let (sr, sw) = tokio::io::split(s);
    // The read half of a split is not line-buffered any more; bytes the
    // header read already buffered are still served first.
    let mut sr = BufReader::with_capacity(64 * 1024, sr);
    // Both the driver's answers and the filter's refusals are written to
    // the stream, one whole line at a time under this lock.
    let sw = Arc::new(Mutex::new(sw));
    let allow = cfg.mcp_tools.as_deref();

    let to_driver = async {
        let mut calls = 0u64;
        let mut refused = 0u64;
        loop {
            let line = match read_line(&mut sr, MAX_RPC_LINE).await {
                Ok(Some(l)) => l,
                Ok(None) => break,
                Err(e) => {
                    logln!("mcp cua: read from synth: {e}");
                    break;
                }
            };
            let line = match filter(line, allow) {
                Verdict::Forward(line) => line,
                Verdict::Refuse(refusal) => {
                    refused += 1;
                    let mut w = sw.lock().await;
                    let mut b = serde_json::to_vec(&refusal).unwrap_or_default();
                    b.push(b'\n');
                    if w.write_all(&b).await.is_err() || w.flush().await.is_err() {
                        break;
                    }
                    continue;
                }
                Verdict::Drop(why) => {
                    refused += 1;
                    logln!("mcp cua: dropped a message from the synth: {why}");
                    continue;
                }
            };
            calls += 1;
            if child_in.write_all(&line).await.is_err()
                || child_in.write_all(b"\n").await.is_err()
                || child_in.flush().await.is_err()
            {
                break;
            }
        }
        (calls, refused)
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
                    logln!("mcp cua: read from driver: {e}");
                    break;
                }
            }
        }
    };

    // Whichever side ends first ends the session: the synth closing the
    // stream stops the proxy, the proxy dying closes the stream.
    let (calls, refused) = tokio::select! {
        counts = to_driver => counts,
        _ = from_driver => (0, 0),
    };
    let _ = sw.lock().await.shutdown().await;
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    logln!("mcp cua: session closed (forwarded {calls} messages, refused {refused})");
}

enum Verdict {
    /// Send these bytes (one line, no terminator) to the driver.
    Forward(Vec<u8>),
    /// Answer the synth with this JSON-RPC error; the driver sees nothing.
    Refuse(Value),
    /// Not answerable (no request id to reply to); logged, not forwarded.
    Drop(&'static str),
}

/// Decide what happens to one line from the synth.
///
/// Without an allowlist the line passes through untouched. With one, the
/// line is parsed and what reaches the driver is OUR re-serialization of
/// the parsed message, so the driver cannot read a different message than
/// the one that was checked (duplicate keys, odd escapes).
fn filter(line: Vec<u8>, allow: Option<&[String]>) -> Verdict {
    let Some(allow) = allow else {
        return Verdict::Forward(line);
    };
    let Ok(msg) = serde_json::from_slice::<Value>(&line) else {
        return Verdict::Drop("not valid JSON");
    };
    if !msg.is_object() {
        // A batch array could hide a call; MCP no longer uses batches.
        return Verdict::Drop("not a JSON object");
    }
    if msg.get("method").and_then(Value::as_str) == Some("tools/call") {
        let name = msg
            .get("params")
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if !allow.iter().any(|t| t == name) {
            let id = msg.get("id").cloned().unwrap_or(Value::Null);
            return Verdict::Refuse(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32001,
                    "message": format!(
                        "denied: tool {name:?} is not allowed by this computer's SNTH Connect settings"
                    ),
                },
            }));
        }
    }
    match serde_json::to_vec(&msg) {
        Ok(b) => Verdict::Forward(b),
        Err(_) => Verdict::Drop("cannot re-serialize"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str) -> Vec<u8> {
        format!(
            r#"{{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{{"name":"{name}","arguments":{{}}}}}}"#
        )
        .into_bytes()
    }

    #[test]
    fn filter_allowlist() {
        let allow = vec!["list_windows".to_string()];

        // No allowlist: bytes pass through untouched.
        let raw = call("click");
        assert!(matches!(filter(raw.clone(), None), Verdict::Forward(b) if b == raw));

        // Allowed tool and non-call methods pass.
        assert!(matches!(filter(call("list_windows"), Some(&allow)), Verdict::Forward(_)));
        let list = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#.to_vec();
        assert!(matches!(filter(list, Some(&allow)), Verdict::Forward(_)));

        // Anything else is answered here, with the request id and a label.
        match filter(call("click"), Some(&allow)) {
            Verdict::Refuse(v) => {
                assert_eq!(v["id"], 7);
                let msg = v["error"]["message"].as_str().unwrap();
                assert!(msg.starts_with("denied: tool \"click\""), "{msg}");
            }
            _ => panic!("click must be refused"),
        }

        // Duplicate keys: what is checked is what is forwarded. serde keeps
        // the last "name"; the forwarded bytes carry only that one.
        let dup = br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"click","name":"list_windows"}}"#.to_vec();
        match filter(dup, Some(&allow)) {
            Verdict::Forward(b) => {
                let text = String::from_utf8(b).unwrap();
                assert!(!text.contains("click"), "{text}");
            }
            _ => panic!("expected a canonical forward"),
        }

        // Batches and garbage never reach the driver while filtering.
        let batch = format!("[{}]", String::from_utf8(call("click")).unwrap()).into_bytes();
        assert!(matches!(filter(batch, Some(&allow)), Verdict::Drop(_)));
        assert!(matches!(filter(b"not json".to_vec(), Some(&allow)), Verdict::Drop(_)));
    }
}
