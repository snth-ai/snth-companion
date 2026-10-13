//! The browser link from the outside: a test plays the extension (behind
//! the helper's first line) and the synth (answering the app's requests),
//! and a scripted approver plays the owner.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{duplex, AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines};

use super::*;
use crate::cookies::MultiReader;
use crate::grant::{Approval, ApprovalFuture, Approver};
use crate::pairing::{ClaimError, ClaimResult, HubClient};
use crate::secret::MemSecretStore;
use crate::{Core, CoreConfig, CuaAttach};

const ORIGIN: &str = "chrome-extension://fmccidehlojeieenopnkfeggeghkhgpa/";
const INSTALL: &str = "0123456789abcdef0123456789abcdef";
const SECRET: &str = "s3cr3t-cookie-value-do-not-log";

/// The owner: answers from a script and remembers what was asked.
#[derive(Default)]
struct Owner {
    allow: Mutex<Option<Approval>>,
    asked: Mutex<Vec<ApprovalRequest>>,
}

impl Owner {
    fn says(&self, a: Approval) {
        *self.allow.lock().unwrap() = Some(a);
    }
    fn asked(&self) -> Vec<ApprovalRequest> {
        self.asked.lock().unwrap().clone()
    }
}

struct OwnerApprover(Arc<Owner>);

impl Approver for OwnerApprover {
    fn ask(&self, req: ApprovalRequest) -> ApprovalFuture {
        self.0.asked.lock().unwrap().push(req);
        let answer = self.0.allow.lock().unwrap().unwrap_or_else(Approval::deny);
        Box::pin(async move { answer })
    }
}

struct FakeHub;
impl HubClient for FakeHub {
    fn claim(&self, _hub: &str, _code: &str) -> Result<ClaimResult, ClaimError> {
        Ok(ClaimResult {
            synth_url: "https://mia.snth.ai".into(),
            synth_id: "mia".into(),
            companion_token: "tok".into(),
        })
    }
}

fn new_core() -> (Core, Arc<Owner>) {
    let owner = Arc::new(Owner::default());
    let core = Core::new(CoreConfig {
        settings_path: None,
        journal_path: None,
        approver: Arc::new(OwnerApprover(owner.clone())),
        cookies: Arc::new(MultiReader {
            chromium: vec![],
            firefox: vec![],
            keys: Arc::new(crate::cookies::fixtures::NoKey),
        }),
        apps: Arc::new(crate::apps::stub::StubBackend::offering(&[])),
        secret: Arc::new(MemSecretStore::new()),
        hub: Arc::new(FakeHub),
        app_version: "9.9.9".into(),
        cua_bin: "cua-driver".into(),
        cua_private_root: None,
        cua_attach: CuaAttach::User,
    });
    (core, owner)
}

async fn linked_core() -> (Core, Arc<Owner>) {
    let (core, owner) = new_core();
    core.pair_begin("hub".into(), "123456".into()).await.unwrap();
    core.pair_confirm().unwrap();
    (core, owner)
}

type Seen = Arc<Mutex<Vec<(String, Value)>>>;

/// The synth's side of the app's requests.
fn synth(core: &Core, answer: impl Fn(&str, &Value) -> Result<Value, String> + Send + 'static) -> Seen {
    let shared = core.shared();
    let (mut rx, _) = shared.requests.attach();
    shared.requests.set_supported(true);
    let seen: Seen = Arc::default();
    let log = seen.clone();
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            let method = req["method"].as_str().unwrap_or("").to_string();
            log.lock().unwrap().push((method.clone(), req["params"].clone()));
            let reply = match answer(&method, &req["params"]) {
                Ok(result) => json!({"type": "response", "id": req["id"], "result": result}),
                Err(e) => json!({"type": "response", "id": req["id"], "error": e}),
            };
            shared.requests.resolve(&reply);
        }
    });
    seen
}

fn usual_synth(core: &Core) -> Seen {
    synth(core, |method, _| match method {
        "vault.pair_code" => Ok(json!({"code": "K7QW3MHX", "expires_in_s": 600})),
        "vault.list" => Ok(json!({
            "sites": [{"site": "github.com", "has_totp": true, "updated": 1_791_700_000i64, "last_used": 0, "use_count": 3}],
            "devices": [{"id": "dev_aa11", "name": "Chrome on macOS", "created": 1_791_600_000i64, "last_seen": 1_791_700_000i64}],
        })),
        "vault.forget" | "vault.revoke_device" => Ok(json!({"ok": true})),
        other => Err(format!("unknown method {other}")),
    })
}

/// The extension's end of one connection (what the helper relays).
struct Ext {
    w: DuplexStream,
    lines: Lines<BufReader<DuplexStream>>,
}

impl Ext {
    async fn send(&mut self, v: Value) {
        let mut line = serde_json::to_vec(&v).unwrap();
        line.push(b'\n');
        self.w.write_all(&line).await.unwrap();
    }

    async fn recv(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(5), self.lines.next_line())
            .await
            .expect("the app said nothing")
            .unwrap()
            .expect("the app closed the connection");
        serde_json::from_str(&line).unwrap()
    }

    /// The next message of this type; others in between are skipped.
    async fn recv_type(&mut self, ty: &str) -> Value {
        loop {
            let v = self.recv().await;
            if v["type"] == ty {
                return v;
            }
        }
    }

    async fn silent(&mut self) -> bool {
        tokio::time::timeout(Duration::from_millis(150), self.lines.next_line()).await.is_err()
    }

    async fn closed(&mut self) -> bool {
        matches!(tokio::time::timeout(Duration::from_secs(5), self.lines.next_line()).await, Ok(Ok(None)))
    }
}

/// Connect as the helper would for `origin`.
async fn open(core: &Core, origin: &str) -> Ext {
    let (app_r, ext_w) = duplex(4 * 1024 * 1024);
    let (ext_r, app_w) = duplex(4 * 1024 * 1024);
    let core = core.clone();
    tokio::spawn(async move {
        core.serve_browser(Conn { reader: Box::new(app_r), writer: Box::new(app_w) }).await;
    });
    let mut ext = Ext { w: ext_w, lines: BufReader::new(ext_r).lines() };
    ext.send(json!({"type": "host.open", "v": 1, "origin": origin})).await;
    ext
}

fn hello(install: &str) -> Value {
    json!({"type": "hello", "v": 1, "browser": "chrome", "ext_version": "0.2.0", "install": install, "paired": false})
}

/// Connect and say hello; returns the extension and the app's hello_ack.
async fn greet(core: &Core, install: &str) -> (Ext, Value) {
    let mut ext = open(core, ORIGIN).await;
    ext.send(hello(install)).await;
    let ack = ext.recv_type("hello_ack").await;
    (ext, ack)
}

/// Pair through the app, as the owner clicking Allow.
async fn link(core: &Core, owner: &Owner, install: &str) -> Ext {
    owner.says(Approval::once());
    let (mut ext, _) = greet(core, install).await;
    ext.send(json!({"type": "pair.request", "id": 1})).await;
    let code = ext.recv_type("pair.code").await;
    assert_eq!(code["code"], "K7QW3MHX");
    ext.send(json!({"type": "pair.done", "id": 1, "ok": true, "device_id": "dev_aa11"})).await;
    wait_for(|| core.browsers().iter().any(|b| b.install == install && b.linked)).await;
    ext
}

async fn wait_for(mut done: impl FnMut() -> bool) {
    for _ in 0..200 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the expected state never came");
}

fn journal_text(core: &Core) -> String {
    serde_json::to_string(&core.recent_journal()).unwrap()
}

#[test]
fn what_an_extension_names_is_cut_to_shape() {
    assert_eq!(clean_browser("Chrome", Door::Chromium), "chrome");
    assert_eq!(clean_browser("edge", Door::Chromium), "edge");
    assert_eq!(clean_browser("<script>", Door::Chromium), "browser");
    // Safari is who comes in on the Safari door, and nobody else.
    assert_eq!(clean_browser("safari", Door::Chromium), "browser");
    assert_eq!(clean_browser("chrome", Door::Safari), "safari");
    assert_eq!(clean_browser("", Door::Safari), "safari");
    assert!(clean_install(INSTALL).is_some());
    assert!(clean_install("short").is_none());
    assert!(clean_install(&"g".repeat(32)).is_none());
    assert!(clean_install(&"a".repeat(65)).is_none());
    assert_eq!(clean_version("0.2.0-beta<x>"), "0.2.0");
    assert_eq!(clean_device_id("dev_ab12cd"), "dev_ab12cd");
    assert_eq!(clean_device_id("dev_AB\n"), "");
    assert_eq!(clean_device_id("other"), "");

    // A site is the registrable domain and nothing else.
    assert_eq!(clean_site("Example.com").as_deref(), Some("example.com"));
    assert_eq!(clean_site("bbc.co.uk").as_deref(), Some("bbc.co.uk"));
    for bad in ["www.example.com", "co.uk", "localhost", "127.0.0.1", "*.com", "example.com/", "", "exa mple.com", ".example.com"] {
        assert_eq!(clean_site(bad), None, "{bad}");
    }
    assert_eq!(clean_code("K7QW3MHX").as_deref(), Some("K7QW3MHX"));
    for bad in ["k7qw3mhx", "K7QW3MH", "K7QW3MHXX", "K7QW3MH0", "K7QW-3MH"] {
        assert_eq!(clean_code(bad), None, "{bad}");
    }
}

#[test]
fn cookies_of_another_site_and_malformed_ones_are_dropped() {
    let raw = json!([
        {"name": "SID", "value": "a", "domain": ".example.com", "path": "/", "expires": 1_900_000_000.5, "secure": true, "http_only": true, "same_site": "lax"},
        {"name": "host", "value": "b", "domain": "www.example.com", "path": "/app"},
        {"name": "EVIL", "value": "x", "domain": ".evil.com", "path": "/"},
        {"name": "LOOK", "value": "x", "domain": "notexample.com", "path": "/"},
        {"name": "SUFFIX", "value": "x", "domain": "example.com.evil.com", "path": "/"},
        {"name": "nl", "value": "a\r\nSet-Cookie: x=y", "domain": ".example.com"},
        {"name": "big", "value": "v".repeat(MAX_COOKIE_FIELD + 1), "domain": ".example.com"},
        {"name": 7, "value": "x", "domain": ".example.com"},
        {"value": "x", "domain": ".example.com"},
        "not an object",
        {"name": "odd", "value": "c", "domain": "EXAMPLE.com", "path": "no-slash", "expires": -5, "same_site": "weird"},
    ]);
    let got = clean_cookies("example.com", Some(&raw));
    let names: Vec<&str> = got.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["SID", "host", "odd"]);
    assert_eq!(got[0].expires, 1_900_000_000);
    assert!(got[0].secure && got[0].http_only);
    assert_eq!(got[0].same_site, "lax");
    assert_eq!(got[1].path, "/app");
    assert_eq!((got[2].domain.as_str(), got[2].path.as_str(), got[2].expires, got[2].same_site.as_str()), ("example.com", "/", 0, ""));

    // The count is bounded whatever arrives.
    let many: Vec<Value> = (0..MAX_COOKIES + 50)
        .map(|i| json!({"name": format!("c{i}"), "value": "v", "domain": ".example.com"}))
        .collect();
    assert_eq!(clean_cookies("example.com", Some(&json!(many))).len(), MAX_COOKIES);
    assert!(clean_cookies("example.com", None).is_empty());
    assert!(clean_cookies("example.com", Some(&json!("nope"))).is_empty());
}

#[tokio::test]
async fn a_connection_for_another_extension_is_closed_unheard() {
    let (core, owner) = linked_core().await;
    for origin in [
        "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/",
        "https://evil.example/",
        "",
    ] {
        let mut ext = open(&core, origin).await;
        assert!(ext.closed().await, "{origin}");
    }
    // A first line that is not the helper's is not a connection either.
    let (app_r, mut ext_w) = duplex(1024);
    let (ext_r, app_w) = duplex(1024);
    let served = core.clone();
    tokio::spawn(async move { served.serve_browser(Conn { reader: Box::new(app_r), writer: Box::new(app_w) }).await });
    ext_w.write_all(format!("{}\n", hello(INSTALL)).as_bytes()).await.unwrap();
    let mut lines = BufReader::new(ext_r).lines();
    assert!(matches!(tokio::time::timeout(Duration::from_secs(5), lines.next_line()).await, Ok(Ok(None))));

    assert!(core.browsers().is_empty());
    assert!(owner.asked().is_empty());
}

#[tokio::test]
async fn nothing_is_heard_before_the_hello() {
    let (core, owner) = linked_core().await;
    usual_synth(&core);
    owner.says(Approval::once());
    let mut ext = open(&core, ORIGIN).await;
    ext.send(json!({"type": "pair.request", "id": 1})).await;
    ext.send(json!({"type": "pair.done", "id": 1, "ok": true, "device_id": "dev_aa11"})).await;
    ext.send(json!({"type": "cookies.result", "id": 1, "ok": true, "cookies": []})).await;
    assert!(ext.silent().await);
    assert!(owner.asked().is_empty());
    assert!(core.browsers().is_empty());

    // A hello that is not one changes nothing either.
    ext.send(json!({"type": "hello", "v": 1, "install": "zz"})).await;
    assert_eq!(ext.recv().await["error"], "bad_hello");
    ext.send(json!({"type": "hello", "v": 2, "install": INSTALL})).await;
    assert_eq!(ext.recv().await["error"], "unsupported_version");
    assert!(core.browsers().is_empty());
}

#[tokio::test]
async fn hello_names_the_app_and_the_browser_is_listed_as_not_linked() {
    let (core, _owner) = linked_core().await;
    let (_ext, ack) = greet(&core, INSTALL).await;
    assert_eq!(ack["v"], 1);
    assert_eq!(ack["app_version"], "9.9.9");
    assert_eq!(ack["linked"], true);
    assert_eq!(ack["synth"], "mia");
    assert_eq!(ack["known"], false);
    // The app tells an extension who it is linked to and nothing else.
    let keys: Vec<&String> = ack.as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 6, "{keys:?}");

    let list = core.browsers();
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].browser.as_str(), list[0].connected, list[0].linked), ("chrome", true, false));
    assert_eq!(list[0].ext_version, "0.2.0");
}

#[tokio::test]
async fn an_app_that_is_not_linked_says_so_and_pairs_nothing() {
    let (core, owner) = new_core();
    owner.says(Approval::once());
    let (mut ext, ack) = greet(&core, INSTALL).await;
    assert_eq!(ack["linked"], false);
    ext.send(json!({"type": "pair.request", "id": 4})).await;
    let no = ext.recv_type("pair.refused").await;
    assert_eq!((no["id"].as_u64(), no["error"].as_str()), (Some(4), Some("not_linked")));
    assert!(owner.asked().is_empty(), "the owner is not asked about something that cannot happen");
}

#[tokio::test]
async fn pairing_asks_the_owner_here_then_hands_over_the_synths_code() {
    let (core, owner) = linked_core().await;
    let seen = usual_synth(&core);
    owner.says(Approval::once());
    let (mut ext, _) = greet(&core, INSTALL).await;

    ext.send(json!({"type": "pair.request", "id": 7})).await;
    let code = ext.recv_type("pair.code").await;
    assert_eq!((code["id"].as_u64(), code["code"].as_str(), code["synth"].as_str()), (Some(7), Some("K7QW3MHX"), Some("mia")));

    // The owner was asked once, about the vault, naming the browser, and
    // the answer could not be remembered.
    let asked = owner.asked();
    assert_eq!(asked.len(), 1);
    assert_eq!((asked[0].capability.as_str(), asked[0].scope.as_str()), ("vault", "Chrome"));
    assert_eq!(asked[0].remember, Remember::Never);
    assert!(core.grant_list().is_empty());
    assert_eq!(seen.lock().unwrap()[0].0, "vault.pair_code");

    // Not linked until the extension reports that the hub took the code.
    assert!(!core.browsers()[0].linked);
    ext.send(json!({"type": "pair.done", "id": 7, "ok": true, "device_id": "dev_aa11"})).await;
    wait_for(|| core.browsers()[0].linked).await;

    // The journal says what happened and never holds the code.
    let journal = journal_text(&core);
    assert!(journal.contains("vault.pair") && journal.contains("Chrome"), "{journal}");
    assert!(!journal.contains("K7QW3MHX"));
}

#[tokio::test]
async fn the_owners_no_ends_it_before_the_synth_is_asked() {
    let (core, owner) = linked_core().await;
    let seen = usual_synth(&core);
    owner.says(Approval::deny());
    let (mut ext, _) = greet(&core, INSTALL).await;
    ext.send(json!({"type": "pair.request", "id": 2})).await;
    assert_eq!(ext.recv_type("pair.refused").await["error"], "denied");
    assert!(seen.lock().unwrap().is_empty());
    assert!(!core.browsers()[0].linked);
    assert!(journal_text(&core).contains("denied: owner"));
}

#[tokio::test]
async fn a_synth_that_is_away_or_too_old_is_named_as_the_reason() {
    // No link at all.
    let (core, owner) = linked_core().await;
    owner.says(Approval::once());
    let (mut ext, _) = greet(&core, INSTALL).await;
    ext.send(json!({"type": "pair.request", "id": 1})).await;
    assert_eq!(ext.recv_type("pair.refused").await["error"], "synth_offline");

    // A link to a synth that does not answer requests.
    let (core, owner) = linked_core().await;
    owner.says(Approval::once());
    let _attached = core.shared().requests.attach();
    let (mut ext, _) = greet(&core, INSTALL).await;
    ext.send(json!({"type": "pair.request", "id": 1})).await;
    assert_eq!(ext.recv_type("pair.refused").await["error"], "synth_outdated");

    // A synth that answers with something that is not a code.
    let (core, owner) = linked_core().await;
    owner.says(Approval::once());
    synth(&core, |_, _| Ok(json!({"code": "<script>"})));
    let (mut ext, _) = greet(&core, INSTALL).await;
    ext.send(json!({"type": "pair.request", "id": 1})).await;
    assert_eq!(ext.recv_type("pair.refused").await["error"], "failed");
}

#[tokio::test]
async fn a_done_nobody_asked_for_links_nothing() {
    let (core, _owner) = linked_core().await;
    usual_synth(&core);
    let (mut ext, _) = greet(&core, INSTALL).await;
    ext.send(json!({"type": "pair.done", "id": 1, "ok": true, "device_id": "dev_aa11"})).await;
    ext.send(json!({"type": "state", "paired": true})).await;
    assert!(ext.silent().await);
    assert!(!core.browsers()[0].linked);
}

#[tokio::test]
async fn a_second_question_while_one_is_open_is_refused_as_busy() {
    let (core, owner) = linked_core().await;
    // The synth never answers, so the first pairing stays open.
    let (_rx, _) = core.shared().requests.attach();
    core.shared().requests.set_supported(true);
    owner.says(Approval::once());
    let (mut ext, _) = greet(&core, INSTALL).await;
    ext.send(json!({"type": "pair.request", "id": 1})).await;
    wait_for(|| owner.asked().len() == 1).await;
    ext.send(json!({"type": "pair.request", "id": 2})).await;
    let no = ext.recv_type("pair.refused").await;
    assert_eq!((no["id"].as_u64(), no["error"].as_str()), (Some(2), Some("busy")));
    assert_eq!(owner.asked().len(), 1, "the owner sees one window, not one per message");
}

#[tokio::test]
async fn a_browser_paired_by_hand_is_linked_only_when_this_synth_knows_it() {
    let (core, owner) = linked_core().await;
    let seen = usual_synth(&core);
    owner.says(Approval::once());

    let mut known = open(&core, ORIGIN).await;
    let mut hi = hello(INSTALL);
    hi["paired"] = json!(true);
    hi["device_id"] = json!("dev_aa11");
    known.send(hi).await;
    known.recv_type("hello_ack").await;
    known.send(json!({"type": "pair.request", "id": 3})).await;
    let ok = known.recv_type("pair.linked").await;
    assert_eq!((ok["id"].as_u64(), ok["synth"].as_str()), (Some(3), Some("mia")));
    assert!(core.browsers().iter().any(|b| b.install == INSTALL && b.linked));
    // No new code was minted for a browser that has a pairing.
    assert!(seen.lock().unwrap().iter().all(|(m, _)| m == "vault.list"));

    let other = "ffffffffffffffffffffffffffffffff";
    let mut stranger = open(&core, ORIGIN).await;
    let mut hi = hello(other);
    hi["paired"] = json!(true);
    hi["device_id"] = json!("dev_zz99");
    stranger.send(hi).await;
    stranger.recv_type("hello_ack").await;
    stranger.send(json!({"type": "pair.request", "id": 4})).await;
    assert_eq!(stranger.recv_type("pair.refused").await["error"], "other_synth");
    assert!(core.browsers().iter().any(|b| b.install == other && !b.linked));
}

#[tokio::test]
async fn the_owner_can_link_a_connected_browser_from_the_app_without_a_second_question() {
    let (core, owner) = linked_core().await;
    usual_synth(&core);
    let (mut ext, _) = greet(&core, INSTALL).await;
    let linking = tokio::spawn({
        let core = core.clone();
        async move { core.browser_link(INSTALL).await }
    });
    let code = ext.recv_type("pair.code").await;
    assert_eq!((code["id"].as_u64(), code["code"].as_str()), (Some(0), Some("K7QW3MHX")));
    assert!(linking.await.unwrap().is_ok());
    assert!(owner.asked().is_empty(), "the click in the app is the confirmation");
    ext.send(json!({"type": "pair.done", "id": 0, "ok": true, "device_id": "dev_aa11"})).await;
    wait_for(|| core.browsers()[0].linked).await;

    // A browser that is not connected cannot be linked.
    assert_eq!(core.browser_link("ffffffffffffffffffffffffffffffff").await, Err(PairRefusal::Failed));
}

fn cookie_reply(id: &Value) -> Value {
    json!({"type": "cookies.result", "id": id, "ok": true, "cookies": [
        {"name": "SID", "value": SECRET, "domain": ".example.com", "path": "/", "expires": 1_900_000_000, "secure": true, "http_only": true, "same_site": "lax"},
        {"name": "OTHER", "value": SECRET, "domain": ".evil.com", "path": "/"},
    ]})
}

#[tokio::test]
async fn cookies_are_never_asked_of_a_browser_the_owner_did_not_link() {
    let (core, _owner) = linked_core().await;
    let (mut ext, _) = greet(&core, INSTALL).await;
    let read = core.shared().cookies_via_extension("example.com", Duration::from_secs(2), true).await;
    assert!(matches!(read, ExtensionRead::NoBrowser));
    assert!(ext.silent().await, "an unlinked browser is not even asked");
}

#[tokio::test]
async fn a_linked_browser_hands_over_the_asked_site_and_only_that() {
    let (core, owner) = linked_core().await;
    usual_synth(&core);
    let mut ext = link(&core, &owner, INSTALL).await;
    assert_eq!(core.shared().extension_for("example.com"), Some(("chrome".into(), false)));

    let reading = tokio::spawn({
        let shared = core.shared();
        async move { shared.cookies_via_extension("example.com", Duration::from_secs(5), true).await }
    });
    let ask = ext.recv_type("cookies.get").await;
    // The app names the site and the synth, and says whether the owner
    // was just asked. Nothing else goes out.
    assert_eq!((ask["site"].as_str(), ask["synth"].as_str(), ask["asked"].as_bool()), (Some("example.com"), Some("mia"), Some(true)));
    assert_eq!(ask.as_object().unwrap().len(), 5);
    ext.send(cookie_reply(&ask["id"])).await;

    let ExtensionRead::Cookies(read) = reading.await.unwrap() else {
        panic!("expected cookies");
    };
    assert_eq!(read.source, "chrome-extension");
    assert_eq!(read.cookies.len(), 1, "the cookie of another site was dropped");
    assert_eq!((read.cookies[0].name.as_str(), read.cookies[0].value.as_str()), ("SID", SECRET));

    // "Ask every time" (the default): the browser gives the site back.
    let revoke = ext.recv_type("site.revoke").await;
    assert_eq!(revoke["site"], "example.com");
    assert_eq!(core.shared().extension_for("example.com"), Some(("chrome".into(), false)));

    // A site that is not a registrable domain is never sent to a browser.
    for bad in ["www.example.com", "com", "127.0.0.1", "exa mple.com"] {
        assert!(matches!(
            core.shared().cookies_via_extension(bad, Duration::from_millis(200), true).await,
            ExtensionRead::NoBrowser
        ));
    }
    assert!(ext.silent().await);
}

#[tokio::test]
async fn a_standing_answer_keeps_the_site_readable_until_the_owner_takes_it_back() {
    let (core, owner) = linked_core().await;
    usual_synth(&core);
    core.mutate_settings(|s| {
        s.cookies.enabled = true;
        s.cookies.mode = CookiesMode::AskNew;
    });
    let mut ext = link(&core, &owner, INSTALL).await;
    core.shared().grants().record("cookies", "example.com", Approval::always());

    let reading = tokio::spawn({
        let shared = core.shared();
        async move { shared.cookies_via_extension("example.com", Duration::from_secs(5), true).await }
    });
    let ask = ext.recv_type("cookies.get").await;
    ext.send(cookie_reply(&ask["id"])).await;
    assert!(matches!(reading.await.unwrap(), ExtensionRead::Cookies(_)));
    assert!(ext.silent().await, "nothing is taken back while the answer stands");
    assert_eq!(core.shared().extension_for("example.com"), Some(("chrome".into(), true)));
    assert_eq!(core.browsers()[0].sites, ["example.com"]);

    // The owner revokes the site in the app: the browser is told.
    core.revoke("cookies", "example.com");
    assert_eq!(ext.recv_type("site.revoke").await["site"], "example.com");
}

#[tokio::test]
async fn an_answer_is_taken_only_from_the_browser_that_was_asked() {
    let (core, owner) = linked_core().await;
    usual_synth(&core);
    let mut linked = link(&core, &owner, INSTALL).await;
    // A second connection of our extension that the owner never linked.
    let (mut other, _) = greet(&core, "ffffffffffffffffffffffffffffffff").await;

    let reading = tokio::spawn({
        let shared = core.shared();
        async move { shared.cookies_via_extension("example.com", Duration::from_secs(5), true).await }
    });
    let ask = linked.recv_type("cookies.get").await;
    // It answers a question it was not asked, guessing the id.
    other
        .send(json!({"type": "cookies.result", "id": ask["id"], "ok": true, "cookies": [
            {"name": "SID", "value": "planted", "domain": ".example.com", "path": "/"}
        ]}))
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!reading.is_finished(), "the stranger's answer was not taken");
    linked.send(cookie_reply(&ask["id"])).await;
    let ExtensionRead::Cookies(read) = reading.await.unwrap() else {
        panic!("expected cookies");
    };
    assert_eq!(read.cookies[0].value, SECRET);
}

#[tokio::test]
async fn refused_or_unanswered_in_the_browser_is_the_owners_decision() {
    let (core, owner) = linked_core().await;
    usual_synth(&core);
    let mut ext = link(&core, &owner, INSTALL).await;

    let reading = tokio::spawn({
        let shared = core.shared();
        async move { shared.cookies_via_extension("example.com", Duration::from_secs(5), true).await }
    });
    let ask = ext.recv_type("cookies.get").await;
    ext.send(json!({"type": "cookies.result", "id": ask["id"], "ok": false, "error": "denied"})).await;
    let ExtensionRead::Denied(why) = reading.await.unwrap() else {
        panic!("expected a refusal");
    };
    assert_eq!(why, "the owner did not confirm example.com in Chrome");

    let read = core.shared().cookies_via_extension("example.com", Duration::from_millis(100), true).await;
    let ExtensionRead::NoAnswer(why) = read else {
        panic!("expected no answer");
    };
    assert!(why.starts_with("no answer from the user"), "{why}");
}

#[tokio::test]
async fn a_site_the_owner_no_longer_allows_is_taken_back_at_hello() {
    let (core, _owner) = linked_core().await;
    core.mutate_settings(|s| s.cookies.mode = CookiesMode::AskNew);
    core.shared().grants().record("cookies", "kept.com", Approval::always());
    let mut ext = open(&core, ORIGIN).await;
    let mut hi = hello(INSTALL);
    hi["sites"] = json!(["kept.com", "gone.com", "www.not-a-site.com", 5]);
    ext.send(hi).await;
    ext.recv_type("hello_ack").await;
    assert_eq!(ext.recv_type("site.revoke").await["site"], "gone.com");
    assert!(ext.silent().await);
    assert_eq!(core.browsers()[0].sites, ["kept.com"]);

    // With "never ask" there are no per-site answers: nothing is taken.
    let (core, _owner) = linked_core().await;
    core.mutate_settings(|s| s.cookies.mode = CookiesMode::Auto);
    let mut ext = open(&core, ORIGIN).await;
    let mut hi = hello(INSTALL);
    hi["sites"] = json!(["any.com"]);
    ext.send(hi).await;
    ext.recv_type("hello_ack").await;
    assert!(ext.silent().await);
}

#[tokio::test]
async fn unlinking_the_computer_clears_every_browser() {
    let (core, owner) = linked_core().await;
    usual_synth(&core);
    let mut ext = link(&core, &owner, INSTALL).await;
    core.wipe_local();
    let status = ext.recv_type("status").await;
    assert_eq!((status["linked"].as_bool(), status["known"].as_bool()), (Some(false), Some(false)));
    assert_eq!(ext.recv().await["type"], "sites.clear");
    assert!(!core.browsers()[0].linked);
    // And no cookie is asked of it any more.
    assert!(matches!(
        core.shared().cookies_via_extension("example.com", Duration::from_millis(100), true).await,
        ExtensionRead::NoBrowser
    ));
}

#[tokio::test]
async fn unlinking_one_browser_tells_it_and_the_vault() {
    let (core, owner) = linked_core().await;
    let seen = usual_synth(&core);
    let mut ext = link(&core, &owner, INSTALL).await;
    core.browser_unlink(INSTALL).await.unwrap();
    assert_eq!(ext.recv_type("unpair").await["type"], "unpair");
    assert_eq!(ext.recv().await["type"], "sites.clear");
    assert!(!core.browsers()[0].linked);
    let calls = seen.lock().unwrap().clone();
    let revoke = calls.iter().find(|(m, _)| m == "vault.revoke_device").expect("the vault was told");
    assert_eq!(revoke.1["device_id"], "dev_aa11");

    // The extension says it lost its pairing: no longer linked.
    let (core, owner) = linked_core().await;
    usual_synth(&core);
    let mut ext = link(&core, &owner, INSTALL).await;
    ext.send(json!({"type": "state", "paired": false})).await;
    wait_for(|| !core.browsers()[0].linked).await;
    // Forgetting works only once it is gone.
    core.browser_forget(INSTALL);
    assert_eq!(core.browsers().len(), 1);
    drop(ext);
    wait_for(|| !core.browsers()[0].connected).await;
    core.browser_forget(INSTALL);
    assert!(core.browsers().is_empty());
}

#[tokio::test]
async fn the_vault_list_is_names_and_dates_only() {
    let (core, _owner) = linked_core().await;
    synth(&core, |method, _| match method {
        "vault.list" => Ok(json!({
            "sites": [
                {"site": "github.com", "has_totp": true, "updated": 1_791_700_000i64, "last_used": 1_791_800_000i64, "use_count": 3,
                 "password": "hunter2", "login": "ada"},
                {"site": "<b>bad</b>", "has_totp": false},
                {"site": "mail.example.org"},
                "junk",
            ],
            "devices": [{"id": "dev_aa11", "name": "Chrome on <macOS>", "created": 5, "last_seen": 6}, {"id": "nope"}],
            "password": "hunter2",
        })),
        _ => Err("unknown method".into()),
    });
    let list = core.vault_list().await.unwrap();
    assert_eq!(list.sites.len(), 2);
    assert_eq!(list.sites[0], VaultSite { site: "github.com".into(), has_totp: true, updated: 1_791_700_000, last_used: 1_791_800_000, use_count: 3 });
    assert_eq!(list.sites[1].site, "mail.example.org");
    assert_eq!(list.devices, [VaultDevice { id: "dev_aa11".into(), name: "Chrome on macOS".into(), created: 5, last_seen: 6 }]);
    // Whatever else a synth put there has no field to land in.
    assert!(!serde_json::to_string(&list).unwrap().contains("hunter2"));
}

#[tokio::test]
async fn forgetting_a_vault_login_goes_to_the_synth_and_into_the_journal() {
    let (core, _owner) = linked_core().await;
    let seen = usual_synth(&core);
    core.vault_forget("GitHub.com").await.unwrap();
    assert_eq!(seen.lock().unwrap()[0], ("vault.forget".to_string(), json!({"site": "github.com"})));
    assert!(journal_text(&core).contains("vault.forget"));
    assert!(core.vault_forget("not a site").await.is_err());
    assert_eq!(seen.lock().unwrap().len(), 1);
}

// --- the whole cookies.get, as the synth calls it ---

async fn cookies_get(core: &Core, domain: &str) -> Result<Value, String> {
    crate::rpc::call_for_test(&core.shared(), "cookies.get", json!({"domain": domain, "reason": "to check your orders"})).await
}

#[tokio::test]
async fn cookies_get_goes_through_the_linked_browser_and_logs_no_value() {
    let (core, owner) = linked_core().await;
    usual_synth(&core);
    core.mutate_settings(|s| {
        s.cookies.enabled = true;
        s.cookies.mode = CookiesMode::Ask;
    });
    core.set_cookies_extension_first(true);
    let mut ext = link(&core, &owner, INSTALL).await;

    let call = tokio::spawn({
        let core = core.clone();
        async move { cookies_get(&core, "shop.example.com").await }
    });
    let ask = ext.recv_type("cookies.get").await;
    assert_eq!(ask["site"], "example.com", "the browser is asked about the site, not the host");
    ext.send(cookie_reply(&ask["id"])).await;
    let got = call.await.unwrap().unwrap();
    assert_eq!(got["source"], "chrome-extension");
    assert_eq!(got["cookies"].as_array().unwrap().len(), 1);
    assert_eq!(got["cookies"][0]["value"], SECRET);

    // The owner was asked in the app FIRST, and told that the browser
    // asks next: one flow, two steps.
    let asked = owner.asked();
    let cookie_ask = asked.iter().find(|a| a.capability == "cookies").expect("asked in the app");
    assert_eq!(cookie_ask.scope, "example.com");
    assert_eq!(cookie_ask.details, [("browser_step".to_string(), "Chrome".to_string())]);

    // Journal: site, count, browser. Never a value.
    let journal = journal_text(&core);
    assert!(journal.contains("example.com (1 from chrome-extension)"), "{journal}");
    assert!(!journal.contains(SECRET));
}

#[tokio::test]
async fn the_browser_is_never_asked_when_the_owner_said_no_in_the_app() {
    let (core, owner) = linked_core().await;
    usual_synth(&core);
    core.mutate_settings(|s| s.cookies.enabled = true);
    core.set_cookies_extension_first(true);
    let mut ext = link(&core, &owner, INSTALL).await;
    owner.says(Approval::deny());
    let e = cookies_get(&core, "example.com").await.unwrap_err();
    assert_eq!(e, "denied: the owner did not allow cookies for example.com");
    assert!(ext.silent().await);

    // Cookies off, or the app paused: the same.
    core.mutate_settings(|s| s.cookies.enabled = false);
    assert!(cookies_get(&core, "example.com").await.unwrap_err().starts_with("denied: cookies are turned off"));
    core.mutate_settings(|s| s.cookies.enabled = true);
    core.pause().await;
    assert!(cookies_get(&core, "example.com").await.unwrap_err().starts_with("denied: cookies are paused"));
    assert!(ext.silent().await);
}

#[tokio::test(start_paused = true)]
async fn waiting_for_the_browser_ends_inside_the_synths_minute() {
    let (core, owner) = linked_core().await;
    usual_synth(&core);
    core.mutate_settings(|s| {
        s.cookies.enabled = true;
        s.cookies.mode = CookiesMode::Auto;
    });
    core.set_cookies_extension_first(true);
    let mut ext = link(&core, &owner, INSTALL).await;

    let started = tokio::time::Instant::now();
    let call = tokio::spawn({
        let core = core.clone();
        async move { cookies_get(&core, "example.com").await }
    });
    ext.recv_type("cookies.get").await; // and never answer
    let e = call.await.unwrap().unwrap_err();
    assert!(e.starts_with("denied: no answer from the user"), "{e}");
    assert!(started.elapsed() <= crate::grant::APPROVAL_TIMEOUT + Duration::from_secs(1), "{:?}", started.elapsed());
    assert!(journal_text(&core).contains("no answer"));
}

#[tokio::test]
async fn without_a_linked_browser_the_old_answers_stand() {
    let (core, owner) = linked_core().await;
    core.mutate_settings(|s| s.cookies.enabled = true);
    owner.says(Approval::once());
    // No file reader has anything and no browser is linked: an empty jar.
    for first in [false, true] {
        core.set_cookies_extension_first(first);
        let got = cookies_get(&core, "example.com").await.unwrap();
        assert_eq!(got, json!({"cookies": [], "source": ""}));
    }
    // And no browser step is announced when no browser could take it.
    assert!(owner.asked().iter().all(|a| a.details.is_empty()));
}

#[tokio::test]
async fn a_public_suffix_is_not_a_site() {
    let (core, owner) = linked_core().await;
    core.mutate_settings(|s| {
        s.cookies.enabled = true;
        s.cookies.mode = CookiesMode::Auto;
    });
    for bad in ["com", "co.uk", "github.io"] {
        let e = cookies_get(&core, bad).await.unwrap_err();
        assert_eq!(e, "bad request: params.domain must name one site", "{bad}");
    }
    assert!(owner.asked().is_empty());
    assert!(crate::core::is_one_site("example.com") && crate::core::is_one_site("192.168.1.1"));
}

// --- the Safari door ---

const SAFARI_ORIGIN: &str = "safari-web-extension://ai.snth.connect.safari/";
const SAFARI_INSTALL: &str = "fedcba9876543210fedcba9876543210";

/// Connect as the Safari extension's handler would: on the Safari door.
async fn open_on(core: &Core, door: Door, origin: &str) -> Ext {
    let (app_r, ext_w) = duplex(4 * 1024 * 1024);
    let (ext_r, app_w) = duplex(4 * 1024 * 1024);
    let core = core.clone();
    tokio::spawn(async move {
        core.serve_browser_on(Conn { reader: Box::new(app_r), writer: Box::new(app_w) }, door).await;
    });
    let mut ext = Ext { w: ext_w, lines: BufReader::new(ext_r).lines() };
    ext.send(json!({"type": "host.open", "v": 1, "origin": origin})).await;
    ext
}

async fn greet_safari(core: &Core, install: &str) -> (Ext, Value) {
    let mut ext = open_on(core, Door::Safari, SAFARI_ORIGIN).await;
    let mut hi = hello(install);
    hi["browser"] = json!("safari");
    ext.send(hi).await;
    let ack = ext.recv_type("hello_ack").await;
    (ext, ack)
}

async fn link_safari(core: &Core, owner: &Owner, install: &str) -> Ext {
    owner.says(Approval::once());
    let (mut ext, _) = greet_safari(core, install).await;
    ext.send(json!({"type": "pair.request", "id": 1})).await;
    let code = ext.recv_type("pair.code").await;
    ext.send(json!({"type": "pair.done", "id": code["id"], "ok": true, "device_id": "dev_5afa"})).await;
    wait_for(|| core.browsers().iter().any(|b| b.install == install && b.linked)).await;
    ext
}

#[tokio::test]
async fn each_door_takes_its_own_callers_and_no_others() {
    let (core, owner) = linked_core().await;
    // The Chromium extension's origin on the Safari door, Safari's on the
    // Chromium door, and strangers on the Safari door: closed unheard.
    for (door, origin) in [
        (Door::Safari, ORIGIN),
        (Door::Chromium, SAFARI_ORIGIN),
        (Door::Safari, "safari-web-extension://ai.snth.vault.Extension/"),
        (Door::Safari, "safari-web-extension://ai.snth.connect.safari"),
        (Door::Safari, "https://evil.example/"),
        (Door::Safari, ""),
    ] {
        let mut ext = open_on(&core, door, origin).await;
        assert!(ext.closed().await, "{door:?} {origin}");
    }
    assert!(core.browsers().is_empty());
    assert!(owner.asked().is_empty());
}

#[tokio::test]
async fn which_browser_it_is_follows_from_the_door_not_from_what_it_says() {
    let (core, _owner) = linked_core().await;
    // On the Safari door a caller is Safari even when it says Chrome.
    let mut ext = open_on(&core, Door::Safari, SAFARI_ORIGIN).await;
    ext.send(hello(SAFARI_INSTALL)).await;
    ext.recv_type("hello_ack").await;
    // On the Chromium door nobody is Safari.
    let mut other = open(&core, ORIGIN).await;
    let mut hi = hello(INSTALL);
    hi["browser"] = json!("safari");
    other.send(hi).await;
    other.recv_type("hello_ack").await;

    let got: Vec<(String, String)> = core.browsers().into_iter().map(|b| (b.install, b.browser)).collect();
    assert_eq!(got, [(INSTALL.to_string(), "browser".to_string()), (SAFARI_INSTALL.to_string(), "safari".to_string())]);
    assert_eq!(browser_name("safari"), "Safari");
}

#[tokio::test]
async fn safari_pairs_through_the_app_like_any_browser() {
    let (core, owner) = linked_core().await;
    let seen = usual_synth(&core);
    owner.says(Approval::once());
    let (mut ext, ack) = greet_safari(&core, SAFARI_INSTALL).await;
    assert_eq!((ack["linked"].as_bool(), ack["known"].as_bool()), (Some(true), Some(false)));

    ext.send(json!({"type": "pair.request", "id": 4})).await;
    let code = ext.recv_type("pair.code").await;
    assert_eq!((code["id"].as_u64(), code["code"].as_str()), (Some(4), Some("K7QW3MHX")));
    // The owner was asked in the app, by the browser's name.
    let asked = owner.asked();
    assert_eq!((asked[0].capability.as_str(), asked[0].scope.as_str()), ("vault", "Safari"));
    assert!(seen.lock().unwrap().iter().any(|(m, p)| m == "vault.pair_code" && p["browser"] == "safari"));

    ext.send(json!({"type": "pair.done", "id": 4, "ok": true, "device_id": "dev_5afa"})).await;
    wait_for(|| core.browsers().iter().any(|b| b.browser == "safari" && b.linked)).await;
    assert!(journal_text(&core).contains("Safari"));

    // And the owner's click in the app links it without a question.
    let (core, owner) = linked_core().await;
    usual_synth(&core);
    let (mut ext, _) = greet_safari(&core, SAFARI_INSTALL).await;
    let linking = tokio::spawn({
        let core = core.clone();
        async move { core.browser_link(SAFARI_INSTALL).await }
    });
    let code = ext.recv_type("pair.code").await;
    assert_eq!(code["id"], 0);
    assert_eq!(linking.await.unwrap(), Ok(()));
    assert!(owner.asked().is_empty());
}

#[tokio::test]
async fn safari_hands_over_one_sites_cookies_under_its_own_label() {
    let (core, owner) = linked_core().await;
    usual_synth(&core);
    core.mutate_settings(|s| {
        s.cookies.enabled = true;
        s.cookies.mode = CookiesMode::Ask;
    });
    // Safari's files are never read, so the extension is the only source:
    // it is asked whichever goes first.
    core.set_cookies_extension_first(false);
    let mut ext = link_safari(&core, &owner, SAFARI_INSTALL).await;

    owner.says(Approval::once());
    let call = tokio::spawn({
        let core = core.clone();
        async move { cookies_get(&core, "shop.example.com").await }
    });
    let ask = ext.recv_type("cookies.get").await;
    assert_eq!(ask["site"], "example.com");
    ext.send(cookie_reply(&ask["id"])).await;
    let got = call.await.unwrap().unwrap();
    assert_eq!(got["source"], "safari-extension");
    assert_eq!(got["cookies"].as_array().unwrap().len(), 1, "the cookie of another site was dropped");
    assert_eq!(ext.recv_type("site.revoke").await["site"], "example.com");

    let journal = journal_text(&core);
    assert!(journal.contains("example.com (1 from safari-extension)"), "{journal}");
    assert!(!journal.contains(SECRET));
}

#[tokio::test]
async fn a_browser_with_no_cookie_of_the_site_is_not_the_end_when_another_is_linked() {
    let (core, owner) = linked_core().await;
    usual_synth(&core);
    let mut chrome = link(&core, &owner, INSTALL).await;
    let mut safari = link_safari(&core, &owner, SAFARI_INSTALL).await;

    let reading = tokio::spawn({
        let shared = core.shared();
        async move { shared.cookies_via_extension("example.com", Duration::from_secs(5), true).await }
    });
    // Safari connected last, so it is asked first. The owner is not
    // signed in there: an empty answer.
    let ask = safari.recv_type("cookies.get").await;
    safari.send(json!({"type": "cookies.result", "id": ask["id"], "ok": true, "cookies": []})).await;
    // Chrome is asked next, and it has the sign-in.
    let ask = chrome.recv_type("cookies.get").await;
    chrome.send(cookie_reply(&ask["id"])).await;
    let ExtensionRead::Cookies(read) = reading.await.unwrap() else {
        panic!("expected cookies");
    };
    assert_eq!((read.source.as_str(), read.cookies.len()), ("chrome-extension", 1));

    // A "no" in the first browser is the owner's answer: nobody else is asked.
    let reading = tokio::spawn({
        let shared = core.shared();
        async move { shared.cookies_via_extension("example.com", Duration::from_secs(5), true).await }
    });
    let ask = safari.recv_type("cookies.get").await;
    safari.send(json!({"type": "cookies.result", "id": ask["id"], "ok": false, "error": "denied"})).await;
    assert!(matches!(reading.await.unwrap(), ExtensionRead::Denied(_)));
    assert!(chrome.recv_type("site.revoke").await["site"] == "example.com");
    assert!(chrome.silent().await, "the second browser was not asked after a refusal");

    // Nobody signed in anywhere: an empty read, from the first one asked.
    let reading = tokio::spawn({
        let shared = core.shared();
        async move { shared.cookies_via_extension("example.com", Duration::from_secs(5), true).await }
    });
    let ask = safari.recv_type("cookies.get").await;
    safari.send(json!({"type": "cookies.result", "id": ask["id"], "ok": true, "cookies": []})).await;
    let ask = chrome.recv_type("cookies.get").await;
    chrome.send(json!({"type": "cookies.result", "id": ask["id"], "ok": true, "cookies": []})).await;
    let ExtensionRead::Cookies(read) = reading.await.unwrap() else {
        panic!("expected an empty read");
    };
    assert!(read.cookies.is_empty());
    assert_eq!(read.source, "safari-extension");
}

#[tokio::test]
async fn a_browser_that_may_read_the_site_already_is_asked_first() {
    let (core, owner) = linked_core().await;
    usual_synth(&core);
    core.mutate_settings(|s| {
        s.cookies.enabled = true;
        s.cookies.mode = CookiesMode::Auto;
    });
    let mut chrome = link(&core, &owner, INSTALL).await;
    let mut safari = link_safari(&core, &owner, SAFARI_INSTALL).await;
    // Chrome says it may read the site; Safari (connected last) may not.
    chrome.send(json!({"type": "state", "paired": true, "sites": ["example.com"]})).await;
    wait_for(|| core.browsers().iter().any(|b| b.browser == "chrome" && b.sites == ["example.com"])).await;
    assert_eq!(core.shared().extension_for("example.com"), Some(("chrome".into(), true)));
    assert_eq!(core.shared().extension_for("other.org"), Some(("safari".into(), false)));

    let reading = tokio::spawn({
        let shared = core.shared();
        async move { shared.cookies_via_extension("example.com", Duration::from_secs(5), false).await }
    });
    let ask = chrome.recv_type("cookies.get").await;
    chrome.send(cookie_reply(&ask["id"])).await;
    assert!(matches!(reading.await.unwrap(), ExtensionRead::Cookies(r) if r.source == "chrome-extension"));
    assert!(safari.silent().await);
}

#[tokio::test]
async fn a_browser_set_to_every_site_is_shown_as_such() {
    let (core, owner) = linked_core().await;
    usual_synth(&core);
    let mut ext = link_safari(&core, &owner, SAFARI_INSTALL).await;
    assert!(!core.browsers()[0].all_sites);
    // The owner chose "Always Allow on Every Website" in Safari.
    ext.send(json!({"type": "state", "paired": true, "sites": [], "all_sites": true})).await;
    wait_for(|| core.browsers()[0].all_sites).await;
    // Then the browser asks nothing for a site: the app says so up front.
    assert_eq!(core.shared().extension_for("example.com"), Some(("safari".into(), true)));
    ext.send(json!({"type": "state", "all_sites": false})).await;
    wait_for(|| !core.browsers()[0].all_sites).await;
    // Anything but a plain yes is a no.
    ext.send(json!({"type": "state", "all_sites": "yes"})).await;
    assert!(ext.silent().await);
    assert!(!core.browsers()[0].all_sites);
}
