//! Several links at once: the migration from one link, links that do not
//! touch each other, grants and the journal per synth, and one synth at a
//! time on the computer.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use super::*;
use crate::core::{Core, CoreConfig, CuaAttach, LinkState, LEASE_QUIET, TOKEN_KEY};
use crate::grant::{Approval, ApprovalFuture, ApprovalRequest, Approver};
use crate::mcp::Gate;
use crate::pairing::{ClaimError, ClaimResult, HubClient, Pending};
use crate::secret::{MemSecretStore, SecretStore};
use crate::settings::{self, LinkSource, Preset, StoredBrowser, StoredGrant};

/// Answers "always" and remembers who asked.
#[derive(Default)]
struct Owner(Mutex<Vec<ApprovalRequest>>);

struct OwnerApprover(Arc<Owner>);
impl Approver for OwnerApprover {
    fn ask(&self, req: ApprovalRequest) -> ApprovalFuture {
        self.0 .0.lock().unwrap().push(req);
        Box::pin(async { Approval::always() })
    }
}

struct NoHub;
impl HubClient for NoHub {
    fn claim(&self, _hub: &str, _code: &str) -> Result<ClaimResult, ClaimError> {
        Err(ClaimError::NotFound)
    }
}

fn core_at(path: Option<std::path::PathBuf>, secret: Arc<MemSecretStore>, owner: Arc<Owner>) -> Core {
    Core::new(CoreConfig {
        settings_path: path,
        journal_path: None,
        approver: Arc::new(OwnerApprover(owner)),
        cookies: Arc::new(crate::cookies::MultiReader { chromium: vec![], firefox: vec![], keys: Arc::new(crate::cookies::fixtures::NoKey) }),
        apps: Arc::new(crate::apps::stub::StubBackend::offering(&[])),
        secret,
        hub: Arc::new(NoHub),
        app_version: "t".into(),
        cua_bin: "cua-driver-not-installed".into(),
        cua_private_root: None,
        cua_attach: CuaAttach::User,
    })
}

fn pending(id: &str, name: &str, source: LinkSource) -> Pending {
    Pending {
        synth_id: id.into(),
        synth_url: format!("https://{id}.snth.ai"),
        token: format!("tok-{id}"),
        synth_name: name.into(),
        source,
        hub_id: String::new(),
        linked_to_account: None,
        reason: None,
    }
}

/// A core linked to Mia and Maya.
fn two_links() -> (Core, Arc<MemSecretStore>, Arc<Owner>) {
    let secret = Arc::new(MemSecretStore::new());
    let owner = Arc::new(Owner::default());
    let core = core_at(None, secret.clone(), owner.clone());
    assert!(core.shared().add_link(&pending("mia", "Mia", LinkSource::Account)).unwrap(), "the first link");
    assert!(!core.shared().add_link(&pending("maya", "Maya", LinkSource::Code)).unwrap());
    (core, secret, owner)
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("connect-slots-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// settings.json as app 0.1.12 wrote it: one `identity`, grants and a
/// linked browser without a synth.
fn old_settings_file() -> serde_json::Value {
    let mut p = serde_json::to_value(settings::Persisted::fresh("dev-old".into())).unwrap();
    p["rev"] = json!(5);
    p["identity"] = json!({"synth_id": "mia", "synth_url": "https://mia.snth.ai", "synth_name": "Mia"});
    p["grants"] = json!([{"capability": "net", "scope": "youtube.com", "until": 0}]);
    p["browsers"] = json!([
        {"install": "0123456789abcdef0123456789abcdef", "browser": "chrome", "linked": true, "device_id": "dev_aa11", "last_seen": 5},
        {"install": "fedcba9876543210fedcba9876543210", "browser": "edge", "linked": false, "last_seen": 6},
    ]);
    p
}

#[tokio::test]
async fn one_link_of_an_older_app_becomes_the_first_of_the_list_once() {
    let dir = temp_dir("migrate");
    let path = dir.join("settings.json");
    std::fs::write(&path, serde_json::to_vec(&old_settings_file()).unwrap()).unwrap();
    let secret = Arc::new(MemSecretStore::new());
    secret.set(TOKEN_KEY, "tok-old").unwrap();

    let core = core_at(Some(path.clone()), secret.clone(), Arc::default());
    assert_eq!(core.link_ids(), ["mia"]);
    assert_eq!(core.links()[0].synth_name.as_deref(), Some("Mia"));
    assert_eq!(core.links()[0].source, LinkSource::Code);
    // The token moved to the synth's own key; the bare key is gone.
    assert_eq!(secret.get(&token_key("mia")).unwrap().as_deref(), Some("tok-old"));
    assert_eq!(secret.get(TOKEN_KEY).unwrap(), None);
    // The grant and the linked browser are Mia's now.
    assert!(core.shared().grants().allows("mia", "net", "youtube.com"));
    // The file is in the new shape, written at once.
    let written: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(written.get("identity").is_none());
    assert_eq!(written["identities"][0]["synth_id"], "mia");
    assert_eq!(written["grants"][0]["synth_id"], "mia");
    assert_eq!(written["browsers"][0]["synth_id"], "mia");
    assert!(written["browsers"][1].get("synth_id").is_none(), "an unlinked browser belongs to nobody");
    assert_eq!(written["rev"], 5, "a migration is not the owner's change");
    drop(core);

    // Again: nothing moves, nothing is lost.
    let core = core_at(Some(path.clone()), secret.clone(), Arc::default());
    assert_eq!(core.link_ids(), ["mia"]);
    assert_eq!(secret.get(&token_key("mia")).unwrap().as_deref(), Some("tok-old"));
    drop(core);

    // The app died after moving the token but before writing the file:
    // the old file is read again, the moved token is used, the bare key
    // (if it is still there) goes.
    std::fs::write(&path, serde_json::to_vec(&old_settings_file()).unwrap()).unwrap();
    secret.set(TOKEN_KEY, "tok-old").unwrap();
    let core = core_at(Some(path.clone()), secret.clone(), Arc::default());
    assert_eq!(core.link_ids(), ["mia"]);
    assert_eq!(secret.get(TOKEN_KEY).unwrap(), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_old_pairing_whose_token_is_gone_is_not_a_link() {
    let dir = temp_dir("migrate-notoken");
    let path = dir.join("settings.json");
    std::fs::write(&path, serde_json::to_vec(&old_settings_file()).unwrap()).unwrap();
    let core = core_at(Some(path), Arc::new(MemSecretStore::new()), Arc::default());
    assert!(core.link_ids().is_empty());
    assert!(!core.is_paired());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn links_survive_a_restart_each_with_its_own_token() {
    let dir = temp_dir("restart");
    let path = dir.join("settings.json");
    let secret = Arc::new(MemSecretStore::new());
    let core = core_at(Some(path.clone()), secret.clone(), Arc::default());
    core.shared().add_link(&pending("mia", "Mia", LinkSource::Account)).unwrap();
    core.shared().add_link(&pending("maya", "Maya", LinkSource::Code)).unwrap();
    drop(core);
    let core = core_at(Some(path), secret.clone(), Arc::default());
    assert_eq!(core.link_ids(), ["mia", "maya"]);
    let links = core.links();
    assert_eq!((links[0].source, links[1].source), (LinkSource::Account, LinkSource::Code));
    assert_eq!(secret.get(&token_key("maya")).unwrap().as_deref(), Some("tok-maya"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn two_links_stand_apart_and_unlinking_one_keeps_the_other_and_the_policy() {
    let (core, secret, _) = two_links();
    core.apply_preset(Preset::Careful);
    let sh = core.shared();
    let (mia, maya) = (sh.slot("mia").unwrap(), sh.slot("maya").unwrap());
    sh.set_slot_state(&mia, LinkState::Linked, "linked");
    sh.set_slot_state(&maya, LinkState::NeedsRepair, "synth refused: HTTP 401 (re-pair needed)");
    let links = core.links();
    assert_eq!((links[0].state, links[1].state), (LinkState::Linked, LinkState::NeedsRepair));
    assert_eq!(core.link_brief().link_state, LinkState::Linked);

    sh.grants().record("mia", "net", "a.com", Approval::always());
    sh.grants().record("maya", "net", "b.com", Approval::always());
    sh.persist_grants();
    {
        let mut p = sh.persisted.write().unwrap();
        for (install, synth) in [("a", "mia"), ("b", "maya")] {
            p.browsers.push(StoredBrowser {
                install: install.into(),
                browser: "chrome".into(),
                linked: true,
                device_id: format!("dev_{install}"),
                synth_id: synth.into(),
                ext_version: String::new(),
                last_seen: 1,
            });
        }
    }
    let rev = sh.settings_rev();

    core.unpair_synth("maya").await;
    assert_eq!(core.link_ids(), ["mia"]);
    assert_eq!(secret.get(&token_key("maya")).unwrap(), None);
    assert_eq!(secret.get(&token_key("mia")).unwrap().as_deref(), Some("tok-mia"));
    assert!(sh.grants().allows("mia", "net", "a.com"));
    assert!(!sh.grants().allows("maya", "net", "b.com"));
    let p = sh.persisted.read().unwrap().clone();
    assert_eq!(p.grants, [StoredGrant { synth_id: "mia".into(), capability: "net".into(), scope: "a.com".into(), until: 0 }]);
    assert_eq!(p.identities.len(), 1);
    let linked: Vec<(&str, bool)> = p.browsers.iter().map(|b| (b.install.as_str(), b.linked)).collect();
    assert_eq!(linked, [("a", true), ("b", false)], "only Maya's browser is unlinked");
    // The policy is the computer's, not a synth's.
    assert_eq!(sh.settings_rev(), rev);
    assert_eq!(core.settings().preset, Preset::Careful);
    assert_eq!(maya.state(), LinkState::Unpaired);
    assert!(maya.stopping());
}

#[tokio::test]
async fn a_new_code_for_a_linked_synth_replaces_its_token_not_the_link() {
    let (core, secret, _) = two_links();
    let mut again = pending("maya", "", LinkSource::Code);
    again.token = "tok-maya-2".into();
    assert!(!core.shared().add_link(&again).unwrap());
    assert_eq!(core.link_ids(), ["mia", "maya"]);
    assert_eq!(secret.get(&token_key("maya")).unwrap().as_deref(), Some("tok-maya-2"));
    assert_eq!(core.links()[1].synth_name.as_deref(), Some("Maya"), "the known name stays");
}

#[tokio::test]
async fn an_answer_is_given_to_the_synth_that_asked_and_named_in_the_window_and_journal() {
    let (core, _, owner) = two_links();
    core.apply_preset(Preset::Careful); // net = ask
    let sh = core.shared();
    let (mia, maya) = (sh.slot("mia").unwrap(), sh.slot("maya").unwrap());

    PEER.scope(mia.clone(), {
        let sh = sh.clone();
        async move { sh.net_allows("www.youtube.com", "a video").await.unwrap() }
    })
    .await;
    assert_eq!(owner.0.lock().unwrap().len(), 1);
    assert_eq!(owner.0.lock().unwrap()[0].synth.as_deref(), Some("Mia"));
    // Mia asks again: remembered.
    PEER.scope(mia.clone(), {
        let sh = sh.clone();
        async move { sh.net_allows("m.youtube.com", "").await.unwrap() }
    })
    .await;
    assert_eq!(owner.0.lock().unwrap().len(), 1);
    // Maya asks for the same site: her own question.
    PEER.scope(maya.clone(), {
        let sh = sh.clone();
        async move { sh.net_allows("youtube.com", "").await.unwrap() }
    })
    .await;
    assert_eq!(owner.0.lock().unwrap().len(), 2);
    assert_eq!(owner.0.lock().unwrap()[1].synth.as_deref(), Some("Maya"));
    tokio::time::sleep(Duration::from_millis(50)).await;
    let rows = core.grant_list();
    assert!(rows.iter().any(|g| g.synth_id == "mia" && g.scope == "youtube.com"));
    assert!(rows.iter().any(|g| g.synth_id == "maya" && g.scope == "youtube.com"));
    // Taking Maya's back leaves Mia's.
    core.revoke(Some("maya"), "net", "youtube.com");
    assert!(sh.grants().allows("mia", "net", "youtube.com"));
    assert!(!sh.grants().allows("maya", "net", "youtube.com"));

    // A journal line written for a synth's stream names that synth.
    PEER.scope(maya, {
        let sh = sh.clone();
        async move { sh.journal().record(crate::journal::Entry::new("net", "dial", "example.com:443", "ok")) }
    })
    .await;
    sh.journal().record(crate::journal::Entry::new("app", "update", "-", "ok"));
    let recent = core.recent_journal();
    assert_eq!(recent[recent.len() - 2].synth.as_deref(), Some("Maya"));
    assert_eq!(recent[recent.len() - 1].synth, None);
}

fn call(tool: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": tool, "arguments": {"pid": 1, "x": 1, "y": 1}}})).unwrap()
}

async fn gate(sh: &Arc<crate::core::Shared>, slot: &Arc<LinkSlot>, tool: &str) -> Gate {
    PEER.scope(slot.clone(), {
        let sh = sh.clone();
        let line = call(tool);
        async move { crate::mcp::filter_line(&sh, false, &line).await }
    })
    .await
}

fn refused_with(g: &Gate) -> Option<String> {
    match g {
        Gate::Refuse(v) => Some(v["error"]["message"].as_str().unwrap_or("").to_string()),
        _ => None,
    }
}

#[tokio::test]
async fn one_synth_drives_the_computer_at_a_time_and_another_may_only_look() {
    let (core, _, _) = two_links();
    // These calls now also pass through the live computer-enabled gate.
    core.mutate_settings(|s| s.computer.enabled = true);
    let sh = core.shared();
    let (mia, maya) = (sh.slot("mia").unwrap(), sh.slot("maya").unwrap());

    assert!(matches!(gate(&sh, &mia, "click").await, Gate::Forward(_)));
    assert_eq!(sh.computer_driver().map(|(_, n)| n).as_deref(), Some("Mia"));
    assert_eq!(core.snapshot().await.computer_driver.as_deref(), Some("Mia"));

    // Maya's action is refused in one line that names the driver...
    let said = refused_with(&gate(&sh, &maya, "type_text").await).expect("refused");
    assert_eq!(said, "unavailable: Mia is controlling this computer right now; try again in a minute");
    assert!(serde_json::to_string(&core.recent_journal()).unwrap().contains("denied: Mia is controlling the computer"));
    // ...while she may look.
    assert!(matches!(gate(&sh, &maya, "get_window_state").await, Gate::Forward(_)));
    // Mia keeps acting.
    assert!(matches!(gate(&sh, &mia, "press_key").await, Gate::Forward(_)));

    // A minute of quiet: the computer is free, Maya takes it.
    sh.lease_age(LEASE_QUIET + Duration::from_secs(1));
    assert!(sh.computer_driver().is_none());
    assert!(matches!(gate(&sh, &maya, "click").await, Gate::Forward(_)));
    assert!(refused_with(&gate(&sh, &mia, "click").await).unwrap().contains("Maya"));

    // Unlinking the driver frees the computer at once.
    core.unpair_synth("maya").await;
    assert!(sh.computer_driver().is_none());
    assert!(matches!(gate(&sh, &mia, "click").await, Gate::Forward(_)));
}

#[tokio::test]
async fn a_single_link_is_found_without_a_stream_and_several_are_not() {
    let secret = Arc::new(MemSecretStore::new());
    let core = core_at(None, secret, Arc::default());
    assert!(core.shared().current_slot().is_none());
    core.shared().add_link(&pending("mia", "Mia", LinkSource::Code)).unwrap();
    assert_eq!(core.shared().current_synth_id(), "mia");
    core.shared().add_link(&pending("maya", "Maya", LinkSource::Code)).unwrap();
    assert_eq!(core.shared().current_synth_id(), "", "with two links a host call is nobody's");
}

/// A keychain whose writes fail while `fail_sets` is above zero.
struct FlakySecret {
    inner: MemSecretStore,
    fail_sets: Mutex<usize>,
}

impl SecretStore for FlakySecret {
    fn set(&self, key: &str, value: &str) -> std::io::Result<()> {
        let mut left = self.fail_sets.lock().unwrap();
        if *left > 0 {
            *left -= 1;
            return Err(std::io::Error::other("the keychain said no"));
        }
        self.inner.set(key, value)
    }
    fn get(&self, key: &str) -> std::io::Result<Option<String>> {
        self.inner.get(key)
    }
    fn delete(&self, key: &str) -> std::io::Result<()> {
        self.inner.delete(key)
    }
}

fn core_with(path: std::path::PathBuf, secret: Arc<FlakySecret>) -> Core {
    Core::new(CoreConfig {
        settings_path: Some(path),
        journal_path: None,
        approver: Arc::new(OwnerApprover(Arc::default())),
        cookies: Arc::new(crate::cookies::MultiReader { chromium: vec![], firefox: vec![], keys: Arc::new(crate::cookies::fixtures::NoKey) }),
        apps: Arc::new(crate::apps::stub::StubBackend::offering(&[])),
        secret,
        hub: Arc::new(NoHub),
        app_version: "t".into(),
        cua_bin: "cua-driver-not-installed".into(),
        cua_private_root: None,
        cua_attach: CuaAttach::User,
    })
}

#[tokio::test]
async fn a_token_the_keychain_would_not_move_keeps_the_link_until_a_later_start_moves_it() {
    let dir = temp_dir("migrate-flaky");
    let path = dir.join("settings.json");
    std::fs::write(&path, serde_json::to_vec(&old_settings_file()).unwrap()).unwrap();
    // Every write fails during the first start (the move is tried twice).
    let secret = Arc::new(FlakySecret { inner: MemSecretStore::new(), fail_sets: Mutex::new(2) });
    secret.inner.set(TOKEN_KEY, "tok-old").unwrap();

    let core = core_with(path.clone(), secret.clone());
    assert_eq!(core.link_ids(), ["mia"], "the link is there, on the old key");
    let mia = core.shared().slot("mia").unwrap();
    assert_eq!(core.shared().token_of(&mia).as_deref(), Some("tok-old"));
    assert_eq!(secret.get(TOKEN_KEY).unwrap().as_deref(), Some("tok-old"), "the bare key is kept");
    assert_eq!(secret.get(&token_key("mia")).unwrap(), None);
    // An ordinary save writes the new shape: no `identity` any more.
    core.apply_preset(Preset::Careful);
    let written: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(written.get("identity").is_none());
    drop((mia, core));

    // The next start: the keychain works again; the token moves.
    let core = core_with(path.clone(), secret.clone());
    assert_eq!(core.link_ids(), ["mia"]);
    assert_eq!(secret.get(&token_key("mia")).unwrap().as_deref(), Some("tok-old"));
    assert_eq!(secret.get(TOKEN_KEY).unwrap(), None);
    drop(core);

    // Still failing at the next start: the link stays on the old key, and
    // unlinking it removes that key too.
    std::fs::write(&path, serde_json::to_vec(&old_settings_file()).unwrap()).unwrap();
    secret.inner.delete(&token_key("mia")).unwrap();
    secret.inner.set(TOKEN_KEY, "tok-old").unwrap();
    *secret.fail_sets.lock().unwrap() = 2;
    let core = core_with(path.clone(), secret.clone());
    assert_eq!(core.link_ids(), ["mia"]);
    core.unpair_synth("mia").await;
    assert_eq!(secret.get(TOKEN_KEY).unwrap(), None);
    assert!(core.link_ids().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_bare_token_is_not_given_to_one_of_several_pairings() {
    let dir = temp_dir("bare-several");
    let path = dir.join("settings.json");
    let secret = Arc::new(MemSecretStore::new());
    let core = core_at(Some(path.clone()), secret.clone(), Arc::default());
    core.shared().add_link(&pending("mia", "Mia", LinkSource::Code)).unwrap();
    core.shared().add_link(&pending("maya", "Maya", LinkSource::Code)).unwrap();
    drop(core);
    secret.delete(&token_key("maya")).unwrap();
    secret.set(TOKEN_KEY, "tok-stray").unwrap();
    let core = core_at(Some(path), secret.clone(), Arc::default());
    assert_eq!(core.link_ids(), ["mia"]);
    assert_eq!(secret.get(TOKEN_KEY).unwrap().as_deref(), Some("tok-stray"));
    let _ = std::fs::remove_dir_all(&dir);
}
