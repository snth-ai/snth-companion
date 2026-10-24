//! The owner's platform account (platform.snth.ai).
//!
//! The app signs in to the owner's account and, through it, links this
//! computer to every synth of the account at once. The code from a
//! synth's bot stays as a second way in (`Core::pair_begin`).
//!
//! Sign-in is OAuth-like with PKCE, back through the app's own `snth://`
//! scheme:
//!
//!  1. `Core::login_url` makes a `state` and a PKCE verifier (32 random
//!     bytes each, memory only, 10 minutes) and returns the platform's
//!     `/connect/login` address with the S256 challenge; the host opens it
//!     in the browser.
//!  2. The owner allows it there; the browser opens
//!     `snth://login?code=...&state=...` (or `?error=denied&state=...`).
//!  3. `Core::login_complete` checks the state (a link with another state,
//!     or with no sign-in waiting, is ignored and journaled), exchanges the
//!     code with the verifier for the app's own token (`snthapp_...`, kept
//!     in the SecretStore under `account_token`, never shown) and reads
//!     the account's synths.
//!  4. The owner confirms in the app; `Core::account_connect` asks the
//!     platform for a Connect token per synth and links each one.
//!
//! The app's token is good for `/api/app/v1/*` of the platform only. It is
//! not the platform's browser session and not a synth's portal token.
//!
//! The sync (`Core::account_sync`, run by the host at start, every 10
//! minutes and when the window comes up) keeps the links in step with the
//! account: a new synth of the account is linked (once the owner has
//! confirmed on this computer), a synth that left the account is unlinked,
//! and a refused app token (the owner removed this computer in the
//! platform) signs the app out: every link made through the account goes,
//! links made with a bot's code stay.
//!
//! Every platform call is behind `PlatformClient`, so tests use a fake.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::core::{Core, CoreEvent, LinkState, Shared};
use crate::journal::Entry;
use crate::pairing::Pending;
use crate::settings::{LinkSource, StoredAccount};
use crate::slot::LinkSlot;

/// The keychain key the app's platform token is stored under.
pub const ACCOUNT_TOKEN_KEY: &str = "account_token";

/// How long a sign-in started in the browser may take.
pub const LOGIN_TTL: Duration = Duration::from_secs(10 * 60);

/// How often the host reads the account's synths again.
pub const SYNC_EVERY: Duration = Duration::from_secs(10 * 60);

/// One platform call.
const PLATFORM_TIMEOUT: Duration = Duration::from_secs(20);

/// A neutral agent string: never the owner's email or name.
const USER_AGENT: &str = "snth-connect";

// --- what the platform says ---

/// The account, as the platform names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AccountInfo {
    pub id: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub name: String,
}

/// One synth of the account (`GET /api/app/v1/me`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSynth {
    pub hub_id: String,
    pub synth_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

/// `GET /api/app/v1/me`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Me {
    pub account: AccountInfo,
    #[serde(default)]
    pub synths: Vec<AccountSynth>,
}

/// `POST /api/app/v1/token`.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenAnswer {
    pub token: String,
    pub account: AccountInfo,
}

/// `POST /api/app/v1/synths/<hub_id>/<synth_id>/connect`: a Connect token
/// for this computer, minted by the synth.
#[derive(Debug, Clone, Deserialize)]
pub struct Minted {
    pub synth_id: String,
    pub synth_url: String,
    #[serde(default)]
    pub synth_name: String,
    pub companion_token: String,
}

/// `POST /api/app/v1/link-code`: a bot's code redeemed through the
/// account. `linked`: the platform tied the synth to the account (false:
/// it belongs to another account, `reason` says so; the token still works
/// for this computer).
#[derive(Debug, Clone, Deserialize)]
pub struct LinkCodeAnswer {
    pub synth_id: String,
    pub synth_url: String,
    #[serde(default)]
    pub synth_name: String,
    pub companion_token: String,
    #[serde(default)]
    pub linked: bool,
    #[serde(default)]
    pub reason: Option<String>,
}

/// Why a platform call did not work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformError {
    /// HTTP 401: the app's token is not (any more) a sign-in.
    Unauthorized,
    /// Any other answer that is not a success: the status and the
    /// platform's `{"error", "reason"}`.
    Status { status: u16, error: String, reason: Option<String> },
    /// No answer (offline, DNS, TLS, timeout).
    Unreachable(String),
    /// An answer that could not be read.
    Bad(String),
}

impl PlatformError {
    /// A short code the host turns into words.
    pub fn code(&self) -> String {
        match self {
            PlatformError::Unauthorized => "signed_out".into(),
            PlatformError::Status { status, error, .. } if error.is_empty() => format!("http_{status}"),
            PlatformError::Status { error, .. } => error.clone(),
            PlatformError::Unreachable(_) => "platform_unreachable".into(),
            PlatformError::Bad(_) => "platform_error".into(),
        }
    }

    fn status(&self) -> Option<u16> {
        match self {
            PlatformError::Status { status, .. } => Some(*status),
            _ => None,
        }
    }

    fn reason(&self) -> Option<String> {
        match self {
            PlatformError::Status { reason, .. } => reason.clone(),
            _ => None,
        }
    }
}

impl std::fmt::Display for PlatformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlatformError::Unauthorized => f.write_str("the platform refused the app's sign-in (401)"),
            PlatformError::Status { status, error, reason } => {
                write!(f, "platform returned {status}: {error}")?;
                if let Some(r) = reason {
                    write!(f, " ({r})")?;
                }
                Ok(())
            }
            PlatformError::Unreachable(e) => write!(f, "could not reach the platform: {e}"),
            PlatformError::Bad(e) => write!(f, "bad answer from the platform: {e}"),
        }
    }
}

/// The platform's app API. Blocking (the core calls it off the runtime).
/// `base` is the platform's address (`https://platform.snth.ai`); `token`
/// the app's token.
pub trait PlatformClient: Send + Sync {
    /// `POST /api/app/v1/token {code, code_verifier, device_id}`.
    fn exchange(&self, base: &str, code: &str, verifier: &str, device_id: &str) -> Result<TokenAnswer, PlatformError>;
    /// `GET /api/app/v1/me`.
    fn me(&self, base: &str, token: &str) -> Result<Me, PlatformError>;
    /// `POST /api/app/v1/synths/<hub_id>/<synth_id>/connect {}`.
    fn connect(&self, base: &str, token: &str, hub_id: &str, synth_id: &str) -> Result<Minted, PlatformError>;
    /// `POST /api/app/v1/link-code {code}`.
    fn link_code(&self, base: &str, token: &str, code: &str) -> Result<LinkCodeAnswer, PlatformError>;
    /// `POST /api/app/v1/logout {}`.
    fn logout(&self, base: &str, token: &str) -> Result<(), PlatformError>;
}

/// The real client: JSON over HTTPS (rustls).
pub struct UreqPlatformClient;

/// Percent-encode one path segment (an id from the platform).
fn segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Percent-encode a query value.
fn query(s: &str) -> String {
    segment(s)
}

impl UreqPlatformClient {
    fn agent() -> ureq::Agent {
        ureq::AgentBuilder::new().timeout(PLATFORM_TIMEOUT).user_agent(USER_AGENT).build()
    }

    fn url(base: &str, path: &str) -> String {
        format!("{}{path}", base.trim_end_matches('/'))
    }

    fn read<T: serde::de::DeserializeOwned>(resp: Result<ureq::Response, ureq::Error>) -> Result<T, PlatformError> {
        match resp {
            Ok(r) => r.into_json::<T>().map_err(|e| PlatformError::Bad(e.to_string())),
            Err(ureq::Error::Status(401, _)) => Err(PlatformError::Unauthorized),
            Err(ureq::Error::Status(status, r)) => {
                let body: serde_json::Value = r.into_json().unwrap_or_default();
                Err(PlatformError::Status {
                    status,
                    error: body.get("error").and_then(|v| v.as_str()).unwrap_or("").chars().take(80).collect(),
                    reason: body.get("reason").and_then(|v| v.as_str()).map(|r| r.chars().take(300).collect()),
                })
            }
            Err(ureq::Error::Transport(t)) => Err(PlatformError::Unreachable(t.to_string())),
        }
    }
}

impl PlatformClient for UreqPlatformClient {
    fn exchange(&self, base: &str, code: &str, verifier: &str, device_id: &str) -> Result<TokenAnswer, PlatformError> {
        let resp = Self::agent().post(&Self::url(base, "/api/app/v1/token")).send_json(ureq::json!({
            "code": code,
            "code_verifier": verifier,
            "device_id": device_id,
        }));
        Self::read(resp)
    }

    fn me(&self, base: &str, token: &str) -> Result<Me, PlatformError> {
        let resp = Self::agent()
            .get(&Self::url(base, "/api/app/v1/me"))
            .set("Authorization", &format!("Bearer {token}"))
            .call();
        Self::read(resp)
    }

    fn connect(&self, base: &str, token: &str, hub_id: &str, synth_id: &str) -> Result<Minted, PlatformError> {
        let path = format!("/api/app/v1/synths/{}/{}/connect", segment(hub_id), segment(synth_id));
        let resp = Self::agent()
            .post(&Self::url(base, &path))
            .set("Authorization", &format!("Bearer {token}"))
            .send_json(ureq::json!({}));
        Self::read(resp)
    }

    fn link_code(&self, base: &str, token: &str, code: &str) -> Result<LinkCodeAnswer, PlatformError> {
        let resp = Self::agent()
            .post(&Self::url(base, "/api/app/v1/link-code"))
            .set("Authorization", &format!("Bearer {token}"))
            .send_json(ureq::json!({ "code": code }));
        Self::read(resp)
    }

    fn logout(&self, base: &str, token: &str) -> Result<(), PlatformError> {
        let resp = Self::agent()
            .post(&Self::url(base, "/api/app/v1/logout"))
            .set("Authorization", &format!("Bearer {token}"))
            .send_json(ureq::json!({}));
        Self::read::<serde_json::Value>(resp).map(|_| ())
    }
}

// --- the core's side ---

/// Where a sign-in stands, for the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum LoginPhase {
    #[default]
    Idle,
    /// The browser is open at the platform; `started_at` unix seconds.
    Waiting { started_at: i64 },
    /// The browser came back; the app is exchanging the code.
    Exchanging,
    /// "denied" (the owner pressed Cancel), "expired", "exchange_failed",
    /// "platform_unreachable", ...
    Failed { code: String },
}

/// A sign-in started in the browser. Memory only.
struct LoginPending {
    state: String,
    verifier: String,
    started: Instant,
    started_unix: i64,
    url: String,
}

/// Why the last attempt to link this computer to a synth of the account
/// did not work.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ConnectError {
    code: String,
    reason: Option<String>,
}

/// The account's part of `Shared`.
#[derive(Default)]
pub struct State {
    client: RwLock<Option<(Arc<dyn PlatformClient>, String)>>,
    login: Mutex<Option<LoginPending>>,
    phase: Mutex<LoginPhase>,
    /// The account's synths from the last `/me` (None: not read yet).
    synths: Mutex<Option<Vec<AccountSynth>>>,
    errors: Mutex<HashMap<String, ConnectError>>,
    last_sync: Mutex<Option<i64>>,
    sync_error: Mutex<Option<String>>,
    /// One sync (or connect, or sign-out) at a time.
    busy: tokio::sync::Mutex<()>,
}

/// One synth of the account, as the UI shows it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AccountSynthView {
    pub hub_id: String,
    pub synth_id: String,
    pub name: String,
    pub avatar_url: Option<String>,
    /// This computer has a link to it.
    pub linked: bool,
    /// The owner disconnected it here: the sync leaves it alone.
    pub skipped: bool,
    /// Why the last try to link it failed ("connect_unavailable",
    /// "synth_outdated", "synth_unreachable", ...).
    pub error: Option<String>,
    /// The synth's or platform's words for it.
    pub reason: Option<String>,
}

/// The account, as the UI shows it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AccountView {
    pub signed_in: bool,
    pub account: Option<AccountInfo>,
    pub login: LoginPhase,
    pub synths: Option<Vec<AccountSynthView>>,
    pub consented: bool,
    pub last_sync: Option<i64>,
    pub sync_error: Option<String>,
    pub platform_url: String,
}

/// What a sync did, for the host's notifications.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Names of the synths this computer was linked to just now.
    pub added: Vec<String>,
    /// Names of the synths it was unlinked from (they left the account).
    pub removed: Vec<String>,
    /// The platform refused the app's token: signed out.
    pub signed_out: bool,
    /// The first link of this computer was made by this run.
    pub first_link: bool,
}

fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn random_b64url() -> String {
    use rand::RngCore;
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    b64url(&b)
}

/// The S256 challenge of a PKCE verifier.
pub fn challenge_of(verifier: &str) -> String {
    b64url(&Sha256::digest(verifier.as_bytes()))
}

/// "macos", "windows" or "linux", as the platform names systems.
fn os_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    }
}

/// The platform call off the async runtime.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(f).await.map_err(|e| format!("join: {e}"))
}

impl Shared {
    fn platform(&self) -> Result<(Arc<dyn PlatformClient>, String), String> {
        self.account.client.read().unwrap().clone().ok_or_else(|| "platform_unavailable".to_string())
    }

    fn account_token(&self) -> Option<String> {
        self.secret().get(ACCOUNT_TOKEN_KEY).ok().flatten().filter(|t| !t.is_empty())
    }

    fn set_phase(&self, phase: LoginPhase) {
        *self.account.phase.lock().unwrap() = phase;
        self.emit(CoreEvent::Account);
    }

    fn note(&self, action: &str, target: &str, outcome: impl Into<String>) {
        self.journal().record(Entry::new("app", action, target, outcome));
    }

    fn stored_account(&self) -> Option<StoredAccount> {
        self.persisted.read().unwrap().account.clone()
    }

    fn update_account(&self, f: impl FnOnce(&mut Option<StoredAccount>)) {
        let mut p = self.persisted.write().unwrap();
        let before = p.account.clone();
        f(&mut p.account);
        if p.account != before {
            self.save_persisted(&p);
        }
    }

    /// Link this computer to one synth of the account: the platform asks
    /// the synth for a Connect token. A synth this computer has a link to
    /// already gets the new token on that link (one link per synth; the
    /// synth drops this device's old token when it mints a new one).
    async fn connect_one(self: &Arc<Self>, synth: &AccountSynth) -> Result<bool, ConnectError> {
        let (client, base) = self.platform().map_err(|code| ConnectError { code, reason: None })?;
        let token = self.account_token().ok_or(ConnectError { code: "signed_out".into(), reason: None })?;
        let (hub, id) = (synth.hub_id.clone(), synth.synth_id.clone());
        let answer = blocking(move || client.connect(&base, &token, &hub, &id))
            .await
            .map_err(|e| ConnectError { code: "platform_error".into(), reason: Some(e) })?;
        match answer {
            Ok(minted) => {
                let pending = Pending {
                    synth_id: if minted.synth_id.is_empty() { synth.synth_id.clone() } else { minted.synth_id },
                    synth_url: minted.synth_url,
                    token: minted.companion_token,
                    synth_name: if minted.synth_name.is_empty() { synth.name.clone() } else { minted.synth_name },
                    source: LinkSource::Account,
                    hub_id: synth.hub_id.clone(),
                    linked_to_account: Some(true),
                    reason: None,
                };
                self.account.errors.lock().unwrap().remove(&synth.synth_id);
                let first = self.add_link(&pending).map_err(|e| ConnectError { code: "store_failed".into(), reason: Some(e) })?;
                self.note("account.connect", &pending.synth_id, "ok");
                Ok(first)
            }
            Err(e) => {
                let err = ConnectError { code: e.code(), reason: e.reason() };
                self.note("account.connect", &synth.synth_id, format!("failed: {}", err.code));
                self.account.errors.lock().unwrap().insert(synth.synth_id.clone(), err.clone());
                Err(err)
            }
        }
    }

    /// Forget the sign-in here: the token, the account, the links made
    /// through it (each is unpaired like the owner's own unlink). Links
    /// made with a bot's code stay.
    async fn sign_out_local(self: &Arc<Self>, core: &Core) -> Vec<String> {
        let _ = self.secret().delete(ACCOUNT_TOKEN_KEY);
        self.update_account(|a| *a = None);
        *self.account.synths.lock().unwrap() = None;
        self.account.errors.lock().unwrap().clear();
        *self.account.sync_error.lock().unwrap() = None;
        *self.account.login.lock().unwrap() = None;
        *self.account.phase.lock().unwrap() = LoginPhase::Idle;
        let mut removed = Vec::new();
        for slot in self.slots() {
            if slot.source() == LinkSource::Account {
                removed.push(slot.display_name());
                core.unpair_synth(&slot.synth_id).await;
            }
        }
        self.emit(CoreEvent::Account);
        removed
    }
}

/// Redeem a bot's code through the signed-in account (`Core::pair_begin`).
/// The synth is tied to the account at the same time when it is free; a
/// synth of another account still gets this computer, as a code link.
/// Errors are the host's codes, before a colon.
pub(crate) async fn link_code(shared: &Arc<Shared>, code: &str) -> Result<Pending, String> {
    let code = crate::pairing::normalize_code(code).ok_or("bad_code: the code must be six digits")?;
    let (client, base) = shared.platform().map_err(|e| format!("{e}: no platform"))?;
    let token = shared.account_token().ok_or("signed_out: not signed in")?;
    let answer = blocking(move || client.link_code(&base, &token, &code)).await?;
    match answer {
        Ok(a) => {
            shared.note("account.link_code", &a.synth_id, if a.linked { "ok" } else { "ok: linked elsewhere" });
            Ok(Pending {
                synth_id: a.synth_id,
                synth_url: a.synth_url,
                token: a.companion_token,
                synth_name: a.synth_name,
                source: if a.linked { LinkSource::Account } else { LinkSource::Code },
                hub_id: String::new(),
                linked_to_account: Some(a.linked),
                reason: a.reason.filter(|r| !r.is_empty()),
            })
        }
        Err(e) => {
            let code = match (&e, e.status()) {
                (PlatformError::Unauthorized, _) => "signed_out",
                (_, Some(404)) => "code_not_found",
                (_, Some(410)) => "code_expired",
                (_, Some(400)) => "bad_code",
                (_, Some(429)) => "rate_limited",
                (_, Some(409)) => "conflict",
                (PlatformError::Unreachable(_), _) => "platform_unreachable",
                _ => "platform_error",
            };
            Err(format!("{code}: {e}"))
        }
    }
}

/// A fresh token for an account link whose synth refused the old one: the
/// link loop asks once per refusal, the owner's "Connect again" asks too.
pub(crate) async fn remint(shared: &Arc<Shared>, slot: &Arc<LinkSlot>) -> Result<(), String> {
    if slot.source() != LinkSource::Account {
        return Err("needs_code".into());
    }
    let hub_id = slot.hub_id();
    if hub_id.is_empty() {
        // Not known yet (a code redeemed through the account): the next
        // sync fills it in.
        return Err("no_hub_id".into());
    }
    let synth = AccountSynth { hub_id, synth_id: slot.synth_id.clone(), name: slot.display_name(), avatar_url: None };
    shared.connect_one(&synth).await.map(|_| ()).map_err(|e| e.code)
}

impl Core {
    /// Where the account's sign-in and app API live, and the client that
    /// talks to it. Without this the core has no account (the CLI).
    pub fn set_platform(&self, client: Arc<dyn PlatformClient>, base_url: impl Into<String>) {
        *self.shared().account.client.write().unwrap() = Some((client, base_url.into()));
    }

    /// The address the browser opens to sign in, with a fresh state and
    /// PKCE challenge, or the same one again while the sign-in started
    /// less than 10 minutes ago ("open again").
    pub fn login_url(&self) -> Result<String, String> {
        let shared = self.shared();
        let (_, base) = shared.platform()?;
        let mut login = shared.account.login.lock().unwrap();
        if let Some(p) = login.as_ref().filter(|p| p.started.elapsed() < LOGIN_TTL) {
            let url = p.url.clone();
            let started_at = p.started_unix;
            drop(login);
            shared.set_phase(LoginPhase::Waiting { started_at });
            return Ok(url);
        }
        let state = random_b64url();
        let verifier = random_b64url();
        let url = format!(
            "{}/connect/login?state={}&code_challenge={}&device_id={}&device_name={}&os={}&app_version={}",
            base.trim_end_matches('/'),
            query(&state),
            query(&challenge_of(&verifier)),
            query(&shared.device_id()),
            query(&shared.device_name()),
            os_name(),
            query(&shared.app_version),
        );
        let started_unix = now_unix();
        *login = Some(LoginPending { state, verifier, started: Instant::now(), started_unix, url: url.clone() });
        drop(login);
        shared.set_phase(LoginPhase::Waiting { started_at: started_unix });
        Ok(url)
    }

    /// The owner gave up on the sign-in in the browser.
    pub fn login_cancel(&self) {
        let shared = self.shared();
        *shared.account.login.lock().unwrap() = None;
        shared.set_phase(LoginPhase::Idle);
    }

    /// The browser came back with `snth://login?...`. A state that is not
    /// the waiting sign-in's (or no sign-in waiting) is ignored and
    /// journaled: any page can open such a link. Otherwise the code is
    /// exchanged for the app's token, the account is stored and its synths
    /// are read. Nothing is linked yet: the owner confirms first
    /// (`account_connect`).
    pub async fn login_complete(&self, code: Result<String, String>, state: &str) -> Result<(), String> {
        let shared = self.shared();
        let pending = {
            let mut login = shared.account.login.lock().unwrap();
            match login.as_ref() {
                Some(p) if p.state == state && p.started.elapsed() < LOGIN_TTL => login.take(),
                Some(p) if p.state == state => {
                    login.take();
                    drop(login);
                    shared.note("account.login", "platform", "failed: the sign-in took too long");
                    shared.set_phase(LoginPhase::Failed { code: "expired".into() });
                    return Err("expired".into());
                }
                _ => None,
            }
        };
        let Some(pending) = pending else {
            shared.note("account.login", "platform", "ignored: a sign-in link this app did not start");
            return Err("state_mismatch".into());
        };
        let code = match code {
            Ok(c) => c,
            Err(why) => {
                let reason = if why == "denied" { "denied" } else { "failed" };
                shared.note("account.login", "platform", format!("denied: {reason} in the browser"));
                shared.set_phase(LoginPhase::Failed { code: if why == "denied" { "denied".into() } else { why } });
                return Err("denied".into());
            }
        };
        shared.set_phase(LoginPhase::Exchanging);
        let (client, base) = shared.platform()?;
        let device_id = shared.device_id();
        let verifier = pending.verifier;
        let answer = blocking(move || client.exchange(&base, &code, &verifier, &device_id)).await?;
        let answer = match answer {
            Ok(a) if a.token.starts_with("snthapp_") && !a.account.id.is_empty() => a,
            Ok(_) => {
                shared.set_phase(LoginPhase::Failed { code: "exchange_failed".into() });
                shared.note("account.login", "platform", "failed: an answer without a token");
                return Err("exchange_failed".into());
            }
            Err(e) => {
                let code = match e {
                    PlatformError::Unreachable(_) => "platform_unreachable".to_string(),
                    _ => "exchange_failed".to_string(),
                };
                shared.note("account.login", "platform", format!("failed: {}", e.code()));
                shared.set_phase(LoginPhase::Failed { code: code.clone() });
                return Err(code);
            }
        };
        if let Err(e) = shared.secret().set(ACCOUNT_TOKEN_KEY, &answer.token) {
            shared.set_phase(LoginPhase::Failed { code: "store_failed".into() });
            return Err(format!("store_failed: {e}"));
        }
        let same = shared.stored_account().is_some_and(|a| a.id == answer.account.id);
        shared.update_account(|a| {
            let keep = a.take().filter(|_| same);
            *a = Some(StoredAccount {
                id: answer.account.id.clone(),
                email: answer.account.email.clone(),
                name: answer.account.name.clone(),
                // The same account signing in again keeps what the owner
                // chose on this computer; another account starts afresh.
                consented: keep.as_ref().is_some_and(|k| k.consented),
                skipped: keep.map(|k| k.skipped).unwrap_or_default(),
            });
        });
        shared.note("account.login", "platform", "ok");
        *shared.account.phase.lock().unwrap() = LoginPhase::Idle;
        // The account's synths, for the confirmation screen.
        let _ = self.account_sync().await;
        shared.emit(CoreEvent::Account);
        Ok(())
    }

    /// Read the account's synths and bring the links in step (see the
    /// module comment). Safe to call any time; one runs at a time.
    pub async fn account_sync(&self) -> SyncReport {
        let shared = self.shared();
        let _one = shared.account.busy.lock().await;
        let mut report = SyncReport::default();
        let (Ok((client, base)), Some(token)) = (shared.platform(), shared.account_token()) else {
            return report;
        };
        if shared.stored_account().is_none() {
            return report;
        }
        let me = match blocking(move || client.me(&base, &token)).await {
            Ok(Ok(me)) => me,
            Ok(Err(PlatformError::Unauthorized)) => {
                // The owner removed this computer in the platform (or the
                // grant ran out): signed out.
                shared.note("account.sync", "platform", "signed out: the platform refused the app's sign-in");
                report.removed = shared.sign_out_local(self).await;
                report.signed_out = true;
                return report;
            }
            Ok(Err(e)) => {
                *shared.account.sync_error.lock().unwrap() = Some(e.code());
                shared.emit(CoreEvent::Account);
                return report;
            }
            Err(e) => {
                *shared.account.sync_error.lock().unwrap() = Some(e);
                return report;
            }
        };
        *shared.account.sync_error.lock().unwrap() = None;
        *shared.account.last_sync.lock().unwrap() = Some(now_unix());
        shared.update_account(|a| {
            if let Some(a) = a.as_mut() {
                if !me.account.email.is_empty() {
                    a.email = me.account.email.clone();
                }
                a.name = me.account.name.clone();
            }
        });
        let synths: Vec<AccountSynth> =
            me.synths.into_iter().filter(|s| !s.synth_id.is_empty() && !s.hub_id.is_empty()).collect();
        *shared.account.synths.lock().unwrap() = Some(synths.clone());

        // Synths that left the account: their account links go. (A link
        // the account has never listed, a code redeemed through it a
        // moment ago, has no hub id yet and is not taken for one.)
        for slot in shared.slots() {
            if slot.source() == LinkSource::Account
                && !slot.hub_id().is_empty()
                && !synths.iter().any(|s| s.synth_id == slot.synth_id)
            {
                report.removed.push(slot.display_name());
                shared.note("account.sync", &slot.synth_id, "unlinked: the synth left the account");
                self.unpair_synth(&slot.synth_id).await;
            }
        }
        // Links the computer has to synths of the account: they are the
        // account's now (one link per synth), with what the platform knows.
        for s in &synths {
            if let Some(slot) = shared.slot(&s.synth_id) {
                shared.adopt(&slot, s);
            }
        }
        // New synths of the account, once the owner confirmed here.
        let account = shared.stored_account().unwrap_or_default();
        if account.consented {
            for s in &synths {
                if shared.slot(&s.synth_id).is_some() || account.skipped.contains(&s.synth_id) {
                    continue;
                }
                if let Ok(first) = shared.connect_one(s).await {
                    report.first_link |= first;
                    report.added.push(if s.name.is_empty() { s.synth_id.clone() } else { s.name.clone() });
                }
            }
        }
        shared.emit(CoreEvent::Account);
        report
    }

    /// The owner confirmed on this computer: link it to every synth of the
    /// account (a link that the synth refused gets a fresh token too). From
    /// now on the sync links new synths by itself. Returns what was done;
    /// `first_link` tells the host to start its onboarding.
    pub async fn account_connect(&self) -> Result<SyncReport, String> {
        let shared = self.shared();
        if !shared.account_signed_in() {
            return Err("not_signed_in".into());
        }
        shared.update_account(|a| {
            if let Some(a) = a.as_mut() {
                a.consented = true;
                a.skipped.clear();
            }
        });
        let mut report = self.account_sync().await;
        if report.signed_out {
            return Err("signed_out".into());
        }
        // The sync linked what had no link; what needs a new token gets
        // one here.
        let _one = shared.account.busy.lock().await;
        let synths = shared.account.synths.lock().unwrap().clone().unwrap_or_default();
        for s in &synths {
            let needs = shared.slot(&s.synth_id).is_some_and(|slot| slot.state() == LinkState::NeedsRepair);
            if needs {
                if let Ok(first) = shared.connect_one(s).await {
                    report.first_link |= first;
                    report.added.push(if s.name.is_empty() { s.synth_id.clone() } else { s.name.clone() });
                }
            }
        }
        shared.emit(CoreEvent::Account);
        Ok(report)
    }

    /// The owner disconnected this synth here: the sync leaves it alone
    /// until the owner connects again (`account_connect`).
    pub fn account_skip(&self, synth_id: &str) {
        let shared = self.shared();
        let in_account = shared
            .account
            .synths
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|list| list.iter().any(|s| s.synth_id == synth_id));
        if !in_account {
            return;
        }
        shared.update_account(|a| {
            if let Some(a) = a.as_mut() {
                if !a.skipped.iter().any(|s| s == synth_id) {
                    a.skipped.push(synth_id.to_string());
                }
            }
        });
        shared.emit(CoreEvent::Account);
    }

    /// Sign out: the platform revokes the app's token and unlinks this
    /// computer from the account's synths (best effort: an error goes to
    /// the log only); here the token, the account and every link made
    /// through it go. Links made with a bot's code stay. Returns the names
    /// of the synths unlinked.
    pub async fn sign_out(&self) -> Vec<String> {
        let shared = self.shared();
        let _one = shared.account.busy.lock().await;
        if let (Ok((client, base)), Some(token)) = (shared.platform(), shared.account_token()) {
            match blocking(move || client.logout(&base, &token)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => crate::logln!("account: logout at the platform: {e}"),
                Err(e) => crate::logln!("account: logout at the platform: {e}"),
            }
        }
        shared.note("account.logout", "platform", "ok");
        shared.sign_out_local(self).await
    }

    /// The account as the UI shows it.
    pub fn account_view(&self) -> AccountView {
        let shared = self.shared();
        let stored = shared.stored_account();
        let signed_in = stored.is_some() && shared.account_token().is_some();
        let links = shared.slots();
        let errors = shared.account.errors.lock().unwrap().clone();
        let login = shared.account.phase.lock().unwrap().clone();
        let last_sync = *shared.account.last_sync.lock().unwrap();
        let sync_error = shared.account.sync_error.lock().unwrap().clone();
        let synths = shared.account.synths.lock().unwrap().clone().map(|list| {
            list.into_iter()
                .map(|s| {
                    let err = errors.get(&s.synth_id).cloned();
                    AccountSynthView {
                        linked: links.iter().any(|l| l.synth_id == s.synth_id),
                        skipped: stored.as_ref().is_some_and(|a| a.skipped.contains(&s.synth_id)),
                        error: err.as_ref().map(|e| e.code.clone()),
                        reason: err.and_then(|e| e.reason),
                        hub_id: s.hub_id,
                        name: if s.name.is_empty() { s.synth_id.clone() } else { s.name },
                        synth_id: s.synth_id,
                        avatar_url: s.avatar_url,
                    }
                })
                .collect()
        });
        AccountView {
            signed_in,
            account: stored.as_ref().filter(|_| signed_in).map(|a| AccountInfo {
                id: a.id.clone(),
                email: a.email.clone(),
                name: a.name.clone(),
            }),
            login,
            synths: if signed_in { synths } else { None },
            consented: signed_in && stored.as_ref().is_some_and(|a| a.consented),
            last_sync,
            sync_error,
            platform_url: shared.platform().map(|(_, b)| b).unwrap_or_default(),
        }
    }
}

impl Shared {
    /// A link to a synth that the account lists: it is the account's (so
    /// it goes when the account signs out and can get a fresh token), with
    /// the hub id and name the platform knows.
    fn adopt(&self, slot: &Arc<LinkSlot>, s: &AccountSynth) {
        let changed = slot.source() != LinkSource::Account || slot.hub_id() != s.hub_id;
        if !changed {
            return;
        }
        *slot.source.write().unwrap() = LinkSource::Account;
        *slot.hub_id.write().unwrap() = s.hub_id.clone();
        if !s.name.is_empty() && slot.stored_name.read().unwrap().is_empty() {
            *slot.stored_name.write().unwrap() = s.name.clone();
        }
        let mut p = self.persisted.write().unwrap();
        if let Some(id) = p.identities.iter_mut().find(|i| i.synth_id == slot.synth_id) {
            id.source = LinkSource::Account;
            id.hub_id = s.hub_id.clone();
            if id.synth_name.is_empty() {
                id.synth_name = s.name.clone();
            }
            self.save_persisted(&p);
        }
    }
}

#[cfg(test)]
mod tests;
