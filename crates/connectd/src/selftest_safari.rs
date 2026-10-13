//! `connectd selftest safari`: the Safari door, end to end, without
//! Safari. A core listens on an endpoint as the app does for Safari, the
//! relay the Safari extension's native handler is made of runs in a test
//! host (`connect-relay-harness`, built from `app/safari` with
//! `swift build`), and the test plays the extension's JavaScript: one
//! native message in, one answer out.
//!
//!  1. the relay says so while the app is not running, and finds the app
//!     by itself when it starts
//!  2. hello / hello_ack through the relay and the socket; the app lists
//!     the browser as Safari whatever it calls itself
//!  3. an answer is held until the app has something to say, while
//!     another request of the same extension goes through
//!  4. a second Safari profile at the same time; closing it
//!  5. a pairing request reaches the app's approver (which refuses here)
//!  6. what is not a message never reaches the app
//!  7. the door for the Chromium helper does not take the Safari relay
//!  8. the same user only, both ways: the relay does not connect to an
//!     app of another user, and the app drops a relay of another user
//!     (nobody can be another user in a test, so each side in turn is
//!     told to expect one)
//!  9. the app stops and returns: the relay reports it and reconnects
//!
//! `--relay <path>`: the test host. `--group <id>`: bind the endpoint in
//! that app group's folder (`~/Library/Group Containers/<id>/run`) and let
//! the relay find it the way the extension does. That is the real thing
//! only when this program and the relay are signed into the group (the
//! relay sandboxed, as the extension is); `scripts/safari-sandbox-probe.sh`
//! does that with a group of its own. `--remove-group-folder`: delete that
//! group's folder at the end (refused for the app's own group).
//! `--remove-container <id>`: delete the sandbox container macOS made for
//! the test host (`~/Library/Containers/<id>`; only an id of the probe).
//! `--only-remove`: do the removals and nothing else. `--app-first`: bind
//! the endpoint once before any relay runs, so the app is the one that
//! makes the group's folder (the order on a computer where the app was
//! opened before Safari ever started the extension).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use connect_host::endpoint::Endpoint;
use connect_host::transport::Listener;
use connect_host::{safari, Door};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::watch;

use crate::selftest_host::new_core;

const ANSWER: Duration = Duration::from_secs(15);

fn step(text: &str) {
    println!("==> {text}");
}

/// The test host: one line in, one line out, any number open at once.
struct Relay {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Lines<BufReader<ChildStdout>>,
    next: u64,
    /// Answers that arrived while another one was waited for.
    early: HashMap<u64, Value>,
}

/// Where the relay looks for the app.
#[derive(Clone)]
enum Target {
    Socket(String),
    Group(String),
}

impl Relay {
    fn start(program: &Path, target: &Target, expect_uid: Option<u32>) -> Result<Relay, String> {
        let mut cmd = connect_core::proc::command(program);
        match target {
            Target::Socket(path) => cmd.args(["--socket", path]),
            Target::Group(id) => cmd.args(["--group", id]),
        };
        if let Some(uid) = expect_uid {
            cmd.args(["--expect-uid", &uid.to_string()]);
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| format!("start {}: {e}", program.display()))?;
        let stdin = child.stdin.take().ok_or("the relay has no stdin")?;
        let stdout = child.stdout.take().ok_or("the relay has no stdout")?;
        Ok(Relay { child, stdin: Some(stdin), lines: BufReader::new(stdout).lines(), next: 0, early: HashMap::new() })
    }

    /// Send one native message; the answer is collected with `answer`.
    async fn ask(&mut self, message: Value) -> Result<u64, String> {
        self.next += 1;
        let mut line = serde_json::to_vec(&json!({"id": self.next, "message": message})).map_err(|e| e.to_string())?;
        line.push(b'\n');
        let stdin = self.stdin.as_mut().ok_or("the relay's input is closed")?;
        stdin.write_all(&line).await.map_err(|e| format!("write to the relay: {e}"))?;
        stdin.flush().await.map_err(|e| e.to_string())?;
        Ok(self.next)
    }

    async fn answer(&mut self, id: u64, within: Duration) -> Result<Value, String> {
        if let Some(v) = self.early.remove(&id) {
            return Ok(v);
        }
        let deadline = Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = tokio::time::timeout(left, self.lines.next_line())
                .await
                .map_err(|_| "the relay did not answer in time".to_string())?
                .map_err(|e| e.to_string())?
                .ok_or("the relay closed its output")?;
            let v: Value = serde_json::from_str(&line).map_err(|e| format!("the relay wrote something that is not JSON: {e}"))?;
            let got = v["id"].as_u64().unwrap_or(0);
            let reply = v["reply"].clone();
            if got == id {
                return Ok(reply);
            }
            self.early.insert(got, reply);
        }
    }

    async fn call(&mut self, message: Value) -> Result<Value, String> {
        let id = self.ask(message).await?;
        self.answer(id, ANSWER + Duration::from_secs(30)).await
    }

    /// Close its input, as Safari ending the extension's process would.
    async fn finish(mut self) -> Result<(), String> {
        drop(self.stdin.take());
        let status = tokio::time::timeout(ANSWER, self.child.wait())
            .await
            .map_err(|_| "the relay outlived its input".to_string())?
            .map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("the relay exited with {status}"))
        }
    }
}

/// One life of the extension's background script, as its JavaScript
/// behaves: numbered messages, each taken once, acknowledged in the next
/// request.
struct Ext {
    sid: String,
    ack: u64,
    inbox: VecDeque<Value>,
}

impl Ext {
    fn new(sid: &str) -> Ext {
        Ext { sid: sid.to_string(), ack: 0, inbox: VecDeque::new() }
    }

    fn request(&self, send: Vec<Value>, wait_ms: u64) -> Value {
        json!({"type": "relay", "v": 1, "sid": self.sid, "ack": self.ack, "send": send, "wait": wait_ms})
    }

    fn take(&mut self, reply: &Value) -> Result<(), String> {
        if reply["type"] != "relay" || reply.get("error").is_some() {
            return Err(format!("the relay refused the exchange: {reply}"));
        }
        for entry in reply["messages"].as_array().cloned().unwrap_or_default() {
            let seq = entry["seq"].as_u64().ok_or_else(|| format!("a message without a number: {entry}"))?;
            if seq > self.ack {
                if seq != self.ack + 1 {
                    return Err(format!("the relay skipped from {} to {seq}", self.ack));
                }
                self.ack = seq;
                self.inbox.push_back(entry["msg"].clone());
            }
        }
        Ok(())
    }

    async fn send(&mut self, relay: &mut Relay, msg: Value) -> Result<(), String> {
        let reply = relay.call(self.request(vec![msg], 0)).await?;
        self.take(&reply)
    }

    /// The next message from the app or the relay.
    async fn recv(&mut self, relay: &mut Relay) -> Result<Value, String> {
        let deadline = Instant::now() + ANSWER;
        loop {
            if let Some(v) = self.inbox.pop_front() {
                return Ok(v);
            }
            if Instant::now() >= deadline {
                return Err("nothing came through the relay in time".into());
            }
            let reply = relay.call(self.request(vec![], 2000)).await?;
            self.take(&reply)?;
        }
    }

    async fn expect(&mut self, relay: &mut Relay, want: Value) -> Result<(), String> {
        let got = self.recv(relay).await?;
        if got == want {
            Ok(())
        } else {
            Err(format!("expected {want}, got {got}"))
        }
    }

    async fn recv_type(&mut self, relay: &mut Relay, ty: &str) -> Result<Value, String> {
        for _ in 0..20 {
            let v = self.recv(relay).await?;
            if v["type"] == ty {
                return Ok(v);
            }
        }
        Err(format!("no {ty} message came"))
    }

    /// Nothing arrives for a while.
    async fn silent(&mut self, relay: &mut Relay, ms: u64) -> Result<bool, String> {
        if !self.inbox.is_empty() {
            return Ok(false);
        }
        let reply = relay.call(self.request(vec![], ms)).await?;
        self.take(&reply)?;
        Ok(self.inbox.is_empty())
    }
}

fn hello(install: &str, browser: &str) -> Value {
    json!({"type": "hello", "v": 1, "browser": browser, "ext_version": "0.0.0", "install": install, "paired": false})
}

fn not_running() -> Value {
    json!({"type": "host", "app": "not_running"})
}

fn connected() -> Value {
    json!({"type": "host", "app": "connected"})
}

pub async fn safari(args: &[String]) -> Result<(), String> {
    let mut relay: Option<PathBuf> = None;
    let mut group: Option<String> = None;
    let mut remove_group = false;
    let mut remove_container: Option<String> = None;
    let mut only_remove = false;
    let mut app_first = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--relay" => {
                relay = args.get(i + 1).map(PathBuf::from);
                i += 1;
            }
            "--group" => {
                group = args.get(i + 1).cloned();
                i += 1;
            }
            "--remove-group-folder" => remove_group = true,
            "--remove-container" => {
                remove_container = args.get(i + 1).cloned();
                i += 1;
            }
            "--only-remove" => only_remove = true,
            "--app-first" => app_first = true,
            other => return Err(format!("unknown option {other}")),
        }
        i += 1;
    }
    if remove_group && group.as_deref().is_none_or(|g| g == safari::APP_GROUP) {
        return Err("--remove-group-folder needs --group with a test group, never the app's own".into());
    }
    let probe_id = |id: &str| {
        id.starts_with("ai.snth.connect.probe") && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    };
    if remove_container.as_deref().is_some_and(|id| !probe_id(id)) {
        return Err("--remove-container takes an id of the probe only (ai.snth.connect.probe...)".into());
    }
    let removals = |group_folder: Option<&Path>| {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let container = remove_container.as_deref().zip(home).map(|(id, home)| home.join("Library/Containers").join(id));
        for folder in group_folder.filter(|_| remove_group).map(Path::to_path_buf).into_iter().chain(container) {
            match std::fs::remove_dir_all(&folder) {
                Ok(()) => println!("removed {}", folder.display()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => println!("could not remove {}: {e}", folder.display()),
            }
        }
    };
    if only_remove {
        let endpoint = group.as_deref().map(Endpoint::in_app_group).transpose().map_err(|e| e.to_string())?;
        removals(endpoint.as_ref().and_then(|e| Path::new(e.as_str()).parent()).and_then(Path::parent));
        return Ok(());
    }
    let relay = relay.ok_or("--relay <path to connect-relay-harness> is required (build it: swift build --package-path app/safari)")?;
    let relay = std::fs::canonicalize(&relay).map_err(|e| format!("the relay {} is not there ({e})", relay.display()))?;
    println!("relay: {}", relay.display());

    // A short path: a Unix socket path holds about 100 bytes.
    let root = PathBuf::from(format!("/tmp/connect-safari-{}", std::process::id()));
    let (endpoint, target) = match &group {
        Some(id) => {
            let endpoint = Endpoint::in_app_group(id).map_err(|e| format!("--group {id}: {e}"))?;
            (endpoint, Target::Group(id.clone()))
        }
        None => {
            let _ = std::fs::remove_dir_all(&root);
            let endpoint = Endpoint::at(root.join("run/app.sock").to_string_lossy());
            (endpoint.clone(), Target::Socket(endpoint.as_str().to_string()))
        }
    };
    println!("endpoint: {endpoint}");
    if app_first {
        step("the app is there first: it makes the folder, then goes away again");
        let folder = Path::new(endpoint.as_str()).parent().map(Path::to_path_buf).unwrap_or_default();
        let existed = folder.parent().is_some_and(Path::exists);
        drop(Listener::bind(&endpoint).map_err(|e| format!("bind {endpoint}: {e}"))?);
        println!("the folder {} before this run", if existed { "existed" } else { "did not exist" });
    }
    let result = run(&relay, &endpoint, &target).await;
    let _ = std::fs::remove_dir_all(&root);
    removals(group.as_ref().and_then(|_| Path::new(endpoint.as_str()).parent()).and_then(Path::parent));
    result
}

async fn run(relay: &Path, endpoint: &Endpoint, target: &Target) -> Result<(), String> {
    let asked = Arc::new(AtomicUsize::new(0));
    let core = new_core(asked.clone());
    core.pair_begin("local".into(), "000000".into()).await.map_err(|e| format!("pair: {e}"))?;
    core.pair_confirm()?;

    step("the relay says so when the app is not running");
    let mut r = Relay::start(relay, target, None)?;
    let mut ext = Ext::new("5afa5afa5afa5afa5afa5afa5afa5afa");
    let install = "5e1f7e575e1f7e575e1f7e575e1f7e57";
    ext.send(&mut r, hello(install, "safari")).await?;
    ext.expect(&mut r, not_running()).await?;
    if !ext.silent(&mut r, 300).await? {
        return Err("the relay repeated itself while the app was away".into());
    }

    step("the app starts on the Safari door: the relay finds it by itself");
    let (stop, stop_rx) = watch::channel(false);
    let serving = tokio::spawn({
        let (core, endpoint) = (core.clone(), endpoint.clone());
        async move { core.run_browser_door(&endpoint, Door::Safari, stop_rx).await }
    });
    ext.expect(&mut r, connected()).await?;
    {
        use std::os::unix::fs::PermissionsExt;
        let socket = Path::new(endpoint.as_str());
        let mode = |p: &Path| std::fs::metadata(p).map(|m| m.permissions().mode() & 0o777);
        let folder = socket.parent().ok_or("the endpoint has no folder")?;
        if mode(folder).ok() != Some(0o700) || mode(socket).ok() != Some(0o600) {
            return Err(format!("the endpoint is open to others: {:?} {:?}", mode(folder), mode(socket)));
        }
    }

    step("hello goes through the relay and the socket, and back; the app lists Safari");
    // It says Chrome on purpose: the door decides.
    ext.send(&mut r, hello(install, "chrome")).await?;
    let ack = ext.recv_type(&mut r, "hello_ack").await?;
    if ack["linked"] != true || ack["synth"] != "selftest" || ack["known"] != false {
        return Err(format!("unexpected hello_ack: {ack}"));
    }
    let listed = core.browsers();
    if listed.len() != 1 || !listed[0].connected || listed[0].browser != safari::BROWSER_ID {
        return Err(format!("the app does not list the browser as Safari: {listed:?}"));
    }

    step("an answer is held until the app speaks, and another request goes through meanwhile");
    let held = r.ask(ext.request(vec![], 20_000)).await?;
    let asked_at = Instant::now();
    // The same extension says something while its long request is open.
    let quick = r.ask(ext.request(vec![json!({"type": "pair.request", "id": 5})], 0)).await?;
    let quick_reply = r.answer(quick, ANSWER).await?;
    ext.take(&quick_reply)?;
    let held_reply = r.answer(held, ANSWER).await?;
    ext.take(&held_reply)?;
    if asked_at.elapsed() > Duration::from_secs(10) {
        return Err("the held answer did not come back when the app spoke".into());
    }

    step("the pairing request reached the owner's question (refused here)");
    let refused = ext.recv_type(&mut r, "pair.refused").await?;
    if refused["id"] != 5 || refused["error"] != "denied" || asked.load(Ordering::Acquire) != 1 {
        return Err(format!("unexpected answer to the pairing request: {refused} (asked {})", asked.load(Ordering::Acquire)));
    }

    step("a second Safari profile at the same time, then it closes");
    let mut second = Ext::new("b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0");
    second.send(&mut r, hello("ed9eed9eed9eed9eed9eed9eed9eed9e", "safari")).await?;
    second.expect(&mut r, connected()).await?;
    second.recv_type(&mut r, "hello_ack").await?;
    if core.browsers().iter().filter(|b| b.connected).count() != 2 {
        return Err("the app does not see both profiles".into());
    }
    let closed = r.call(json!({"type": "relay", "v": 1, "sid": second.sid, "close": true})).await?;
    if closed["closed"] != true {
        return Err(format!("closing a session: {closed}"));
    }
    wait_until(|| core.browsers().iter().filter(|b| b.connected).count() == 1, "the closed profile is still connected").await?;

    step("what is not a message is stopped by the relay");
    for bad in [
        json!(["not", "an object"]),
        json!({"type": "host.open", "v": 1, "origin": connect_host::origin_of(connect_host::EXTENSION_IDS[0])}),
        json!({"type": "host", "app": "connected"}),
    ] {
        ext.send(&mut r, bad).await?;
        ext.expect(&mut r, json!({"type": "host", "error": "malformed"})).await?;
    }
    for bad in [json!({"type": "hello"}), json!({"type": "relay", "v": 9, "sid": ext.sid}), json!({"type": "relay", "v": 1, "sid": "x"}), json!("relay")] {
        let reply = r.call(bad.clone()).await?;
        if reply.get("error").is_none() {
            return Err(format!("the relay took {bad}: {reply}"));
        }
    }
    if core.browsers().iter().filter(|b| b.connected).count() != 1 {
        return Err("a refused request changed what the app sees".into());
    }

    step("the app stops: the relay reports it");
    let _ = stop.send(true);
    tokio::time::timeout(ANSWER, serving)
        .await
        .map_err(|_| "the Safari door did not stop".to_string())?
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("the Safari door ended with an error: {e}"))?;
    ext.expect(&mut r, not_running()).await?;
    if core.browsers().iter().any(|b| b.connected) {
        return Err("a browser is still listed as connected after the door closed".into());
    }

    step("the app returns (not linked this time): the same relay reconnects by itself");
    let again = new_core(asked.clone());
    let (stop2, stop2_rx) = watch::channel(false);
    let serving2 = tokio::spawn({
        let (again, endpoint) = (again.clone(), endpoint.clone());
        async move { again.run_browser_door(&endpoint, Door::Safari, stop2_rx).await }
    });
    ext.expect(&mut r, connected()).await?;
    ext.send(&mut r, hello(install, "safari")).await?;
    let ack = ext.recv_type(&mut r, "hello_ack").await?;
    if ack["linked"] != false {
        return Err(format!("an app that is not linked said otherwise: {ack}"));
    }
    let _ = stop2.send(true);
    let _ = tokio::time::timeout(ANSWER, serving2).await;
    ext.expect(&mut r, not_running()).await?;

    step("the door of the Chromium helper does not take the Safari relay");
    let (stop3, stop3_rx) = watch::channel(false);
    let serving3 = tokio::spawn({
        let (core, endpoint) = (core.clone(), endpoint.clone());
        async move { core.run_browser_link(&endpoint, stop3_rx).await }
    });
    // It reaches the socket, names itself, and is hung up on unheard.
    ext.expect(&mut r, connected()).await?;
    ext.send(&mut r, hello(install, "safari")).await?;
    ext.expect(&mut r, not_running()).await?;
    if core.browsers().iter().any(|b| b.connected) {
        return Err("the Chromium door served the Safari relay".into());
    }
    let _ = stop3.send(true);
    let _ = tokio::time::timeout(ANSWER, serving3).await;
    r.finish().await?;

    step("the same user only: the relay does not connect to an app of another user");
    let me = unsafe { libc::getuid() };
    {
        let mut listener = Listener::bind(endpoint).map_err(|e| format!("bind: {e}"))?;
        // This relay is told the app must run as somebody else, so the
        // real socket of this user is another user's to it.
        let mut wary = Relay::start(relay, target, Some(me + 1))?;
        let mut ext = Ext::new("0dd00dd00dd00dd00dd00dd00dd00dd0");
        ext.send(&mut wary, hello(install, "safari")).await?;
        ext.expect(&mut wary, not_running()).await?;
        if tokio::time::timeout(Duration::from_millis(1500), listener.accept()).await.is_ok() {
            return Err("a relay that expects another user connected all the same".into());
        }
        if !ext.silent(&mut wary, 300).await? {
            return Err("the wary relay heard something".into());
        }
        wary.finish().await?;
    }

    step("the same user only: the app drops a relay of another user unheard");
    {
        let mut listener = Listener::bind(endpoint).map_err(|e| format!("bind: {e}"))?;
        // Here the app is told to expect somebody else.
        listener.expect_peer_uid(me + 1);
        let served = Arc::new(AtomicUsize::new(0));
        let accepting = tokio::spawn({
            let (core, served) = (core.clone(), served.clone());
            async move {
                while let Ok(conn) = listener.accept().await {
                    served.fetch_add(1, Ordering::AcqRel);
                    core.serve_browser_on(conn, Door::Safari).await;
                }
            }
        });
        let mut stranger = Relay::start(relay, target, None)?;
        let mut ext = Ext::new("5712a9e25712a9e25712a9e25712a9e2");
        ext.send(&mut stranger, hello(install, "safari")).await?;
        // It may get as far as "connected" (the socket took the call)
        // before the app hangs up; it never gets an answer to its hello.
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            let v = match ext.inbox.pop_front() {
                Some(v) => v,
                None => {
                    ext.silent(&mut stranger, 300).await?;
                    continue;
                }
            };
            if v["type"] != "host" {
                return Err(format!("a relay of another user was answered: {v}"));
            }
        }
        if served.load(Ordering::Acquire) != 0 || core.browsers().iter().any(|b| b.connected) {
            return Err("the app served a relay of another user".into());
        }
        accepting.abort();
        stranger.finish().await?;
    }
    Ok(())
}

async fn wait_until(mut done: impl FnMut() -> bool, or: &str) -> Result<(), String> {
    for _ in 0..100 {
        if done() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(or.to_string())
}
