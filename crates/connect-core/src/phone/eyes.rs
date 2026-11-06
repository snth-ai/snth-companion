//! The phone's screen: frames from the capture helper (`snth-phonecap`,
//! app/macos/phonecap/main.swift) behind the `Eyes` trait.
//!
//! The helper is a program inside the app bundle (Contents/MacOS, signed
//! with the app, so the camera permission macOS asks for is the app's). It
//! keeps a live capture of the iPhone on the cable (the CoreMediaIO screen
//! device QuickTime uses, only a device whose model is "iOS Device", never
//! a Continuity Camera) and answers on its stdin/stdout. It listens on no
//! port.
//!
//! The helper protocol: one JSON request per line on stdin,
//!   {"id": 1, "cmd": "status"}
//!   {"id": 2, "cmd": "jpeg", "after_ms": 0, "max_edge": 0, "quality": 0.85}
//!   {"id": 3, "cmd": "gray", "max_edge": 448}
//! and for each, on stdout, one JSON header line followed by exactly `len`
//! bytes of payload (none when `len` is 0):
//!   {"id": 1, "ok": true, "len": 0, "device": "iPhone", "width": 828, ...}
//!   {"id": 2, "ok": true, "len": 181234, "width": .., "height": ..,
//!    "native_width": .., "native_height": .., "frame_ms": ..}  + JPEG
//!   {"id": 3, "ok": true, "len": 92736, "width": 207, "height": 448, ...} + gray
//!   {"id": 4, "ok": false, "len": 0, "error": "no_device: ..."}
//! Its log lines go to stderr. The core starts it when the phone is
//! switched on and kills it when the phone is switched off.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

use super::diff::Gray;

pub type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The capture as `phone_control.status` reports it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ScreenStatus {
    pub ok: bool,
    pub device: String,
    pub width: u32,
    pub height: u32,
    /// Unix ms of the newest frame (0 = none yet).
    pub last_frame_ms: i64,
    /// Why there is no screen.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// A frame as JPEG.
#[derive(Debug, Clone)]
pub struct Jpeg {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub native_width: u32,
    pub native_height: u32,
    pub frame_ms: i64,
}

/// A small gray frame for change detection, and the screen's real size.
#[derive(Debug, Clone)]
pub struct GrayFrame {
    pub gray: Gray,
    pub native_width: u32,
    pub native_height: u32,
    pub frame_ms: i64,
}

/// Where frames come from. Errors are plain words; the gate labels them
/// `no_screen`.
pub trait Eyes: Send + Sync {
    fn status(&self) -> Fut<'_, ScreenStatus>;
    /// The newest frame, waiting up to 1.5 s for one newer than
    /// `after_ms`; scaled so its long edge is at most `max_edge` (0 =
    /// native size).
    fn jpeg(&self, after_ms: i64, max_edge: u32, quality: f64) -> Fut<'_, Result<Jpeg, String>>;
    /// The newest frame, gray, long edge `max_edge`.
    fn gray(&self, max_edge: u32) -> Fut<'_, Result<GrayFrame, String>>;
}

/// No screen at all (a system without the helper).
pub struct NoEyes(pub String);

impl Eyes for NoEyes {
    fn status(&self) -> Fut<'_, ScreenStatus> {
        let why = self.0.clone();
        Box::pin(async move { ScreenStatus { error: why, ..Default::default() } })
    }
    fn jpeg(&self, _: i64, _: u32, _: f64) -> Fut<'_, Result<Jpeg, String>> {
        let why = self.0.clone();
        Box::pin(async move { Err(why) })
    }
    fn gray(&self, _: u32) -> Fut<'_, Result<GrayFrame, String>> {
        let why = self.0.clone();
        Box::pin(async move { Err(why) })
    }
}

// --- the helper protocol ---

/// The biggest payload the helper may announce (a 4K frame as raw gray is
/// 8 MB; a JPEG of a phone screen is well under 1 MB).
pub const MAX_PAYLOAD: usize = 32 * 1024 * 1024;
/// The longest header line.
const MAX_HEADER: usize = 64 * 1024;

/// A request line for the helper.
pub fn request_line(id: u64, mut req: Value) -> Vec<u8> {
    if let Some(o) = req.as_object_mut() {
        o.insert("id".into(), json!(id));
    }
    let mut line = serde_json::to_vec(&req).unwrap_or_default();
    line.push(b'\n');
    line
}

/// Read one reply: the header line and its payload. Lines that are not a
/// JSON object are skipped (nothing else should be on stdout, but a
/// stray print must not break the stream).
pub async fn read_reply<R: AsyncBufRead + Unpin>(r: &mut R) -> std::io::Result<(Value, Vec<u8>)> {
    loop {
        let mut line = Vec::new();
        let n = (&mut *r).take(MAX_HEADER as u64).read_until(b'\n', &mut line).await?;
        if n == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the helper closed its output"));
        }
        if line.last() != Some(&b'\n') {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "a header line without its end"));
        }
        let header: Value = match serde_json::from_slice(&line) {
            Ok(v @ Value::Object(_)) => v,
            _ => continue,
        };
        let len = header.get("len").and_then(Value::as_u64).unwrap_or(0) as usize;
        if len > MAX_PAYLOAD {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("a payload of {len} bytes")));
        }
        let mut payload = vec![0u8; len];
        r.read_exact(&mut payload).await?;
        return Ok((header, payload));
    }
}

/// A running helper: where to write requests and read replies.
pub struct Conn {
    pub reader: Box<dyn AsyncBufRead + Unpin + Send>,
    pub writer: Box<dyn AsyncWrite + Unpin + Send>,
    /// The process, killed when the connection is dropped.
    pub child: Option<tokio::process::Child>,
}

/// Starts the helper (a process; a fake in tests).
pub trait Spawn: Send + Sync {
    fn spawn(&self) -> Result<Conn, String>;
}

/// The helper program next to the app's own.
pub struct ProgramSpawn(pub PathBuf);

impl Spawn for ProgramSpawn {
    fn spawn(&self) -> Result<Conn, String> {
        let mut cmd = crate::proc::command(&self.0);
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| format!("cannot start the screen helper {}: {e}", self.0.display()))?;
        let stdin = child.stdin.take().ok_or("the screen helper has no stdin")?;
        let stdout = child.stdout.take().ok_or("the screen helper has no stdout")?;
        if let Some(err) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(err).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    crate::logln!("phonecap: {l}");
                }
            });
        }
        Ok(Conn { reader: Box::new(BufReader::with_capacity(256 * 1024, stdout)), writer: Box::new(stdin), child: Some(child) })
    }
}

/// Where the helper is: `SNTH_PHONECAP`, else next to this program (in
/// the bundle: Contents/MacOS/snth-phonecap; in a dev build: the target
/// folder, where tauri-build copies it).
pub fn helper_path() -> PathBuf {
    if let Some(p) = std::env::var_os("SNTH_PHONECAP").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent().map(|d| d.join(HELPER_NAME)).unwrap_or_else(|| PathBuf::from(HELPER_NAME))
}

/// The helper's file name.
pub const HELPER_NAME: &str = "snth-phonecap";

struct Call {
    req: Value,
    timeout: Duration,
    reply: oneshot::Sender<Result<(Value, Vec<u8>), String>>,
}

/// How long the helper may take to answer: a frame waits up to 1.5 s for
/// a new one, then encodes.
const CALL_TIMEOUT: Duration = Duration::from_secs(6);
/// After the helper failed to start or died, the next start waits this
/// long (it retries the device itself every second while it runs).
const RESTART_BACKOFF: Duration = Duration::from_secs(1);

/// The helper, owned by one task that sends one request at a time.
/// Dropping it ends the task and kills the helper.
pub struct HelperEyes {
    tx: mpsc::Sender<Call>,
}

impl HelperEyes {
    /// Start the owner task; the helper itself starts with the first
    /// request (and right away, to warm the capture). Needs a tokio
    /// runtime.
    pub fn start(spawn: Arc<dyn Spawn>) -> HelperEyes {
        let (tx, rx) = mpsc::channel::<Call>(16);
        tokio::spawn(owner(spawn, rx));
        let eyes = HelperEyes { tx };
        // Warm up: the iOS device shows up a few seconds after the helper
        // allowed screen devices.
        let warm = eyes.tx.clone();
        tokio::spawn(async move {
            let (reply, _rx) = oneshot::channel();
            let _ = warm.send(Call { req: json!({"cmd": "status"}), timeout: CALL_TIMEOUT, reply }).await;
        });
        eyes
    }

    async fn call(&self, req: Value) -> Result<(Value, Vec<u8>), String> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Call { req, timeout: CALL_TIMEOUT, reply })
            .await
            .map_err(|_| "the screen helper is stopped".to_string())?;
        rx.await.map_err(|_| "the screen helper is stopped".to_string())?
    }
}

async fn owner(spawn: Arc<dyn Spawn>, mut rx: mpsc::Receiver<Call>) {
    let mut conn: Option<Conn> = None;
    let mut next_id: u64 = 1;
    let mut not_before: Option<tokio::time::Instant> = None;
    let mut last_fail = String::new();
    while let Some(call) = rx.recv().await {
        if conn.is_none() {
            if not_before.is_some_and(|t| tokio::time::Instant::now() < t) {
                let _ = call.reply.send(Err(last_fail.clone()));
                continue;
            }
            match spawn.spawn() {
                Ok(c) => conn = Some(c),
                Err(why) => {
                    crate::logln!("phone: {why}");
                    last_fail = why.clone();
                    not_before = Some(tokio::time::Instant::now() + RESTART_BACKOFF);
                    let _ = call.reply.send(Err(why));
                    continue;
                }
            }
        }
        let c = conn.as_mut().expect("just set");
        let id = next_id;
        next_id += 1;
        let line = request_line(id, call.req);
        let exchange = async {
            c.writer.write_all(&line).await?;
            c.writer.flush().await?;
            loop {
                let (header, payload) = read_reply(&mut c.reader).await?;
                // A late answer to a request that timed out: skip it.
                if header.get("id").and_then(Value::as_u64) == Some(id) {
                    return Ok::<_, std::io::Error>((header, payload));
                }
            }
        };
        let result = match tokio::time::timeout(call.timeout, exchange).await {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(e)) => {
                // The helper died or wrote garbage: start a fresh one next
                // time.
                let why = format!("the screen helper stopped: {e}");
                crate::logln!("phone: {why}");
                conn = None;
                last_fail = why.clone();
                not_before = Some(tokio::time::Instant::now() + RESTART_BACKOFF);
                Err(why)
            }
            Err(_) => {
                // A stuck helper: replaced, its stream may be mid-frame.
                conn = None;
                Err(format!("the screen helper did not answer in {} s", call.timeout.as_secs()))
            }
        };
        let _ = call.reply.send(result);
    }
    // The phone was switched off: the helper goes with the connection.
    if let Some(mut c) = conn.take() {
        if let Some(child) = c.child.as_mut() {
            let _ = child.start_kill();
        }
    }
}

/// A failed reply's words.
fn failed(header: &Value) -> Option<String> {
    if header.get("ok").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    Some(header.get("error").and_then(Value::as_str).unwrap_or("the screen helper failed").to_string())
}

fn u32_of(v: &Value, k: &str) -> u32 {
    v.get(k).and_then(Value::as_u64).unwrap_or(0).min(u32::MAX as u64) as u32
}

impl Eyes for HelperEyes {
    fn status(&self) -> Fut<'_, ScreenStatus> {
        Box::pin(async move {
            match self.call(json!({"cmd": "status"})).await {
                Ok((h, _)) => {
                    let mut s = ScreenStatus {
                        ok: h.get("ok").and_then(Value::as_bool) == Some(true)
                            && h.get("device").and_then(Value::as_str).is_some_and(|d| !d.is_empty()),
                        device: h.get("device").and_then(Value::as_str).unwrap_or("").to_string(),
                        width: u32_of(&h, "width"),
                        height: u32_of(&h, "height"),
                        last_frame_ms: h.get("last_frame_ms").and_then(Value::as_i64).unwrap_or(0),
                        error: h.get("error").and_then(Value::as_str).unwrap_or("").to_string(),
                    };
                    if s.ok && s.width == 0 {
                        s.ok = false;
                        s.error = "the phone is connected but no frame came yet".into();
                    }
                    s
                }
                Err(why) => ScreenStatus { error: why, ..Default::default() },
            }
        })
    }

    fn jpeg(&self, after_ms: i64, max_edge: u32, quality: f64) -> Fut<'_, Result<Jpeg, String>> {
        Box::pin(async move {
            let (h, data) = self
                .call(json!({"cmd": "jpeg", "after_ms": after_ms, "max_edge": max_edge, "quality": quality}))
                .await?;
            if let Some(why) = failed(&h) {
                return Err(why);
            }
            if data.is_empty() {
                return Err("the screen helper sent an empty frame".into());
            }
            Ok(Jpeg {
                data,
                width: u32_of(&h, "width"),
                height: u32_of(&h, "height"),
                native_width: u32_of(&h, "native_width"),
                native_height: u32_of(&h, "native_height"),
                frame_ms: h.get("frame_ms").and_then(Value::as_i64).unwrap_or(0),
            })
        })
    }

    fn gray(&self, max_edge: u32) -> Fut<'_, Result<GrayFrame, String>> {
        Box::pin(async move {
            let (h, data) = self.call(json!({"cmd": "gray", "max_edge": max_edge})).await?;
            if let Some(why) = failed(&h) {
                return Err(why);
            }
            Ok(GrayFrame {
                gray: Gray::new(u32_of(&h, "width"), u32_of(&h, "height"), data)?,
                native_width: u32_of(&h, "native_width"),
                native_height: u32_of(&h, "native_height"),
                frame_ms: h.get("frame_ms").and_then(Value::as_i64).unwrap_or(0),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::duplex;

    #[tokio::test]
    async fn a_reply_is_a_header_line_and_exactly_len_bytes() {
        let mut data = Vec::new();
        data.extend_from_slice(b"not json, skipped\n");
        data.extend_from_slice(b"{\"id\":1,\"ok\":true,\"len\":5}\n");
        data.extend_from_slice(b"ab\ncd");
        data.extend_from_slice(b"{\"id\":2,\"ok\":false,\"len\":0,\"error\":\"no_device: none\"}\n");
        let mut r = BufReader::new(&data[..]);
        let (h, p) = read_reply(&mut r).await.unwrap();
        assert_eq!(h["id"], 1);
        assert_eq!(p, b"ab\ncd", "a payload may hold newlines");
        let (h, p) = read_reply(&mut r).await.unwrap();
        assert_eq!(h["error"], "no_device: none");
        assert!(p.is_empty());
        assert!(read_reply(&mut r).await.is_err(), "the end of the stream");
    }

    #[tokio::test]
    async fn a_short_payload_or_an_absurd_length_is_an_error() {
        let data = b"{\"id\":1,\"ok\":true,\"len\":10}\nabc".to_vec();
        assert!(read_reply(&mut BufReader::new(&data[..])).await.is_err());
        let data = format!("{{\"id\":1,\"ok\":true,\"len\":{}}}\n", MAX_PAYLOAD + 1).into_bytes();
        assert!(read_reply(&mut BufReader::new(&data[..])).await.is_err());
    }

    #[test]
    fn a_request_carries_its_id() {
        let line = request_line(7, json!({"cmd": "gray", "max_edge": 448}));
        assert_eq!(*line.last().unwrap(), b'\n');
        let v: Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(v, json!({"id": 7, "cmd": "gray", "max_edge": 448}));
    }

    /// A fake helper on an in-memory pipe: answers status, gray and jpeg;
    /// `die_after` requests it closes its output.
    struct FakeSpawn {
        starts: AtomicUsize,
        die_after: usize,
    }

    impl Spawn for FakeSpawn {
        fn spawn(&self) -> Result<Conn, String> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            let (ours, theirs) = duplex(1 << 20);
            let (their_r, mut their_w) = tokio::io::split(theirs);
            let die_after = self.die_after;
            tokio::spawn(async move {
                let mut lines = BufReader::new(their_r).lines();
                let mut served = 0;
                while let Ok(Some(l)) = lines.next_line().await {
                    if served == die_after {
                        return;
                    }
                    served += 1;
                    let req: Value = serde_json::from_str(&l).unwrap();
                    let id = req["id"].clone();
                    let (head, body): (Value, Vec<u8>) = match req["cmd"].as_str().unwrap() {
                        "status" => (json!({"id": id, "ok": true, "len": 0, "device": "iPhone", "width": 828, "height": 1792, "last_frame_ms": 5}), vec![]),
                        "gray" => (json!({"id": id, "ok": true, "len": 6, "width": 2, "height": 3, "native_width": 828, "native_height": 1792, "frame_ms": 9}), vec![1, 2, 3, 4, 5, 6]),
                        "jpeg" => (json!({"id": id, "ok": true, "len": 4, "width": 384, "height": 831, "native_width": 828, "native_height": 1792, "frame_ms": 9}), vec![0xff, 0xd8, b'\n', 0xd9]),
                        _ => (json!({"id": id, "ok": false, "len": 0, "error": "unknown"}), vec![]),
                    };
                    let mut out = serde_json::to_vec(&head).unwrap();
                    out.push(b'\n');
                    out.extend(body);
                    if their_w.write_all(&out).await.is_err() {
                        return;
                    }
                }
            });
            let (r, w) = tokio::io::split(ours);
            Ok(Conn { reader: Box::new(BufReader::new(r)), writer: Box::new(w), child: None })
        }
    }

    #[tokio::test]
    async fn the_owner_task_talks_to_the_helper_and_restarts_it_when_it_dies() {
        let spawn = Arc::new(FakeSpawn { starts: AtomicUsize::new(0), die_after: 3 });
        let eyes = HelperEyes::start(spawn.clone());
        // The warm-up status is the first request.
        let s = eyes.status().await;
        assert!(s.ok, "{s:?}");
        assert_eq!((s.width, s.height, s.device.as_str()), (828, 1792, "iPhone"));
        let g = eyes.gray(448).await.unwrap();
        assert_eq!((g.gray.w, g.gray.h, g.native_width), (2, 3, 828));
        assert_eq!(g.gray.px, [1, 2, 3, 4, 5, 6]);
        // The fourth request finds the helper gone.
        let e = eyes.jpeg(0, 768, 0.8).await.unwrap_err();
        assert!(e.contains("stopped"), "{e}");
        assert_eq!(spawn.starts.load(Ordering::SeqCst), 1);
        // After the back-off a new helper serves.
        tokio::time::sleep(RESTART_BACKOFF + Duration::from_millis(50)).await;
        let j = eyes.jpeg(0, 768, 0.8).await.unwrap();
        assert_eq!(j.data, [0xff, 0xd8, b'\n', 0xd9]);
        assert_eq!((j.width, j.native_height), (384, 1792));
        assert_eq!(spawn.starts.load(Ordering::SeqCst), 2);
    }

    struct NoProgram;
    impl Spawn for NoProgram {
        fn spawn(&self) -> Result<Conn, String> {
            Err("cannot start the screen helper /nowhere: not found".into())
        }
    }

    #[tokio::test]
    async fn a_missing_helper_is_a_status_with_the_reason() {
        let eyes = HelperEyes::start(Arc::new(NoProgram));
        let s = eyes.status().await;
        assert!(!s.ok);
        assert!(s.error.contains("cannot start"), "{s:?}");
        assert!(eyes.gray(448).await.is_err());
    }
}
