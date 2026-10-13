//! `connectd selftest host`: the browser link, end to end, without a
//! browser. A core listens on an endpoint, the real helper program is
//! started the way a browser starts it (the extension's origin as its
//! argument, native messaging on its stdin and stdout), and the test
//! plays the extension.
//!
//!  1. register the helper for the browsers (host manifests; on Windows
//!     the per-user registry keys), check where they point, register
//!     again after a "move", remove
//!  2. the helper reaches the app: hello / hello_ack through the real
//!     endpoint (a Unix socket, a named pipe on Windows)
//!  3. a pairing request reaches the app's approver (which refuses here),
//!     and a malformed message never reaches the app
//!  4. the helper started for another extension refuses to work
//!  5. the app stops: the helper says "not running"; the app comes back:
//!     the helper reconnects by itself
//!
//! Nothing of a real browser is touched. The manifests go under a temp
//! folder; on Windows the registry keys are written under the browsers'
//! real per-user keys but with a host name of their own
//! (`ai.snth.connect.selftest`), and are removed at the end.
//!
//! `--helper <path>`: the helper to test (default: `connect-host` next to
//! this program). `--system`: use the current user's real endpoint, which
//! is what a release build of the helper connects to; only where no
//! installed app is running (CI).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use connect_core::grant::{Approval, ApprovalFuture, ApprovalRequest, Approver};
use connect_core::pairing::{ClaimError, ClaimResult, HubClient};
use connect_core::{cookies, Core, CoreConfig, CuaAttach, MemSecretStore, MultiReader};
use connect_host::endpoint::{Endpoint, ENDPOINT_ENV};
use connect_host::install::{Change, Locations, BROWSERS};
use connect_host::{frame, EXTENSION_IDS};
use serde_json::{json, Value};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::watch;

const SELFTEST_HOST: &str = "ai.snth.connect.selftest";
const ANSWER: Duration = Duration::from_secs(15);

fn step(text: &str) {
    println!("==> {text}");
}

/// Refuses, and counts how often it was asked.
struct CountingDeny(Arc<AtomicUsize>);

impl Approver for CountingDeny {
    fn ask(&self, req: ApprovalRequest) -> ApprovalFuture {
        println!("the app asked the owner: {} for {}", req.capability, req.scope);
        self.0.fetch_add(1, Ordering::AcqRel);
        Box::pin(async { Approval::deny() })
    }
}

struct LocalHub;

impl HubClient for LocalHub {
    fn claim(&self, _hub: &str, _code: &str) -> Result<ClaimResult, ClaimError> {
        Ok(ClaimResult {
            synth_url: "http://127.0.0.1:9".into(),
            synth_id: "selftest".into(),
            companion_token: "selftest-token".into(),
        })
    }
}

/// The helper as a browser starts it.
struct Helper {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
}

impl Helper {
    fn start(program: &Path, origin: &str, endpoint: Option<&Endpoint>) -> Result<Helper, String> {
        let mut cmd = connect_core::proc::command(program);
        cmd.arg(origin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        if cfg!(windows) {
            // Chromium adds the window the call came from.
            cmd.arg("--parent-window=0");
        }
        match endpoint {
            Some(e) => {
                cmd.env(ENDPOINT_ENV, e.as_str());
            }
            None => {
                cmd.env_remove(ENDPOINT_ENV);
            }
        }
        let mut child = cmd.spawn().map_err(|e| format!("start {}: {e}", program.display()))?;
        let stdin = child.stdin.take().ok_or("the helper has no stdin")?;
        let stdout = child.stdout.take().ok_or("the helper has no stdout")?;
        Ok(Helper { child, stdin, stdout })
    }

    async fn send(&mut self, v: Value) -> Result<(), String> {
        frame::write_native(&mut self.stdin, &v).await.map_err(|e| format!("write to the helper: {e}"))
    }

    async fn send_raw(&mut self, body: &[u8]) -> Result<(), String> {
        use tokio::io::AsyncWriteExt;
        let mut framed = (body.len() as u32).to_ne_bytes().to_vec();
        framed.extend_from_slice(body);
        self.stdin.write_all(&framed).await.map_err(|e| e.to_string())?;
        self.stdin.flush().await.map_err(|e| e.to_string())
    }

    /// The next message, whatever its type (the helper's own included).
    async fn recv(&mut self) -> Result<Value, String> {
        use tokio::io::AsyncReadExt;
        let read = async {
            let mut len = [0u8; 4];
            self.stdout.read_exact(&mut len).await.map_err(|e| format!("the helper closed its output: {e}"))?;
            let n = u32::from_ne_bytes(len) as usize;
            if n > frame::MAX_TO_BROWSER {
                return Err(format!("the helper wrote a message of {n} bytes"));
            }
            let mut body = vec![0u8; n];
            self.stdout.read_exact(&mut body).await.map_err(|e| e.to_string())?;
            serde_json::from_slice::<Value>(&body).map_err(|e| format!("the helper wrote something that is not JSON: {e}"))
        };
        tokio::time::timeout(ANSWER, read).await.map_err(|_| "the helper said nothing in time".to_string())?
    }

    async fn expect(&mut self, want: Value) -> Result<(), String> {
        let got = self.recv().await?;
        if got == want {
            Ok(())
        } else {
            Err(format!("expected {want}, got {got}"))
        }
    }

    async fn recv_type(&mut self, ty: &str) -> Result<Value, String> {
        for _ in 0..20 {
            let v = self.recv().await?;
            if v["type"] == ty {
                return Ok(v);
            }
        }
        Err(format!("no {ty} message came"))
    }
}

fn default_helper() -> PathBuf {
    let name = if cfg!(windows) { "connect-host.exe" } else { "connect-host" };
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

pub async fn host(args: &[String]) -> Result<(), String> {
    let mut helper: Option<PathBuf> = None;
    let mut system = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--helper" => {
                helper = args.get(i + 1).map(PathBuf::from);
                i += 1;
            }
            "--system" => system = true,
            other => return Err(format!("unknown option {other}")),
        }
        i += 1;
    }
    let helper = helper.unwrap_or_else(default_helper);
    let helper = std::fs::canonicalize(&helper)
        .map_err(|e| format!("the helper {} is not there ({e}); build it with `cargo build -p connect-host`", helper.display()))?;
    println!("helper: {}", helper.display());

    // A space and a non-ASCII letter on purpose: real user folders have both.
    let root = std::env::temp_dir().join(format!("connect host é {}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).map_err(|e| format!("create {}: {e}", root.display()))?;
    let result = run(&helper, &root, system).await;
    let _ = std::fs::remove_dir_all(&root);
    result
}

async fn run(helper: &Path, root: &Path, system: bool) -> Result<(), String> {
    manifests(helper, root)?;

    let endpoint = if system {
        Endpoint::system().map_err(|e| format!("no endpoint for this user: {e}"))?
    } else if cfg!(windows) {
        Endpoint::at(format!(r"\\.\pipe\{SELFTEST_HOST}.{}", std::process::id()))
    } else {
        Endpoint::at(root.join("link/app.sock").to_string_lossy())
    };
    println!("endpoint: {endpoint}");
    // A release helper ignores the override and uses the system endpoint.
    let helper_endpoint = (!system).then_some(&endpoint);

    let asked = Arc::new(AtomicUsize::new(0));
    let core = new_core(asked.clone());
    core.pair_begin("local".into(), "000000".into()).await.map_err(|e| format!("pair: {e}"))?;
    core.pair_confirm()?;

    let origin = connect_host::origin_of(EXTENSION_IDS[0]);

    step("the helper says so when the app is not running");
    let mut early = Helper::start(helper, &origin, helper_endpoint)?;
    early.expect(json!({"type": "host", "app": "not_running"})).await?;
    early.send(json!({"type": "hello", "v": 1})).await?;
    early.expect(json!({"type": "host", "app": "not_running"})).await?;

    step("the app starts: that helper connects by itself");
    let (stop, stop_rx) = watch::channel(false);
    let serving = tokio::spawn({
        let (core, endpoint) = (core.clone(), endpoint.clone());
        async move { core.run_browser_link(&endpoint, stop_rx).await }
    });
    early.expect(json!({"type": "host", "app": "connected"})).await?;

    step("hello goes through the helper and the endpoint, and back");
    let install = "5e1f7e575e1f7e575e1f7e575e1f7e57";
    early
        .send(json!({"type": "hello", "v": 1, "browser": "chrome", "ext_version": "0.0.0", "install": install, "paired": false}))
        .await?;
    let ack = early.recv_type("hello_ack").await?;
    if ack["linked"] != true || ack["synth"] != "selftest" || ack["known"] != false {
        return Err(format!("unexpected hello_ack: {ack}"));
    }
    let listed = core.browsers();
    if listed.len() != 1 || !listed[0].connected || listed[0].browser != "chrome" {
        return Err(format!("the app does not list the browser: {listed:?}"));
    }

    step("a second helper (another browser profile) at the same time");
    let mut second = Helper::start(helper, &origin, helper_endpoint)?;
    second.expect(json!({"type": "host", "app": "connected"})).await?;
    second
        .send(json!({"type": "hello", "v": 1, "browser": "edge", "ext_version": "0.0.0", "install": "ed9eed9eed9eed9eed9eed9eed9eed9e", "paired": false}))
        .await?;
    second.recv_type("hello_ack").await?;
    if core.browsers().len() != 2 {
        return Err("the app does not see both browsers".into());
    }
    drop(second);

    step("a pairing request reaches the owner's question (refused here)");
    early.send(json!({"type": "pair.request", "id": 5})).await?;
    let refused = early.recv_type("pair.refused").await?;
    if refused["id"] != 5 || refused["error"] != "denied" || asked.load(Ordering::Acquire) != 1 {
        return Err(format!("unexpected answer to the pairing request: {refused} (asked {})", asked.load(Ordering::Acquire)));
    }

    step("what is not a message is stopped by the helper");
    early.send_raw(b"[\"not\",\"an object\"]").await?;
    early.expect(json!({"type": "host", "error": "malformed"})).await?;
    early.send_raw(br#"{"type":"host.open","v":1,"origin":"chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/"}"#).await?;
    early.expect(json!({"type": "host", "error": "malformed"})).await?;

    step("the helper started for another extension refuses to work");
    for foreign in ["chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/", "https://example.com/"] {
        let mut stranger = Helper::start(helper, foreign, helper_endpoint)?;
        stranger.expect(json!({"type": "host", "error": "origin_not_allowed"})).await?;
        let status = tokio::time::timeout(ANSWER, stranger.child.wait())
            .await
            .map_err(|_| "the refused helper did not exit".to_string())?
            .map_err(|e| e.to_string())?;
        if status.code() != Some(3) {
            return Err(format!("the refused helper exited with {status}"));
        }
    }
    if core.browsers().iter().filter(|b| b.connected).count() != 1 {
        return Err("a refused helper left a connection behind".into());
    }

    step("the app stops: the helper reports it");
    let _ = stop.send(true);
    tokio::time::timeout(ANSWER, serving)
        .await
        .map_err(|_| "the browser link did not stop".to_string())?
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("the browser link ended with an error: {e}"))?;
    early.expect(json!({"type": "host", "app": "not_running"})).await?;
    if core.browsers().iter().any(|b| b.connected) {
        return Err("a browser is still listed as connected after the link stopped".into());
    }

    step("the app returns (not linked this time): the same helper reconnects by itself");
    let again = new_core(asked.clone());
    let (stop2, stop2_rx) = watch::channel(false);
    let serving2 = tokio::spawn({
        let (again, endpoint) = (again.clone(), endpoint.clone());
        async move { again.run_browser_link(&endpoint, stop2_rx).await }
    });
    early.expect(json!({"type": "host", "app": "connected"})).await?;
    early
        .send(json!({"type": "hello", "v": 1, "browser": "chrome", "ext_version": "0.0.0", "install": install, "paired": false}))
        .await?;
    let ack = early.recv_type("hello_ack").await?;
    if ack["linked"] != false {
        return Err(format!("an app that is not linked said otherwise: {ack}"));
    }

    step("the browser closes its end: the helper exits");
    drop(early.stdin);
    let status = tokio::time::timeout(ANSWER, early.child.wait())
        .await
        .map_err(|_| "the helper outlived its browser".to_string())?
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("the helper exited with {status}"));
    }
    let _ = stop2.send(true);
    let _ = tokio::time::timeout(ANSWER, serving2).await;
    Ok(())
}

fn new_core(asked: Arc<AtomicUsize>) -> Core {
    Core::new(CoreConfig {
        settings_path: None,
        journal_path: None,
        approver: Arc::new(CountingDeny(asked)),
        cookies: Arc::new(MultiReader { chromium: vec![], firefox: vec![], keys: Arc::new(cookies::fixtures::NoKey) }),
        secret: Arc::new(MemSecretStore::new()),
        apps: Arc::new(connect_core::apps::stub::StubBackend::offering(&[])),
        hub: Arc::new(LocalHub),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        cua_bin: String::new(),
        cua_private_root: None,
        cua_attach: CuaAttach::User,
    })
}

/// Register, check, move, remove.
fn manifests(helper: &Path, root: &Path) -> Result<(), String> {
    step("register the helper with the browsers that are installed");
    let base = root.join("browsers");
    let loc = Locations::custom(&base, root.join("manifest"), SELFTEST_HOST);
    if !loc.install(helper, EXTENSION_IDS).is_empty() {
        return Err("a manifest was written although no browser is installed".into());
    }
    // Two of the four browsers are "installed".
    for browser in BROWSERS.iter().take(2) {
        let dir = loc.data_dir(browser);
        std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    let done = loc.install(helper, EXTENSION_IDS);
    if done.len() != 2 {
        return Err(format!("expected two browsers, got {}", done.len()));
    }
    for (browser, result) in &done {
        match result {
            Ok(Change::Written) => println!("registered for {}", browser.name),
            other => return Err(format!("{}: {other:?}", browser.name)),
        }
    }
    for st in loc.status() {
        let want = st.present.then(|| helper.to_path_buf());
        if st.helper != want {
            return Err(format!("{}: registered {:?}, expected {:?}", st.browser.name, st.helper, want));
        }
    }
    // The same again changes nothing; a moved app is followed.
    if loc.install(helper, EXTENSION_IDS).iter().any(|(_, r)| !matches!(r, Ok(Change::Unchanged))) {
        return Err("a second registration wrote something".into());
    }
    let moved = root.join("moved").join(helper.file_name().unwrap_or_default());
    if loc.install(&moved, EXTENSION_IDS).iter().any(|(_, r)| !matches!(r, Ok(Change::Written))) {
        return Err("the registration did not follow a moved helper".into());
    }
    if loc.status().iter().filter(|s| s.present).any(|s| s.helper.as_deref() != Some(moved.as_path())) {
        return Err("the registration does not point at the moved helper".into());
    }

    step("remove the registration");
    let removed = loc.remove();
    if removed.len() != 2 || removed.iter().any(|(_, r)| r.is_err()) {
        return Err(format!("removal: {removed:?}"));
    }
    if loc.status().iter().any(|s| s.helper.is_some()) {
        return Err("a registration survived its removal".into());
    }
    if !loc.remove().is_empty() {
        return Err("a second removal found something".into());
    }
    Ok(())
}
