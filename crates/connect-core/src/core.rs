//! `Core`: the one object a UI (or the headless CLI) holds. It owns the
//! settings, grants, journal, cookie source, secret store and Cua
//! manager, drives the link, and answers commands. Everything is async,
//! `Send + Sync`, and there are no globals, so a Tauri shell can keep a
//! single instance behind its managed state.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
    /// A browser's extension connected, left, was linked or unlinked, or
    /// the sites it may read changed.
    Browsers,
    /// The name of the computer-control task that is running changed (or
    /// there is none any more): see `Core::computer_task`.
    ComputerTask,
}

/// After a failed start of our daemon the next try waits: 5 s, 15 s, 1 min,
/// then 5 min. Without this a synth that retries would have the app start
/// (and lose) a process on every attempt.
const DAEMON_RETRY_AFTER: [Duration; 4] = [
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(60),
    Duration::from_secs(300),
];

#[derive(Default)]
struct StartBackoff {
    failures: usize,
    not_before: Option<Instant>,
    last_error: String,
}

impl StartBackoff {
    /// Some(seconds) while a new start must still wait.
    fn wait_left(&self) -> Option<u64> {
        let until = self.not_before?;
        let left = until.checked_duration_since(Instant::now())?;
        Some(left.as_secs().max(1))
    }

    fn failed(&mut self, why: String) {
        let step = self.failures.min(DAEMON_RETRY_AFTER.len() - 1);
        self.failures += 1;
        self.not_before = Some(Instant::now() + DAEMON_RETRY_AFTER[step]);
        self.last_error = why;
    }
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
    /// The owner's own apps: the backend for this system and the little
    /// state the gate keeps (see apps/mod.rs).
    pub(crate) apps: crate::apps::State,
    secret: Arc<dyn SecretStore>,
    hub: Arc<dyn HubClient>,
    pub(crate) app_version: String,
    /// The driver binary of the pinned version (see `cua_bin_now` for the
    /// one to run).
    pub(crate) cua_bin: String,
    /// The binary our running daemon was started from, for the processes
    /// that talk to it (`mcp::spawn_proxy`). None when none runs.
    cua_daemon_bin: Mutex<Option<String>>,
    /// What every driver process we start gets in its environment.
    pub(crate) cua_env: DriverEnv,
    /// Set when the app keeps its own private copy of the driver.
    cua_installer: Option<Arc<CuaInstaller>>,
    pub(crate) cua_attach: CuaAttach,
    pub(crate) cua_daemon: tokio::sync::Mutex<Option<PrivateDaemon>>,
    /// The last `cua-driver --version` probe, reused for a few seconds so
    /// a burst of snapshots does not spawn a process each.
    cua_probe: Mutex<Option<(Instant, cua::Detect)>>,
    /// Failed starts of our daemon: how many in a row, when the next try
    /// is allowed, and why the last one failed.
    cua_start_backoff: Mutex<StartBackoff>,

    paused: AtomicBool,
    metered: AtomicBool,
    pub(crate) stopping: AtomicBool, // unpair asked the link loop to stop reconnecting
    /// Streams the synth has open right now (see `Core::active_streams`).
    pub(crate) streams_open: AtomicUsize,
    /// Computer-control sessions that hold our daemon right now (see
    /// `CuaSession`). The daemon is replaced only while this is zero.
    cua_sessions: AtomicUsize,
    /// The apps and windows of this computer as the driver lists them: the
    /// app's own reading, for the owner's picker and for telling which app
    /// a call is about (`cua_apps.rs`). The synth never reaches it.
    pub(crate) cua_dir: crate::cua_apps::Directory,
    /// The label the synth gave the task it is running, as the driver
    /// shows it under the agent's cursor, and when it was last used.
    /// Memory only: never in the journal, never on disk.
    cua_task: Mutex<Option<(String, Instant)>>,
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
    /// What the app asks the synth over the control stream (requests.rs).
    pub(crate) requests: crate::requests::LinkRequests,
    /// The owner's browsers, through the SNTH Vault extension (browser.rs).
    pub(crate) browsers: crate::browser::Hub,
    /// Ask a linked browser for cookies BEFORE reading browser files.
    /// True where the files of the common browsers cannot be read at all
    /// (Chrome and Edge on Windows); elsewhere the extension is the
    /// fallback.
    pub(crate) cookies_extension_first: AtomicBool,
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
    /// A confirmed pairing with a synth exists (link up or not).
    pub(crate) fn is_paired(&self) -> bool {
        self.identity.read().unwrap().is_some()
    }
    /// This computer has a synth: a confirmed pairing, or (a host that
    /// was handed a token and an address directly, like the CLI) a link
    /// that has been up.
    pub(crate) fn has_synth(&self) -> bool {
        self.is_paired() || self.synth.read().unwrap().is_some()
    }
    pub(crate) fn emit(&self, event: CoreEvent) {
        let _ = self.events.send(event);
    }
    /// Write the persisted record; the caller holds its write lock.
    pub(crate) fn save_persisted(&self, p: &Persisted) {
        self.save_locked(p);
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
        let remember = req.remember;
        let fut = self.approver.ask(req);
        let me = self.clone();
        let cap = capability.to_string();
        let sc = scope.to_string();
        tokio::spawn(async move {
            let mut appr = fut.await;
            // An answer this ask may not be remembered for counts as
            // "this time", whatever the approver sent.
            if appr.allow && !remember.allows(appr.duration) {
                appr.duration = crate::grant::Duration_::Once;
            }
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

    /// The socket + embedded flag of the daemon to talk to, starting our
    /// private daemon on demand. A labeled refusal when computer use is
    /// off/paused or the daemon cannot be reached. For a caller that only
    /// asks (the self-test); a session takes `cua_session`.
    pub async fn cua_socket(self: &Arc<Self>) -> Result<(String, bool), Refusal> {
        self.cua_endpoint(false).await
    }

    /// The same for one computer-control session (one mcp stream): the
    /// answer holds our daemon until it is dropped, so the daemon is not
    /// replaced under a session that is running.
    pub async fn cua_session(self: &Arc<Self>) -> Result<CuaSession, Refusal> {
        let at = self.cua_endpoint_at(true).await?;
        Ok(CuaSession {
            socket: at.socket,
            embedded: at.embedded,
            scope: at.scope,
            look: at.look,
            started: at.started,
            bin: at.bin,
            shared: self.clone(),
        })
    }

    /// How many computer-control sessions hold our daemon right now.
    pub fn cua_sessions_open(&self) -> usize {
        self.cua_sessions.load(Ordering::Acquire)
    }

    async fn cua_endpoint(self: &Arc<Self>, session: bool) -> Result<(String, bool), Refusal> {
        let at = self.cua_endpoint_at(session).await?;
        Ok((at.socket, at.embedded))
    }

    /// `session`: count the caller as a session. Counted while the daemon
    /// lock is held, so no other caller can replace the daemon in between.
    async fn cua_endpoint_at(self: &Arc<Self>, session: bool) -> Result<Endpoint, Refusal> {
        if self.paused() {
            return Err(Refusal::Denied("computer use is paused by the owner".into()));
        }
        if !self.settings().computer.enabled {
            return Err(Refusal::Denied("computer use is turned off on this computer".into()));
        }
        match self.cua_attach {
            CuaAttach::User => {
                let socket = user_daemon_socket(&self.cua_bin).await?;
                if session {
                    self.cua_sessions.fetch_add(1, Ordering::AcqRel);
                }
                // A daemon of the user's own: nothing of ours limits it,
                // so the owner's choice of apps is held by the pipe alone.
                Ok(Endpoint {
                    socket,
                    embedded: false,
                    scope: Arc::new(crate::cua_apps::Scope::of(&self.settings())),
                    look: None,
                    started: Instant::now(),
                    bin: None,
                })
            }
            CuaAttach::Private => {
                let mut guard = self.cua_daemon.lock().await;
                // A daemon that died by itself is not a daemon: start anew.
                if guard.as_mut().is_some_and(|d| !d.is_alive()) {
                    if let Some(dead) = guard.take() {
                        dead.discard().await;
                        self.daemon_gone();
                    }
                }
                let s = self.settings();
                let bin = self.cua_bin_now();
                // The look is the daemon's start flags, and the owner's
                // choice of apps is its permission mode and manifest: a
                // new look or a new choice needs a new daemon. So does a
                // bounded daemon whose day is half over. It is replaced
                // here, where a session is about to start, and only when no
                // other session holds it: work in progress keeps what it
                // began with.
                if self.cua_sessions.load(Ordering::Acquire) == 0 {
                    let why = guard.as_ref().and_then(|d| {
                        if d.look != crate::cua_look::resolve(&s.computer.look, &self.cua_env) {
                            Some("the look changed")
                        } else if *d.scope != crate::cua_apps::Scope::of(&s) {
                            Some("the owner's choice of apps changed")
                        } else if lease_old(&d.scope, d.started) {
                            Some("its permissions are half a day old")
                        } else if d.bin != bin {
                            Some("a new version of the helper is in place")
                        } else {
                            None
                        }
                    });
                    if let Some(why) = why {
                        crate::logln!("cua daemon: {why}, starting a new one");
                        if let Some(old) = guard.take() {
                            old.stop(&self.cua_env).await;
                            self.daemon_gone();
                        }
                    }
                }
                if guard.is_none() {
                    if self.cua_installer.is_some() && !std::path::Path::new(&bin).is_file() {
                        return Err(Refusal::Failed(
                            "unavailable: the computer-control helper is not installed yet".into(),
                        ));
                    }
                    // After a failed start, wait before the next one
                    // instead of starting a process per attempt.
                    {
                        let backoff = self.cua_start_backoff.lock().unwrap();
                        if let Some(secs) = backoff.wait_left() {
                            return Err(Refusal::Failed(format!(
                                "unavailable: the computer-control helper did not start ({}); next try in {secs} s",
                                backoff.last_error
                            )));
                        }
                    }
                    match PrivateDaemon::start(&bin, &s, &self.cua_env).await {
                        Ok(d) => {
                            *self.cua_start_backoff.lock().unwrap() = StartBackoff::default();
                            // A copy of the version before stays on disk
                            // for as long as this daemon runs from it.
                            if let Some(installer) = &self.cua_installer {
                                installer.hold(std::path::Path::new(&d.bin));
                            }
                            *self.cua_daemon_bin.lock().unwrap() = Some(d.bin.clone());
                            *guard = Some(d);
                        }
                        Err(e) => {
                            let why = e.to_string();
                            crate::logln!("cua daemon: start failed: {why}");
                            self.cua_start_backoff.lock().unwrap().failed(why.clone());
                            return Err(Refusal::Failed(format!(
                                "unavailable: start private Cua daemon: {why}"
                            )));
                        }
                    }
                }
                if session {
                    self.cua_sessions.fetch_add(1, Ordering::AcqRel);
                }
                let d = guard.as_ref().unwrap();
                Ok(Endpoint {
                    socket: d.socket.clone(),
                    embedded: true,
                    scope: d.scope.clone(),
                    look: Some(d.look.clone()),
                    started: d.started,
                    bin: Some(d.bin.clone()),
                })
            }
        }
    }

    /// The owner's choice of apps our running daemon was started with;
    /// None when no daemon of ours runs.
    pub async fn cua_daemon_scope(&self) -> Option<Arc<crate::cua_apps::Scope>> {
        self.cua_daemon.lock().await.as_ref().map(|d| d.scope.clone())
    }

    /// The apps and windows of this computer, as the app itself reads them
    /// from the driver.
    pub fn cua_directory(&self) -> &crate::cua_apps::Directory {
        &self.cua_dir
    }

    /// The synth used this label on a call just now.
    pub(crate) fn note_task(&self, raw: Option<&str>) {
        let Some(label) = raw.and_then(task_label) else {
            return;
        };
        let changed = {
            let mut task = self.cua_task.lock().unwrap();
            let changed = task.as_ref().is_none_or(|(have, at)| *have != label || at.elapsed() >= TASK_LABEL_TTL);
            *task = Some((label, Instant::now()));
            changed
        };
        if changed {
            self.emit(CoreEvent::ComputerTask);
        }
    }

    /// The computer-control session ended: its task has no name any more.
    pub(crate) fn clear_task(&self) {
        if self.cua_task.lock().unwrap().take().is_some() {
            self.emit(CoreEvent::ComputerTask);
        }
    }

    /// The label of the computer-control task that is running, if the
    /// synth named it and used the name in the last few minutes.
    pub fn computer_task(&self) -> Option<String> {
        let task = self.cua_task.lock().unwrap();
        task.as_ref().filter(|(_, at)| at.elapsed() < TASK_LABEL_TTL).map(|(label, _)| label.clone())
    }

    /// The look flags our running daemon was started with; None when no
    /// daemon of ours runs.
    pub async fn cua_daemon_look(&self) -> Option<Vec<String>> {
        self.cua_daemon.lock().await.as_ref().map(|d| d.look.clone())
    }

    /// Is the driver installed, and which version? Cached for
    /// `CUA_PROBE_TTL`; `fresh` skips the cache (the UI's "check again").
    pub async fn cua_detect(&self, fresh: bool) -> cua::Detect {
        // The app's own copy changes only through the installer, which
        // knows its state: no process is started to ask the binary. (Only
        // before the installer's first check of this run is the binary
        // asked directly, so the first hello is not wrong.)
        if let Some(installer) = &self.cua_installer {
            let known = match installer.state() {
                InstallState::Ready { version } => Some(Some(version)),
                InstallState::Idle | InstallState::Checking => None,
                // The pinned version is not in place (still coming, or it
                // could not be fetched): the copy of the version before,
                // if there is one, is what runs meanwhile.
                _ => Some(installer.usable_bin().map(|(version, _)| version)),
            };
            if let Some(version) = known {
                return cua::Detect {
                    installed: version.is_some(),
                    version,
                    path: self.cua_bin.clone(),
                };
            }
        }
        if !fresh {
            if let Some((at, d)) = self.cua_probe.lock().unwrap().as_ref() {
                if at.elapsed() < CUA_PROBE_TTL {
                    return d.clone();
                }
            }
        }
        let d = cua::detect(&self.cua_bin_now(), &self.cua_env).await;
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
            d.stop(&self.cua_env).await;
        }
        self.daemon_gone();
        self.cua_dir.stop().await;
        self.clear_task();
    }

    /// Our daemon was stopped: nothing runs from a copy of an older
    /// version any more, so that copy may go.
    fn daemon_gone(&self) {
        *self.cua_daemon_bin.lock().unwrap() = None;
        if let Some(installer) = &self.cua_installer {
            installer.release_old();
        }
    }

    /// The driver binary to start now. The pinned copy when it is in
    /// place. After an update of the app, while the new version is still
    /// being fetched or could not be fetched, the copy of the version
    /// before: computer control keeps working on the driver the owner
    /// already had.
    pub fn cua_bin_now(&self) -> String {
        match &self.cua_installer {
            Some(installer) => installer
                .usable_bin()
                .map(|(_, bin)| bin.to_string_lossy().to_string())
                .unwrap_or_else(|| self.cua_bin.clone()),
            None => self.cua_bin.clone(),
        }
    }

    /// The binary for a process that talks to our running daemon: the one
    /// the daemon itself runs.
    pub(crate) fn cua_proxy_bin(&self) -> String {
        self.cua_daemon_bin.lock().unwrap().clone().unwrap_or_else(|| self.cua_bin_now())
    }
}

/// Where a daemon is and what it was started with.
struct Endpoint {
    socket: String,
    embedded: bool,
    scope: Arc<crate::cua_apps::Scope>,
    /// None for a daemon of the user's own: its look is not ours.
    look: Option<Vec<String>>,
    started: Instant,
    /// The binary our daemon runs; None for a daemon of the user's own.
    bin: Option<String>,
}

/// A bounded daemon lives a day (`cua_apps::LEASE`). After half of it, it
/// is replaced the next time nobody holds it.
fn lease_old(scope: &crate::cua_apps::Scope, started: Instant) -> bool {
    scope.policy == crate::settings::AppsPolicy::Only && started.elapsed() >= crate::cua_apps::RENEW_AFTER
}

/// How long a task label counts as "running" after its last call: the
/// driver ends a named session after five minutes without one.
const TASK_LABEL_TTL: Duration = Duration::from_secs(5 * 60);

/// The most the driver shows of a session label (`cursor-overlay`,
/// `MAX_SESSION_LABEL_CHARS`).
pub const TASK_LABEL_CHARS: usize = 28;

/// A session label as the driver itself shows it under the agent's cursor
/// (`cursor-overlay/src/session_badge.rs`, `sanitize_session_label`):
/// control characters are dropped, every run of white space becomes one
/// space, and what is longer than 28 characters is cut to 27 and an
/// ellipsis. None when nothing is left. The label is the synth's own
/// text: the app shows it to the owner in exactly this bounded form and
/// keeps it nowhere.
pub fn task_label(raw: &str) -> Option<String> {
    let mut out = String::new();
    let mut gap = false;
    for c in raw.chars().take(4096) {
        if c.is_control() {
            continue;
        }
        if c.is_whitespace() {
            gap = !out.is_empty();
            continue;
        }
        if gap {
            out.push(' ');
            gap = false;
        }
        out.push(c);
    }
    if out.is_empty() {
        return None;
    }
    if out.chars().count() > TASK_LABEL_CHARS {
        out = out.chars().take(TASK_LABEL_CHARS - 1).collect();
        out.push('\u{2026}');
    }
    Some(out)
}

/// One computer-control session's hold on the daemon it talks to. While
/// one exists our daemon is not replaced (a new look waits for the last
/// one to end). Dropping it, however the session ends, lets go.
pub struct CuaSession {
    /// The daemon's socket (Unix) or named pipe (Windows).
    pub socket: String,
    /// Our own embedded daemon (true) or the user's (false).
    pub embedded: bool,
    /// Which apps the synth may use, as this session's daemon was started
    /// with it. The pipe decides every call of the session by this, so a
    /// change of the owner's choice starts with the next session, like the
    /// look.
    pub scope: Arc<crate::cua_apps::Scope>,
    /// The look flags of the daemon; None for a daemon of the user's own.
    look: Option<Vec<String>>,
    /// When the daemon was started.
    started: Instant,
    /// The driver binary our daemon runs; None for a daemon of the user's.
    bin: Option<String>,
    shared: Arc<Shared>,
}

impl CuaSession {
    /// Would a session that starts now get another daemon than this one:
    /// the owner changed the look or the choice of apps, a bounded
    /// daemon's day is half over, or a new version of the driver is in
    /// place? The pipe asks before a call that comes
    /// after a quiet spell, and ends the session there so that the change
    /// applies (`mcp::handle`).
    pub fn outdated(&self) -> bool {
        let s = self.shared.settings();
        *self.scope != crate::cua_apps::Scope::of(&s)
            || (self.embedded && lease_old(&self.scope, self.started))
            || self.bin.as_ref().is_some_and(|bin| *bin != self.shared.cua_bin_now())
            || self
                .look
                .as_ref()
                .is_some_and(|look| *look != crate::cua_look::resolve(&s.computer.look, &self.shared.cua_env))
    }
}

impl Drop for CuaSession {
    fn drop(&mut self) {
        self.shared.cua_sessions.fetch_sub(1, Ordering::AcqRel);
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
    /// The owner's own apps on this computer. The product passes
    /// `apps::system_backend(..)`; tests pass a stub.
    pub apps: Arc<dyn crate::apps::Backend>,
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

        // The app's own reading of the driver's lists runs whichever copy
        // is usable when it is started.
        let cua_dir = {
            let (installer, pinned) = (cua_installer.clone(), cua_bin.clone());
            let bin: crate::cua_apps::BinProvider = Arc::new(move || match &installer {
                Some(installer) => installer
                    .usable_bin()
                    .map(|(_, bin)| bin.to_string_lossy().to_string())
                    .unwrap_or_else(|| pinned.clone()),
                None => pinned.clone(),
            });
            crate::cua_apps::Directory::driver(bin, cua_env.clone())
        };
        let shared = Arc::new(Shared {
            persisted: RwLock::new(persisted),
            settings_path: cfg.settings_path,
            grants,
            approver: cfg.approver,
            journal,
            cookies: cfg.cookies,
            apps: crate::apps::State::new(cfg.apps),
            secret: cfg.secret,
            hub: cfg.hub,
            app_version: cfg.app_version,
            cua_bin,
            cua_daemon_bin: Mutex::new(None),
            cua_env,
            cua_installer,
            cua_attach: cfg.cua_attach,
            cua_daemon: tokio::sync::Mutex::new(None),
            cua_probe: Mutex::new(None),
            cua_start_backoff: Mutex::new(StartBackoff::default()),
            paused: AtomicBool::new(false),
            metered: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            streams_open: AtomicUsize::new(0),
            cua_sessions: AtomicUsize::new(0),
            cua_dir,
            cua_task: Mutex::new(None),
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
            requests: crate::requests::LinkRequests::default(),
            browsers: crate::browser::Hub::default(),
            cookies_extension_first: AtomicBool::new(cfg!(windows)),
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
    ///
    /// A change of nothing but the look (the cursor, its speed, the
    /// screenshot limit) is the app's own business: it is saved, but the
    /// rev does not move and the synth is sent nothing.
    pub fn mutate_settings(&self, f: impl FnOnce(&mut Settings)) -> u64 {
        let (rev, told) = {
            let mut p = self.shared.persisted.write().unwrap();
            let before = p.settings.clone();
            f(&mut p.settings);
            let look_only = p.settings != before && {
                let mut same_look = p.settings.clone();
                same_look.computer.look = before.computer.look.clone();
                same_look == before
            };
            if !look_only {
                p.rev += 1;
            }
            if let Some(path) = &self.shared.settings_path {
                let _ = settings::save(path, &p);
            }
            (p.rev, !look_only)
        };
        if told {
            let _ = self.shared.settings_tx.send(rev);
        }
        let _ = self.shared.events.send(CoreEvent::Settings { rev });
        rev
    }

    /// A preset is a policy. How computer control looks is not part of
    /// one (every preset has the same, default look), so the look the
    /// owner chose stays, like the shared folders. So does the owner's
    /// choice of apps: a preset says when the synth may act and whether it
    /// asks, never which apps, and a list the owner made by hand must not
    /// be lost to a click on a preset. Nor is the experiment: it says how
    /// the synth drives the computer, not when it may.
    pub fn apply_preset(&self, preset: Preset) -> u64 {
        self.mutate_settings(|s| {
            let roots = s.files.roots.clone();
            let look = s.computer.look.clone();
            let apps = s.computer.apps.clone();
            let experiment = s.computer.experiment;
            *s = Settings::from_preset(preset, roots);
            s.computer.look = look;
            s.computer.apps = apps;
            s.computer.experiment = experiment;
        })
    }

    /// The owner's picker: every app the driver lists on this computer,
    /// read now by the app's own runtime of the driver (never through the
    /// synth's daemon, which may be limited to a few apps). Slow on a
    /// computer with many apps (a few seconds).
    pub async fn computer_apps(&self) -> Result<Vec<crate::cua_apps::AppChoice>, String> {
        if self.shared.cua_installer.is_some() && !std::path::Path::new(&self.shared.cua_bin_now()).is_file() {
            return Err("not_installed".into());
        }
        let rows = self.shared.cua_dir.apps_fresh().await;
        // Nobody is being served by it: let the runtime go.
        if self.shared.cua_sessions_open() == 0 {
            self.shared.cua_dir.stop().await;
        }
        Ok(crate::cua_apps::choices(&rows?))
    }

    /// "Check the helper": the driver's own health report from OUR daemon,
    /// with what the app itself knows (the pinned version, the pointer,
    /// the owner's choice of apps). See `cua_health.rs`.
    pub async fn cua_health(&self) -> crate::cua_health::Report {
        let app_data = crate::cua_health::app_data_of(&self.shared.cua_env);
        crate::cua_health::check(&self.shared, app_data.as_deref()).await
    }

    /// The label of the computer-control task that is running (see
    /// `Shared::computer_task`).
    pub fn computer_task(&self) -> Option<String> {
        self.shared.computer_task()
    }

    /// What this computer's driver can show of the look settings, so the
    /// host hides a control that would do nothing here.
    pub fn look_support(&self) -> crate::cua_look::Support {
        crate::cua_look::support()
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
            // The system's own answer first (on Windows these folders
            // can be moved, for example into OneDrive), then the usual
            // place under the home folder.
            let dl = base
                .download_dir()
                .map(|d| d.to_path_buf())
                .unwrap_or_else(|| base.home_dir().join("Downloads"));
            if dl.is_dir() {
                v.push(Root { path: dl, write: true });
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
        // A site taken back here is taken back in the browsers too: no
        // extension keeps the right to read its cookies.
        if capability == "cookies" {
            self.shared.browsers_revoke_site(scope);
        }
    }

    /// Which cookie source goes first: a linked browser's extension
    /// (true) or the browsers' files (false). The default follows the
    /// system: the extension first on Windows, the files first elsewhere.
    pub fn set_cookies_extension_first(&self, first: bool) {
        self.shared.cookies_extension_first.store(first, Ordering::Release);
    }

    // --- connection facts the host observes ---

    /// How many streams the synth has open on this computer right now: a
    /// connection through its internet, a computer-control session, a
    /// file or cookie request being answered. Zero means nothing is in
    /// flight, so a host may pick that moment to restart itself (an
    /// update) without cutting work short.
    pub fn active_streams(&self) -> usize {
        self.shared.streams_open.load(Ordering::Acquire)
    }

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
        self.shared.browsers_link_changed(true);
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
        self.shared.browsers_link_changed(false);
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

    /// A command of the driver this core runs, with the environment every
    /// driver process of ours gets (private home, private theme folder,
    /// telemetry off). For the driver's own read-only subcommands, such as
    /// `cursor-theme list`.
    pub fn cua_driver_command(&self) -> tokio::process::Command {
        let mut cmd = crate::proc::command(&self.shared.cua_bin);
        self.shared.cua_env.apply(&mut cmd);
        cmd
    }

    /// Where our driver looks for cursor themes (inside the app's own
    /// driver home). None when this core has no private driver home.
    pub fn cua_theme_dir(&self) -> Option<PathBuf> {
        self.shared.cua_env.theme_dir()
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
            // A daemon that runs from the copy of the version before is
            // left alone while a session holds it: the session goes on,
            // and the next one starts on the new copy (`cua_endpoint_at`).
            if self.shared.cua_sessions_open() == 0 {
                self.shared.stop_cua().await;
            }
            *self.shared.cua_probe.lock().unwrap() = None;
            self.mutate_settings(|_| {});
        }
        state
    }

    /// Stop our own daemon if one runs (the next computer-control session
    /// starts a new one). The owner's daemon is never touched.
    pub async fn cua_stop_daemon(&self) {
        self.shared.stop_cua().await;
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

    // --- the owner's own apps ---

    /// The apps this computer offers, with the owner's switches. A Mac
    /// app is not in the list on Windows.
    pub fn apps_views(&self) -> Vec<crate::apps::AppView> {
        let s = self.shared.settings();
        crate::apps::views(self.shared.apps.backend().as_ref(), &s.apps)
    }

    /// Where the system permissions of one app stand. Read-only: never
    /// raises a prompt. May take a moment: call it off the UI thread.
    pub fn apps_permissions(&self, app: &str) -> Vec<crate::apps::Permission> {
        self.shared.apps.backend().permissions(app)
    }

    /// Make the system ask for a permission of one app. Shows a system
    /// window: call only from the owner's click.
    pub fn apps_request_permission(&self, app: &str, kind: crate::apps::PermissionKind) -> Result<(), String> {
        self.shared.apps.backend().request_permission(app, kind)
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
            computer_task: self.shared.computer_task(),
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
    /// The label the synth gave the computer-control task it is running,
    /// cut to what the driver shows under the agent's cursor. The synth's
    /// own words: show them as such. None when nothing is running.
    pub computer_task: Option<String>,
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

/// Is this something a cookie grant can be for: one site (a registrable
/// domain, as `registrable_domain` returns it) or one address? A bare
/// public suffix ("com", "co.uk") is not: its "subdomains" are every
/// site under it.
pub fn is_one_site(domain: &str) -> bool {
    domain.parse::<std::net::IpAddr>().is_ok() || psl::domain_str(domain).is_some_and(|d| d == domain)
}

/// Read the user's running daemon socket from `cua-driver status`
/// (read-only; we never touch that daemon).
async fn user_daemon_socket(bin: &str) -> Result<String, Refusal> {
    let out = crate::proc::command(bin)
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
            apps: Arc::new(crate::apps::stub::StubBackend::offering(&[])),
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
    async fn the_look_is_the_apps_own_business() {
        use crate::settings::{CursorColor, CursorSpeed};
        let (core, _) = test_core();
        let rev = core.apply_preset(Preset::Balanced);
        let mut told = core.shared().settings_tx.subscribe();
        told.borrow_and_update();

        // A change of the look is kept, and the synth hears nothing.
        let same = core.mutate_settings(|s| {
            s.computer.look.color = CursorColor::Rose;
            s.computer.look.speed = CursorSpeed::Fast;
            s.computer.look.screenshot_max = 1024;
        });
        assert_eq!(same, rev, "the rev must not move for a look change");
        assert!(!told.has_changed().unwrap(), "no settings message for a look change");
        assert_eq!(core.settings().computer.look.color, CursorColor::Rose);
        // Nothing of it is in what the synth is told.
        let caps = wire::capabilities_value(
            &core.settings(),
            "0.33.1",
            "ok",
            core.shared().apps.backend().as_ref(),
        )
        .to_string();
        for word in ["look", "rose", "cursor", "screenshot", "1024"] {
            assert!(!caps.contains(word), "the capabilities mention {word}: {caps}");
        }

        // A look change together with a policy change is a settings change.
        let next = core.mutate_settings(|s| {
            s.computer.look.color = CursorColor::Green;
            s.computer.foreground = crate::settings::Foreground::Never;
        });
        assert_eq!(next, rev + 1);
        assert!(told.has_changed().unwrap());

        // A preset is a policy: the owner's look stays.
        core.apply_preset(Preset::Careful);
        assert_eq!(core.settings().computer.look.color, CursorColor::Green);
        assert_eq!(core.settings().computer.look.screenshot_max, 1024);
    }

    #[tokio::test]
    async fn the_experiment_is_told_to_the_synth() {
        use crate::settings::Experiment;
        let (core, _) = test_core();
        let rev = core.apply_preset(Preset::Balanced);
        let mut told = core.shared().settings_tx.subscribe();
        told.borrow_and_update();
        let caps = |core: &Core| {
            wire::capabilities_value(&core.settings(), "0.33.1", "ok", core.shared().apps.backend().as_ref())
        };
        assert_eq!(caps(&core)["computer"]["experiment"], "default");

        // A change of the experiment is a settings change: the rev moves
        // and the synth is sent new capabilities.
        let next = core.mutate_settings(|s| s.computer.experiment = Experiment::Off);
        assert_eq!(next, rev + 1);
        assert!(told.has_changed().unwrap(), "a settings message for an experiment change");
        assert_eq!(caps(&core)["computer"]["experiment"], "off");
        told.borrow_and_update();
        core.mutate_settings(|s| s.computer.experiment = Experiment::On);
        assert!(told.has_changed().unwrap());
        assert_eq!(caps(&core)["computer"]["experiment"], "on");

        // A preset is a policy: the owner's choice of the experiment stays.
        core.apply_preset(Preset::Careful);
        assert_eq!(core.settings().computer.experiment, Experiment::On);
        // The look words stay out of the capabilities.
        let text = caps(&core).to_string();
        for word in ["look", "cursor", "screenshot"] {
            assert!(!text.contains(word), "the capabilities mention {word}: {text}");
        }
    }

    #[tokio::test]
    async fn the_pipe_refuses_what_the_synth_may_never_trigger() {
        use crate::mcp::{filter_line, Gate};
        use serde_json::json;
        let (core, _) = test_core();
        // The most permissive settings there are.
        core.apply_preset(Preset::Full);
        let sh = core.shared();
        let line = |tool: &str, args: serde_json::Value| {
            serde_json::to_vec(&json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": tool, "arguments": args}})).unwrap()
        };
        // The sentence, and the class and code of `error.data`.
        let refused_as = |gate: Gate| match gate {
            Gate::Refuse(v) => {
                assert_eq!(v["id"], 9, "the answer must carry the call's id");
                (
                    v["error"]["message"].as_str().unwrap_or("").to_string(),
                    v["error"]["data"]["class"].as_str().unwrap_or("").to_string(),
                    v["error"]["data"]["code"].as_str().unwrap_or("").to_string(),
                )
            }
            Gate::Forward(b) => panic!("forwarded: {}", String::from_utf8_lossy(&b)),
            Gate::Answer(v) => panic!("answered by the app: {v}"),
            Gate::Drop(why) => panic!("dropped: {why}"),
        };
        let refused = |gate: Gate| match gate {
            Gate::Refuse(v) => {
                assert_eq!(v["id"], 9, "the answer must carry the call's id");
                v["error"]["message"].as_str().unwrap_or("").to_string()
            }
            Gate::Forward(b) => panic!("forwarded: {}", String::from_utf8_lossy(&b)),
            Gate::Answer(v) => panic!("answered by the app: {v}"),
            Gate::Drop(why) => panic!("dropped: {why}"),
        };

        for (tool, args) in [
            ("start_recording", json!({"output_dir": "/tmp/rec", "record_video": true})),
            ("install_ffmpeg", json!({"confirm": true})),
            ("install_extension", json!({"name": "perception", "confirm": true})),
            ("check_for_update", json!({})),
            ("set_config", json!({"experimental_pip": true})),
            ("replay_trajectory", json!({"dir": "/tmp/rec"})),
        ] {
            let (said, class, code) = refused_as(filter_line(&sh, false, &line(tool, args)).await);
            assert!(said.starts_with("denied: "), "{tool}: {said}");
            assert!(said.contains("owner"), "{tool}: no words for the owner: {said}");
            assert_eq!((class.as_str(), code.as_str()), ("owner_policy", "never_allowed"), "{tool}");
        }
        // Ending and reading a recording, and ordinary work, pass.
        for tool in ["stop_recording", "get_recording_state", "get_config", "click", "get_window_state"] {
            match filter_line(&sh, false, &line(tool, json!({"session": "task-a"}))).await {
                Gate::Forward(_) => {}
                _ => panic!("{tool} must pass"),
            }
        }
        // A file the driver writes: no folder is shared, so nowhere.
        let (said, class, code) = refused_as(
            filter_line(&sh, false, &line("get_desktop_state", json!({"screenshot_out_file": "/tmp/shot.png"}))).await,
        );
        assert!(said.starts_with("denied: ") && said.contains("owner"), "{said}");
        // Corrected by leaving the argument out.
        assert_eq!(class, "bad_argument", "{said}");
        assert!(code.starts_with("write_arg_"), "{code}");
        let said = refused(
            filter_line(&sh, false, &line("browser_download", json!({"ref": "p1:1", "destination_root": "/tmp"}))).await,
        );
        assert!(said.starts_with("denied: "), "{said}");

        // The owner's switches: each refusal says its class and code.
        core.mutate_settings(|s| {
            s.computer.tool_denylist = vec!["press_key".into()];
            s.computer.foreground = crate::settings::Foreground::Never;
            s.computer.look.cursor = false;
        });
        for (tool, args, class, code) in [
            ("press_key", json!({"pid": 1, "window_id": 1, "key": "return"}), "owner_policy", "tool_denylist"),
            ("click", json!({"pid": 1, "window_id": 1, "x": 1, "y": 1, "delivery_mode": "foreground"}), "owner_policy", "foreground_off"),
            ("set_agent_cursor_theme", json!({"session": "task-a"}), "bad_argument", "cursor_look_is_owners"),
            ("move_cursor", json!({"session": "task-a", "x": 1, "y": 1}), "bad_argument", "agent_cursor_off"),
        ] {
            let (said, got_class, got_code) = refused_as(filter_line(&sh, false, &line(tool, args)).await);
            assert!(said.starts_with("denied: "), "{tool}: {said}");
            assert_eq!((got_class.as_str(), got_code.as_str()), (class, code), "{tool}: {said}");
        }

        // One line in the journal per refusal, with a reason the UI knows.
        let journal = core.recent_journal();
        assert_eq!(journal.iter().filter(|e| e.outcome == "denied: never allowed for the synth").count(), 6);
        assert_eq!(journal.iter().filter(|e| e.outcome == "denied: outside the shared folders").count(), 2);
    }

    /// A directory of a test's own in place of the driver: a unit test
    /// never starts a driver process.
    fn fixed_apps(core: &Core) {
        use crate::cua_apps::{AppRow, WinRow};
        let app = |pid: i64, name: &str, bundle_id: &str| AppRow {
            pid,
            name: name.into(),
            bundle_id: Some(bundle_id.into()),
            path: Some(format!("/Applications/{name}.app")),
            running: pid > 0,
        };
        core.shared().cua_directory().fix(
            vec![app(200, "Safari", "com.apple.Safari"), app(300, "1Password", "com.1password.1password"), app(0, "Mail", "com.apple.mail")],
            vec![WinRow { window_id: 21, pid: 200 }, WinRow { window_id: 31, pid: 300 }],
        );
    }

    #[tokio::test]
    async fn the_pipe_holds_the_owners_choice_of_apps() {
        use crate::cua_apps::Scope;
        use crate::mcp::{filter_line, filter_scoped, Gate};
        use crate::settings::{AppRef, AppsPolicy};
        use serde_json::json;
        let (core, _) = test_core();
        core.apply_preset(Preset::Full);
        fixed_apps(&core);
        let sh = core.shared();
        let line = |tool: &str, args: serde_json::Value| {
            serde_json::to_vec(&json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": tool, "arguments": args}})).unwrap()
        };
        let secret_window = json!({"session": "Mia: flights", "pid": 300, "window_id": 31});

        // Every app: the call goes to the driver as it is.
        assert!(matches!(filter_line(&sh, false, &line("get_window_state", secret_window.clone())).await, Gate::Forward(_)));
        assert!(matches!(filter_line(&sh, false, &line("get_desktop_state", json!({}))).await, Gate::Forward(_)));

        // Every app except the password manager.
        core.mutate_settings(|s| {
            s.computer.apps.policy = AppsPolicy::Except;
            s.computer.apps.except = vec![AppRef { id: "com.1password.1password".into(), name: "1Password".into(), loose: false }];
        });
        match filter_line(&sh, false, &line("get_window_state", secret_window.clone())).await {
            Gate::Refuse(v) => {
                assert_eq!(v["id"], 4);
                assert_eq!(v["error"]["data"], json!({"class": "owner_policy", "code": "app_excluded"}));
                let said = v["error"]["message"].as_str().unwrap();
                assert!(said.starts_with("denied: ") && said.contains("owner"), "{said}");
                assert!(!said.to_lowercase().contains("1password"), "the synth is not told the list: {said}");
            }
            _ => panic!("the excluded app's window was not refused"),
        }
        assert!(matches!(filter_line(&sh, false, &line("click", json!({"x": 5, "y": 5, "scope": "desktop"}))).await, Gate::Refuse(_)));
        match filter_line(&sh, false, &line("get_desktop_state", json!({}))).await {
            Gate::Refuse(v) => {
                assert_eq!(v["error"]["data"], json!({"class": "bad_argument", "code": "whole_screen_while_limited"}));
            }
            _ => panic!("the whole screen was not refused while apps are limited"),
        }
        assert!(matches!(filter_line(&sh, false, &line("get_window_state", json!({"pid": 200, "window_id": 21}))).await, Gate::Forward(_)));
        // The list is the app's own answer, with the call's id, and the
        // excluded app is not on it.
        match filter_line(&sh, false, &line("list_windows", json!({}))).await {
            Gate::Answer(v) => {
                assert_eq!(v["id"], 4);
                assert_eq!(v["jsonrpc"], "2.0");
                let windows = v["result"]["structuredContent"]["windows"].as_array().unwrap();
                assert_eq!(windows.len(), 1);
                assert_eq!(windows[0]["pid"], 200);
            }
            _ => panic!("list_windows was not answered by the app"),
        }
        // A catalog stream calls nothing, lists included.
        match filter_line(&sh, true, &line("list_windows", json!({}))).await {
            Gate::Refuse(v) => assert_eq!(v["error"]["data"], json!({"class": "owner_policy", "code": "catalog_stream"})),
            _ => panic!("a catalog stream called a tool"),
        }
        // What is never allowed stays never allowed, whichever app.
        assert!(matches!(filter_line(&sh, false, &line("start_recording", json!({"output_dir": "/tmp/r"}))).await, Gate::Refuse(_)));

        // A session decides by the choice its daemon was started with,
        // not by a change the owner made meanwhile.
        let began_open = Scope::open();
        assert!(matches!(filter_scoped(&sh, &began_open, false, &line("get_window_state", secret_window.clone())).await, Gate::Forward(_)));

        // The journal: what happened, never which app or which window.
        let journal = serde_json::to_string(&core.recent_journal()).unwrap();
        assert!(journal.contains("denied: an app the owner excluded"));
        assert!(journal.contains("denied: the whole screen, while apps are limited"));
        for word in ["1Password", "1password", "Safari", "Mia", "flights", "\"31\"", "300"] {
            assert!(!journal.contains(word), "the journal holds {word}: {journal}");
        }
    }

    #[tokio::test]
    async fn a_preset_keeps_the_owners_choice_of_apps_and_a_change_of_it_is_a_settings_change() {
        use crate::settings::{AppRef, AppsPolicy};
        let (core, _) = test_core();
        let rev = core.apply_preset(Preset::Balanced);
        let next = core.mutate_settings(|s| {
            s.computer.apps.policy = AppsPolicy::Only;
            s.computer.apps.only = vec![AppRef { id: "com.apple.Safari".into(), name: "Safari".into(), loose: false }];
        });
        assert_eq!(next, rev + 1);
        core.apply_preset(Preset::Careful);
        let scope = core.settings().computer.apps;
        assert_eq!(scope.policy, AppsPolicy::Only);
        assert_eq!(scope.only.len(), 1);
        // The synth is told nothing of it: no list, no choice.
        let caps = wire::capabilities_value(&core.settings(), "0.33.1", "ok", core.shared().apps.backend().as_ref()).to_string();
        for word in ["Safari", "safari", "only", "except", "policy"] {
            assert!(!caps.contains(word), "the capabilities mention {word}: {caps}");
        }
    }

    #[tokio::test]
    async fn a_session_is_out_of_date_when_the_owners_choice_moved_on() {
        use crate::cua_apps::Scope;
        use crate::settings::{AppRef, AppsPolicy};
        let (core, _) = test_core();
        core.apply_preset(Preset::Full);
        let sh = core.shared();
        let held = |scope: Scope, started: Instant| CuaSession {
            socket: "s".into(),
            embedded: true,
            scope: Arc::new(scope),
            look: None,
            started,
            bin: None,
            shared: sh.clone(),
        };
        sh.cua_sessions.fetch_add(3, Ordering::AcqRel);
        let now = Instant::now();
        let a = held(Scope::of(&core.settings()), now);
        assert!(!a.outdated());
        core.mutate_settings(|s| {
            s.computer.apps.policy = AppsPolicy::Only;
            s.computer.apps.only = vec![AppRef { id: "com.apple.Safari".into(), name: "Safari".into(), loose: false }];
        });
        assert!(a.outdated(), "the session began with every app, the owner now allows one");
        // A session of the new choice is current, until its bounded daemon
        // is half a day old.
        let b = held(Scope::of(&core.settings()), now);
        assert!(!b.outdated());
        if let Some(long_ago) = now.checked_sub(crate::cua_apps::RENEW_AFTER + Duration::from_secs(1)) {
            let c = held(Scope::of(&core.settings()), long_ago);
            assert!(c.outdated(), "a bounded daemon is renewed after half its day");
            // Only a bounded daemon has a day to run out.
            core.mutate_settings(|s| s.computer.apps.policy = AppsPolicy::All);
            assert!(!held(Scope::open(), long_ago).outdated());
        }
    }

    #[tokio::test]
    async fn the_task_label_is_shown_as_the_driver_shows_it_and_kept_nowhere() {
        use crate::mcp::filter_line;
        use serde_json::json;
        assert_eq!(task_label("Mia: flights").as_deref(), Some("Mia: flights"));
        // Exactly the driver's rule: a control character (a line break
        // and a tab are two) is dropped, not turned into a space.
        assert_eq!(task_label("  Mia:\n\tflights \u{7} to   Lisbon ").as_deref(), Some("Mia:flights to Lisbon"));
        assert_eq!(task_label("  Research\n\t run \u{7} ").as_deref(), Some("Research run"));
        assert_eq!(task_label("\u{41c}\u{438}\u{44f}: \u{431}\u{438}\u{43b}\u{435}\u{442}\u{44b}").as_deref(), Some("\u{41c}\u{438}\u{44f}: \u{431}\u{438}\u{43b}\u{435}\u{442}\u{44b}"));
        assert_eq!(task_label(""), None);
        assert_eq!(task_label(" \n\t"), None);
        // 28 characters at most: 27 and an ellipsis, as under the cursor.
        let long = task_label("Mia: a label that is much longer than the driver shows").unwrap();
        assert_eq!(long.chars().count(), TASK_LABEL_CHARS);
        assert_eq!(long, "Mia: a label that is much l\u{2026}");
        assert_eq!(task_label("abcdefghijklmnopqrstuvwxyz01").unwrap().chars().count(), 28);

        let (core, _) = test_core();
        core.apply_preset(Preset::Full);
        let sh = core.shared();
        let mut events = core.subscribe();
        assert_eq!(core.computer_task(), None);
        let line = |args: serde_json::Value| {
            serde_json::to_vec(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "get_screen_size", "arguments": args}})).unwrap()
        };
        filter_line(&sh, false, &line(json!({"session": "Mia:  flights\n<b>now</b>"}))).await;
        assert_eq!(core.computer_task().as_deref(), Some("Mia: flights<b>now</b>"));
        assert_eq!(core.snapshot().await.computer_task.as_deref(), Some("Mia: flights<b>now</b>"));
        assert!(matches!(events.try_recv(), Ok(CoreEvent::ComputerTask)));
        // The same label again is no news; a call without one changes nothing.
        filter_line(&sh, false, &line(json!({"session": "Mia:  flights\n<b>now</b>"}))).await;
        filter_line(&sh, false, &line(json!({}))).await;
        assert!(events.try_recv().is_err());
        assert_eq!(core.computer_task().as_deref(), Some("Mia: flights<b>now</b>"));
        // The journal says which tool, never what the synth called the task.
        let journal = serde_json::to_string(&core.recent_journal()).unwrap();
        assert!(journal.contains("get_screen_size") && !journal.contains("Mia") && !journal.contains("flights"), "{journal}");
        // The session ended: no task.
        sh.clear_task();
        assert_eq!(core.computer_task(), None);
        assert!(matches!(events.try_recv(), Ok(CoreEvent::ComputerTask)));
    }

    #[tokio::test]
    async fn after_an_update_the_copy_of_the_version_before_runs_until_the_new_one_is_in_place() {
        let root = std::env::temp_dir().join(format!("connect-core-oldcopy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let bin_name = if cfg!(windows) { "cua-driver.exe" } else { "cua-driver" };
        let old = root.join("releases").join("0.1.0").join(bin_name);
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, b"the copy the app version before installed").unwrap();
        let core = Core::new(CoreConfig {
            settings_path: None,
            journal_path: None,
            approver: Arc::new(AutoAllow),
            cookies: Arc::new(MultiReader { chromium: vec![], firefox: vec![], keys: Arc::new(crate::cookies::fixtures::NoKey) }),
            apps: Arc::new(crate::apps::stub::StubBackend::offering(&[])),
            secret: Arc::new(MemSecretStore::new()),
            hub: Arc::new(FakeHub),
            app_version: "test".into(),
            cua_bin: String::new(),
            cua_private_root: Some(root.clone()),
            cua_attach: CuaAttach::Private,
        });
        core.apply_preset(Preset::Full);
        let sh = core.shared();
        let pinned = root.join("releases").join(cua::PINNED_VERSION).join(bin_name);
        assert_eq!(core.cua_bin(), pinned.to_string_lossy());
        // The pinned version is not there yet: the old copy is what runs.
        assert_eq!(sh.cua_bin_now(), old.to_string_lossy());
        assert_eq!(sh.cua_proxy_bin(), old.to_string_lossy());
        // A session on a daemon of the old copy is current for now.
        let held = CuaSession {
            socket: "s".into(),
            embedded: true,
            scope: Arc::new(crate::cua_apps::Scope::of(&core.settings())),
            look: None,
            started: Instant::now(),
            bin: Some(old.to_string_lossy().to_string()),
            shared: sh.clone(),
        };
        sh.cua_sessions.fetch_add(1, Ordering::AcqRel);
        assert!(!held.outdated());
        // The new version lands: new daemons get it, and the open session
        // learns that it is on the old one.
        std::fs::create_dir_all(pinned.parent().unwrap()).unwrap();
        std::fs::write(&pinned, b"the pinned copy").unwrap();
        assert_eq!(sh.cua_bin_now(), pinned.to_string_lossy());
        assert!(held.outdated());
        drop(held);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_session_holds_the_daemon_until_it_ends() {
        let (core, _) = test_core();
        let sh = core.shared();
        assert_eq!(sh.cua_sessions_open(), 0);
        let held = |socket: &str| CuaSession {
            socket: socket.into(),
            embedded: true,
            scope: Arc::new(crate::cua_apps::Scope::open()),
            look: None,
            started: Instant::now(),
            bin: None,
            shared: sh.clone(),
        };
        let a = held("a");
        sh.cua_sessions.fetch_add(1, Ordering::AcqRel);
        let b = held("b");
        sh.cua_sessions.fetch_add(1, Ordering::AcqRel);
        assert_eq!(sh.cua_sessions_open(), 2);
        drop(a);
        // A session that is aborted (pause, a dropped link) lets go too.
        let task = tokio::spawn(async move {
            let _held = b;
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;
        task.abort();
        let _ = task.await;
        assert_eq!(sh.cua_sessions_open(), 0);
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
            apps: Arc::new(crate::apps::stub::StubBackend::offering(&[])),
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
            apps: Arc::new(crate::apps::stub::StubBackend::offering(&[])),
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
    async fn open_streams_are_counted_until_they_end() {
        let (core, _) = test_core();
        assert_eq!(core.active_streams(), 0);
        let a = crate::link::OpenStream::begin(&core.shared());
        let b = crate::link::OpenStream::begin(&core.shared());
        assert_eq!(core.active_streams(), 2);
        drop(a);
        assert_eq!(core.active_streams(), 1);
        // A task that is aborted (pause, a dropped link) frees its count too.
        let task = tokio::spawn(async move {
            let _open = b;
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;
        task.abort();
        let _ = task.await;
        assert_eq!(core.active_streams(), 0);
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
