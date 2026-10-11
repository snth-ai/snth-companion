//! `Core`: the one object a UI (or the headless CLI) holds. It owns the
//! settings, grants, journal, cookie source, secret store and Cua
//! manager, drives the link, and answers commands. Everything is async,
//! `Send + Sync`, and there are no globals, so a Tauri shell can keep a
//! single instance behind its managed state.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::{broadcast, oneshot, watch};

use crate::cookies::CookieSource;
use crate::cua::{self, DriverEnv, PrivateDaemon};
use crate::cua_install::{CuaInstaller, FailCode, InstallState, UreqFetcher};
use crate::grant::{Approval, ApprovalRequest, Approver, GrantStore, APPROVAL_TIMEOUT};
use crate::journal::{Entry, Journal};
use crate::pairing::{HubClient, Pending};
use crate::proto::Refusal;
use crate::secret::SecretStore;
use crate::settings::{self, Persisted, Preset, Root, Settings, StoredGrant, StoredIdentity};
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

/// Monthly egress accounting. Written to disk at most every
/// `EGRESS_FLUSH_EVERY` (or `EGRESS_FLUSH_BYTES`), and on `Core::flush`.
struct Counters {
    month: String,
    bytes: u64,
    unsaved: u64,
    last_flush: Instant,
}

/// How long one `cua-driver --version` probe is reused.
const CUA_PROBE_TTL: Duration = Duration::from_secs(5);

const EGRESS_FLUSH_EVERY: Duration = Duration::from_secs(30);
const EGRESS_FLUSH_BYTES: u64 = 4 * 1024 * 1024;

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
    /// What every driver process we start gets in its environment.
    pub(crate) cua_env: DriverEnv,
    /// Set when the app keeps its own private copy of the driver.
    cua_installer: Option<Arc<CuaInstaller>>,
    pub(crate) cua_attach: CuaAttach,
    pub(crate) cua_daemon: tokio::sync::Mutex<Option<PrivateDaemon>>,
    /// The last `cua-driver --version` probe, reused for a few seconds so
    /// a burst of snapshots does not spawn a process each.
    cua_probe: Mutex<Option<(Instant, cua::Detect)>>,

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

    /// The link learned the synth's identity from hello_ack. The friendly
    /// name is kept on disk too, for the status line while the link is down.
    pub(crate) fn set_synth(&self, ack: wire::HelloAck) {
        let name = ack.synth_name.clone();
        *self.synth.write().unwrap() = Some(ack);
        let mut p = self.persisted.write().unwrap();
        if let Some(id) = p.identity.as_mut() {
            if id.synth_name != name && !name.is_empty() {
                id.synth_name = name;
                self.save_locked(&p);
            }
        }
    }

    /// Write the persisted record. The caller holds the write lock, which
    /// also serializes the writers (they share one temp file).
    fn save_locked(&self, p: &Persisted) {
        if let Some(path) = &self.settings_path {
            let _ = settings::save(path, p);
        }
    }

    /// Mirror the durable grants ("always" and until-time) to disk, so they
    /// survive a restart. Session grants are never written. Not a settings
    /// change: `settings_rev` does not move.
    pub(crate) fn persist_grants(&self) {
        let durable: Vec<StoredGrant> = self
            .grants
            .list()
            .into_iter()
            .filter(|(_, _, until)| *until >= 0)
            .map(|(capability, scope, until)| StoredGrant { capability, scope, until })
            .collect();
        let mut p = self.persisted.write().unwrap();
        if p.grants != durable {
            p.grants = durable;
            self.save_locked(&p);
        }
    }

    /// Write the egress counters to disk now.
    fn flush_egress(&self) {
        let (month, bytes) = {
            let mut c = self.counters.lock().unwrap();
            c.unsaved = 0;
            c.last_flush = Instant::now();
            (c.month.clone(), c.bytes)
        };
        let mut p = self.persisted.write().unwrap();
        if p.egress_month != month || p.egress_bytes != bytes {
            p.egress_month = month;
            p.egress_bytes = bytes;
            self.save_locked(&p);
        }
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
            me.persist_grants();
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
        let flush = {
            let mut c = self.counters.lock().unwrap();
            let m = month_key();
            if c.month != m {
                c.month = m;
                c.bytes = 0;
            }
            c.bytes = c.bytes.saturating_add(n);
            c.unsaved = c.unsaved.saturating_add(n);
            c.unsaved >= EGRESS_FLUSH_BYTES || c.last_flush.elapsed() >= EGRESS_FLUSH_EVERY
        };
        if flush {
            self.flush_egress();
        }
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
                    let d = PrivateDaemon::start(&self.cua_bin, &s, &self.cua_env).await.map_err(|e| {
                        Refusal::Failed(format!("unavailable: start private Cua daemon: {e}"))
                    })?;
                    *guard = Some(d);
                }
                Ok((guard.as_ref().unwrap().socket.clone(), true))
            }
        }
    }

    /// Is the driver installed, and which version? Cached for
    /// `CUA_PROBE_TTL`; `fresh` skips the cache (the UI's "check again").
    pub async fn cua_detect(&self, fresh: bool) -> cua::Detect {
        if !fresh {
            if let Some((at, d)) = self.cua_probe.lock().unwrap().as_ref() {
                if at.elapsed() < CUA_PROBE_TTL {
                    return d.clone();
                }
            }
        }
        let d = cua::detect(&self.cua_bin, &self.cua_env).await;
        *self.cua_probe.lock().unwrap() = Some((Instant::now(), d.clone()));
        d
    }

    /// The hello `computer.status` label.
    pub async fn cua_status(&self) -> (String, String) {
        let d = self.cua_detect(false).await;
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
            // The private daemon is started on demand, so installed is
            // enough, except that on macOS it can do nothing until this
            // app has its two permissions (see tcc.rs).
            CuaAttach::Private => match crate::tcc::status() {
                Some(p) if !(p.accessibility && p.screen_recording) => {
                    (version, cua::Status::NoPermissions.label())
                }
                _ => (version, cua::Status::Ok.label()),
            },
        }
    }

    async fn stop_cua(&self) {
        let d = self.cua_daemon.lock().await.take();
        if let Some(d) = d {
            d.stop(&self.cua_bin, &self.cua_env).await;
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
    /// The driver binary to run. Ignored when `cua_private_root` is set.
    pub cua_bin: String,
    /// Some(<app data>/cua) = the app keeps its own pinned copy of the
    /// driver there and installs it itself (`Core::cua_ensure`); the
    /// binary it runs is then that copy and nothing else. None = run
    /// `cua_bin` as given (the CLI, tests).
    pub cua_private_root: Option<PathBuf>,
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

        // State that survives a restart: the confirmed pairing (only while
        // its token is still in the secret store), the durable grants and
        // this month's egress count.
        let has_token = cfg
            .secret
            .get(TOKEN_KEY)
            .ok()
            .flatten()
            .is_some_and(|t| !t.is_empty());
        let identity = match (&persisted.identity, has_token) {
            (Some(id), true) => Some(Pending {
                synth_id: id.synth_id.clone(),
                synth_url: id.synth_url.clone(),
                token: String::new(), // the link reads it from the secret store
            }),
            _ => None,
        };
        let grants = GrantStore::new();
        if has_token {
            for g in &persisted.grants {
                grants.restore(&g.capability, &g.scope, g.until);
            }
        }
        let month = month_key();
        let egress_bytes = if persisted.egress_month == month {
            persisted.egress_bytes
        } else {
            0
        };

        let journal = Arc::new(Journal::new(cfg.journal_path));
        let (cua_bin, cua_env, cua_installer) = match cfg.cua_private_root {
            Some(root) => {
                let installer = CuaInstaller::new(
                    root,
                    crate::cua_install::pinned_release(),
                    Arc::new(UreqFetcher),
                    journal.clone(),
                );
                (
                    installer.bin_path().to_string_lossy().to_string(),
                    installer.env(),
                    Some(Arc::new(installer)),
                )
            }
            None => (cfg.cua_bin, DriverEnv::default(), None),
        };

        let shared = Arc::new(Shared {
            persisted: RwLock::new(persisted),
            settings_path: cfg.settings_path,
            grants,
            approver: cfg.approver,
            journal,
            cookies: cfg.cookies,
            secret: cfg.secret,
            hub: cfg.hub,
            app_version: cfg.app_version,
            cua_bin,
            cua_env,
            cua_installer,
            cua_attach: cfg.cua_attach,
            cua_daemon: tokio::sync::Mutex::new(None),
            cua_probe: Mutex::new(None),
            paused: AtomicBool::new(false),
            metered: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            counters: Mutex::new(Counters {
                month,
                bytes: egress_bytes,
                unsaved: 0,
                last_flush: Instant::now(),
            }),
            identity: RwLock::new(identity),
            pending: Mutex::new(None),
            synth: RwLock::new(None),
            link_state: RwLock::new(LinkState::Unpaired),
            events,
            settings_tx,
            control_tx,
            unpair_req: tokio::sync::Notify::new(),
            unpair_ack: tokio::sync::Notify::new(),
        });
        // A stored token means we are already paired: reflect it. The
        // synth url/id come back from settings.json (see above); the CLI
        // may also pass the url to run_link().
        if has_token {
            shared.set_link_state(LinkState::Connecting, "token present");
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
        self.shared.persist_grants();
    }

    // --- connection facts the host observes ---

    /// The host tells the core whether the current connection is metered
    /// (the core has no portable way to know). Reported in presence and
    /// used by the "do not use on metered connections" setting.
    pub fn set_metered(&self, metered: bool) {
        if self.shared.metered.swap(metered, Ordering::AcqRel) != metered {
            self.shared.bump_control();
        }
    }

    /// Write anything held back for batching (the egress counters). Call
    /// before the process exits.
    pub fn flush(&self) {
        self.shared.flush_egress();
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
        {
            let mut persisted = self.shared.persisted.write().unwrap();
            persisted.identity = Some(StoredIdentity {
                synth_id: p.synth_id.clone(),
                synth_url: p.synth_url.clone(),
                synth_name: String::new(),
            });
            self.shared.save_locked(&persisted);
        }
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

    /// Is this computer paired (a confirmed synth and its token on hand)?
    pub fn is_paired(&self) -> bool {
        self.shared.identity.read().unwrap().is_some()
    }

    /// Wipe the token, grants and pending/identity. Called after a bounded
    /// wait for `unpair_ack` (the link sends `unpair` first). Idempotent.
    pub fn wipe_local(&self) {
        let _ = self.shared.secret.delete(TOKEN_KEY);
        self.shared.grants.clear();
        *self.shared.identity.write().unwrap() = None;
        *self.shared.pending.lock().unwrap() = None;
        *self.shared.synth.write().unwrap() = None;
        {
            let mut persisted = self.shared.persisted.write().unwrap();
            if persisted.identity.is_some() || !persisted.grants.is_empty() {
                persisted.identity = None;
                persisted.grants.clear();
                self.shared.save_locked(&persisted);
            }
        }
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
        cua::permissions_status(&self.shared.cua_bin, None, &self.shared.cua_env).await
    }

    // --- the app's own copy of the driver ---

    /// The binary this core runs for computer control.
    pub fn cua_bin(&self) -> String {
        self.shared.cua_bin.clone()
    }

    /// Where the helper stands (checking, downloading, ready, failed...).
    /// `Idle` until the first `cua_ensure`.
    pub fn cua_helper_state(&self) -> InstallState {
        match &self.shared.cua_installer {
            Some(i) => i.state(),
            None => InstallState::Idle,
        }
    }

    /// Live helper states, for a progress line. None when the core does
    /// not manage a private copy.
    pub fn subscribe_cua_helper(&self) -> Option<watch::Receiver<InstallState>> {
        self.shared.cua_installer.as_ref().map(|i| i.subscribe())
    }

    /// Make sure the pinned driver is installed: check the private copy
    /// and download it when it is missing, broken or of another version
    /// (a new app version with a new pin). Idempotent, one run at a time,
    /// safe to call again after a failure. Never touches a driver the
    /// owner installed themselves.
    ///
    /// When a new copy went in, our daemon (if one runs the old copy) is
    /// stopped so the next task starts the new one, and the synth is told
    /// that computer control changed (`settings_rev` moves).
    pub async fn cua_ensure(&self) -> InstallState {
        let Some(installer) = self.shared.cua_installer.clone() else {
            // Not managed: report what is there.
            let d = self.shared.cua_detect(true).await;
            return match d.version {
                Some(version) if d.installed => InstallState::Ready { version },
                _ => InstallState::Failed {
                    code: FailCode::Unsupported,
                    detail: "this core does not manage a private driver copy".into(),
                },
            };
        };
        let (state, installed) = installer.ensure().await;
        if installed {
            self.shared.stop_cua().await;
            *self.shared.cua_probe.lock().unwrap() = None;
            self.mutate_settings(|_| {});
        }
        state
    }

    /// The app is quitting: stop a running install, start no new one.
    pub fn cua_stop_install(&self) {
        if let Some(i) = &self.shared.cua_installer {
            i.shutdown();
        }
    }

    /// What the OS allows for computer control, as the app the embedded
    /// driver runs under. macOS: Accessibility and Screen Recording of
    /// THIS app (read-only, no prompt). None on systems without such
    /// permissions.
    pub fn computer_permissions(&self) -> Option<crate::tcc::HostPermissions> {
        crate::tcc::status()
    }

    /// Ask macOS for the permissions still missing. Shows system dialogs:
    /// call only from the owner's click.
    pub fn request_computer_permissions(&self) {
        crate::tcc::request();
    }

    /// Probe the driver now, bypassing the short cache.
    pub async fn cua_detect(&self) -> cua::Detect {
        self.shared.cua_detect(true).await
    }

    // --- snapshot ---

    /// The cheap part of the snapshot: no Cua probe, no await. For a tray
    /// label that is refreshed on every event.
    pub fn link_brief(&self) -> LinkBrief {
        let synth = self.shared.synth.read().unwrap().clone();
        let identity = self.shared.identity.read().unwrap().clone();
        let paired = identity.is_some();
        let stored_name = self
            .shared
            .persisted
            .read()
            .unwrap()
            .identity
            .as_ref()
            .map(|i| i.synth_name.clone())
            .filter(|n| !n.is_empty());
        LinkBrief {
            link_state: *self.shared.link_state.read().unwrap(),
            paired,
            paused: self.shared.paused(),
            synth_id: synth
                .as_ref()
                .map(|s| s.synth_id.clone())
                .or_else(|| identity.as_ref().map(|i| i.synth_id.clone())),
            // Before the first hello_ack of this run, fall back to what
            // the pairing stored, so the UI can name the synth offline.
            synth_name: synth
                .as_ref()
                .map(|s| s.synth_name.clone())
                .filter(|n| !n.is_empty())
                .or(if paired { stored_name } else { None }),
            synth_url: identity.as_ref().map(|i| i.synth_url.clone()),
            synth_version: synth.as_ref().map(|s| s.synth_version.clone()),
        }
    }

    pub async fn snapshot(&self) -> Snapshot {
        let (cua_version, cua_status) = self.shared.cua_status().await;
        let presence = crate::presence::read();
        let counters = {
            let c = self.shared.counters.lock().unwrap();
            (c.month.clone(), c.bytes)
        };
        let brief = self.link_brief();
        Snapshot {
            link_state: brief.link_state,
            paired: brief.paired,
            synth_id: brief.synth_id,
            synth_name: brief.synth_name,
            synth_url: brief.synth_url,
            synth_version: brief.synth_version,
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

/// Who we are linked to and how the link stands (see `Core::link_brief`).
#[derive(Debug, Clone, Serialize)]
pub struct LinkBrief {
    pub link_state: LinkState,
    pub paired: bool,
    pub paused: bool,
    pub synth_id: Option<String>,
    pub synth_name: Option<String>,
    pub synth_url: Option<String>,
    pub synth_version: Option<String>,
}

/// A read-only picture of the core for the UI.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub link_state: LinkState,
    /// A confirmed pairing exists (whether or not the link is up).
    pub paired: bool,
    pub synth_id: Option<String>,
    pub synth_name: Option<String>,
    /// The paired synth's base url (https://...), if paired.
    pub synth_url: Option<String>,
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

pub fn registrable_domain(host: &str) -> String {
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
            cua_private_root: None,
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
            cua_private_root: None,
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

    /// A core over real files in a temp dir, to prove what survives a
    /// restart. The secret store is shared between the two "runs".
    fn file_core(dir: &std::path::Path, secret: Arc<MemSecretStore>) -> Core {
        let cookies = Arc::new(MultiReader {
            chromium: vec![],
            firefox: vec![],
            keys: Arc::new(crate::cookies::fixtures::NoKey),
        });
        Core::new(CoreConfig {
            settings_path: Some(dir.join("settings.json")),
            journal_path: None,
            approver: Arc::new(AutoAllow),
            cookies,
            secret,
            hub: Arc::new(FakeHub),
            app_version: "t".into(),
            cua_bin: "cua-driver-not-installed".into(),
            cua_private_root: None,
            cua_attach: CuaAttach::User,
        })
    }

    #[tokio::test]
    async fn pairing_grants_and_egress_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("connect-core-restart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let secret = Arc::new(MemSecretStore::new());

        let core = file_core(&dir, secret.clone());
        assert!(!core.is_paired());
        core.pair_begin("https://hub".into(), "123456".into()).await.unwrap();
        core.pair_confirm().unwrap();
        core.apply_preset(Preset::Careful); // net = ask
        let sh = core.shared();
        // AutoAllow answers "always": a durable grant.
        sh.net_allows("www.youtube.com", "").await.unwrap();
        // A session grant must NOT be written.
        sh.grants().record(
            "cookies",
            "session-only.com",
            Approval { allow: true, duration: crate::grant::Duration_::Session },
        );
        sh.persist_grants();
        sh.add_egress(1234);
        core.flush();
        drop(sh);
        drop(core);

        let core = file_core(&dir, secret.clone());
        assert!(core.is_paired());
        assert_eq!(core.synth_url().as_deref(), Some("wss://mia.snth.ai/api/connect/ws"));
        let snap = core.snapshot().await;
        assert!(snap.paired);
        assert_eq!(snap.synth_id.as_deref(), Some("mia"));
        assert_eq!(snap.synth_url.as_deref(), Some("https://mia.snth.ai"));
        assert_eq!(snap.egress_bytes, 1234);
        let sh = core.shared();
        assert!(sh.grants().allows("net", "youtube.com"));
        assert!(!sh.grants().allows("cookies", "session-only.com"));

        // Revoke is durable too.
        core.revoke("net", "youtube.com");
        drop(sh);
        drop(core);
        let core = file_core(&dir, secret.clone());
        assert!(!core.shared().grants().allows("net", "youtube.com"));

        // Unpair wipes the stored pairing.
        core.wipe_local();
        drop(core);
        let core = file_core(&dir, secret.clone());
        assert!(!core.is_paired());
        assert!(core.snapshot().await.synth_id.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_stored_pairing_without_its_token_is_not_paired() {
        let dir = std::env::temp_dir().join(format!("connect-core-notoken-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let secret = Arc::new(MemSecretStore::new());
        let core = file_core(&dir, secret.clone());
        core.pair_begin("https://hub".into(), "123456".into()).await.unwrap();
        core.pair_confirm().unwrap();
        drop(core);
        // The keychain entry is gone (say, the user deleted it).
        secret.delete(TOKEN_KEY).unwrap();
        let core = file_core(&dir, secret);
        assert!(!core.is_paired());
        assert!(!core.snapshot().await.paired);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn metered_flag_gates_net_when_asked_to() {
        let (core, _) = test_core();
        core.apply_preset(Preset::Full);
        core.mutate_settings(|s| s.net.skip_on_metered = true);
        let sh = core.shared();
        assert!(sh.net_allows("example.com", "").await.is_ok());
        core.set_metered(true);
        assert!(sh.net_allows("example.com", "").await.is_err());
        core.set_metered(false);
        assert!(sh.net_allows("example.com", "").await.is_ok());
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
