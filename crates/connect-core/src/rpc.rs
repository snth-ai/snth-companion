//! `rpc` streams: sequential request/response calls (connect.md).
//!
//! Methods: files.list / files.stat / files.read / files.write,
//! cookies.get, permission.request, device.info, apps.<app>.<action>
//! (every one of those goes through the one gate in apps/mod.rs), and
//! shell.exec (the one gate in shell.rs). One stream serves any
//! number of sequential requests; the synth opens one per call and the
//! app also serves many on one.

use std::sync::Arc;

use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::core::{self, Shared};
use crate::files;
use crate::grant::{until_unix, ApprovalRequest};
use crate::journal::Entry;
use crate::proto::{reply_ok, write_json_line};
use crate::{logln, wire};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

/// The longest request line this app reads: a 4 MiB `files.write` chunk
/// is ~5.6 MB of base64, plus the envelope. Up to 0.1.11 it was 1 MiB,
/// which dropped every push chunk the synth sent (bug #367). Told to the
/// synth in the hello (`capabilities.files.max_request_bytes`), so it can
/// size its chunks; a synth that does not read it sends 512 KiB chunks.
pub const MAX_REQUEST_LINE: usize = 6 * 1024 * 1024;

/// How much of an oversized line is read (and thrown away) to count it
/// and to let the synth finish writing, so it reads the error line
/// instead of a reset. Past this the stream is just closed.
const MAX_DRAIN: usize = 64 * 1024 * 1024;

/// One request line, or what was wrong with it.
enum Line {
    Request(Vec<u8>),
    /// The line was longer than MAX_REQUEST_LINE: its length (exact when
    /// `whole`, else at least this much).
    TooLarge { bytes: usize, whole: bool },
    /// Clean end of the stream before a line started.
    End,
}

/// Read one '\n'-terminated line of at most `max` bytes. A longer one is
/// read to its end (up to `drain`) and reported with its length, so the
/// caller can answer it instead of dropping the stream.
async fn read_request<R: AsyncBufRead + Unpin>(r: &mut R, max: usize, drain: usize) -> std::io::Result<Line> {
    let mut line = Vec::new();
    let mut seen = 0usize;
    let mut over = false;
    loop {
        let chunk = r.fill_buf().await?;
        if chunk.is_empty() {
            if seen == 0 {
                return Ok(Line::End);
            }
            if over {
                return Ok(Line::TooLarge { bytes: seen, whole: false });
            }
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "stream ended in the middle of a line"));
        }
        let (take, done) = match chunk.iter().position(|b| *b == b'\n') {
            Some(i) => (i + 1, true),
            None => (chunk.len(), false),
        };
        seen += take;
        if !over && seen > max + 1 {
            over = true;
            line = Vec::new();
        }
        if !over {
            line.extend_from_slice(&chunk[..take]);
        }
        r.consume(take);
        if done {
            if over {
                return Ok(Line::TooLarge { bytes: seen - 1, whole: true });
            }
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(Line::Request(line));
        }
        if over && seen >= drain {
            return Ok(Line::TooLarge { bytes: seen, whole: false });
        }
    }
}

#[derive(Deserialize)]
struct Request {
    #[serde(default)]
    id: Value,
    #[serde(default)]
    method: String,
    #[serde(default)]
    params: Value,
}

pub async fn handle<S>(mut s: S, shared: &Arc<Shared>)
where
    S: AsyncBufRead + AsyncWrite + Unpin,
{
    if reply_ok(&mut s).await.is_err() {
        return;
    }
    loop {
        let line = match read_request(&mut s, MAX_REQUEST_LINE, MAX_DRAIN).await {
            Ok(Line::Request(l)) => l,
            Ok(Line::End) => break,
            Ok(Line::TooLarge { bytes, whole }) => {
                // One answer, then the stream is closed: a line this long
                // is not a request this app serves.
                let at_least = if whole { "" } else { "at least " };
                logln!("rpc: a request line of {at_least}{bytes} bytes (max {MAX_REQUEST_LINE})");
                let why = format!("request_too_large: {at_least}{bytes} bytes, max {MAX_REQUEST_LINE}");
                let _ = write_json_line(&mut s, &json!({"id": Value::Null, "error": why})).await;
                break;
            }
            Err(e) => {
                logln!("rpc: read: {e}");
                return;
            }
        };
        let req: Request = match serde_json::from_slice(&line) {
            Ok(r) => r,
            Err(e) => {
                let _ = write_json_line(
                    &mut s,
                    &json!({"id": Value::Null, "error": format!("bad request: {e}")}),
                )
                .await;
                continue;
            }
        };
        // A command runs until it ends, unless the synth closes the
        // stream first: that is its cancel. The synth sends nothing else
        // while it waits, so the end of the stream is the only thing to
        // watch for; a next request (bytes) is left for the next turn.
        let result = if req.method == "shell.exec" {
            let closed = async {
                match s.fill_buf().await {
                    Ok(b) if !b.is_empty() => std::future::pending::<()>().await,
                    _ => {}
                }
            };
            crate::shell::exec(shared, &req.params, closed).await
        } else {
            dispatch(shared, &req).await
        };
        let reply = match result {
            Ok(result) => json!({"id": req.id, "result": result}),
            Err(why) => {
                logln!("rpc {}: {why}", req.method);
                json!({"id": req.id, "error": why})
            }
        };
        if write_json_line(&mut s, &reply).await.is_err() {
            return;
        }
    }
    let _ = s.shutdown().await;
}

async fn dispatch(shared: &Arc<Shared>, req: &Request) -> Result<Value, String> {
    match req.method.as_str() {
        "files.list" | "files.stat" | "files.read" | "files.write" => {
            files_method(shared, req).await
        }
        "cookies.get" => cookies_get(shared, req).await,
        m if m.starts_with("apps.") => crate::apps::handle(shared, m, &req.params).await,
        "shell.exec" => crate::shell::exec(shared, &req.params, std::future::pending::<()>()).await,
        "permission.request" => permission_request(shared, req).await,
        "device.info" => Ok(wire::device_info(
            &shared.device_id(),
            &shared.device_name(),
            &shared.app_version,
            &cua_version(shared).await,
        )),
        other => Err(format!("denied: unknown method {other:?}")),
    }
}

/// One call as a stream would make it, for tests of other modules.
#[cfg(test)]
pub(crate) async fn call_for_test(shared: &Arc<Shared>, method: &str, params: Value) -> Result<Value, String> {
    dispatch(shared, &Request { id: Value::Null, method: method.to_string(), params }).await
}

async fn cua_version(shared: &Shared) -> String {
    shared.cua_detect(false).await.version
        .unwrap_or_default()
}

async fn files_method(shared: &Arc<Shared>, req: &Request) -> Result<Value, String> {
    if shared.paused() {
        return Err("denied: files are paused by the owner".into());
    }
    let s = shared.settings();
    if !s.files.enabled {
        return Err("denied: files are turned off on this computer".into());
    }
    let path = req
        .params
        .get("path")
        .and_then(Value::as_str)
        .ok_or("bad request: params.path is required")?;

    match req.method.as_str() {
        "files.list" => {
            let real = files::resolve_read(path, &s.files.roots).await?;
            files::list(&real).await.map_err(|e| format!("files.list: {e}"))
        }
        "files.stat" => {
            let real = files::resolve_read(path, &s.files.roots).await?;
            files::stat(&real).await.map_err(|e| format!("files.stat: {e}"))
        }
        "files.read" => {
            let real = files::resolve_read(path, &s.files.roots).await?;
            let offset = req.params.get("offset").and_then(Value::as_u64).unwrap_or(0);
            let length = req.params.get("length").and_then(Value::as_u64).unwrap_or(0);
            let out = files::read(&real, offset, length)
                .await
                .map_err(|e| format!("files.read: {e}"))?;
            shared
                .journal()
                .record(Entry::new("files", "files.read", path.to_string(), "ok"));
            Ok(out)
        }
        "files.write" => {
            let abs = files::resolve_write(path, &s.files.roots).await?;
            let mode = req
                .params
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or("create");
            let offset = req.params.get("offset").and_then(Value::as_u64).unwrap_or(0);
            let data_b64 = req.params.get("data_b64").and_then(Value::as_str).unwrap_or("");
            let data = base64::engine::general_purpose::STANDARD
                .decode(data_b64)
                .map_err(|e| format!("files.write: bad base64: {e}"))?;

            // The careful preset asks the owner before any write. The
            // append chunks of a file the owner just allowed are part of
            // that same write, not new ones: a 50 MB push is one question,
            // not a hundred (`Transfers`).
            let continuing = mode == "append" && shared.transfers.continues(path, offset);
            if s.files.ask_on_write && !continuing && !shared.grant_allows("files", path) {
                let req_ask = ApprovalRequest::new(
                    "files",
                    path,
                    "",
                    format!("let the synth write to {path}"),
                );
                let appr = shared.ask_scoped("files", path, req_ask).await;
                if !appr.allow {
                    shared.journal().record(Entry::new(
                        "files",
                        "files.write",
                        path.to_string(),
                        "denied: owner",
                    ));
                    return Err(format!("denied: the owner did not allow writing to {path}"));
                }
            }

            let size = files::write(&abs, mode, offset, &data).await.map_err(|e| {
                // files::write returns a labeled string already (exists:, not found:, denied:)
                if e.starts_with("exists:") || e.starts_with("not found:") || e.starts_with("denied:") {
                    e
                } else {
                    format!("files.write: {e}")
                }
            })?;
            if mode != "append" || continuing {
                shared.transfers.note(path, size);
            }
            shared
                .journal()
                .record(Entry::new("files", "files.write", path.to_string(), "ok").with_bytes(data.len() as u64));
            Ok(json!({"size": size}))
        }
        _ => unreachable!(),
    }
}

/// The files being written right now, chunk by chunk: the path as the
/// synth names it and the size after the last chunk. An `append` at
/// exactly that size, soon after, continues the write the owner already
/// allowed (or that needed no ask). Memory only, a handful of entries.
#[derive(Default)]
pub(crate) struct Transfers {
    open: std::sync::Mutex<Vec<(String, u64, std::time::Instant)>>,
}

/// How long after its last chunk a write may be continued.
const TRANSFER_IDLE: std::time::Duration = std::time::Duration::from_secs(10 * 60);
const TRANSFERS_MAX: usize = 16;

impl Transfers {
    pub(crate) fn continues(&self, path: &str, offset: u64) -> bool {
        let mut open = self.open.lock().unwrap();
        open.retain(|(_, _, at)| at.elapsed() < TRANSFER_IDLE);
        open.iter().any(|(p, size, _)| p == path && *size == offset)
    }

    pub(crate) fn note(&self, path: &str, size: u64) {
        let mut open = self.open.lock().unwrap();
        open.retain(|(p, _, at)| p != path && at.elapsed() < TRANSFER_IDLE);
        if open.len() == TRANSFERS_MAX {
            open.remove(0);
        }
        open.push((path.to_string(), size, std::time::Instant::now()));
    }
}

async fn cookies_get(shared: &Arc<Shared>, req: &Request) -> Result<Value, String> {
    let started = std::time::Instant::now();
    if shared.paused() {
        return Err("denied: cookies are paused by the owner".into());
    }
    let s = shared.settings();
    if !s.cookies.enabled {
        return Err("denied: cookies are turned off on this computer".into());
    }
    let raw = req
        .params
        .get("domain")
        .and_then(Value::as_str)
        .ok_or("bad request: params.domain is required")?;
    let reason = req.params.get("reason").and_then(Value::as_str).unwrap_or("");
    let domain = core::registrable_domain(raw);
    // One site at a time. "com" or "co.uk" would cover every site under
    // it, so it is refused before the owner is asked anything.
    if !core::is_one_site(&domain) {
        return Err("bad request: params.domain must name one site".into());
    }
    let extension_first = shared.cookies_extension_first.load(std::sync::atomic::Ordering::Acquire);

    use crate::settings::CookiesMode;
    let need_ask = match s.cookies.mode {
        CookiesMode::Auto => false,
        CookiesMode::AskNew => !shared.grant_allows("cookies", &domain),
        CookiesMode::Ask => true,
    };
    if need_ask {
        let mut ask = ApprovalRequest::new(
            "cookies",
            &domain,
            reason,
            format!("hand the synth your browser cookies for {domain}"),
        );
        // When the answer leads to a second question, in the browser, the
        // window says so before the owner answers: the two read as one.
        if extension_first {
            if let Some((browser, false)) = shared.extension_for(&domain) {
                ask = ask.with_details(vec![(
                    "browser_step".to_string(),
                    crate::browser::browser_name(&browser).to_string(),
                )]);
            }
        }
        let appr = shared.ask_scoped("cookies", &domain, ask).await;
        if !appr.allow {
            shared
                .journal()
                .record(Entry::new("cookies", "cookies.get", domain.clone(), "denied: owner"));
            return Err(format!("denied: the owner did not allow cookies for {domain}"));
        }
    }

    // The owner allowed the site (now, earlier, or by "never ask"). Two
    // sources: a browser the owner linked through the SNTH Vault
    // extension, and the browsers' own files. Where the files of the
    // common browsers cannot be read (Chrome and Edge on Windows) the
    // extension goes first; elsewhere it is the fallback.
    let done = |r: crate::cookies::CookieRead| {
        // Journal: domain, count, browser. NEVER a value.
        shared.journal().record(Entry::new(
            "cookies",
            "cookies.get",
            format!("{domain} ({} from {})", r.cookies.len(), r.source),
            "ok",
        ));
        Ok(json!({"cookies": r.cookies, "source": r.source}))
    };
    // What is left of the answer window (the synth gives up at 60 s).
    let left = || {
        crate::grant::APPROVAL_TIMEOUT
            .saturating_sub(started.elapsed())
            .max(std::time::Duration::from_secs(3))
    };
    use crate::browser::ExtensionRead;
    let refused = |outcome: ExtensionRead| -> Option<String> {
        let why = match outcome {
            ExtensionRead::Denied(why) | ExtensionRead::NoAnswer(why) => format!("denied: {why}"),
            ExtensionRead::Failed(why) => {
                // Not the owner's decision: the other source may still serve.
                logln!("cookies.get: {why}");
                return None;
            }
            _ => return None,
        };
        shared.journal().record(Entry::new("cookies", "cookies.get", domain.clone(), why.clone()));
        Some(why)
    };

    if extension_first {
        match shared.cookies_via_extension(&domain, left(), need_ask).await {
            ExtensionRead::Cookies(r) if !r.cookies.is_empty() => return done(r),
            other => {
                if let Some(why) = refused(other) {
                    return Err(why);
                }
            }
        }
    }

    // Read on a blocking pool: rusqlite + file copy are blocking.
    let source = shared.cookies.clone();
    let dom = domain.clone();
    let read = tokio::task::spawn_blocking(move || source.get(&dom))
        .await
        .map_err(|e| format!("cookies.get: join: {e}"))?;
    let unavailable = match read {
        Ok(r) if !r.cookies.is_empty() => return done(r),
        Ok(_) | Err(crate::cookies::CookieError::NotFound) => None,
        Err(crate::cookies::CookieError::Unavailable(m)) => Some(m),
        Err(crate::cookies::CookieError::Failed(m)) => return Err(format!("cookies.get: {m}")),
    };

    if !extension_first {
        match shared.cookies_via_extension(&domain, left(), need_ask).await {
            ExtensionRead::Cookies(r) if !r.cookies.is_empty() => return done(r),
            other => {
                if let Some(why) = refused(other) {
                    return Err(why);
                }
            }
        }
    }

    match unavailable {
        // The files could not be read and no linked browser could help.
        Some(m) => Err(format!(
            "unavailable: {m}; the owner can link that browser in the SNTH Connect app (Logins) to share its sign-ins"
        )),
        None => Ok(json!({"cookies": [], "source": ""})),
    }
}

async fn permission_request(shared: &Arc<Shared>, req: &Request) -> Result<Value, String> {
    if shared.paused() {
        return Err("denied: paused by the owner".into());
    }
    let capability = req
        .params
        .get("capability")
        .and_then(Value::as_str)
        .ok_or("bad request: params.capability is required")?;
    let scope = req.params.get("scope").and_then(Value::as_str).unwrap_or("");
    let reason = req.params.get("reason").and_then(Value::as_str).unwrap_or("");
    // Nothing in the owner's apps can be allowed ahead of time. Each
    // change is asked about with its own facts on the screen (who a
    // message goes to and what it says), by the gate in apps/mod.rs; a
    // grant made here would be a way around that.
    if capability == crate::apps::CAP {
        return Err("denied: actions in the owner's apps are asked about one by one, when they happen".into());
    }
    // Nor can a command be allowed ahead of time: the owner sees each one
    // when it is about to run (or chose to be asked nothing).
    if capability == crate::shell::CAP {
        return Err("denied: commands are asked about one by one, when they run".into());
    }
    let ask = ApprovalRequest::new(
        capability,
        scope,
        reason,
        format!("grant {capability} for {scope}"),
    );
    let appr = shared.ask_scoped(capability, scope, ask).await;
    if appr.allow {
        Ok(json!({"granted": true, "until_unix": until_unix(appr)}))
    } else {
        // connect.md: a refusal is the error "denied: ...", not granted:false.
        Err("denied: the owner did not allow it".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grant::{Approval, Duration_};
    use crate::settings::Root;
    use crate::testutil;
    use tokio::io::{AsyncReadExt, BufReader};

    fn write_line(path: &str, mode: &str, offset: u64, data: &[u8]) -> Vec<u8> {
        let req = json!({
            "id": 1,
            "method": "files.write",
            "params": {"path": path, "mode": mode, "offset": offset,
                       "data_b64": base64::engine::general_purpose::STANDARD.encode(data)},
        });
        let mut line = serde_json::to_vec(&req).unwrap();
        line.push(b'\n');
        line
    }

    /// A temp folder shared for writing, and a core with files on.
    fn shared_folder(tag: &str, ask_on_write: bool, owner: Arc<testutil::Owner>) -> (crate::core::Core, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("connect-rpc-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        let core = testutil::core(owner);
        let root = dir.clone();
        core.mutate_settings(move |s| {
            s.files.enabled = true;
            s.files.ask_on_write = ask_on_write;
            s.files.roots = vec![Root { path: root, write: true }];
        });
        (core, dir)
    }

    /// Run `handle` over an in-memory stream: write `input` (on its own
    /// task, so a long line does not block on the pipe), return all the
    /// lines the app answered.
    async fn serve(core: &crate::core::Core, input: Vec<u8>) -> Vec<Value> {
        let (mut synth, app) = tokio::io::duplex(64 * 1024);
        let shared = core.shared();
        let server = tokio::spawn(async move { handle(BufReader::new(app), &shared).await });
        let (mut rd, mut wr) = tokio::io::split(&mut synth);
        let writer = async move {
            let _ = wr.write_all(&input).await;
            let _ = wr.shutdown().await;
        };
        let reader = async move {
            let mut out = Vec::new();
            let _ = rd.read_to_end(&mut out).await;
            out
        };
        let (_, out) = tokio::join!(writer, reader);
        server.await.unwrap();
        out.split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_slice(l).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn a_4_mib_chunk_line_is_accepted() {
        let (core, dir) = shared_folder("4mib", false, testutil::Owner::new(Approval::deny()));
        let path = dir.join("clip.mp4").to_string_lossy().to_string();
        let data: Vec<u8> = (0..files::WRITE_MAX).map(|i| (i % 251) as u8).collect();
        let line = write_line(&path, "create", 0, &data);
        assert!(line.len() > 5 * 1024 * 1024, "a 4 MiB chunk is a ~5.6 MB line");
        assert!(line.len() <= MAX_REQUEST_LINE);
        let answers = serve(&core, line).await;
        assert_eq!(answers.len(), 2, "{answers:?}");
        assert_eq!(answers[0]["ok"], true);
        assert_eq!(answers[1]["result"]["size"], files::WRITE_MAX as u64);
        assert_eq!(std::fs::read(&path).unwrap(), data);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn an_oversized_line_gets_one_error_line() {
        let (core, dir) = shared_folder("big", false, testutil::Owner::new(Approval::deny()));
        let mut line = vec![b'x'; MAX_REQUEST_LINE + 1000];
        line.push(b'\n');
        // A request after it is never served: the stream is closed.
        line.extend_from_slice(b"{\"id\":2,\"method\":\"device.info\",\"params\":{}}\n");
        let answers = serve(&core, line).await;
        assert_eq!(answers.len(), 2, "{answers:?}");
        assert_eq!(answers[0]["ok"], true);
        assert_eq!(answers[1]["id"], Value::Null);
        assert_eq!(
            answers[1]["error"],
            format!("request_too_large: {} bytes, max {MAX_REQUEST_LINE}", MAX_REQUEST_LINE + 1000)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A command over a real stream: answered while the synth keeps the
    /// stream open; killed when the synth closes its side.
    #[tokio::test]
    async fn shell_exec_runs_while_the_stream_is_open_and_stops_when_it_closes() {
        use tokio::io::AsyncBufReadExt;
        let core = testutil::core(testutil::Owner::new(Approval::deny()));
        core.mutate_settings(|s| {
            s.shell.enabled = true;
            s.shell.mode = crate::settings::ShellMode::Auto;
        });
        let sleep = if crate::shell::os_shell().powershell { "Start-Sleep -Milliseconds 300; 'done'" } else { "sleep 0.3; echo done" };
        let line = |command: &str| {
            let mut l = serde_json::to_vec(&json!({"id": 5, "method": "shell.exec", "params": {"command": command}})).unwrap();
            l.push(b'\n');
            l
        };

        // Open: the answer comes when the command is done.
        let (synth, app) = tokio::io::duplex(64 * 1024);
        let shared = core.shared();
        let server = tokio::spawn(async move { handle(BufReader::new(app), &shared).await });
        let (rd, mut wr) = tokio::io::split(synth);
        wr.write_all(&line(sleep)).await.unwrap();
        let mut lines = BufReader::new(rd).lines();
        assert!(lines.next_line().await.unwrap().unwrap().contains("\"ok\":true"));
        let answer: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(answer["id"], 5);
        assert_eq!(answer["result"]["exit_code"], 0, "{answer}");
        wr.shutdown().await.unwrap();
        server.await.unwrap();

        // Closed right after the request: the command is stopped.
        let long = if crate::shell::os_shell().powershell { "Start-Sleep -Seconds 30" } else { "sleep 30" };
        let started = std::time::Instant::now();
        let answers = serve(&core, line(long)).await;
        assert!(started.elapsed() < std::time::Duration::from_secs(15));
        assert_eq!(answers[1]["result"]["exit_code"], Value::Null);
        let last = core.recent_journal().pop().unwrap();
        assert_eq!(last.outcome, "cancelled: the synth went away");
    }

    #[tokio::test]
    async fn the_line_reader_reports_lines_beyond_the_drain_cap() {
        // Longer than the reader's buffer, so the end is not seen at once.
        let mut data = vec![b'y'; 30_000];
        data.push(b'\n');
        let mut r = BufReader::with_capacity(4096, &data[..]);
        match read_request(&mut r, 100, 20_000).await.unwrap() {
            Line::TooLarge { bytes, whole } => assert!(!whole && bytes >= 20_000 && bytes < 30_000, "{bytes} {whole}"),
            _ => panic!("not too large"),
        }
        let mut ok = BufReader::new(&b"abc\r\n"[..]);
        assert!(matches!(read_request(&mut ok, 100, 200).await.unwrap(), Line::Request(l) if l == b"abc"));
        let mut end = BufReader::new(&b""[..]);
        assert!(matches!(read_request(&mut end, 100, 200).await.unwrap(), Line::End));
    }

    #[test]
    fn the_hello_tells_the_synth_the_line_limit() {
        let core = testutil::core(testutil::Owner::new(Approval::deny()));
        let caps = wire::capabilities_value(&core.settings(), "", "ok", core.shared().apps.backend().as_ref());
        assert_eq!(caps["files"]["max_request_bytes"], MAX_REQUEST_LINE as u64);
    }

    /// The careful preset asks before a write: once per file, not once per
    /// chunk. The appends that continue an allowed write pass; an append
    /// that continues nothing asks.
    #[tokio::test]
    async fn an_allowed_write_is_one_question_for_all_its_chunks() {
        let owner = testutil::Owner::new(Approval::deny());
        owner.next(Approval { allow: true, duration: Duration_::Once });
        let (core, dir) = shared_folder("ask", true, owner.clone());
        let path = dir.join("clip.mp4").to_string_lossy().to_string();
        let shared = core.shared();
        let chunk = vec![7u8; 1000];
        let write = |mode: &str, offset: u64| {
            let shared = shared.clone();
            let params = json!({"path": path, "mode": mode, "offset": offset,
                                "data_b64": base64::engine::general_purpose::STANDARD.encode(&chunk)});
            async move { call_for_test(&shared, "files.write", params).await }
        };
        write("create", 0).await.unwrap();
        write("append", 1000).await.unwrap();
        write("append", 2000).await.unwrap();
        assert_eq!(owner.asks(), 1);
        // Not at the end of what was written: not a continuation.
        assert!(write("append", 1000).await.unwrap_err().starts_with("denied: "));
        assert_eq!(owner.asks(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
