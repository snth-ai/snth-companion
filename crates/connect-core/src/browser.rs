//! The browser link: what the app says to, and accepts from, the SNTH
//! Vault extension in the owner's browsers.
//!
//! The extension reaches the app through the helper of `connect-host`
//! (native messaging, then a per-user local endpoint). Each browser
//! profile that runs the extension is one connection, a `Session` here.
//!
//! Three things travel on it:
//!
//!  1. PAIRING. The extension asks to be linked to the synth's login
//!     vault. The owner confirms in THIS app, the app gets a pairing code
//!     from the synth over the existing link (`vault.pair_code`) and hands
//!     it to the extension, which redeems it at the hub like a code typed
//!     by hand. The app never sees a password and never talks to the hub
//!     about the vault.
//!  2. COOKIES. When the synth asks for a site's cookies and the owner
//!     allowed that site, the app may ask a LINKED browser for them. The
//!     extension asks the owner once per site through the browser's own
//!     permission prompt. Cookies travel extension -> app only; the app
//!     sends the extension a site name and nothing else.
//!  3. STATE. Which browser it is, whether it holds a vault pairing, which
//!     sites it may read.
//!
//! THE RULE this module holds up: passwords are for the synth's cloud
//! browser only (the hub types them, nobody else sees them); cookies are
//! for requests made from this computer only. Nothing here sends a cookie
//! anywhere but back to the caller in this process, and nothing here ever
//! holds a password.
//!
//! Everything a session sends is untrusted. The extension's id is checked
//! by the browser and again by the helper, but a program of the same user
//! could speak this protocol too, so the app is built to give such a peer
//! nothing: it is never sent a secret, a pairing code leaves only after
//! the owner's click in this app, and cookies are accepted only from a
//! browser the owner linked here, only for the site just asked about.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use connect_host::frame::{self, FrameError, MAX_FROM_BROWSER, MAX_TO_BROWSER};
use connect_host::transport::{Conn, Listener};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::BufReader;
use tokio::sync::{mpsc, oneshot, watch};

use crate::cookies::{host_applies, CookieRead, WireCookie};
use crate::core::{Core, CoreEvent, Shared};
use crate::grant::{ApprovalRequest, Remember};
use crate::journal::Entry;
use crate::logln;
use crate::settings::{CookiesMode, StoredBrowser};

/// The protocol version both sides name in their hello.
pub const VERSION: u32 = 1;

/// The capability name of a pairing question in the approval window and
/// in the journal.
pub const CAP: &str = "vault";

/// How long a new connection has to say who it is.
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);
/// Messages waiting to be written to one extension.
const OUT_QUEUE: usize = 32;
/// At most this many site names are kept from one extension.
const MAX_SITES: usize = 200;
/// At most this many cookies are taken for one site (the synth keeps no
/// more than that either).
pub const MAX_COOKIES: usize = 400;
/// A cookie name or value longer than this is not a cookie a browser
/// would send (browsers cap name + value near 4 KiB).
const MAX_COOKIE_FIELD: usize = 8 * 1024;
/// After a code went to an extension, its "done" is accepted this long.
const PAIR_WINDOW: Duration = Duration::from_secs(180);
/// The least time between two pairing questions from one browser.
const PAIR_COOLDOWN: Duration = Duration::from_secs(3);
/// Remembered browsers.
const MAX_BROWSERS: usize = 32;
/// Connected extensions at once.
const MAX_SESSIONS: usize = 16;

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// --- cleaning what an extension says ---

/// The browser's short id, when it is one we know.
fn clean_browser(raw: &str) -> String {
    let id = raw.trim().to_ascii_lowercase();
    if connect_host::install::browser(&id).is_some() {
        id
    } else {
        "browser".to_string()
    }
}

/// The name people know a browser by.
pub fn browser_name(id: &str) -> &'static str {
    connect_host::install::browser(id).map(|b| b.name).unwrap_or("Browser")
}

/// The random id an extension made for its profile: 16 to 64 hex digits.
fn clean_install(raw: &str) -> Option<String> {
    let ok = (16..=64).contains(&raw.len()) && raw.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    ok.then(|| raw.to_string())
}

fn clean_version(raw: &str) -> String {
    raw.chars().filter(|c| c.is_ascii_digit() || *c == '.').take(20).collect()
}

/// The vault's device id: "dev_" and hex, as the hub mints it.
fn clean_device_id(raw: &str) -> String {
    let ok = raw.len() <= 40
        && raw.starts_with("dev_")
        && raw.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if ok {
        raw.to_string()
    } else {
        String::new()
    }
}

/// A site as this app names it: the registrable domain, lowercase. None
/// for anything else (a subdomain, an address, a single label).
pub fn clean_site(raw: &str) -> Option<String> {
    let s = raw.trim().to_ascii_lowercase();
    let shaped = !s.is_empty()
        && s.len() <= 253
        && s.contains('.')
        && !s.starts_with('.')
        && !s.ends_with('.')
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.');
    (shaped && s.parse::<std::net::IpAddr>().is_err() && psl::domain_str(&s).is_some_and(|d| d == s)).then_some(s)
}

fn clean_sites(raw: Option<&Value>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in raw.and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
        if out.len() >= MAX_SITES {
            break;
        }
        if let Some(site) = item.as_str().and_then(clean_site) {
            if !out.contains(&site) {
                out.push(site);
            }
        }
    }
    out.sort();
    out
}

fn text_field(c: &Value, key: &str, max: usize) -> Option<String> {
    let s = c.get(key)?.as_str()?;
    (s.len() <= max && !s.chars().any(|ch| ch == '\0' || ch == '\r' || ch == '\n')).then(|| s.to_string())
}

/// The cookies an extension returned for `site`, cut down to what a
/// cookie of that site can be. A cookie of another site, a malformed one
/// or one past the count limit is dropped, never an error.
pub fn clean_cookies(site: &str, raw: Option<&Value>) -> Vec<WireCookie> {
    let mut out = Vec::new();
    for c in raw.and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
        if out.len() >= MAX_COOKIES {
            break;
        }
        let (Some(name), Some(value), Some(domain)) = (
            text_field(c, "name", MAX_COOKIE_FIELD),
            text_field(c, "value", MAX_COOKIE_FIELD),
            text_field(c, "domain", 254),
        ) else {
            continue;
        };
        let host = domain.trim_start_matches('.');
        let host_ok = !host.is_empty()
            && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
            && host_applies(&domain, site);
        if !host_ok || name.is_empty() && value.is_empty() {
            continue;
        }
        let path = text_field(c, "path", 2048).filter(|p| p.starts_with('/')).unwrap_or_else(|| "/".into());
        let expires = c.get("expires").and_then(Value::as_f64).unwrap_or(0.0);
        let same_site = match c.get("same_site").and_then(Value::as_str).unwrap_or("") {
            s @ ("none" | "lax" | "strict") => s.to_string(),
            _ => String::new(),
        };
        out.push(WireCookie {
            name,
            value,
            domain: domain.to_ascii_lowercase(),
            path,
            expires: if expires.is_finite() && expires > 0.0 { expires as i64 } else { 0 },
            secure: c.get("secure").and_then(Value::as_bool).unwrap_or(false),
            http_only: c.get("http_only").and_then(Value::as_bool).unwrap_or(false),
            same_site,
        });
    }
    out
}

/// The alphabet of a vault pairing code (the hub's: no I, O, 0 or 1).
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

fn clean_code(raw: &str) -> Option<String> {
    (raw.len() == 8 && raw.bytes().all(|b| CODE_ALPHABET.contains(&b))).then(|| raw.to_string())
}

// --- sessions ---

#[derive(Default, Clone)]
struct SessionState {
    /// The extension said hello.
    greeted: bool,
    browser: String,
    install: String,
    ext_version: String,
    /// It says it holds a vault pairing.
    paired: bool,
    device_id: String,
    /// The sites it may read cookies of.
    sites: Vec<String>,
    /// A pairing question from it is being handled.
    pairing: bool,
    /// When a code went out to it.
    code_sent: Option<Instant>,
    last_pair: Option<Instant>,
}

pub(crate) struct Session {
    id: u64,
    tx: mpsc::Sender<Value>,
    state: Mutex<SessionState>,
}

impl Session {
    fn send(&self, msg: Value) -> bool {
        self.tx.try_send(msg).is_ok()
    }
    fn snapshot(&self) -> SessionState {
        self.state.lock().unwrap().clone()
    }
}

struct Waiting {
    session: u64,
    tx: oneshot::Sender<Value>,
}

/// The connected extensions and the questions waiting for their answers.
#[derive(Default)]
pub struct Hub {
    next_session: AtomicU64,
    next_request: AtomicU64,
    sessions: Mutex<Vec<Arc<Session>>>,
    waiting: Mutex<HashMap<u64, Waiting>>,
}

impl Hub {
    fn greeted(&self) -> Vec<Arc<Session>> {
        self.sessions
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.state.lock().unwrap().greeted)
            .cloned()
            .collect()
    }

    fn by_install(&self, install: &str) -> Option<Arc<Session>> {
        self.greeted().into_iter().rev().find(|s| s.state.lock().unwrap().install == install)
    }

    /// Say something to every connected extension.
    fn tell_all(&self, msg: &Value) {
        for s in self.greeted() {
            s.send(msg.clone());
        }
    }

    fn remove(&self, id: u64) {
        self.sessions.lock().unwrap().retain(|s| s.id != id);
        // Whoever waited for this extension gets no answer.
        self.waiting.lock().unwrap().retain(|_, w| w.session != id);
    }

    /// Ask one extension and wait for the message that carries the same
    /// id. None = no answer in time, or it went away.
    async fn ask(&self, session: &Arc<Session>, mut msg: Value, wait: Duration) -> Option<Value> {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed) + 1;
        msg["id"] = json!(id);
        let (tx, rx) = oneshot::channel();
        self.waiting.lock().unwrap().insert(id, Waiting { session: session.id, tx });
        if !session.send(msg) {
            self.waiting.lock().unwrap().remove(&id);
            return None;
        }
        let got = tokio::time::timeout(wait, rx).await;
        self.waiting.lock().unwrap().remove(&id);
        got.ok().and_then(Result::ok)
    }

    /// An answer arrived: hand it to whoever asked THAT session.
    fn answer(&self, session: u64, msg: Value) {
        let Some(id) = msg.get("id").and_then(Value::as_u64) else {
            return;
        };
        let mut waiting = self.waiting.lock().unwrap();
        if waiting.get(&id).is_some_and(|w| w.session == session) {
            if let Some(w) = waiting.remove(&id) {
                let _ = w.tx.send(msg);
            }
        }
    }
}

/// One browser as the Logins page shows it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BrowserView {
    pub install: String,
    /// "chrome", "edge", "brave", "chromium" or "browser".
    pub browser: String,
    /// Its extension is talking to the app right now.
    pub connected: bool,
    /// The owner linked it here and it holds a vault pairing of this
    /// synth. Only a linked browser is asked for cookies.
    pub linked: bool,
    pub ext_version: String,
    /// The sites it may hand cookies of (known while connected).
    pub sites: Vec<String>,
    /// Unix seconds of its last hello.
    pub last_seen: i64,
}

/// One login the synth's vault holds. Never the login itself.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct VaultSite {
    pub site: String,
    pub has_totp: bool,
    /// Unix seconds; 0 = unknown / never.
    pub updated: i64,
    pub last_used: i64,
    pub use_count: u64,
}

/// One browser paired with the vault, as the hub lists it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct VaultDevice {
    pub id: String,
    pub name: String,
    pub created: i64,
    pub last_seen: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct VaultList {
    pub sites: Vec<VaultSite>,
    pub devices: Vec<VaultDevice>,
}

/// Why a pairing did not happen, as the extension is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairRefusal {
    /// This computer is not linked to a synth.
    NotLinked,
    Paused,
    /// The owner said no (or did not answer).
    Denied,
    /// Another pairing of this browser is in flight, or it asks too often.
    Busy,
    /// The link to the synth is down.
    SynthOffline,
    /// The synth is too old to hand out codes over the link.
    SynthOutdated,
    /// The extension is paired with a vault this synth does not know.
    OtherSynth,
    Failed,
}

impl PairRefusal {
    pub fn code(self) -> &'static str {
        match self {
            PairRefusal::NotLinked => "not_linked",
            PairRefusal::Paused => "paused",
            PairRefusal::Denied => "denied",
            PairRefusal::Busy => "busy",
            PairRefusal::SynthOffline => "synth_offline",
            PairRefusal::SynthOutdated => "synth_outdated",
            PairRefusal::OtherSynth => "other_synth",
            PairRefusal::Failed => "failed",
        }
    }

    fn from_link_error(e: &str) -> PairRefusal {
        if e.contains("needs an update") || e.contains("unknown method") {
            PairRefusal::SynthOutdated
        } else if e.contains("not linked") || e.contains("dropped") || e.contains("did not answer") {
            PairRefusal::SynthOffline
        } else {
            PairRefusal::Failed
        }
    }
}

/// How a read through an extension ended.
pub(crate) enum ExtensionRead {
    Cookies(CookieRead),
    /// No linked browser is connected: the caller falls back.
    NoBrowser,
    /// The owner refused in the browser.
    Denied(String),
    /// The browser is still waiting for the owner.
    NoAnswer(String),
    Failed(String),
}

fn timestamp(v: Option<&Value>) -> i64 {
    v.and_then(Value::as_i64).filter(|t| *t > 0 && *t < 100_000_000_000).unwrap_or(0)
}

fn short_text(v: Option<&Value>, max: usize) -> String {
    v.and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control() && *c != '<' && *c != '>')
        .take(max)
        .collect()
}

/// The synth's vault list, cut down to names and facts.
fn parse_vault_list(v: &Value) -> VaultList {
    let rows = |key: &str| v.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
    let mut list = VaultList::default();
    for row in rows("sites").iter().take(500) {
        // A vault site is a host ("mail.example.com" is fine here).
        let site = short_text(row.get("site"), 253).to_ascii_lowercase();
        let ok = site.contains('.') && site.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
        if !ok {
            continue;
        }
        list.sites.push(VaultSite {
            site,
            has_totp: row.get("has_totp").and_then(Value::as_bool).unwrap_or(false),
            updated: timestamp(row.get("updated")),
            last_used: timestamp(row.get("last_used")),
            use_count: row.get("use_count").and_then(Value::as_u64).unwrap_or(0),
        });
    }
    list.sites.sort_by(|a, b| a.site.cmp(&b.site));
    for row in rows("devices").iter().take(50) {
        let id = clean_device_id(row.get("id").and_then(Value::as_str).unwrap_or(""));
        if id.is_empty() {
            continue;
        }
        list.devices.push(VaultDevice {
            id,
            name: short_text(row.get("name"), 60),
            created: timestamp(row.get("created")),
            last_seen: timestamp(row.get("last_seen")),
        });
    }
    list
}

impl Shared {
    fn synth_label(&self) -> String {
        if let Some(ack) = self.synth.read().unwrap().as_ref() {
            if !ack.synth_name.is_empty() {
                return ack.synth_name.clone();
            }
        }
        self.persisted
            .read()
            .unwrap()
            .identity
            .as_ref()
            .map(|i| if i.synth_name.is_empty() { i.synth_id.clone() } else { i.synth_name.clone() })
            .unwrap_or_default()
    }

    fn stored_browser(&self, install: &str) -> Option<StoredBrowser> {
        self.persisted.read().unwrap().browsers.iter().find(|b| b.install == install).cloned()
    }

    /// Change (or add) the record of one browser and write it.
    fn update_browser(&self, install: &str, f: impl FnOnce(&mut StoredBrowser)) {
        {
            let mut p = self.persisted.write().unwrap();
            let before = p.browsers.clone();
            if !p.browsers.iter().any(|b| b.install == install) {
                if p.browsers.len() >= MAX_BROWSERS {
                    // Drop the one not seen for the longest, never a linked one.
                    let oldest = p
                        .browsers
                        .iter()
                        .enumerate()
                        .filter(|(_, b)| !b.linked)
                        .min_by_key(|(_, b)| b.last_seen)
                        .map(|(i, _)| i);
                    match oldest {
                        Some(i) => {
                            p.browsers.remove(i);
                        }
                        None => return,
                    }
                }
                p.browsers.push(StoredBrowser {
                    install: install.to_string(),
                    browser: "browser".into(),
                    linked: false,
                    device_id: String::new(),
                    ext_version: String::new(),
                    last_seen: 0,
                });
            }
            if let Some(b) = p.browsers.iter_mut().find(|b| b.install == install) {
                f(b);
            }
            if p.browsers != before {
                self.save_persisted(&p);
            }
        }
        self.emit(CoreEvent::Browsers);
    }

    /// What every extension is told about the app: linked to whom.
    fn status_for(&self, install: &str) -> Value {
        json!({
            "linked": self.has_synth(),
            "synth": self.synth_label(),
            "known": self.stored_browser(install).is_some_and(|b| b.linked),
        })
    }

    /// The app was linked to a synth or unlinked from it.
    pub(crate) fn browsers_link_changed(&self, linked: bool) {
        if !linked {
            // The vault pairings were with the synth that is gone now.
            let mut p = self.persisted.write().unwrap();
            if p.browsers.iter().any(|b| b.linked) {
                for b in p.browsers.iter_mut() {
                    b.linked = false;
                }
                self.save_persisted(&p);
            }
        }
        for s in self.browsers.greeted() {
            let install = s.snapshot().install;
            let mut msg = self.status_for(&install);
            msg["type"] = json!("status");
            s.send(msg);
            if !linked {
                // Cookies are a function of the link: with no link no
                // site stays readable.
                s.send(json!({"type": "sites.clear"}));
            }
        }
        self.emit(CoreEvent::Browsers);
    }

    /// The owner took a site back: no browser keeps the right to read it.
    pub(crate) fn browsers_revoke_site(&self, site: &str) {
        if clean_site(site).is_some() {
            self.browsers.tell_all(&json!({"type": "site.revoke", "site": site}));
        }
    }

    /// May a browser keep its permission for this site after a read? Only
    /// while the owner's answer for the site is a standing one.
    fn keeps_site(&self, site: &str) -> bool {
        match self.settings().cookies.mode {
            CookiesMode::Auto => true,
            CookiesMode::AskNew => self.grants().allows("cookies", site),
            CookiesMode::Ask => false,
        }
    }

    /// The linked browser a cookie read would go to right now, and
    /// whether it may read `site` already.
    pub(crate) fn extension_for(&self, site: &str) -> Option<(String, bool)> {
        let s = self.linked_sessions().into_iter().next()?;
        let st = s.snapshot();
        Some((st.browser, st.sites.iter().any(|x| x == site)))
    }

    /// Connected extensions of browsers the owner linked, the one that
    /// connected last first.
    fn linked_sessions(&self) -> Vec<Arc<Session>> {
        let linked: Vec<String> = self
            .persisted
            .read()
            .unwrap()
            .browsers
            .iter()
            .filter(|b| b.linked)
            .map(|b| b.install.clone())
            .collect();
        let mut out: Vec<Arc<Session>> = self
            .browsers
            .greeted()
            .into_iter()
            .filter(|s| linked.contains(&s.state.lock().unwrap().install))
            .collect();
        out.reverse();
        out
    }

    /// Read a site's cookies through a linked browser. `site` is a
    /// registrable domain; `wait` is how long the owner has to confirm in
    /// the browser; `asked` says the owner answered a question about this
    /// site in the app a moment ago (the browser's window then reads as
    /// the second step of the same question).
    pub(crate) async fn cookies_via_extension(self: &Arc<Self>, site: &str, wait: Duration, asked: bool) -> ExtensionRead {
        if clean_site(site).as_deref() != Some(site) {
            return ExtensionRead::NoBrowser;
        }
        let Some(session) = self.linked_sessions().into_iter().next() else {
            return ExtensionRead::NoBrowser;
        };
        let st = session.snapshot();
        let name = browser_name(&st.browser);
        let ask = json!({"type": "cookies.get", "site": site, "synth": self.synth_label(), "asked": asked});
        let Some(reply) = self.browsers.ask(&session, ask, wait).await else {
            return ExtensionRead::NoAnswer(format!(
                "no answer from the user ({name} is waiting for the owner to confirm {site}; try again once they did)"
            ));
        };
        if reply.get("ok").and_then(Value::as_bool) != Some(true) {
            return match reply.get("error").and_then(Value::as_str).unwrap_or("") {
                "denied" => ExtensionRead::Denied(format!("the owner did not confirm {site} in {name}")),
                _ => ExtensionRead::Failed(format!("{name} could not read the cookies of {site}")),
            };
        }
        let cookies = clean_cookies(site, reply.get("cookies"));
        // The extension reads on a standing permission of the browser.
        // Unless the owner's answer for this site is a standing one too,
        // the browser gives that permission back now.
        if self.keeps_site(site) {
            let mut st = session.state.lock().unwrap();
            if !st.sites.iter().any(|x| x == site) {
                st.sites.push(site.to_string());
                st.sites.sort();
            }
        } else {
            session.send(json!({"type": "site.revoke", "site": site}));
            session.state.lock().unwrap().sites.retain(|x| x != site);
        }
        self.emit(CoreEvent::Browsers);
        // The synth keeps a source as a plain label (letters, digits,
        // dots, dashes), so this one is written that way from the start.
        ExtensionRead::Cookies(CookieRead { cookies, source: format!("{}-extension", st.browser) })
    }

    fn pair_refused(&self, session: &Session, id: u64, why: PairRefusal, journal: bool) {
        let st = session.snapshot();
        session.send(json!({"type": "pair.refused", "id": id, "error": why.code()}));
        if journal {
            let outcome = match why {
                PairRefusal::Denied => "denied: owner".to_string(),
                PairRefusal::Paused => "denied: paused by the owner".to_string(),
                other => format!("failed: {}", other.code()),
            };
            self.journal().record(Entry::new(CAP, "vault.pair", browser_name(&st.browser), outcome));
        }
    }

    /// Link one connected browser to the synth's vault. `ask` = the
    /// request came from the extension, so the owner confirms it here
    /// first; false = the owner clicked "link this browser" in this app.
    async fn pair(self: &Arc<Self>, session: Arc<Session>, id: u64, ask: bool) -> Result<(), PairRefusal> {
        {
            let mut st = session.state.lock().unwrap();
            if st.pairing || st.last_pair.is_some_and(|t| t.elapsed() < PAIR_COOLDOWN) {
                drop(st);
                self.pair_refused(&session, id, PairRefusal::Busy, false);
                return Err(PairRefusal::Busy);
            }
            st.pairing = true;
        }
        let result = self.pair_inner(&session, id, ask).await;
        {
            let mut st = session.state.lock().unwrap();
            st.pairing = false;
            st.last_pair = Some(Instant::now());
        }
        if let Err(why) = result {
            self.pair_refused(&session, id, why, true);
        }
        result
    }

    async fn pair_inner(self: &Arc<Self>, session: &Arc<Session>, id: u64, ask: bool) -> Result<(), PairRefusal> {
        let st = session.snapshot();
        let name = browser_name(&st.browser);
        if !self.has_synth() {
            return Err(PairRefusal::NotLinked);
        }
        if self.paused() {
            return Err(PairRefusal::Paused);
        }
        if ask {
            let req = ApprovalRequest::new(CAP, name, "", format!("link {name} to the synth's login vault"))
                .with_remember(Remember::Never);
            if !self.ask_scoped(CAP, name, req).await.allow {
                return Err(PairRefusal::Denied);
            }
        }

        if st.paired {
            // It holds a pairing already (made with a code typed by
            // hand). It is linked here only when that pairing is with
            // THIS synth's vault: the synth lists the browsers it knows.
            let list = self
                .requests
                .call("vault.list", json!({}))
                .await
                .map_err(|e| PairRefusal::from_link_error(&e))?;
            let known = parse_vault_list(&list).devices.iter().any(|d| !st.device_id.is_empty() && d.id == st.device_id);
            if !known {
                return Err(PairRefusal::OtherSynth);
            }
            self.update_browser(&st.install, |b| {
                b.linked = true;
                b.device_id = st.device_id.clone();
            });
            session.send(json!({"type": "pair.linked", "id": id, "synth": self.synth_label()}));
            self.journal().record(Entry::new(CAP, "vault.pair", name, "ok"));
            return Ok(());
        }

        let answer = self
            .requests
            .call("vault.pair_code", json!({"browser": st.browser}))
            .await
            .map_err(|e| {
                logln!("vault.pair_code: {e}");
                PairRefusal::from_link_error(&e)
            })?;
        let code = answer.get("code").and_then(Value::as_str).and_then(clean_code).ok_or(PairRefusal::Failed)?;
        session.state.lock().unwrap().code_sent = Some(Instant::now());
        // The code is short-lived and single use. It goes to the browser
        // the owner just confirmed and is written nowhere.
        if !session.send(json!({"type": "pair.code", "id": id, "code": code, "synth": self.synth_label()})) {
            return Err(PairRefusal::Failed);
        }
        Ok(())
    }

    /// The extension redeemed the code at the hub (or failed to).
    fn pair_done(&self, session: &Session, msg: &Value) {
        let waiting = {
            let mut st = session.state.lock().unwrap();
            st.code_sent.take().is_some_and(|t| t.elapsed() < PAIR_WINDOW)
        };
        if !waiting {
            return; // nobody asked: a "done" out of nowhere links nothing
        }
        let st = session.snapshot();
        let name = browser_name(&st.browser);
        if msg.get("ok").and_then(Value::as_bool) != Some(true) {
            self.journal().record(Entry::new(CAP, "vault.pair", name, "failed: the code was not accepted"));
            return;
        }
        let device_id = clean_device_id(msg.get("device_id").and_then(Value::as_str).unwrap_or(""));
        {
            let mut live = session.state.lock().unwrap();
            live.paired = true;
            live.device_id = device_id.clone();
        }
        self.update_browser(&st.install, |b| {
            b.linked = true;
            b.device_id = device_id;
        });
        self.journal().record(Entry::new(CAP, "vault.pair", name, "ok"));
    }

    fn hello(&self, session: &Session, msg: &Value) {
        let Some(install) = msg.get("install").and_then(Value::as_str).and_then(clean_install) else {
            session.send(json!({"type": "error", "error": "bad_hello"}));
            return;
        };
        if msg.get("v").and_then(Value::as_u64) != Some(u64::from(VERSION)) {
            session.send(json!({"type": "error", "error": "unsupported_version", "v": VERSION}));
            return;
        }
        let browser = clean_browser(msg.get("browser").and_then(Value::as_str).unwrap_or(""));
        let ext_version = clean_version(msg.get("ext_version").and_then(Value::as_str).unwrap_or(""));
        let paired = msg.get("paired").and_then(Value::as_bool).unwrap_or(false);
        let device_id = clean_device_id(msg.get("device_id").and_then(Value::as_str).unwrap_or(""));
        let sites = clean_sites(msg.get("sites"));
        {
            let mut st = session.state.lock().unwrap();
            st.greeted = true;
            st.browser = browser.clone();
            st.install = install.clone();
            st.ext_version = ext_version.clone();
            st.paired = paired;
            st.device_id = device_id.clone();
            st.sites = sites.clone();
        }
        self.update_browser(&install, |b| {
            b.browser = browser;
            b.ext_version = ext_version;
            b.last_seen = now_unix();
            // It lost its pairing (unpaired in the extension, or removed
            // at the hub), or holds another one now: no longer linked.
            if !paired || (!b.device_id.is_empty() && b.device_id != device_id) {
                b.linked = false;
                b.device_id = String::new();
            }
        });
        let mut ack = self.status_for(&install);
        ack["type"] = json!("hello_ack");
        ack["v"] = json!(VERSION);
        ack["app_version"] = json!(self.app_version);
        session.send(ack);

        // A site the owner no longer allows must not stay readable in
        // the browser. (With "never ask" there are no per-site answers
        // to compare with, so nothing is taken back.)
        if self.settings().cookies.mode != CookiesMode::Auto {
            for site in sites {
                if !self.grants().allows("cookies", &site) {
                    session.send(json!({"type": "site.revoke", "site": site}));
                    session.state.lock().unwrap().sites.retain(|x| *x != site);
                }
            }
        }
    }

    fn state_update(&self, session: &Session, msg: &Value) {
        let install = {
            let mut st = session.state.lock().unwrap();
            if let Some(paired) = msg.get("paired").and_then(Value::as_bool) {
                st.paired = paired;
                if !paired {
                    st.device_id.clear();
                }
            }
            if msg.get("sites").is_some() {
                st.sites = clean_sites(msg.get("sites"));
            }
            st.install.clone()
        };
        if msg.get("paired").and_then(Value::as_bool) == Some(false) {
            self.update_browser(&install, |b| {
                b.linked = false;
                b.device_id = String::new();
            });
        } else {
            self.emit(CoreEvent::Browsers);
        }
    }

    /// One message from an extension that has said hello.
    fn dispatch(self: &Arc<Self>, session: &Arc<Session>, ty: &str, msg: Value) {
        match ty {
            "hello" => self.hello(session, &msg),
            "pair.request" => {
                let id = msg.get("id").and_then(Value::as_u64).unwrap_or(0);
                let (me, session) = (self.clone(), session.clone());
                tokio::spawn(async move {
                    let _ = me.pair(session, id, true).await;
                });
            }
            "pair.done" => self.pair_done(session, &msg),
            "state" => self.state_update(session, &msg),
            "cookies.result" => self.browsers.answer(session.id, msg),
            _ => {} // a newer extension may say more; ignored
        }
    }

    /// Serve one helper connection until it closes.
    pub(crate) async fn serve_browser(self: Arc<Self>, conn: Conn) {
        let mut reader = BufReader::new(conn.reader);
        // The helper's first line names the extension the browser
        // started it for. Checked by the browser, by the helper, and here.
        let open = tokio::time::timeout(OPEN_TIMEOUT, frame::read_line(&mut reader, 4096)).await;
        let origin = match open {
            Ok(Ok(v)) if v["type"] == "host.open" && v["v"] == json!(connect_host::host::VERSION) => {
                v["origin"].as_str().unwrap_or("").to_string()
            }
            _ => return,
        };
        if !connect_host::origin_allowed(&origin) {
            logln!("browser link: a connection for an unknown extension was closed");
            return;
        }
        if self.browsers.sessions.lock().unwrap().len() >= MAX_SESSIONS {
            return;
        }

        let (tx, mut rx) = mpsc::channel::<Value>(OUT_QUEUE);
        let session = Arc::new(Session {
            id: self.browsers.next_session.fetch_add(1, Ordering::Relaxed) + 1,
            tx,
            state: Mutex::new(SessionState::default()),
        });
        self.browsers.sessions.lock().unwrap().push(session.clone());
        // Leaves the list however this task ends (the helper went away,
        // or the app shuts the link down and the task is dropped).
        struct Leave(Arc<Shared>, u64, tokio::task::JoinHandle<()>);
        impl Drop for Leave {
            fn drop(&mut self) {
                self.2.abort();
                self.0.browsers.remove(self.1);
                self.0.emit(CoreEvent::Browsers);
            }
        }

        let mut writer = conn.writer;
        let write = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                // The helper would drop what the browser refuses anyway.
                if serde_json::to_vec(&msg).map(|b| b.len()).unwrap_or(usize::MAX) > MAX_TO_BROWSER {
                    continue;
                }
                if frame::write_line(&mut writer, &msg).await.is_err() {
                    break;
                }
            }
        });

        let _leave = Leave(self.clone(), session.id, write);

        loop {
            // A little over the helper's own limit: it re-serializes.
            match frame::read_line(&mut reader, MAX_FROM_BROWSER + 4096).await {
                Ok(msg) => {
                    let ty = msg["type"].as_str().unwrap_or("").to_string();
                    if ty != "hello" && !session.state.lock().unwrap().greeted {
                        continue; // nothing is heard before the hello
                    }
                    self.dispatch(&session, &ty, msg);
                }
                Err(FrameError::Malformed(_)) => continue,
                Err(_) => break,
            }
        }
    }
}

impl Core {
    /// Listen for the extension's helper on `endpoint` until shutdown.
    /// Returns an error at once when the endpoint cannot be bound (a
    /// second copy of the app, a folder that is not ours).
    pub async fn run_browser_link(
        &self,
        endpoint: &connect_host::endpoint::Endpoint,
        mut shutdown: watch::Receiver<bool>,
    ) -> std::io::Result<()> {
        // An app that restarts (an update) can come up a moment before
        // the endpoint of its previous run is gone: try for a few seconds
        // before deciding that another copy of the app holds it.
        let mut tries = 0;
        let mut listener = loop {
            match Listener::bind(endpoint) {
                Ok(l) => break l,
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && tries < 10 => {
                    tries += 1;
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(300)) => {}
                        _ = shutdown.changed() => return Ok(()),
                    }
                }
                Err(e) => return Err(e),
            }
        };
        // Dropped on return: every open connection closes with the
        // listener, so a helper learns at once that the app is gone.
        let mut serving = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                conn = listener.accept() => match conn {
                    Ok(conn) => {
                        serving.spawn(self.shared().serve_browser(conn));
                    }
                    Err(e) => {
                        logln!("browser link: accept: {e}");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                },
                Some(_) = serving.join_next(), if !serving.is_empty() => {}
                _ = shutdown.changed() => return Ok(()),
            }
        }
    }

    /// Serve one connection directly (tests, and hosts with their own
    /// listener).
    pub async fn serve_browser(&self, conn: Conn) {
        self.shared().serve_browser(conn).await
    }

    /// The browsers whose extension has talked to this app, connected or
    /// not.
    pub fn browsers(&self) -> Vec<BrowserView> {
        let shared = self.shared();
        let stored = shared.persisted.read().unwrap().browsers.clone();
        let live = shared.browsers.greeted();
        let mut out: Vec<BrowserView> = stored
            .into_iter()
            .map(|b| {
                let session = live.iter().rev().find(|s| s.state.lock().unwrap().install == b.install);
                let sites = session.map(|s| s.snapshot().sites).unwrap_or_default();
                BrowserView {
                    install: b.install,
                    browser: b.browser,
                    connected: session.is_some(),
                    linked: b.linked,
                    ext_version: b.ext_version,
                    sites,
                    last_seen: b.last_seen,
                }
            })
            .collect();
        out.sort_by(|a, b| (a.browser.as_str(), a.install.as_str()).cmp(&(b.browser.as_str(), b.install.as_str())));
        out
    }

    /// The owner clicked "link this browser" in the app. The browser's
    /// extension must be connected.
    pub async fn browser_link(&self, install: &str) -> Result<(), PairRefusal> {
        let shared = self.shared();
        let session = shared.browsers.by_install(install).ok_or(PairRefusal::Failed)?;
        shared.pair(session, 0, false).await
    }

    /// Unlink one browser: its extension forgets the vault pairing and
    /// every site permission, and the synth removes the pairing at the
    /// vault. What the browser saved to the vault earlier stays there.
    pub async fn browser_unlink(&self, install: &str) -> Result<(), String> {
        let shared = self.shared();
        let stored = shared.stored_browser(install).ok_or("unknown browser")?;
        if let Some(session) = shared.browsers.by_install(install) {
            session.send(json!({"type": "unpair"}));
            session.send(json!({"type": "sites.clear"}));
            let mut st = session.state.lock().unwrap();
            st.paired = false;
            st.device_id.clear();
            st.sites.clear();
        }
        shared.update_browser(install, |b| {
            b.linked = false;
            b.device_id = String::new();
        });
        shared
            .journal()
            .record(Entry::new(CAP, "vault.unpair", browser_name(&stored.browser), "ok"));
        if stored.device_id.is_empty() {
            return Ok(());
        }
        // Best effort: when the synth is offline the pairing stays at the
        // vault until the owner removes it there (the synth can list it).
        shared
            .requests
            .call("vault.revoke_device", json!({"device_id": stored.device_id}))
            .await
            .map(|_| ())
    }

    /// Forget a browser that is not connected (it is gone, or its
    /// extension was removed).
    pub fn browser_forget(&self, install: &str) {
        let shared = self.shared();
        {
            let mut p = shared.persisted.write().unwrap();
            let before = p.browsers.len();
            p.browsers.retain(|b| b.install != install || shared.browsers.by_install(install).is_some());
            if p.browsers.len() != before {
                shared.save_persisted(&p);
            }
        }
        shared.emit(CoreEvent::Browsers);
    }

    /// Which sites the synth's vault holds a login for, and which
    /// browsers are paired with it. Names and dates only: there is no
    /// request that returns a login.
    pub async fn vault_list(&self) -> Result<VaultList, String> {
        let v = self.shared().requests.call("vault.list", json!({})).await?;
        Ok(parse_vault_list(&v))
    }

    /// Delete one login from the synth's vault.
    pub async fn vault_forget(&self, site: &str) -> Result<(), String> {
        let shared = self.shared();
        let site = short_text(Some(&json!(site)), 253).to_ascii_lowercase();
        if !site.contains('.') || !site.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.') {
            return Err("bad request: not a site".into());
        }
        let done = shared.requests.call("vault.forget", json!({"site": site})).await;
        let outcome = match &done {
            Ok(_) => "ok".to_string(),
            Err(e) => format!("failed: {e}"),
        };
        shared.journal().record(Entry::new(CAP, "vault.forget", site, outcome));
        done.map(|_| ())
    }
}

#[cfg(test)]
mod tests;
