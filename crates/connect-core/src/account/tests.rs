//! The account from the outside: a fake platform answers, a scripted owner
//! approves, and a tiny local server plays a synth that refuses tokens.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::grant::{Approval, ApprovalFuture, ApprovalRequest, Approver};
use crate::pairing::{ClaimError, ClaimResult, HubClient};
use crate::secret::{MemSecretStore, SecretStore};
use crate::{CoreConfig, CuaAttach};

const BASE: &str = "https://platform.test";

struct Yes;
impl Approver for Yes {
    fn ask(&self, _req: ApprovalRequest) -> ApprovalFuture {
        Box::pin(async { Approval::always() })
    }
}

/// The hub of a bot's code: one synth, "kai".
struct Hub;
impl HubClient for Hub {
    fn claim(&self, _hub: &str, _code: &str) -> Result<ClaimResult, ClaimError> {
        Ok(ClaimResult { synth_url: "https://kai.snth.ai".into(), synth_id: "kai".into(), companion_token: "tok-kai".into() })
    }
}

/// The platform, scripted.
#[derive(Default)]
struct Fake {
    synths: Mutex<Vec<AccountSynth>>,
    /// `/me` and `/connect` answer 401 (the owner removed this computer).
    revoked: Mutex<bool>,
    /// Per synth: an error `/connect` answers instead of a token.
    refuse: Mutex<Vec<(String, PlatformError)>>,
    /// Synth urls by id (for the link tests: a local server).
    urls: Mutex<Vec<(String, String)>>,
    minted: AtomicUsize,
    exchanged: Mutex<Vec<(String, String, String)>>,
    logouts: AtomicUsize,
    link_code_linked: Mutex<bool>,
}

impl Fake {
    fn with(synths: &[(&str, &str)]) -> Arc<Fake> {
        let fake = Fake::default();
        *fake.synths.lock().unwrap() = synths
            .iter()
            .map(|(id, name)| AccountSynth { hub_id: "hub-1".into(), synth_id: id.to_string(), name: name.to_string(), avatar_url: None })
            .collect();
        *fake.link_code_linked.lock().unwrap() = true;
        Arc::new(fake)
    }
    fn url_of(&self, id: &str) -> String {
        self.urls
            .lock()
            .unwrap()
            .iter()
            .find(|(i, _)| i == id)
            .map(|(_, u)| u.clone())
            .unwrap_or_else(|| format!("https://{id}.snth.ai"))
    }
}

impl PlatformClient for Fake {
    fn exchange(&self, base: &str, code: &str, verifier: &str, device_id: &str) -> Result<TokenAnswer, PlatformError> {
        assert_eq!(base, BASE);
        self.exchanged.lock().unwrap().push((code.into(), verifier.into(), device_id.into()));
        if code != "cac_good" {
            return Err(PlatformError::Status { status: 400, error: "invalid_code".into(), reason: None });
        }
        Ok(TokenAnswer {
            token: "snthapp_0123".into(),
            account: AccountInfo { id: "acc_1".into(), email: "sasha@example.com".into(), name: "Sasha".into() },
        })
    }
    fn me(&self, _base: &str, token: &str) -> Result<Me, PlatformError> {
        assert_eq!(token, "snthapp_0123");
        if *self.revoked.lock().unwrap() {
            return Err(PlatformError::Unauthorized);
        }
        Ok(Me {
            account: AccountInfo { id: "acc_1".into(), email: "sasha@example.com".into(), name: "Sasha".into() },
            synths: self.synths.lock().unwrap().clone(),
        })
    }
    fn connect(&self, _base: &str, _token: &str, hub_id: &str, synth_id: &str) -> Result<Minted, PlatformError> {
        if *self.revoked.lock().unwrap() {
            return Err(PlatformError::Unauthorized);
        }
        assert_eq!(hub_id, "hub-1");
        if let Some((_, e)) = self.refuse.lock().unwrap().iter().find(|(id, _)| id == synth_id) {
            return Err(e.clone());
        }
        let n = self.minted.fetch_add(1, Ordering::AcqRel) + 1;
        Ok(Minted {
            synth_id: synth_id.into(),
            synth_url: self.url_of(synth_id),
            synth_name: String::new(),
            companion_token: format!("tok-{synth_id}-{n}"),
        })
    }
    fn link_code(&self, _base: &str, _token: &str, code: &str) -> Result<LinkCodeAnswer, PlatformError> {
        if code == "404404" {
            return Err(PlatformError::Status { status: 404, error: "code_not_found".into(), reason: None });
        }
        let linked = *self.link_code_linked.lock().unwrap();
        Ok(LinkCodeAnswer {
            synth_id: "zoe".into(),
            synth_url: "https://zoe.snth.ai".into(),
            synth_name: "Zoe".into(),
            companion_token: "tok-zoe".into(),
            linked,
            reason: (!linked).then(|| "synth_linked_elsewhere".into()),
        })
    }
    fn logout(&self, _base: &str, _token: &str) -> Result<(), PlatformError> {
        self.logouts.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}

fn new_core(fake: Arc<Fake>) -> (Core, Arc<MemSecretStore>) {
    let secret = Arc::new(MemSecretStore::new());
    let core = Core::new(CoreConfig {
        settings_path: None,
        journal_path: None,
        approver: Arc::new(Yes),
        cookies: Arc::new(crate::cookies::MultiReader { chromium: vec![], firefox: vec![], keys: Arc::new(crate::cookies::fixtures::NoKey) }),
        apps: Arc::new(crate::apps::stub::StubBackend::offering(&[])),
        secret: secret.clone(),
        hub: Arc::new(Hub),
        app_version: "0.1.13".into(),
        cua_bin: "cua-driver-not-installed".into(),
        cua_private_root: None,
        cua_attach: CuaAttach::User,
    });
    core.set_platform(fake, BASE);
    (core, secret)
}

fn param<'a>(url: &'a str, key: &str) -> &'a str {
    let q = url.split_once('?').unwrap().1;
    q.split('&').find_map(|p| p.strip_prefix(&format!("{key}="))).unwrap_or_else(|| panic!("no {key} in {url}"))
}

/// Sign in all the way (the browser said yes).
async fn signed_in(fake: Arc<Fake>) -> (Core, Arc<MemSecretStore>) {
    let (core, secret) = new_core(fake);
    let url = core.login_url().unwrap();
    let state = param(&url, "state").to_string();
    core.login_complete(Ok("cac_good".into()), &state).await.unwrap();
    (core, secret)
}

fn journal(core: &Core) -> String {
    serde_json::to_string(&core.recent_journal()).unwrap()
}

#[tokio::test]
async fn the_sign_in_address_carries_a_fresh_state_and_the_s256_challenge() {
    let fake = Fake::with(&[]);
    let (core, _) = new_core(fake.clone());
    let url = core.login_url().unwrap();
    assert!(url.starts_with("https://platform.test/connect/login?state="), "{url}");
    let state = param(&url, "state");
    assert!(state.len() >= 43, "32 random bytes, b64url");
    assert_eq!(param(&url, "device_id"), core.shared().device_id());
    assert_eq!(param(&url, "app_version"), "0.1.13");
    assert!(["macos", "windows", "linux"].contains(&param(&url, "os")));
    assert!(!param(&url, "device_name").contains(' '), "the name is url-encoded");
    // "Open again" while waiting: the same sign-in, not a second one.
    assert_eq!(core.login_url().unwrap(), url);
    assert!(matches!(core.account_view().login, LoginPhase::Waiting { .. }));

    // The verifier the exchange sends matches the challenge in the address.
    let challenge = param(&url, "code_challenge").to_string();
    core.login_complete(Ok("cac_good".into()), param(&url, "state")).await.unwrap();
    let (code, verifier, device) = fake.exchanged.lock().unwrap()[0].clone();
    assert_eq!(code, "cac_good");
    assert_eq!(challenge_of(&verifier), challenge);
    assert_eq!(device, core.shared().device_id());
    // A new sign-in has a new state.
    let again = core.login_url().unwrap();
    assert_ne!(param(&again, "state"), state_of(&url));
}

fn state_of(url: &str) -> String {
    param(url, "state").to_string()
}

#[tokio::test]
async fn a_login_link_with_another_state_or_none_waiting_is_ignored_and_journaled() {
    let fake = Fake::with(&[("mia", "Mia")]);
    let (core, secret) = new_core(fake.clone());
    // Nobody started a sign-in.
    assert_eq!(core.login_complete(Ok("cac_good".into()), "forged").await, Err("state_mismatch".into()));
    let url = core.login_url().unwrap();
    // Another state: ignored, the real sign-in keeps waiting.
    assert_eq!(core.login_complete(Ok("cac_good".into()), "forged").await, Err("state_mismatch".into()));
    assert!(fake.exchanged.lock().unwrap().is_empty(), "nothing was exchanged");
    assert!(secret.get(ACCOUNT_TOKEN_KEY).unwrap().is_none());
    assert!(matches!(core.account_view().login, LoginPhase::Waiting { .. }));
    assert!(journal(&core).contains("ignored: a sign-in link this app did not start"));
    // The real one still works.
    core.login_complete(Ok("cac_good".into()), &state_of(&url)).await.unwrap();
    // And a state is good once.
    assert_eq!(core.login_complete(Ok("cac_good".into()), &state_of(&url)).await, Err("state_mismatch".into()));
}

#[tokio::test]
async fn cancel_in_the_browser_or_a_bad_code_signs_nobody_in() {
    let fake = Fake::with(&[]);
    let (core, secret) = new_core(fake.clone());
    let url = core.login_url().unwrap();
    assert_eq!(core.login_complete(Err("denied".into()), &state_of(&url)).await, Err("denied".into()));
    assert_eq!(core.account_view().login, LoginPhase::Failed { code: "denied".into() });
    let url = core.login_url().unwrap();
    assert_eq!(core.login_complete(Ok("cac_bad".into()), &state_of(&url)).await, Err("exchange_failed".into()));
    assert!(secret.get(ACCOUNT_TOKEN_KEY).unwrap().is_none());
    assert!(!core.account_view().signed_in);
}

#[tokio::test]
async fn signed_in_the_owner_confirms_and_every_synth_of_the_account_is_linked() {
    let fake = Fake::with(&[("mia", "Mia"), ("maya", "Maya")]);
    let (core, secret) = signed_in(fake.clone()).await;
    let view = core.account_view();
    assert!(view.signed_in);
    assert_eq!(view.account.as_ref().unwrap().email, "sasha@example.com");
    assert_eq!(secret.get(ACCOUNT_TOKEN_KEY).unwrap().as_deref(), Some("snthapp_0123"));
    // Nothing is linked before the owner confirms.
    assert!(!view.consented);
    assert_eq!(view.synths.as_ref().unwrap().len(), 2);
    assert!(core.link_ids().is_empty());
    assert_eq!(fake.minted.load(Ordering::Acquire), 0);

    let report = core.account_connect().await.unwrap();
    assert!(report.first_link);
    assert_eq!(report.added, ["Mia", "Maya"]);
    assert_eq!(core.link_ids(), ["mia", "maya"]);
    for l in core.links() {
        assert_eq!(l.source, LinkSource::Account);
        assert_eq!(l.hub_id.as_deref(), Some("hub-1"));
    }
    assert_eq!(secret.get(&crate::slot::token_key("mia")).unwrap().as_deref(), Some("tok-mia-1"));
    assert_eq!(secret.get(&crate::slot::token_key("maya")).unwrap().as_deref(), Some("tok-maya-2"));
    let view = core.account_view();
    assert!(view.consented);
    assert!(view.synths.unwrap().iter().all(|s| s.linked));
}

#[tokio::test]
async fn the_sync_links_new_synths_drops_gone_ones_and_skips_what_the_owner_disconnected() {
    let fake = Fake::with(&[("mia", "Mia")]);
    let (core, _) = signed_in(fake.clone()).await;
    core.account_connect().await.unwrap();
    // Maya joins the account, Mia leaves it.
    *fake.synths.lock().unwrap() = vec![AccountSynth { hub_id: "hub-1".into(), synth_id: "maya".into(), name: "Maya".into(), avatar_url: None }];
    let report = core.account_sync().await;
    assert_eq!(report.added, ["Maya"]);
    assert_eq!(report.removed, ["Mia"]);
    assert_eq!(core.link_ids(), ["maya"]);

    // The owner disconnects Maya here: the next sync leaves her alone.
    core.account_skip("maya");
    core.unpair_synth("maya").await;
    let report = core.account_sync().await;
    assert!(report.added.is_empty());
    assert!(core.link_ids().is_empty());
    let view = core.account_view();
    assert!(view.synths.unwrap()[0].skipped);
    // Connecting again from the app clears that.
    core.account_connect().await.unwrap();
    assert_eq!(core.link_ids(), ["maya"]);
}

#[tokio::test]
async fn a_synth_that_cannot_be_linked_says_why_and_the_others_are_linked() {
    let fake = Fake::with(&[("mia", "Mia"), ("old", "Old")]);
    fake.refuse.lock().unwrap().push((
        "old".into(),
        PlatformError::Status { status: 409, error: "connect_unavailable".into(), reason: Some("Connect is off".into()) },
    ));
    let (core, _) = signed_in(fake.clone()).await;
    let report = core.account_connect().await.unwrap();
    assert_eq!(report.added, ["Mia"]);
    let old = core.account_view().synths.unwrap().into_iter().find(|s| s.synth_id == "old").unwrap();
    assert!(!old.linked);
    assert_eq!(old.error.as_deref(), Some("connect_unavailable"));
    assert_eq!(old.reason.as_deref(), Some("Connect is off"));
}

#[tokio::test]
async fn a_revoked_app_token_signs_out_and_drops_only_the_account_links() {
    let fake = Fake::with(&[("mia", "Mia")]);
    let (core, secret) = signed_in(fake.clone()).await;
    core.account_connect().await.unwrap();
    // A synth linked with a bot's code (not in the account).
    *fake.link_code_linked.lock().unwrap() = false;
    core.pair_begin("https://hub".into(), "123456".into()).await.unwrap();
    core.pair_confirm().unwrap();
    assert_eq!(core.link_ids(), ["mia", "zoe"]);
    assert_eq!(core.shared().slot("zoe").unwrap().source(), LinkSource::Code);

    *fake.revoked.lock().unwrap() = true;
    let report = core.account_sync().await;
    assert!(report.signed_out);
    assert_eq!(report.removed, ["Mia"]);
    assert_eq!(core.link_ids(), ["zoe"], "the code link stays");
    assert!(secret.get(ACCOUNT_TOKEN_KEY).unwrap().is_none());
    assert!(secret.get(&crate::slot::token_key("mia")).unwrap().is_none());
    assert!(!core.account_view().signed_in);
    // Signed out, a code goes to the hub again.
    let p = core.pair_begin("https://hub".into(), "123456".into()).await.unwrap();
    assert_eq!(p.synth_id, "kai");
    assert_eq!(p.linked_to_account, None);
}

#[tokio::test]
async fn sign_out_tells_the_platform_and_keeps_code_links() {
    let fake = Fake::with(&[("mia", "Mia")]);
    let (core, _) = new_core(fake.clone());
    // A code link first, before signing in.
    core.pair_begin("https://hub".into(), "123456".into()).await.unwrap();
    core.pair_confirm().unwrap();
    let url = core.login_url().unwrap();
    core.login_complete(Ok("cac_good".into()), &state_of(&url)).await.unwrap();
    let report = core.account_connect().await.unwrap();
    assert!(!report.first_link, "kai was linked already");
    assert_eq!(core.link_ids(), ["kai", "mia"]);
    let gone = core.sign_out().await;
    assert_eq!(gone, ["Mia"]);
    assert_eq!(fake.logouts.load(Ordering::Acquire), 1);
    assert_eq!(core.link_ids(), ["kai"]);
}

#[tokio::test]
async fn a_code_link_to_a_synth_of_the_account_becomes_its_account_link_without_a_new_token() {
    let fake = Fake::with(&[("kai", "Kai")]);
    let (core, secret) = new_core(fake.clone());
    core.pair_begin("https://hub".into(), "123456".into()).await.unwrap();
    core.pair_confirm().unwrap();
    let url = core.login_url().unwrap();
    core.login_complete(Ok("cac_good".into()), &state_of(&url)).await.unwrap();
    core.account_connect().await.unwrap();
    // One link per synth, and its token is the one it had.
    assert_eq!(core.link_ids(), ["kai"]);
    let slot = core.shared().slot("kai").unwrap();
    assert_eq!(slot.source(), LinkSource::Account);
    assert_eq!(slot.hub_id(), "hub-1");
    assert_eq!(secret.get(&crate::slot::token_key("kai")).unwrap().as_deref(), Some("tok-kai"));
    assert_eq!(fake.minted.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn a_code_while_signed_in_goes_through_the_account() {
    let fake = Fake::with(&[]);
    let (core, _) = signed_in(fake.clone()).await;
    let p = core.pair_begin("https://hub".into(), "123456".into()).await.unwrap();
    assert_eq!((p.synth_id.as_str(), p.synth_name.as_str()), ("zoe", "Zoe"));
    assert_eq!(p.linked_to_account, Some(true));
    assert_eq!(p.source, LinkSource::Account);
    // Another account's synth: still linked here, as a code link.
    *fake.link_code_linked.lock().unwrap() = false;
    let p = core.pair_begin("https://hub".into(), "123456".into()).await.unwrap();
    assert_eq!(p.linked_to_account, Some(false));
    assert_eq!(p.reason.as_deref(), Some("synth_linked_elsewhere"));
    assert_eq!(p.source, LinkSource::Code);
    let e = core.pair_begin("https://hub".into(), "404404".into()).await.unwrap_err();
    assert!(e.starts_with("code_not_found:"), "{e}");
}

// --- a synth that refuses the token (HTTP 401) ---

/// A local "synth" that answers every WebSocket dial with 401 and counts
/// the dials.
async fn refusing_synth() -> (String, Arc<AtomicUsize>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dials = Arc::new(AtomicUsize::new(0));
    let count = dials.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            count.fetch_add(1, Ordering::AcqRel);
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let _ = sock
                .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
        }
    });
    (format!("http://{addr}"), dials)
}

async fn wait_state(core: &Core, id: &str, want: LinkState) {
    for _ in 0..200 {
        if core.shared().slot(id).is_some_and(|s| s.state() == want) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{id} never reached {want:?}");
}

#[tokio::test]
async fn a_refused_code_link_stops_dialing_and_waits_for_a_new_code() {
    let (url, dials) = refusing_synth().await;
    let fake = Fake::with(&[]);
    let (core, _) = new_core(fake);
    core.shared()
        .add_link(&Pending {
            synth_id: "kai".into(),
            synth_url: url.clone(),
            token: "tok-old".into(),
            synth_name: "Kai".into(),
            source: LinkSource::Code,
            hub_id: String::new(),
            linked_to_account: None,
            reason: None,
        })
        .unwrap();
    let (stop, stop_rx) = tokio::sync::watch::channel(false);
    let run = tokio::spawn({
        let core = core.clone();
        async move { core.run_slot("kai", stop_rx).await }
    });
    wait_state(&core, "kai", LinkState::NeedsRepair).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(dials.load(Ordering::Acquire), 1, "no redial after a 401");
    assert_eq!(core.reconnect_synth("kai").await, Err("needs_code".into()));
    // A new token (a new code from the bot) wakes it: one more dial.
    core.shared()
        .add_link(&Pending {
            synth_id: "kai".into(),
            synth_url: url,
            token: "tok-new".into(),
            synth_name: String::new(),
            source: LinkSource::Code,
            hub_id: String::new(),
            linked_to_account: None,
            reason: None,
        })
        .unwrap();
    for _ in 0..200 {
        if dials.load(Ordering::Acquire) >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    wait_state(&core, "kai", LinkState::NeedsRepair).await;
    assert_eq!(dials.load(Ordering::Acquire), 2);
    let _ = stop.send(true);
    let _ = run.await;
}

#[tokio::test]
async fn a_refused_account_link_gets_one_fresh_token_then_waits() {
    let (url, dials) = refusing_synth().await;
    let fake = Fake::with(&[("mia", "Mia")]);
    fake.urls.lock().unwrap().push(("mia".into(), url));
    let (core, secret) = signed_in(fake.clone()).await;
    core.account_connect().await.unwrap();
    assert_eq!(fake.minted.load(Ordering::Acquire), 1);
    let (stop, stop_rx) = tokio::sync::watch::channel(false);
    let run = tokio::spawn({
        let core = core.clone();
        async move { core.run_slot("mia", stop_rx).await }
    });
    wait_state(&core, "mia", LinkState::NeedsRepair).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    // The first token was refused, one fresh one was asked for and
    // refused too; then nothing more.
    assert_eq!(fake.minted.load(Ordering::Acquire), 2);
    assert_eq!(dials.load(Ordering::Acquire), 2);
    assert_eq!(secret.get(&crate::slot::token_key("mia")).unwrap().as_deref(), Some("tok-mia-2"));
    // The owner's "Connect again": one more token, one more dial.
    core.reconnect_synth("mia").await.unwrap();
    for _ in 0..200 {
        if dials.load(Ordering::Acquire) >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    wait_state(&core, "mia", LinkState::NeedsRepair).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(dials.load(Ordering::Acquire), 3);
    assert_eq!(fake.minted.load(Ordering::Acquire), 3);
    let _ = stop.send(true);
    let _ = run.await;
}
