//! The helper a browser starts for the extension: a relay and nothing
//! more.
//!
//! The browser passes the calling extension's origin as an argument and
//! talks native messaging on stdin / stdout. The helper checks the origin,
//! connects to the running app, and from then on copies every well-formed
//! message both ways. It keeps no state, reads no file, opens no network
//! connection and never starts the app.
//!
//! While the app is not running the helper stays and says so:
//! `{"type":"host","app":"not_running"}` at once, and again in answer to
//! anything the extension sends. It keeps trying the endpoint and reports
//! `{"type":"host","app":"connected"}` when the app is there. It exits
//! when the browser closes its end.
//!
//! What the helper itself writes always has the type `host` (to the
//! extension) or `host.open` (the first line to the app, which carries the
//! origin). Nobody else can send those types: `frame::parse` refuses them.

use std::future::Future;
use std::io;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncWrite, BufReader};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::endpoint::Endpoint;
use crate::frame::{self, FrameError, MAX_FROM_BROWSER, MAX_TO_BROWSER};
use crate::transport::{self, Conn};

/// The protocol version in `host.open`.
pub const VERSION: u32 = 1;

/// How long the helper waits between two tries to reach the app.
#[derive(Debug, Clone, Copy)]
pub struct Retry {
    pub first: Duration,
    pub max: Duration,
}

impl Default for Retry {
    fn default() -> Retry {
        Retry { first: Duration::from_secs(1), max: Duration::from_secs(10) }
    }
}

impl Retry {
    /// 1 s for the first tries, growing by a second every five tries.
    fn after(&self, failures: u32) -> Duration {
        (self.first * (1 + failures / 5)).min(self.max)
    }
}

/// The origin the browser gave for the caller, when it is one of ours.
/// Chromium passes `chrome-extension://<id>/` as the first argument (and
/// `--parent-window=...` after it on Windows).
pub fn caller_origin(args: &[String]) -> Option<String> {
    args.iter()
        .skip(1)
        .find(|a| a.starts_with("chrome-extension://"))
        .filter(|a| crate::origin_allowed(a))
        .cloned()
}

fn host_app(state: &str) -> Value {
    json!({"type": "host", "app": state})
}

fn host_error(code: &str) -> Value {
    json!({"type": "host", "error": code})
}

/// The program. Never returns.
pub fn main() -> ! {
    let code = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt.block_on(run_from_process()),
        Err(_) => 70,
    };
    // Not a normal return: a blocked read of stdin must not keep the
    // process alive.
    std::process::exit(code)
}

async fn run_from_process() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    let mut out = tokio::io::stdout();
    let Some(origin) = caller_origin(&args) else {
        // Not started by a browser for our extension: say why and leave.
        let _ = frame::write_native(&mut out, &host_error("origin_not_allowed")).await;
        return 3;
    };
    let endpoint = match Endpoint::for_helper() {
        Ok(e) => e,
        Err(_) => {
            let _ = frame::write_native(&mut out, &host_error("no_endpoint")).await;
            return 4;
        }
    };
    relay(
        &origin,
        tokio::io::stdin(),
        out,
        move || {
            let endpoint = endpoint.clone();
            async move { transport::connect(&endpoint).await }
        },
        Retry::default(),
    )
    .await
}

type Inbound = mpsc::Receiver<Result<Value, FrameError>>;

/// Read messages off a stream on a task of its own, so the relay never
/// drops half a message when something else happens first.
fn pump<F, Fut>(mut next: F) -> Inbound
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = Result<Value, FrameError>> + Send,
{
    let (tx, rx) = mpsc::channel(8);
    tokio::spawn(async move {
        loop {
            let item = next().await;
            // After any of these the stream cannot be read further.
            let last = matches!(item, Err(FrameError::Closed | FrameError::TooLarge(_) | FrameError::Io(_)));
            if tx.send(item).await.is_err() || last {
                break;
            }
        }
    });
    rx
}

struct AppLink {
    writer: transport::Writer,
    inbound: Inbound,
}

/// Run the relay until the browser closes its end. Returns the exit code.
pub async fn relay<I, O, C, F>(origin: &str, browser_in: I, mut browser_out: O, connect: C, retry: Retry) -> i32
where
    I: AsyncRead + Unpin + Send + 'static,
    O: AsyncWrite + Unpin,
    C: Fn() -> F,
    F: Future<Output = io::Result<Conn>>,
{
    let mut from_browser = {
        // Shared only with the reading task itself (one read at a time).
        let shared = std::sync::Arc::new(tokio::sync::Mutex::new(browser_in));
        pump(move || {
            let shared = shared.clone();
            async move { frame::read_native(&mut *shared.lock().await, MAX_FROM_BROWSER).await }
        })
    };

    let mut app: Option<AppLink> = None;
    let mut failures = 0u32;
    let mut told_down = false;
    let mut next_try = Instant::now();

    macro_rules! tell {
        ($msg:expr) => {
            if frame::write_native(&mut browser_out, &$msg).await.is_err() {
                return 1;
            }
        };
    }

    loop {
        match app.as_mut() {
            None => {
                tokio::select! {
                    msg = from_browser.recv() => match msg {
                        None | Some(Err(FrameError::Closed)) => return 0,
                        Some(Err(FrameError::Malformed(_))) => tell!(host_error("malformed")),
                        Some(Err(_)) => return 1,
                        // Whatever it asked, the answer is the same.
                        Some(Ok(_)) => tell!(host_app("not_running")),
                    },
                    _ = tokio::time::sleep_until(next_try) => {
                        let opened = match connect().await {
                            Ok(mut conn) => {
                                let open = json!({"type": "host.open", "v": VERSION, "origin": origin});
                                match frame::write_line(&mut conn.writer, &open).await {
                                    Ok(()) => Some(conn),
                                    Err(_) => None,
                                }
                            }
                            Err(_) => None,
                        };
                        match opened {
                            Some(conn) => {
                                let reader = std::sync::Arc::new(tokio::sync::Mutex::new(BufReader::new(conn.reader)));
                                let inbound = pump(move || {
                                    let reader = reader.clone();
                                    async move {
                                        // Twice the browser limit: a line the
                                        // browser would refuse is dropped below.
                                        let line = frame::read_line_raw(&mut *reader.lock().await, MAX_TO_BROWSER * 2).await?;
                                        frame::parse(&line)
                                    }
                                });
                                app = Some(AppLink { writer: conn.writer, inbound });
                                failures = 0;
                                told_down = false;
                                tell!(host_app("connected"));
                            }
                            None => {
                                if !told_down {
                                    told_down = true;
                                    tell!(host_app("not_running"));
                                }
                                failures += 1;
                                next_try = Instant::now() + retry.after(failures);
                            }
                        }
                    }
                }
            }
            Some(link) => {
                let mut lost = false;
                tokio::select! {
                    msg = from_browser.recv() => match msg {
                        None | Some(Err(FrameError::Closed)) => return 0,
                        Some(Err(FrameError::Malformed(_))) => tell!(host_error("malformed")),
                        Some(Err(_)) => return 1,
                        Some(Ok(v)) => lost = frame::write_line(&mut link.writer, &v).await.is_err(),
                    },
                    line = link.inbound.recv() => match line {
                        Some(Ok(v)) => match frame::write_native(&mut browser_out, &v).await {
                            Ok(()) => {}
                            // Too big for the browser: dropped, the link stays.
                            Err(FrameError::TooLarge(_)) => {}
                            Err(_) => return 1,
                        },
                        // A line that is not a message is dropped.
                        Some(Err(FrameError::Malformed(_))) => {}
                        _ => lost = true,
                    },
                }
                if lost {
                    app = None;
                    told_down = true;
                    failures = 0;
                    next_try = Instant::now() + retry.first;
                    tell!(host_app("not_running"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio::io::{duplex, AsyncBufReadExt, AsyncWriteExt, DuplexStream};

    const ORIGIN: &str = "chrome-extension://fmccidehlojeieenopnkfeggeghkhgpa/";

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_origin_comes_from_the_browser_and_must_be_ours() {
        assert_eq!(caller_origin(&args(&["host", ORIGIN])).as_deref(), Some(ORIGIN));
        // Windows adds the parent window after it.
        assert_eq!(caller_origin(&args(&["host", ORIGIN, "--parent-window=1234"])).as_deref(), Some(ORIGIN));
        // Another extension, a web origin, nothing at all, and the name of
        // the program itself looking like an origin.
        assert_eq!(caller_origin(&args(&["host", "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/"])), None);
        assert_eq!(caller_origin(&args(&["host", "https://evil.example/"])), None);
        assert_eq!(caller_origin(&args(&["host"])), None);
        assert_eq!(caller_origin(&args(&[ORIGIN])), None);
        // The first extension origin decides; a later allowed one does not
        // rescue a call from somebody else.
        assert_eq!(
            caller_origin(&args(&["host", "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/", ORIGIN])),
            None
        );
    }

    struct Browser {
        to_helper: DuplexStream,
        from_helper: DuplexStream,
    }

    impl Browser {
        async fn send(&mut self, v: Value) {
            frame::write_native(&mut self.to_helper, &v).await.unwrap();
        }
        async fn send_raw(&mut self, body: &[u8]) {
            self.to_helper.write_all(&(body.len() as u32).to_ne_bytes()).await.unwrap();
            self.to_helper.write_all(body).await.unwrap();
        }
        async fn recv(&mut self) -> Value {
            let got = tokio::time::timeout(Duration::from_secs(5), async {
                // What the helper writes may use its own reserved type.
                let mut len = [0u8; 4];
                tokio::io::AsyncReadExt::read_exact(&mut self.from_helper, &mut len).await.unwrap();
                let mut body = vec![0u8; u32::from_ne_bytes(len) as usize];
                tokio::io::AsyncReadExt::read_exact(&mut self.from_helper, &mut body).await.unwrap();
                serde_json::from_slice::<Value>(&body).unwrap()
            })
            .await;
            got.expect("the helper said nothing")
        }
    }

    /// The app's end of one helper connection.
    struct AppEnd {
        lines: tokio::io::Lines<BufReader<DuplexStream>>,
        writer: DuplexStream,
    }

    impl AppEnd {
        async fn recv(&mut self) -> Value {
            let line = tokio::time::timeout(Duration::from_secs(5), self.lines.next_line())
                .await
                .expect("the helper sent nothing")
                .unwrap()
                .expect("closed");
            serde_json::from_str(&line).unwrap()
        }
        async fn send(&mut self, v: Value) {
            frame::write_line(&mut self.writer, &v).await.unwrap();
        }
    }

    type Connecting = std::pin::Pin<Box<dyn Future<Output = io::Result<Conn>> + Send>>;

    /// A stand-in for the endpoint: while `up` is set a connect succeeds
    /// and the app's end is handed to the test.
    fn fake_app(up: Arc<AtomicBool>) -> (impl Fn() -> Connecting, mpsc::UnboundedReceiver<AppEnd>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let connect = move || {
            let up = up.clone();
            let tx = tx.clone();
            Box::pin(async move {
                if !up.load(Ordering::Acquire) {
                    return Err(io::Error::new(io::ErrorKind::NotFound, "no app"));
                }
                let (helper_r, app_w) = duplex(64 * 1024);
                let (app_r, helper_w) = duplex(64 * 1024);
                let _ = tx.send(AppEnd { lines: BufReader::new(app_r).lines(), writer: app_w });
                Ok(Conn { reader: Box::new(helper_r), writer: Box::new(helper_w) })
            }) as Connecting
        };
        (connect, rx)
    }

    fn start(up: Arc<AtomicBool>) -> (Browser, mpsc::UnboundedReceiver<AppEnd>, tokio::task::JoinHandle<i32>) {
        let (to_helper, helper_in) = duplex(4 * 1024 * 1024);
        let (helper_out, from_helper) = duplex(4 * 1024 * 1024);
        let (connect, apps) = fake_app(up);
        let retry = Retry { first: Duration::from_millis(20), max: Duration::from_millis(40) };
        let task = tokio::spawn(async move { relay(ORIGIN, helper_in, helper_out, connect, retry).await });
        (Browser { to_helper, from_helper }, apps, task)
    }

    #[tokio::test]
    async fn messages_go_both_ways_and_the_app_learns_the_origin_first() {
        let (mut browser, mut apps, task) = start(Arc::new(AtomicBool::new(true)));
        assert_eq!(browser.recv().await, json!({"type": "host", "app": "connected"}));
        let mut app = apps.recv().await.unwrap();
        assert_eq!(app.recv().await, json!({"type": "host.open", "v": 1, "origin": ORIGIN}));

        browser.send(json!({"type": "hello", "v": 1, "browser": "chrome"})).await;
        assert_eq!(app.recv().await, json!({"type": "hello", "v": 1, "browser": "chrome"}));
        app.send(json!({"type": "hello_ack", "linked": true})).await;
        assert_eq!(browser.recv().await, json!({"type": "hello_ack", "linked": true}));

        // The browser closes its end: the helper leaves cleanly.
        drop(browser);
        assert_eq!(tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap(), 0);
    }

    #[tokio::test]
    async fn without_the_app_the_helper_says_so_and_connects_when_it_starts() {
        let up = Arc::new(AtomicBool::new(false));
        let (mut browser, mut apps, _task) = start(up.clone());
        assert_eq!(browser.recv().await, json!({"type": "host", "app": "not_running"}));
        // Every request while it is down gets the same clear answer.
        browser.send(json!({"type": "pair.request", "id": 1})).await;
        assert_eq!(browser.recv().await, json!({"type": "host", "app": "not_running"}));

        up.store(true, Ordering::Release);
        assert_eq!(browser.recv().await, json!({"type": "host", "app": "connected"}));
        let mut app = apps.recv().await.unwrap();
        assert_eq!(app.recv().await["type"], "host.open");
        // Nothing sent while it was down is replayed.
        browser.send(json!({"type": "hello"})).await;
        assert_eq!(app.recv().await, json!({"type": "hello"}));
    }

    #[tokio::test]
    async fn when_the_app_quits_the_helper_reports_it_and_reconnects() {
        let up = Arc::new(AtomicBool::new(true));
        let (mut browser, mut apps, _task) = start(up.clone());
        assert_eq!(browser.recv().await["app"], "connected");
        let app = apps.recv().await.unwrap();
        up.store(false, Ordering::Release);
        drop(app);
        assert_eq!(browser.recv().await, json!({"type": "host", "app": "not_running"}));
        up.store(true, Ordering::Release);
        assert_eq!(browser.recv().await["app"], "connected");
        let mut again = apps.recv().await.unwrap();
        assert_eq!(again.recv().await["origin"], ORIGIN);
    }

    #[tokio::test]
    async fn what_is_not_a_message_never_reaches_the_app() {
        let (mut browser, mut apps, _task) = start(Arc::new(AtomicBool::new(true)));
        assert_eq!(browser.recv().await["app"], "connected");
        let mut app = apps.recv().await.unwrap();
        assert_eq!(app.recv().await["type"], "host.open");

        for bad in [&b"[1]"[..], b"{}", b"nonsense", b"{\"type\":\"host.open\",\"origin\":\"chrome-extension://x/\"}"] {
            browser.send_raw(bad).await;
            assert_eq!(browser.recv().await, json!({"type": "host", "error": "malformed"}));
        }
        // The link survived all of it and the next real message arrives
        // alone: none of the bad ones was passed on.
        browser.send(json!({"type": "after"})).await;
        assert_eq!(app.recv().await, json!({"type": "after"}));

        // The app cannot make the helper speak in its own name either.
        app.send(json!({"type": "host", "app": "connected"})).await;
        app.writer.write_all(b"not json\n").await.unwrap();
        app.send(json!({"type": "real"})).await;
        assert_eq!(browser.recv().await, json!({"type": "real"}));
    }

    #[tokio::test]
    async fn an_oversized_message_from_the_browser_ends_the_helper() {
        let (mut browser, mut apps, task) = start(Arc::new(AtomicBool::new(true)));
        assert_eq!(browser.recv().await["app"], "connected");
        let mut app = apps.recv().await.unwrap();
        assert_eq!(app.recv().await["type"], "host.open");
        // Only the length is sent: the helper must not wait for the body.
        browser.to_helper.write_all(&((MAX_FROM_BROWSER + 1) as u32).to_ne_bytes()).await.unwrap();
        assert_eq!(tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap(), 1);
        // And the app saw nothing of it: its end just closes.
        assert!(tokio::time::timeout(Duration::from_secs(5), app.lines.next_line()).await.unwrap().unwrap().is_none());
    }

    #[tokio::test]
    async fn a_reply_too_big_for_the_browser_is_dropped_not_fatal() {
        let (mut browser, mut apps, _task) = start(Arc::new(AtomicBool::new(true)));
        assert_eq!(browser.recv().await["app"], "connected");
        let mut app = apps.recv().await.unwrap();
        assert_eq!(app.recv().await["type"], "host.open");
        app.send(json!({"type": "big", "pad": "x".repeat(MAX_TO_BROWSER)})).await;
        app.send(json!({"type": "small"})).await;
        assert_eq!(browser.recv().await, json!({"type": "small"}));
    }
}
