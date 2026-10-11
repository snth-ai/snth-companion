//! `Core`: the one object a UI (or the headless CLI) holds. It owns the
//! settings, grants, journal, cookie source, secret store and Cua
//! manager, drives the link, and answers commands. Everything is async,
//! `Send + Sync`, and there are no globals, so a Tauri shell can keep a
//! single instance behind its managed state.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::{broadcast, oneshot, watch};

use crate::cookies::CookieSource;
use crate::cua::{self, PrivateDaemon};
use crate::grant::{Approval, ApprovalRequest, Approver, GrantStore, APPROVAL_TIMEOUT};
use crate::journal::{Entry, Journal};
use crate::pairing::{HubClient, Pending};
use crate::proto::Refusal;
use crate::secret::SecretStore;
use crate::settings::{self, Persisted, Preset, Root, Settings};
use crate::wire;

/// The keychain key the pairing token is stored under.
pub const TOKEN_KEY: &str = "pairing_token";

/// Which Cua daemon the mcp pipe attaches to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CuaAttach {
    /// Our own private embedded daemon (the product path).
    Private,
    /// The user's running daemon, read-only (testing / fallback). We
    /// never start, stop or reconfigure it.
    User,
}

/// Where the synth link stands, for the UI status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkState {
    /// No token yet, or unpaired.
    Unpaired,
    /// Dialing / waiting for hello_ack.
    Connecting,
    /// hello_ack received; the link is up.
    Linked,
    /// Was up, now reconnecting.
    Reconnecting,
    /// Paused by the owner.
    Paused,
}

/// Events for the UI. Journal entries have their own subscription.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum CoreEvent {
    Link { state: LinkState, detail: String },
    Settings { rev: u64 },
    Paired { synth_id: String },
    Unpaired,
    Paused { paused: bool },
}

/// Monthly egress accounting.
#[derive(Default)]
struct Counters {
    month: String,
    bytes: u64,
}

fn month_key() -> String {
    // YYYY-MM from the wall clock, good enough for a monthly cap.
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs / 86_400;
    // Rough civil date from unix days (proleptic Gregorian).
    let (y, m, _d) = civil_from_days(days);
    format!("{y:04}-{m:02}")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The shared handler context (an `Arc<Shared>` is passed to link and
/// stream handlers).
pub struct Shared {
    pub(crate) persisted: RwLock<Persisted>,
    settings_path: Option<PathBuf>,
    pub(crate) grants: GrantStore,
    pub(crate) approver: Arc<dyn Approver>,
    pub(crate) journal: Arc<Journal>,
    pub(crate) cookies: Arc<dyn CookieSource>,
    secret: Arc<dyn SecretStore>,
    hub: Arc<dyn HubClient>,
    pub(crate) app_version: String,
    pub(crate) cua_bin: String,
    pub(crate) cua_attach: CuaAttach,
    pub(crate) cua_daemon: tokio::sync::Mutex<Option<PrivateDaemon>>,

    paused: AtomicBool,
    metered: AtomicBool,
    pub(crate) stopping: AtomicBool, // unpair asked the link loop to stop reconnecting
    counters: Mutex<Counters>,

    identity: RwLock<Option<Pending>>, // confirmed pairing (url, id, token-in-secret)
    pending: Mutex<Option<Pending>>,   // claimed, not yet confirmed
    pub(crate) synth: RwLock<Option<wire::HelloAck>>,
    link_state: RwLock<LinkState>,

    events: broadcast::Sender<CoreEvent>,
    pub(crate) settings_tx: watch::Sender<u64>, // rev, so the link pushes a settings msg
    pub(crate) control_tx: watch::Sender<u64>,  // bumped on pause/resume so the link reacts
    pub(crate) unpair_req: tokio::sync::Notify,  // the owner asked to unpair
    pub(crate) unpair_ack: tokio::sync::Notify,  // the synth acknowledged
}

impl Shared {
    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }
    pub fn metered(&self) -> bool {
        self.metered.load(Ordering::Acquire)
    }
    pub fn settings(&self) -> Settings {
        self.persisted.read().unwrap().settings.clone()
    }
    pub fn settings_rev(&self) -> u64 {
        self.persisted.read().unwrap().rev
    }
    pub fn device_id(&self) -> String {
        self.persisted.read().unwrap().device_id.clone()
    }
    pub fn device_name(&self) -> String {
        wire::hostname()
    }
    pub fn journal(&self) -> &Journal {
        &self.journal
    }
    pub fn grants(&self) -> &GrantStore {
        &self.grants
    }

    pub(crate) fn set_link_state(&self, s: LinkState, detail: &str) {
        *self.link_state.write().unwrap() = s;
        let _ = self.events.send(CoreEvent::Link {
            state: s,
            detail: detail.to_string(),
        });
    }

    /// The link learned the synth's identity from hello_ack.
    pub(crate) fn set_synth(&self, ack: wire::HelloAck) {
        *self.synth.write().unwrap() = Some(ack);
    }

    /// Nudge the control watch so the link re-sends presence. Computes the
    /// next value WITHOUT holding the borrow across `send` (that would
    /// deadlock the watch's internal lock).
    fn bump_control(&self) {
        let next = { self.control_tx.borrow().wrapping_add(1) };
        let _ = self.control_tx.send(next);
    }

    /// The token for the link, from the secret store.
    pub fn token(&self) -> Option<String> {
        self.secret.get(TOKEN_KEY).ok().flatten()
    }

    /// The confirmed synth WebSocket url, if paired.
    pub(crate) fn synth_ws_url(&self) -> Option<String> {
        self.identity.read().unwrap().as_ref().map(|p| p.ws_url())
    }

    /// Ask the owner, bounded at 55 s. A late answer still records a grant
    /// for (capability, scope), so it applies to the next attempt. On
    /// timeout the caller gets a deny.
    pub async fn ask_scoped(
        self: &Arc<Self>,
        capability: &str,
        scope: &str,
        req: ApprovalRequest,
    ) -> Approval {
        let (tx, rx) = oneshot::channel();
        let fut = self.approver.ask(req);
        let me = self.clone();
        let cap = capability.to_string();
        let sc = scope.to_string();
        tokio::spawn(async move {
            let appr = fut.await;
            me.grants.record(&cap, &sc, appr);
            let _ = tx.send(appr);
        });
        match tokio::time::timeout(APPROVAL_TIMEOUT, rx).await {
            Ok(Ok(appr)) => appr,
            _ => Approval::deny(),
        }
    }

    // --- net policy ---

    /// Decide a tcp dial against net settings and grants. Returns Ok to
    /// dial, or a labeled refusal. `host` is the target host (the address
    /// policy itself lives in net.rs).
    pub async fn net_allows(self: &Arc<Self>, host: &str, reason: &str) -> Result<(), Refusal> {
        if self.paused() {
            return Err(Refusal::Denied("net is paused by the owner".into()));
        }
        let s = self.settings();
        if !s.net.enabled {
            return Err(Refusal::Denied("net is turned off on this computer".into()));
        }
        if s.net.skip_on_metered && self.metered() {
            return Err(Refusal::Denied("the connection is metered".into()));
        }
        if s.net.monthly_cap_bytes > 0 {
            let used = self.counters.lock().unwrap().bytes;
            if used >= s.net.monthly_cap_bytes {
                return Err(Refusal::Denied("the monthly traffic cap is reached".into()));
            }
        }
        let domain = registrable_domain(host);
        match s.net.mode {
            crate::settings::NetMode::All => Ok(()),
            crate::settings::NetMode::List => {
                if s.net.domains.iter().any(|d| d == &domain) {
                    Ok(())
                } else {
                    Err(Refusal::Denied(format!(
                        "{domain} is not on the allowed list"
                    )))
                }
            }
            crate::settings::NetMode::Ask => {
                if s.net.domains.iter().any(|d| d == &domain)
                    || self.grants.allows("net", &domain)
                {
                    return Ok(());
                }
                let req = ApprovalRequest::new(
                    "net",
                    &domain,
                    reason,
                    format!("let the synth reach {domain} through this computer"),
                );
                let appr = self.ask_scoped("net", &domain, req).await;
                if appr.allow {
                    Ok(())
                } else {
                    Err(Refusal::Denied(format!(
                        "the owner did not allow traffic to {domain}"
                    )))
                }
            }
        }
    }

    /// Add egress bytes to the month counter.
    pub fn add_egress(&self, n: u64) {
        let mut c = self.counters.lock().unwrap();
        let m = month_key();
        if c.month != m {
            c.month = m;
            c.bytes = 0;
        }
        c.bytes = c.bytes.saturating_add(n);
    }

    pub fn allow_local_network(&self) -> bool {
        self.settings().net.allow_local_network
    }

    // --- cua ---

    /// The socket + embedded flag the mcp pipe should use, starting our
    /// private daemon on demand. A labeled refusal when computer use is
    /// off/paused or the daemon cannot be reached.
    pub async fn cua_socket(self: &Arc<Self>) -> Result<(String, bool), Refusal> {
        if self.paused() {
            return Err(Refusal::Denied("computer use is paused by the owner".into()));
        }
        if !self.settings().computer.enabled {
            return Err(Refusal::Denied("computer use is turned off on this computer".into()));
        }
        match self.cua_attach {
            CuaAttach::User => {
                let socket = user_daemon_socket(&self.cua_bin).await?;
                Ok((socket, false))
            }
            CuaAttach::Private => {
                let mut guard = self.cua_daemon.lock().await;
                if guard.is_none() {
                    let s = self.settings();
                    let d = PrivateDaemon::start(&self.cua_bin, &s).await.map_err(|e| {
                        Refusal::Failed(format!("unavailable: start private Cua daemon: {e}"))
                    })?;
                    *guard = Some(d);
                }
                Ok((guard.as_ref().unwrap().socket.clone(), true))
            }
        }
    }

    /// The hello `computer.status` label.
    pub async fn cua_status(&self) -> (String, String) {
        let d = cua::detect(&self.cua_bin).await;
        if !d.installed {
            return (String::new(), cua::Status::NotInstalled.label());
        }
        let version = d.version.unwrap_or_default();
        if !self.settings().computer.enabled {
            return (version, cua::Status::Ok.label());
        }
        match self.cua_attach {
            CuaAttach::User => match user_daemon_socket(&self.cua_bin).await {
                Ok(_) => (version, cua::Status::Ok.label()),
                Err(_) => (version, cua::Status::DaemonDown.label()),
            },
            // For the private path we report ok on install; the daemon is
            // started on demand. Permission state is checked by the UI.
            CuaAttach::Private => (version, cua::Status::Ok.label()),
        }
    }

    async fn stop_cua(&self) {
        let d = self.cua_daemon.lock().await.take();
        if let Some(d) = d {
            d.stop(&self.cua_bin).await;
        }
    }
}

/// The public handle. Clone is cheap (an `Arc`).
#[derive(Clone)]
pub struct Core {
    shared: Arc<Shared>,
}

/// Everything a `Core` needs that the host provides.
pub struct CoreConfig {
    /// Where settings.json lives (None = in-memory, for tests).
    pub settings_path: Option<PathBuf>,
    /// Where the journal JSONL lives (None = ring + broadcast only).
    pub journal_path: Option<PathBuf>,
    pub approver: Arc<dyn Approver>,
    pub cookies: Arc<dyn CookieSource>,
    pub secret: Arc<dyn SecretStore>,
    pub hub: Arc<dyn HubClient>,
    pub app_version: String,
    pub cua_bin: String,
    pub cua_attach: CuaAttach,
}

impl Core {
    pub fn new(cfg: CoreConfig) -> Core {
        let persisted = match &cfg.settings_path {
            Some(p) => settings::load(p, new_device_id).unwrap_or_else(|_| {
                settings::Persisted::fresh(new_device_id())
            }),
            None => settings::Persisted::fresh(new_device_id()),
        };
        let (events, _) = broadcast::channel(256);
        let (settings_tx, _) = watch::channel(persisted.rev);
        let (control_tx, _) = watch::channel(0);
        let shared = Arc::new(Shared {
            persisted: RwLock::new(persisted),
            settings_path: cfg.settings_path,
            grants: GrantStore::new(),
            approver: cfg.approver,
            journal: Arc::new(Journal::new(cfg.journal_path)),
            cookies: cfg.cookies,
            secret: cfg.secret,
            hub: cfg.hub,
            app_version: cfg.app_version,
            cua_bin: cfg.cua_bin,
            cua_attach: cfg.cua_attach,
            cua_daemon: tokio::sync::Mutex::new(None),
            paused: AtomicBool::new(false),
            metered: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            counters: Mutex::new(Counters {
                month: month_key(),
                bytes: 0,
            }),
            identity: RwLock::new(None),
            pending: Mutex::new(None),
            synth: RwLock::new(None),
            link_state: RwLock::new(LinkState::Unpaired),
            events,
            settings_tx,
            control_tx,
            unpair_req: tokio::sync::Notify::new(),
            unpair_ack: tokio::sync::Notify::new(),
        });
        // A stored token means we are already paired: reflect it.
        if let Some(tok) = shared.token() {
            if !tok.is_empty() {
                // identity url/id are not persisted in P1; the link still
                // works from the token + a stored synth url. For the CLI
                // the url is passed in run(). Mark as paired-but-idle.
                shared.set_link_state(LinkState::Connecting, "token present");
            }
        }
        Core { shared }
    }

    /// Internal handle for the link and handlers.
    pub fn shared(&self) -> Arc<Shared> {
        self.shared.clone()
    }

    // --- events + journal ---

    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.shared.events.subscribe()
    }
    pub fn subscribe_journal(&self) -> broadcast::Receiver<Entry> {
        self.shared.journal.subscribe()
    }
    pub fn recent_journal(&self) -> Vec<Entry> {
        self.shared.journal.recent()
    }

    // --- settings ---

    /// Apply a change under a closure, bump the rev, persist atomically,
    /// and signal the link to push a settings message.
    pub fn mutate_settings(&self, f: impl FnOnce(&mut Settings)) -> u64 {
        let rev = {
            let mut p = self.shared.persisted.write().unwrap();
            f(&mut p.settings);
            p.rev += 1;
            if let Some(path) = &self.shared.settings_path {
                let _ = settings::save(path, &p);
            }
            p.rev
        };
        let _ = self.shared.settings_tx.send(rev);
        let _ = self.shared.events.send(CoreEvent::Settings { rev });
        rev
    }

    pub fn apply_preset(&self, preset: Preset) -> u64 {
        self.mutate_settings(|s| {
            let roots = s.files.roots.clone();
            *s = Settings::from_preset(preset, roots);
        })
    }

    pub fn settings(&self) -> Settings {
        self.shared.settings()
    }

    pub fn add_root(&self, path: PathBuf, write: bool) -> u64 {
        self.mutate_settings(|s| {
            s.files.roots.retain(|r| r.path != path);
            s.files.roots.push(Root { path, write });
            s.files.enabled = true;
            s.preset = Preset::Custom;
        })
    }

    pub fn remove_root(&self, path: &std::path::Path) -> u64 {
        self.mutate_settings(|s| {
            s.files.roots.retain(|r| r.path != path);
            s.preset = Preset::Custom;
        })
    }

    /// The default roots onboarding offers: Desktop and Downloads, when
    /// they exist.
    pub fn default_roots() -> Vec<Root> {
        let mut v = Vec::new();
        if let Some(base) = directories::UserDirs::new() {
            if let Some(d) = base.desktop_dir() {
                if d.is_dir() {
                    v.push(Root { path: d.to_path_buf(), write: true });
                }
            }
            if let Some(home) = Some(base.home_dir()) {
                let dl = home.join("Downloads");
                if dl.is_dir() {
                    v.push(Root { path: dl, write: true });
                }
            }
        }
        v
    }

    // --- grants ---

    pub fn grant_list(&self) -> Vec<(String, String, i64)> {
        self.shared.grants.list()
    }
    pub fn revoke(&self, capability: &str, scope: &str) {
        self.shared.grants.revoke(capability, scope);
    }

    // --- pause ---

    pub async fn pause(&self) {
        self.shared.paused.store(true, Ordering::Release);
        self.shared.stop_cua().await;
        self.shared.set_link_state(LinkState::Paused, "paused by the owner");
        let _ = self.shared.events.send(CoreEvent::Paused { paused: true });
        self.shared.bump_control();
    }

    pub fn resume(&self) {
        self.shared.paused.store(false, Ordering::Release);
        // Restore the link status if we had paused a live link.
        if self.shared.synth.read().unwrap().is_some() {
            self.shared.set_link_state(LinkState::Linked, "resumed");
        }
        let _ = self.shared.events.send(CoreEvent::Paused { paused: false });
        self.shared.bump_control();
    }

    // --- approvals (the UI can also answer by implementing Approver) ---

    // --- pairing ---

    /// Claim a code against the hub. The result is held in memory only;
    /// NOTHING is stored and no link is opened until `pair_confirm`.
    /// Returns the synth identity to show on the confirmation screen.
    pub async fn pair_begin(&self, hub_url: String, code: String) -> Result<Pending, String> {
        let hub = self.shared.hub.clone();
        let pending = tokio::task::spawn_blocking(move || hub.claim(&hub_url, &code))
            .await
            .map_err(|e| format!("join: {e}"))?
            .map_err(|e| e.to_string())?;
        let p = Pending::from_claim(pending);
        *self.shared.pending.lock().unwrap() = Some(p.clone());
        Ok(p)
    }

    /// Confirm the pending pairing: store the token and remember the synth
    /// url/id. Only now may a link open.
    pub fn pair_confirm(&self) -> Result<Pending, String> {
        let p = self
            .shared
            .pending
            .lock()
            .unwrap()
            .take()
            .ok_or("no pending pairing to confirm")?;
        self.shared
            .secret
            .set(TOKEN_KEY, &p.token)
            .map_err(|e| format!("store token: {e}"))?;
        self.shared.stopping.store(false, Ordering::Release);
        *self.shared.identity.write().unwrap() = Some(p.clone());
        self.shared.set_link_state(LinkState::Connecting, "paired");
        let _ = self.shared.events.send(CoreEvent::Paired {
            synth_id: p.synth_id.clone(),
        });
        Ok(p)
    }

    /// Unpair: tell the synth, wait briefly for `unpair_ack`, then wipe
    /// the token, grants and local state regardless of the answer.
    pub async fn unpair(&self) {
        // Stop the reconnect loop so a link that drops during unpair does
        // not race back online before we wipe the token.
        self.shared.stopping.store(true, Ordering::Release);
        // Send the unpair over any live link (Linked or Paused), then wait
        // briefly for the ack. We wipe regardless of the answer.
        let live = !matches!(
            *self.shared.link_state.read().unwrap(),
            LinkState::Unpaired | LinkState::Connecting | LinkState::Reconnecting
        );
        if live {
            self.shared.unpair_req.notify_one();
            // The synth acks then closes at once, so our reader may see EOF
            // before the ack; a short bounded wait is enough and we wipe
            // regardless (connect.md).
            let _ = tokio::time::timeout(
                Duration::from_secs(3),
                self.shared.unpair_ack.notified(),
            )
            .await;
        }
        self.wipe_local();
    }

    /// Drop a pending pairing without storing anything.
    pub fn pair_cancel(&self) {
        *self.shared.pending.lock().unwrap() = None;
    }

    pub fn pending(&self) -> Option<Pending> {
        self.shared.pending.lock().unwrap().clone()
    }

    /// The confirmed synth url, if paired.
    pub fn synth_url(&self) -> Option<String> {
        self.shared.identity.read().unwrap().as_ref().map(|p| p.ws_url())
    }

    /// Wipe the token, grants and pending/identity. Called after a bounded
    /// wait for `unpair_ack` (the link sends `unpair` first). Idempotent.
    pub fn wipe_local(&self) {
        let _ = self.shared.secret.delete(TOKEN_KEY);
        self.shared.grants.clear();
        *self.shared.identity.write().unwrap() = None;
        *self.shared.pending.lock().unwrap() = None;
        *self.shared.synth.write().unwrap() = None;
        self.shared.set_link_state(LinkState::Unpaired, "unpaired");
        let _ = self.shared.events.send(CoreEvent::Unpaired);
    }

    // --- cua helpers for the UI ---

    pub fn cua_install_argv(&self) -> Vec<String> {
        cua::install_argv(&self.shared.cua_bin)
    }
    pub fn cua_permissions_grant_argv(&self) -> Vec<String> {
        cua::permissions_grant_argv(&self.shared.cua_bin)
    }
    pub async fn cua_permissions_status(&self) -> std::io::Result<cua::Permissions> {
        cua::permissions_status(&self.shared.cua_bin, None).await
    }

    // --- snapshot ---

    pub async fn snapshot(&self) -> Snapshot {
        let (cua_version, cua_status) = self.shared.cua_status().await;
        let presence = crate::presence::read();
        let counters = {
            let c = self.shared.counters.lock().unwrap();
            (c.month.clone(), c.bytes)
        };
        let synth = self.shared.synth.read().unwrap().clone();
        Snapshot {
            link_state: *self.shared.link_state.read().unwrap(),
            synth_id: synth.as_ref().map(|s| s.synth_id.clone()),
            synth_name: synth.as_ref().map(|s| s.synth_name.clone()),
            synth_version: synth.as_ref().map(|s| s.synth_version.clone()),
            idle_seconds: presence.idle_seconds,
            screen_locked: presence.screen_locked,
            paused: self.shared.paused(),
            metered: self.shared.metered(),
            settings: self.shared.settings(),
            settings_rev: self.shared.settings_rev(),
            cua_version,
            cua_status,
            egress_month: counters.0,
            egress_bytes: counters.1,
        }
    }

    /// Run the link loop until shutdown. The CLI calls this; a Tauri shell
    /// spawns it. `ws_url` overrides the paired synth url (for tests).
    pub async fn run_link(&self, ws_url: Option<String>, shutdown: watch::Receiver<bool>) {
        crate::link::run_forever(self.shared.clone(), ws_url, shutdown).await;
    }
}

/// A read-only picture of the core for the UI.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub link_state: LinkState,
    pub synth_id: Option<String>,
    pub synth_name: Option<String>,
    pub synth_version: Option<String>,
    pub idle_seconds: u64,
    pub screen_locked: bool,
    pub paused: bool,
    pub metered: bool,
    pub settings: Settings,
    pub settings_rev: u64,
    pub cua_version: String,
    pub cua_status: String,
    pub egress_month: String,
    pub egress_bytes: u64,
}

fn new_device_id() -> String {
    use rand::RngCore;
    use std::fmt::Write;
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    let mut hex = String::from("dev-");
    for x in b {
        let _ = write!(hex, "{x:02x}");
    }
    hex
}

pub(crate) fn registrable_domain(host: &str) -> String {
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    if host.parse::<std::net::IpAddr>().is_ok() {
        return host;
    }
    match psl::domain_str(&host) {
        Some(d) => d.to_string(),
        None => host,
    }
}

/// Read the user's running daemon socket from `cua-driver status`
/// (read-only; we never touch that daemon).
async fn user_daemon_socket(bin: &str) -> Result<String, Refusal> {
    let out = tokio::process::Command::new(bin)
        .arg("status")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| Refusal::Failed(format!("unavailable: cannot run {bin}: {e}")))?;
    let text = String::from_utf8_lossy(&out.stdout);
    if !text.contains("daemon is running") {
        return Err(Refusal::Failed(
            "unavailable: the Cua Driver daemon is not running".into(),
        ));
    }
    for line in text.lines() {
        if let Some(p) = line.trim().strip_prefix("socket:") {
            let p = p.trim();
            if !p.is_empty() {
                return Ok(p.to_string());
            }
        }
    }
    Err(Refusal::Failed(
        "unavailable: cua-driver status reported no socket".into(),
    ))
}

/// Keep the one-grant demo note alive: the Drop here stops a lingering
/// private daemon if the Core is dropped mid-run.
impl Drop for Shared {
    fn drop(&mut self) {
        if let Ok(mut g) = self.cua_daemon.try_lock() {
            if let Some(d) = g.take() {
                // Best-effort: we are in a sync Drop, so just kill the
                // child; the temp dir is cleaned on next boot.
                let _ = d.pid; // pid recorded; kill_on_drop handles the child
                drop(d);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cookies::MultiReader;
    use crate::grant::ApprovalFuture;
    use crate::pairing::{ClaimError, ClaimResult};
    use crate::secret::MemSecretStore;
    use std::sync::Arc;

    struct AutoAllow;
    impl Approver for AutoAllow {
        fn ask(&self, _req: ApprovalRequest) -> ApprovalFuture {
            Box::pin(async { Approval::always() })
        }
    }

    struct FakeHub;
    impl HubClient for FakeHub {
        fn claim(&self, _hub: &str, code: &str) -> Result<ClaimResult, ClaimError> {
            if code == "123456" {
                Ok(ClaimResult {
                    synth_url: "https://mia.snth.ai".into(),
                    synth_id: "mia".into(),
                    companion_token: "tok-abc".into(),
                })
            } else {
                Err(ClaimError::NotFound)
            }
        }
    }

    fn test_core() -> (Core, Arc<MemSecretStore>) {
        let secret = Arc::new(MemSecretStore::new());
        let cookies = Arc::new(MultiReader {
            chromium: vec![],
            firefox: vec![],
            keys: Arc::new(crate::cookies::fixtures::NoKey),
        });
        let core = Core::new(CoreConfig {
            settings_path: None,
            journal_path: None,
            approver: Arc::new(AutoAllow),
            cookies,
            secret: secret.clone(),
            hub: Arc::new(FakeHub),
            app_version: "0.1.0-test".into(),
            cua_bin: "cua-driver".into(),
            cua_attach: CuaAttach::User,
        });
        (core, secret)
    }

    #[tokio::test]
    async fn settings_rev_grows_and_preset_applies() {
        let (core, _) = test_core();
        let r0 = core.settings().net.enabled;
        assert!(!r0);
        let rev = core.apply_preset(Preset::Balanced);
        assert_eq!(rev, 1);
        assert!(core.settings().net.enabled);
        let rev2 = core.add_root(PathBuf::from("/tmp"), true);
        assert_eq!(rev2, 2);
    }

    #[tokio::test]
    async fn pairing_requires_confirmation() {
        let (core, secret) = test_core();
        // begin stores nothing
        let p = core.pair_begin("https://hub".into(), "123456".into()).await.unwrap();
        assert_eq!(p.synth_id, "mia");
        assert_eq!(secret.get(TOKEN_KEY).unwrap(), None);
        // confirm stores the token
        core.pair_confirm().unwrap();
        assert_eq!(secret.get(TOKEN_KEY).unwrap().as_deref(), Some("tok-abc"));
        assert_eq!(core.synth_url().as_deref(), Some("wss://mia.snth.ai/api/connect/ws"));
        // unpair wipes it
        core.wipe_local();
        assert_eq!(secret.get(TOKEN_KEY).unwrap(), None);
    }

    #[tokio::test]
    async fn pair_cancel_forgets() {
        let (core, secret) = test_core();
        core.pair_begin("https://hub".into(), "123456".into()).await.unwrap();
        core.pair_cancel();
        assert!(core.pending().is_none());
        core.pair_confirm().unwrap_err();
        assert_eq!(secret.get(TOKEN_KEY).unwrap(), None);
    }

    #[tokio::test]
    async fn bad_code_is_rejected() {
        let (core, _) = test_core();
        let e = core.pair_begin("https://hub".into(), "000000".into()).await.unwrap_err();
        assert!(e.contains("not found"));
    }

    #[tokio::test]
    async fn net_ask_grant_then_cached() {
        let (core, _) = test_core();
        core.apply_preset(Preset::Careful); // net = ask
        let sh = core.shared();
        // AutoAllow grants Always, so the first ask caches the domain.
        sh.net_allows("www.youtube.com", "the synth says: watch a video").await.unwrap();
        assert!(sh.grants().allows("net", "youtube.com"));
        // second call passes without asking (grant cached)
        sh.net_allows("m.youtube.com", "again").await.unwrap();
    }

    #[tokio::test]
    async fn net_list_mode_enforced() {
        let (core, _) = test_core();
        core.mutate_settings(|s| {
            s.net.enabled = true;
            s.net.mode = crate::settings::NetMode::List;
            s.net.domains = vec!["example.com".into()];
        });
        let sh = core.shared();
        assert!(sh.net_allows("www.example.com", "").await.is_ok());
        assert!(sh.net_allows("evil.com", "").await.is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn approval_times_out_then_a_late_grant_applies_to_the_next_attempt() {
        use crate::grant::ApprovalFuture;
        struct Delayed;
        impl Approver for Delayed {
            fn ask(&self, _req: ApprovalRequest) -> ApprovalFuture {
                Box::pin(async {
                    // The owner answers, but only after the 55 s window.
                    tokio::time::sleep(Duration::from_secs(70)).await;
                    Approval::always()
                })
            }
        }
        let secret = Arc::new(MemSecretStore::new());
        let cookies = Arc::new(MultiReader {
            chromium: vec![],
            firefox: vec![],
            keys: Arc::new(crate::cookies::fixtures::NoKey),
        });
        let core = Core::new(CoreConfig {
            settings_path: None,
            journal_path: None,
            approver: Arc::new(Delayed),
            cookies,
            secret,
            hub: Arc::new(FakeHub),
            app_version: "t".into(),
            cua_bin: "cua-driver".into(),
            cua_attach: CuaAttach::User,
        });
        core.apply_preset(Preset::Careful); // net = ask
        let sh = core.shared();
        // First attempt: no answer within 55 s -> labeled denial.
        let e = sh.net_allows("youtube.com", "the synth says: hi").await.unwrap_err();
        assert!(matches!(e, Refusal::Denied(_)));
        // The late answer lands; the grant is recorded for next time.
        tokio::time::sleep(Duration::from_secs(80)).await;
        assert!(sh.grants().allows("net", "youtube.com"));
        // Next attempt passes without asking.
        sh.net_allows("m.youtube.com", "").await.unwrap();
    }

    #[tokio::test]
    async fn paused_refuses_net() {
        let (core, _) = test_core();
        core.apply_preset(Preset::Full);
        core.pause().await;
        let sh = core.shared();
        let e = sh.net_allows("example.com", "").await.unwrap_err();
        assert!(matches!(e, Refusal::Denied(_)));
        core.resume();
        assert!(sh.net_allows("example.com", "").await.is_ok());
    }
}
