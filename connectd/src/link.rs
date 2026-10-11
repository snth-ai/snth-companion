//! One link: dial the synth, run the multiplexer, serve streams until the
//! link drops.
//!
//! Roles: this app is the yamux client (odd stream ids). It opens exactly
//! one stream, `control`, for hello and presence. Every other stream is
//! opened by the synth and dispatched by its header kind.

use std::future::poll_fn;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_util::compat::FuturesAsyncReadCompatExt;

use crate::proto::{
    read_line, reply_refusal, write_json_line, HeaderReply, MuxStream, Refusal, StreamHeader,
    MAX_HEADER_LINE,
};
use crate::ws_io::WsIo;
use crate::{files, logln, mcp, tcp, Config};

/// How often a WebSocket ping goes to the synth (TZ: keepalive 20 s).
pub const KEEPALIVE: Duration = Duration::from_secs(20);
/// Silence longer than this means the link is dead (3 keepalives; the
/// synth also pings every 20 s).
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const HELLO_TIMEOUT: Duration = Duration::from_secs(15);
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const PRESENCE_EVERY: Duration = Duration::from_secs(15);

/// Streams the synth may have open at once.
const MAX_STREAMS: usize = 128;
/// Receive budget across all streams of the link. Each stream starts at
/// 256 KiB and may be auto-tuned up; the part above the per-stream minimum
/// (128 x 256 KiB = 32 MiB) is the shared growth budget, so one stream
/// buffers at most 16.25 MiB when its local consumer stalls.
const MAX_CONNECTION_WINDOW: usize = 48 * 1024 * 1024;
/// Largest data frame we send.
const SPLIT_SEND: usize = 64 * 1024;
/// Largest WebSocket message we accept from the synth. The synth caps its
/// messages at 1 MiB.
const WS_MAX_MESSAGE: usize = 8 * 1024 * 1024;

pub enum LinkEnd {
    /// Never got a working link (dial failed, refused, no hello ack).
    Failed(String),
    /// The link was up (hello acknowledged) and then ended.
    Dropped(String),
    /// Shutdown was requested.
    Shutdown,
}

pub async fn run(cfg: Arc<Config>, shutdown: &mut tokio::sync::watch::Receiver<bool>) -> LinkEnd {
    let mut req = match cfg.url.as_str().into_client_request() {
        Ok(r) => r,
        Err(e) => return LinkEnd::Failed(format!("bad url: {e}")),
    };
    match HeaderValue::from_str(&format!("Bearer {}", cfg.token)) {
        Ok(v) => {
            req.headers_mut().insert("Authorization", v);
        }
        Err(_) => return LinkEnd::Failed("token contains characters not valid in a header".into()),
    }
    let ws_cfg = WebSocketConfig::default()
        .max_message_size(Some(WS_MAX_MESSAGE))
        .max_frame_size(Some(WS_MAX_MESSAGE));
    let dial = tokio_tungstenite::connect_async_with_config(req, Some(ws_cfg), true);
    let ws = match tokio::time::timeout(CONNECT_TIMEOUT, dial).await {
        Ok(Ok((ws, _resp))) => ws,
        Ok(Err(WsError::Http(resp))) => {
            return LinkEnd::Failed(format!("synth refused the connection: HTTP {}", resp.status()));
        }
        Ok(Err(e)) => return LinkEnd::Failed(format!("connect: {e}")),
        Err(_) => {
            return LinkEnd::Failed(format!("connect: no answer in {} s", CONNECT_TIMEOUT.as_secs()));
        }
    };

    let mut ycfg = yamux::Config::default();
    ycfg.set_max_num_streams(MAX_STREAMS);
    ycfg.set_max_connection_receive_window(Some(MAX_CONNECTION_WINDOW));
    ycfg.set_split_send_size(SPLIT_SEND);
    let io = WsIo::new(ws, KEEPALIVE, READ_TIMEOUT);
    let mut conn = yamux::Connection::new(io, ycfg, yamux::Mode::Client);

    // The control stream is the only one this side opens. Its SYN goes out
    // with the first bytes once the connection is polled below.
    let control = match poll_fn(|cx| conn.poll_new_outbound(cx)).await {
        Ok(s) => s,
        Err(e) => return LinkEnd::Failed(format!("open control stream: {e}")),
    };
    let established = Arc::new(AtomicBool::new(false));
    let mut control_task = tokio::spawn(control_loop(
        BufReader::new(control.compat()),
        cfg.clone(),
        established.clone(),
    ));

    let mut streams: JoinSet<()> = JoinSet::new();
    let reason: Option<String> = loop {
        tokio::select! {
            inbound = poll_fn(|cx| conn.poll_next_inbound(cx)) => match inbound {
                Some(Ok(stream)) => {
                    let cfg = cfg.clone();
                    streams.spawn(async move { serve_stream(stream, cfg).await });
                }
                Some(Err(e)) => break Some(format!("link error: {e}")),
                None => break Some("closed by the synth".into()),
            },
            res = &mut control_task => {
                break Some(match res {
                    Ok(why) => format!("control stream ended: {why}"),
                    Err(e) => format!("control task failed: {e}"),
                });
            }
            Some(res) = streams.join_next(), if !streams.is_empty() => {
                if let Err(e) = res {
                    if e.is_panic() {
                        logln!("a stream handler panicked: {e}");
                    }
                }
            }
            _ = shutdown.changed() => break None,
        }
    };

    control_task.abort();
    streams.abort_all();
    // Best effort close frame; the socket is dropped right after.
    let _ = tokio::time::timeout(Duration::from_secs(2), poll_fn(|cx| conn.poll_close(cx))).await;
    drop(conn);

    match reason {
        None => LinkEnd::Shutdown,
        Some(why) if established.load(Ordering::Acquire) => LinkEnd::Dropped(why),
        Some(why) => LinkEnd::Failed(why),
    }
}

/// Hello, then presence every `PRESENCE_EVERY`. Returns why it ended.
async fn control_loop(mut s: MuxStream, cfg: Arc<Config>, established: Arc<AtomicBool>) -> String {
    if let Err(e) = write_json_line(&mut s, &json!({"kind": "control"})).await {
        return format!("send header: {e}");
    }
    let reply = match tokio::time::timeout(HELLO_TIMEOUT, read_line(&mut s, MAX_HEADER_LINE)).await {
        Ok(Ok(Some(line))) => line,
        Ok(Ok(None)) => return "closed before the header reply".into(),
        Ok(Err(e)) => return format!("read header reply: {e}"),
        Err(_) => return "no header reply from the synth".into(),
    };
    match serde_json::from_slice::<HeaderReply>(&reply) {
        Ok(r) if r.ok => {}
        Ok(r) => return format!("synth refused the control stream: {}", r.error),
        Err(e) => return format!("bad header reply: {e}"),
    }

    let mut caps = capabilities(&cfg).await;
    let hello = json!({
        "type": "hello",
        "device_id": cfg.device_id,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "hostname": cfg.hostname,
        "app_version": env!("CARGO_PKG_VERSION"),
        "capabilities": caps,
    });
    if let Err(e) = write_json_line(&mut s, &hello).await {
        return format!("send hello: {e}");
    }

    // Reading and writing run as two futures that live for the whole link
    // and are raced ONCE. A read is therefore never cancelled halfway
    // through a line by a presence tick.
    let (rd, mut wr) = tokio::io::split(s);
    let mut rd = BufReader::new(rd);
    let hello_caps = caps.join(", ");

    let reader = async {
        loop {
            match read_line(&mut rd, MAX_HEADER_LINE).await {
                Ok(Some(line)) => {
                    let msg: serde_json::Value = serde_json::from_slice(&line).unwrap_or_default();
                    match msg.get("type").and_then(|t| t.as_str()) {
                        Some("hello_ack") => {
                            established.store(true, Ordering::Release);
                            logln!("linked as device {} (capabilities: {hello_caps})", cfg.device_id);
                        }
                        Some("error") => {
                            let text = msg.get("error").and_then(|e| e.as_str()).unwrap_or("error");
                            return format!("synth: {text}");
                        }
                        _ => {} // newer synth, older app: ignore what we do not know
                    }
                }
                Ok(None) => return "closed by the synth".to_string(),
                Err(e) => return format!("read: {e}"),
            }
        }
    };

    let writer = async {
        let mut tick = tokio::time::interval(PRESENCE_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tick.tick().await; // the first tick is immediate; hello already carried the state
        let mut ticks = 0u32;
        loop {
            tick.tick().await;
            ticks += 1;
            // The Cua daemon can come and go; look again once a minute.
            if ticks % 4 == 0 {
                caps = capabilities(&cfg).await;
            }
            let presence = json!({
                "type": "presence",
                "idle_seconds": idle_seconds(),
                "screen_locked": screen_locked(),
                "capabilities": caps,
            });
            if let Err(e) = write_json_line(&mut wr, &presence).await {
                return format!("send presence: {e}");
            }
        }
    };

    tokio::select! {
        why = reader => why,
        why = writer => why,
    }
}

/// What this computer currently offers the synth.
async fn capabilities(cfg: &Config) -> Vec<&'static str> {
    let mut caps = vec!["net"];
    if !cfg.roots.is_empty() {
        caps.push("files");
    }
    if !cfg.no_computer && mcp::daemon_socket(&cfg.cua_bin).await.is_ok() {
        caps.push("computer");
    }
    caps
}

/// Seconds since the last keyboard or pointer input.
/// TODO(P1): real value per OS (macOS CGEventSourceSecondsSinceLastEventType,
/// Windows GetLastInputInfo, X11 XScreenSaverQueryInfo). P0 reports 0,
/// which reads as "the user is active": the conservative answer.
fn idle_seconds() -> u64 {
    0
}

/// TODO(P1): real value per OS. P0 reports false.
fn screen_locked() -> bool {
    false
}

/// Read the header of a synth-opened stream and hand it to its handler.
async fn serve_stream(stream: yamux::Stream, cfg: Arc<Config>) {
    let mut s: MuxStream = BufReader::with_capacity(64 * 1024, stream.compat());
    let line = match tokio::time::timeout(HEADER_TIMEOUT, read_line(&mut s, MAX_HEADER_LINE)).await {
        Ok(Ok(Some(line))) => line,
        // No header: nothing to answer. Dropping the stream resets it.
        _ => return,
    };
    let hdr: StreamHeader = match serde_json::from_slice(&line) {
        Ok(h) => h,
        Err(e) => {
            reply_refusal(&mut s, &Refusal::Denied(format!("bad stream header: {e}"))).await;
            return;
        }
    };
    match hdr.kind.as_str() {
        "tcp" => tcp::handle(s, hdr, &cfg).await,
        "mcp" => mcp::handle(s, hdr, &cfg).await,
        "rpc" => files::handle(s, &cfg).await,
        other => {
            let why = Refusal::Denied(format!("unknown stream kind {other:?}"));
            logln!("stream refused: {why}");
            reply_refusal(&mut s, &why).await;
            let _ = s.shutdown().await;
        }
    }
}
