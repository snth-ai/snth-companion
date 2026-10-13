//! One outbound link to the synth, and the reconnect loop around it.
//!
//! Roles (connect.md): this app is the yamux CLIENT (odd stream ids). It
//! opens exactly one stream, `control`, for hello / presence / settings /
//! unpair. Every other stream is opened by the synth and dispatched by
//! its header kind (tcp / mcp / rpc).

use std::future::poll_fn;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_util::compat::FuturesAsyncReadCompatExt;

use crate::core::{LinkState, Shared};
use crate::proto::{
    read_line, reply_refusal, write_json_line, HeaderReply, MuxStream, Refusal, StreamHeader,
    MAX_HEADER_LINE,
};
use crate::ws_io::WsIo;
use crate::wire;
use crate::{logln, mcp, rpc, tcp};

const KEEPALIVE: Duration = Duration::from_secs(20);
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const HELLO_TIMEOUT: Duration = Duration::from_secs(15);
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const PRESENCE_EVERY: Duration = Duration::from_secs(15);

/// One control line from the synth (connect.md: 256 KiB).
const MAX_CONTROL_LINE: usize = 256 * 1024;

const MAX_STREAMS: usize = 128;
const MAX_CONNECTION_WINDOW: usize = 48 * 1024 * 1024;
const SPLIT_SEND: usize = 64 * 1024;
const WS_MAX_MESSAGE: usize = 8 * 1024 * 1024;

const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

enum End {
    Shutdown,
    Dropped(String),
    Failed(String),
}

/// The reconnect loop. Resolves the token and the synth url each pass;
/// without them it idles until paired or shutdown.
pub async fn run_forever(
    shared: Arc<Shared>,
    ws_url_override: Option<String>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut backoff = BACKOFF_MIN;
    loop {
        if *shutdown.borrow() || shared.stopping.load(Ordering::Acquire) {
            break;
        }
        let token = shared.token();
        let url = ws_url_override.clone().or_else(|| shared.synth_ws_url());
        let (token, url) = match (token, url) {
            (Some(t), Some(u)) if !t.is_empty() => (t, u),
            _ => {
                shared.set_link_state(LinkState::Unpaired, "no pairing");
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                    _ = shutdown.changed() => {}
                }
                continue;
            }
        };
        shared.set_link_state(LinkState::Connecting, "dialing");
        match run_one(&shared, &url, &token, &mut shutdown).await {
            End::Shutdown => break,
            End::Dropped(why) => {
                logln!("link dropped: {why}");
                shared.set_link_state(LinkState::Reconnecting, &why);
                backoff = BACKOFF_MIN;
            }
            End::Failed(why) => {
                logln!("link failed: {why}");
                shared.set_link_state(LinkState::Reconnecting, &why);
            }
        }
        if shared.stopping.load(Ordering::Acquire) {
            break;
        }
        let wait = jitter(backoff);
        logln!("reconnecting in {:.1} s", wait.as_secs_f64());
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = shutdown.changed() => break,
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
    shared.set_link_state(LinkState::Unpaired, "stopped");
}

async fn run_one(
    shared: &Arc<Shared>,
    url: &str,
    token: &str,
    shutdown: &mut watch::Receiver<bool>,
) -> End {
    let mut req = match url.into_client_request() {
        Ok(r) => r,
        Err(e) => return End::Failed(format!("bad url: {e}")),
    };
    match HeaderValue::from_str(&format!("Bearer {token}")) {
        Ok(v) => {
            req.headers_mut().insert("Authorization", v);
        }
        Err(_) => return End::Failed("token has characters not valid in a header".into()),
    }
    let ws_cfg = WebSocketConfig::default()
        .max_message_size(Some(WS_MAX_MESSAGE))
        .max_frame_size(Some(WS_MAX_MESSAGE));
    let dial = tokio_tungstenite::connect_async_with_config(req, Some(ws_cfg), true);
    let ws = match tokio::time::timeout(CONNECT_TIMEOUT, dial).await {
        Ok(Ok((ws, _resp))) => ws,
        Ok(Err(WsError::Http(resp))) => {
            let code = resp.status();
            if code == 401 {
                // The token is not a pairing: stop redialing this pass and
                // let the owner re-pair. We keep the token (the UI decides
                // to wipe) but report it.
                return End::Failed(format!("synth refused: HTTP {code} (re-pair needed)"));
            }
            return End::Failed(format!("synth refused: HTTP {code}"));
        }
        Ok(Err(e)) => return End::Failed(format!("connect: {e}")),
        Err(_) => return End::Failed(format!("connect: no answer in {}s", CONNECT_TIMEOUT.as_secs())),
    };

    let mut ycfg = yamux::Config::default();
    ycfg.set_max_num_streams(MAX_STREAMS);
    ycfg.set_max_connection_receive_window(Some(MAX_CONNECTION_WINDOW));
    ycfg.set_split_send_size(SPLIT_SEND);
    let io = WsIo::new(ws, KEEPALIVE, READ_TIMEOUT);
    let mut conn = yamux::Connection::new(io, ycfg, yamux::Mode::Client);

    let control = match poll_fn(|cx| conn.poll_new_outbound(cx)).await {
        Ok(s) => s,
        Err(e) => return End::Failed(format!("open control stream: {e}")),
    };
    let established = Arc::new(AtomicBool::new(false));
    let mut control_task = tokio::spawn(control_loop(
        BufReader::new(control.compat()),
        shared.clone(),
        established.clone(),
    ));

    let mut control_rx = shared.control_tx.subscribe();
    let mut streams: JoinSet<()> = JoinSet::new();
    let reason: Option<String> = loop {
        tokio::select! {
            inbound = poll_fn(|cx| conn.poll_next_inbound(cx)) => match inbound {
                Some(Ok(stream)) => {
                    let sh = shared.clone();
                    streams.spawn(async move { serve_stream(stream, sh).await });
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
                    if e.is_panic() { logln!("a stream handler panicked: {e}"); }
                }
            }
            _ = control_rx.changed() => {
                // Pause closes every open stream; new ones are refused by
                // the handlers while paused.
                if shared.paused() {
                    streams.abort_all();
                }
            }
            _ = shutdown.changed() => break None,
        }
    };

    control_task.abort();
    streams.abort_all();
    let _ = tokio::time::timeout(Duration::from_secs(2), poll_fn(|cx| conn.poll_close(cx))).await;
    drop(conn);

    match reason {
        None => End::Shutdown,
        Some(why) if established.load(Ordering::Acquire) => End::Dropped(why),
        Some(why) => End::Failed(why),
    }
}

/// Ends the app's requests to the synth when the control stream ends,
/// however it ends (a dropped link, a shutdown, an aborted task).
struct RequestsAttached(Arc<Shared>, u64);

impl Drop for RequestsAttached {
    fn drop(&mut self) {
        self.0.requests.detach(self.1);
    }
}

/// hello, then presence / settings / unpair / requests until the link
/// drops.
async fn control_loop(mut s: MuxStream, shared: Arc<Shared>, established: Arc<AtomicBool>) -> String {
    let (mut requests_rx, generation) = shared.requests.attach();
    let _requests = RequestsAttached(shared.clone(), generation);
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

    let (cua_version, cua_status) = shared.cua_status().await;
    let caps = wire::capabilities_value(
        &shared.settings(),
        &cua_version,
        &cua_status,
        shared.apps.backend().as_ref(),
    );
    let hello = wire::hello(
        &shared.device_id(),
        &shared.device_name(),
        &shared.app_version,
        shared.settings_rev(),
        caps,
    );
    if let Err(e) = write_json_line(&mut s, &hello).await {
        return format!("send hello: {e}");
    }

    let (rd, mut wr) = tokio::io::split(s);
    let mut rd = BufReader::new(rd);

    let shared_r = shared.clone();
    let est = established.clone();
    let reader = async move {
        loop {
            match read_line(&mut rd, MAX_CONTROL_LINE).await {
                Ok(Some(line)) => {
                    let msg: Value = serde_json::from_slice(&line).unwrap_or_default();
                    match msg.get("type").and_then(|t| t.as_str()) {
                        Some("hello_ack") => {
                            let ack: wire::HelloAck =
                                serde_json::from_slice(&line).unwrap_or_default();
                            shared_r.requests.set_supported(ack.has(crate::requests::FEATURE));
                            shared_r.set_synth(ack.clone());
                            shared_r.set_link_state(
                                LinkState::Linked,
                                &format!("linked to {}", ack.synth_name),
                            );
                            est.store(true, Ordering::Release);
                            logln!("linked as device {}", shared_r.device_id());
                        }
                        Some("response") => shared_r.requests.resolve(&msg),
                        Some("unpair_ack") => {
                            shared_r.unpair_ack.notify_one();
                            return "unpaired (ack)".to_string();
                        }
                        Some("error") => {
                            let t = msg.get("error").and_then(|e| e.as_str()).unwrap_or("error");
                            return format!("synth: {t}");
                        }
                        _ => {}
                    }
                }
                Ok(None) => return "closed by the synth".to_string(),
                Err(e) => return format!("read: {e}"),
            }
        }
    };

    let mut settings_rx = shared.settings_tx.subscribe();
    let mut control_rx = shared.control_tx.subscribe();
    let shared_w = shared.clone();
    let writer = async move {
        let mut tick = tokio::time::interval(PRESENCE_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tick.tick().await; // first tick is immediate; hello already carried the state
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if let Err(e) = send_presence(&mut wr, &shared_w).await {
                        return format!("send presence: {e}");
                    }
                }
                _ = settings_rx.changed() => {
                    let rev = *settings_rx.borrow();
                    let (v, st) = shared_w.cua_status().await;
                    let caps = wire::capabilities_value(
                        &shared_w.settings(),
                        &v,
                        &st,
                        shared_w.apps.backend().as_ref(),
                    );
                    if let Err(e) = write_json_line(&mut wr, &wire::settings_msg(rev, caps)).await {
                        return format!("send settings: {e}");
                    }
                    if let Err(e) = send_presence(&mut wr, &shared_w).await {
                        return format!("send presence: {e}");
                    }
                }
                _ = control_rx.changed() => {
                    // pause / resume changed: reflect it in presence at once.
                    if let Err(e) = send_presence(&mut wr, &shared_w).await {
                        return format!("send presence: {e}");
                    }
                }
                _ = shared_w.unpair_req.notified() => {
                    if let Err(e) = write_json_line(&mut wr, &json!({"type":"unpair"})).await {
                        return format!("send unpair: {e}");
                    }
                }
                Some(request) = requests_rx.recv() => {
                    if let Err(e) = write_json_line(&mut wr, &request).await {
                        return format!("send request: {e}");
                    }
                }
            }
        }
    };

    tokio::select! {
        why = reader => why,
        why = writer => why,
    }
}

async fn send_presence<W>(wr: &mut W, shared: &Arc<Shared>) -> std::io::Result<()>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let p = crate::presence::read();
    let msg = wire::presence(p.idle_seconds, p.screen_locked, shared.paused(), shared.metered());
    write_json_line(wr, &msg).await
}

/// Counts a stream as open for as long as its task lives, however it ends
/// (finished, refused, aborted by a pause or a dropped link).
pub(crate) struct OpenStream(Arc<Shared>);

impl OpenStream {
    pub(crate) fn begin(shared: &Arc<Shared>) -> OpenStream {
        shared.streams_open.fetch_add(1, Ordering::AcqRel);
        OpenStream(shared.clone())
    }
}

impl Drop for OpenStream {
    fn drop(&mut self) {
        self.0.streams_open.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Read a synth-opened stream's header and hand it to its handler.
async fn serve_stream(stream: yamux::Stream, shared: Arc<Shared>) {
    let _open = OpenStream::begin(&shared);
    let mut s: MuxStream = BufReader::with_capacity(64 * 1024, stream.compat());
    let line = match tokio::time::timeout(HEADER_TIMEOUT, read_line(&mut s, MAX_HEADER_LINE)).await {
        Ok(Ok(Some(line))) => line,
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
        "tcp" => tcp::handle(s, hdr, &shared).await,
        "mcp" => mcp::handle(s, hdr, &shared).await,
        "rpc" => rpc::handle(s, &shared).await,
        other => {
            let why = Refusal::Denied(format!("unknown stream kind {other:?}"));
            logln!("stream refused: {why}");
            reply_refusal(&mut s, &why).await;
            let _ = s.shutdown().await;
        }
    }
}

fn jitter(d: Duration) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|t| t.subsec_nanos())
        .unwrap_or(0);
    d.mul_f64(0.75 + f64::from(nanos % 1000) / 2000.0)
}
