//! `apps`: the owner's own apps on this computer (Messages, Contacts,
//! Calendar, Reminders, Notes, Shortcuts on a Mac; notifications, the
//! clipboard and "open" everywhere).
//!
//! How it is put together:
//!
//! - `CATALOG` names every action and its risk class: READ (list, search,
//!   read), ACT (create, change, send, run, write) or ACT-light (show a
//!   notification).
//! - `Request::parse` (request.rs) checks and bounds everything the synth
//!   sent, before anyone is asked anything.
//! - `handle` is the ONE gate. Every `apps.*` rpc call goes through it and
//!   nothing else reaches a backend: pause, the master switch, what this
//!   computer offers, the per-app switch, the risk class against the
//!   owner's mode, the ask, the bounds on the result, the journal line.
//!   A backend never decides whether something is allowed.
//! - `Backend` does the work for one system: `mac.rs`, `win.rs`,
//!   `linux.rs`. `stub.rs` is the in-memory stand-in for tests and for the
//!   loopback script. A backend offers only what its system has, so a
//!   Windows computer never lists Calendar, to the owner or to the synth.
//!
//! Rules the gate holds (tests at the bottom):
//!
//! - READ never asks: the per-app switch is the consent for reading.
//!   Results are bounded and filtered (`bound`).
//! - ACT follows `apps.mode`: `ask` asks every time and remembers nothing,
//!   `ask_new` asks and may remember per kind of change, `auto` does not
//!   ask.
//! - Sending a message as the owner asks EVERY time, in every mode, with
//!   the recipient and the whole text in the window. The only thing that
//!   can skip the ask is the owner's own "always" for that exact
//!   recipient (scope `messages.send:<recipient>`), and not even that in
//!   `ask` mode. No other grant, mode or request covers it.
//! - The journal gets one line per call: app, action, outcome, a count.
//!   Never a message, a contact, a title or a note.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::core::Shared;
use crate::grant::{ApprovalRequest, Remember};
use crate::journal::Entry;
use crate::settings::{AppsMode, AppsSettings, Root, Settings};

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod mac;
pub mod notify;
pub mod request;
pub mod stub;
pub mod time;
#[cfg(windows)]
pub mod win;

pub use notify::{Notifier, SystemNotifier};
pub use request::Request;

/// The capability name on the wire and in grants.
pub const CAP: &str = "apps";

/// How long a backend may take for one action.
const CALL_TIMEOUT: Duration = Duration::from_secs(45);
/// A shortcut is the owner's own program and may take longer.
const SHORTCUT_TIMEOUT: Duration = Duration::from_secs(120);
/// At most this many notifications per minute.
const NOTIFY_PER_MINUTE: usize = 6;

/// What an action may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    /// Looks at the owner's data. Never asks; bounded.
    Read,
    /// Creates, changes, sends or runs something.
    Act,
    /// Shows a notification: an action, but one that never asks.
    ActLight,
}

/// One action of one app.
#[derive(Debug, Clone, Copy)]
pub struct Action {
    pub app: &'static str,
    pub action: &'static str,
    pub risk: Risk,
}

const fn a(app: &'static str, action: &'static str, risk: Risk) -> Action {
    Action { app, action, risk }
}

/// Every action there is. The rpc method is `apps.<app>.<action>`.
pub const CATALOG: &[Action] = &[
    a("messages", "recent", Risk::Read),
    a("messages", "send", Risk::Act),
    a("contacts", "search", Risk::Read),
    a("calendar", "list", Risk::Read),
    a("calendar", "create", Risk::Act),
    a("reminders", "list", Risk::Read),
    a("reminders", "create", Risk::Act),
    a("reminders", "complete", Risk::Act),
    a("notes", "search", Risk::Read),
    a("notes", "read", Risk::Read),
    a("notes", "create", Risk::Act),
    a("shortcuts", "list", Risk::Read),
    a("shortcuts", "run", Risk::Act),
    a("notify", "show", Risk::ActLight),
    a("clipboard", "read", Risk::Read),
    a("clipboard", "write", Risk::Act),
    a("open", "url", Risk::Act),
    a("open", "file", Risk::Act),
    a("open", "app", Risk::Act),
];

/// Every app id, in the order they are listed everywhere.
pub const APPS: &[&str] = &[
    "messages", "contacts", "calendar", "reminders", "notes", "shortcuts", "notify", "clipboard", "open",
];

/// The apps that exist only on a Mac.
pub const MAC_ONLY: &[&str] = &["messages", "contacts", "calendar", "reminders", "notes", "shortcuts"];

pub fn find(app: &str, action: &str) -> Option<&'static Action> {
    CATALOG.iter().find(|x| x.app == app && x.action == action)
}

/// All catalog actions of one app.
pub fn actions_of(app: &str) -> Vec<&'static str> {
    CATALOG.iter().filter(|x| x.app == app).map(|x| x.action).collect()
}

/// The name of an app in sentences the synth reads.
pub fn app_title(app: &str) -> &'static str {
    match app {
        "messages" => "Messages",
        "contacts" => "Contacts",
        "calendar" => "Calendar",
        "reminders" => "Reminders",
        "notes" => "Notes",
        "shortcuts" => "Shortcuts",
        "notify" => "notifications",
        "clipboard" => "the clipboard",
        "open" => "opening links, files and apps",
        _ => "this app",
    }
}

/// Why a call did not produce a result. `Display` is what the synth sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppError {
    /// A policy decision (settings, the owner said no).
    Denied(String),
    /// Cannot work right now (a missing permission, an app not set up).
    Unavailable(String),
    /// The named thing does not exist.
    NotFound(String),
    /// The call itself is wrong.
    BadRequest(String),
    /// It was allowed and it failed.
    Failed(String),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::Denied(m) => write!(f, "denied: {m}"),
            AppError::Unavailable(m) => write!(f, "unavailable: {m}"),
            AppError::NotFound(m) => write!(f, "not found: {m}"),
            AppError::BadRequest(m) => write!(f, "bad request: {m}"),
            AppError::Failed(m) => f.write_str(m),
        }
    }
}

/// One app this computer has, and what it can do here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub app: &'static str,
    pub actions: Vec<&'static str>,
}

impl Offer {
    /// An app with every action the catalog has for it.
    pub fn all(app: &'static str) -> Offer {
        Offer { app, actions: actions_of(app) }
    }
}

/// A permission of the operating system an app needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    /// macOS Automation: this app may send commands to that app.
    Automation,
    /// macOS Full Disk Access (reading the Messages history).
    FullDisk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Granted,
    /// The owner (or a policy) said no: only System Settings changes it.
    Denied,
    /// The system has not asked yet: it will on first use, or on request.
    NotAsked,
    /// Cannot be told without using the app (for example it is not open).
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Permission {
    pub kind: PermissionKind,
    pub state: PermissionState,
}

pub type CallFuture = Pin<Box<dyn Future<Output = Result<Value, AppError>> + Send>>;

/// What a backend is handed for one call. The request is checked already.
#[derive(Debug, Clone)]
pub struct Call {
    pub request: Request,
    /// The synth's name, for the title of a notification.
    pub synth_name: String,
}

/// The apps of one system. Implementations do the work and nothing else:
/// whether a call is allowed was decided before `call`.
pub trait Backend: Send + Sync {
    /// The apps and actions this computer has. An app that does not exist
    /// on this system must not be listed.
    fn offered(&self) -> Vec<Offer>;

    /// Carry out one checked request.
    fn call(&self, call: Call) -> CallFuture;

    /// The system permissions one app needs and where they stand.
    /// Read-only: must never raise a prompt. Empty = none needed.
    fn permissions(&self, _app: &str) -> Vec<Permission> {
        Vec::new()
    }

    /// Make the system ask for a permission (or do the first harmless
    /// call that makes it ask). Only ever from the owner's click.
    fn request_permission(&self, _app: &str, _kind: PermissionKind) -> Result<(), String> {
        Ok(())
    }
}

/// The backend for the system this build runs on. `notifier` shows
/// notifications; None = the system's own tools (`SystemNotifier`).
pub fn system_backend(notifier: Option<Arc<dyn Notifier>>) -> Arc<dyn Backend> {
    let notifier = notifier.unwrap_or_else(|| Arc::new(SystemNotifier));
    #[cfg(target_os = "macos")]
    {
        Arc::new(mac::MacBackend::system(notifier))
    }
    #[cfg(windows)]
    {
        Arc::new(win::WinBackend::new(notifier))
    }
    #[cfg(target_os = "linux")]
    {
        Arc::new(linux::LinuxBackend::detect(notifier))
    }
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    {
        let _ = notifier;
        Arc::new(stub::StubBackend::offering(&[]))
    }
}

/// What the core keeps for apps.
pub struct State {
    pub(crate) backend: Arc<dyn Backend>,
    notified: Mutex<VecDeque<Instant>>,
}

impl State {
    pub fn new(backend: Arc<dyn Backend>) -> State {
        State { backend, notified: Mutex::new(VecDeque::new()) }
    }

    pub fn backend(&self) -> &Arc<dyn Backend> {
        &self.backend
    }

    /// Count one notification; false when the minute is full.
    fn notify_allowed(&self) -> bool {
        let mut q = self.notified.lock().unwrap();
        let now = Instant::now();
        while q.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60)) {
            q.pop_front();
        }
        if q.len() >= NOTIFY_PER_MINUTE {
            return false;
        }
        q.push_back(now);
        true
    }
}

/// One app as the owner and the synth see it: what this computer offers,
/// and whether the owner's switch for it is on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppView {
    pub id: &'static str,
    pub on: bool,
    pub actions: Vec<&'static str>,
}

/// The offered apps in catalog order, with the owner's switches. Apps the
/// backend does not offer are simply absent.
pub fn views(backend: &dyn Backend, settings: &AppsSettings) -> Vec<AppView> {
    let offered = backend.offered();
    APPS.iter()
        .filter_map(|id| {
            let offer = offered.iter().find(|o| o.app == *id)?;
            // Only actions the catalog knows, in catalog order.
            let actions: Vec<&'static str> =
                actions_of(id).into_iter().filter(|x| offer.actions.contains(x)).collect();
            if actions.is_empty() {
                return None;
            }
            Some(AppView { id, on: settings.app_on(id), actions })
        })
        .collect()
}

/// The `capabilities.apps` object of hello / settings (connect.md).
pub fn wire_value(backend: &dyn Backend, settings: &AppsSettings) -> Value {
    let mode = match settings.mode {
        AppsMode::Ask => "ask",
        AppsMode::AskNew => "ask_new",
        AppsMode::Auto => "auto",
    };
    json!({
        "enabled": settings.enabled,
        "mode": mode,
        "apps": views(backend, settings),
    })
}

/// What the gate does with a request once the switches allow it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Carry it out.
    Pass,
    /// Ask the owner. `scope` is what a remembered answer is stored
    /// under, `remember` what the owner may ask to be remembered.
    Ask { scope: String, remember: Remember },
}

/// The policy, as a pure function: risk class + the owner's mode + the
/// request -> pass or ask. This is the only place that decides.
pub fn decide(mode: AppsMode, risk: Risk, req: &Request) -> Decision {
    // Sending a message as the owner: outside the mode. It asks in every
    // mode. The scope names the exact recipient, so the only answer that
    // can be remembered is "always for this person".
    if let Request::MessagesSend { to, .. } = req {
        return Decision::Ask {
            scope: format!("messages.send:{to}"),
            remember: if mode == AppsMode::Ask { Remember::Never } else { Remember::AlwaysOnly },
        };
    }
    match risk {
        Risk::Read | Risk::ActLight => Decision::Pass,
        Risk::Act => match mode {
            AppsMode::Auto => Decision::Pass,
            AppsMode::Ask => Decision::Ask { scope: scope_of(req), remember: Remember::Never },
            AppsMode::AskNew => {
                // A shortcut that gets input may run that input (a "run
                // shell script" step): a remembered yes for the shortcut
                // never covers a run with input.
                let remember = match req {
                    Request::ShortcutsRun { input, .. } if !input.is_empty() => Remember::Never,
                    _ => Remember::Any,
                };
                Decision::Ask { scope: scope_of(req), remember }
            }
        },
    }
}

/// The grant scope of an ACT request: the kind of change, narrowed to the
/// one thing where that is what the owner would mean by "always".
fn scope_of(req: &Request) -> String {
    match req {
        Request::MessagesSend { to, .. } => format!("messages.send:{to}"),
        Request::CalendarCreate { .. } => "calendar.create".into(),
        Request::RemindersCreate { .. } => "reminders.create".into(),
        Request::RemindersComplete { .. } => "reminders.complete".into(),
        Request::NotesCreate { .. } => "notes.create".into(),
        Request::ShortcutsRun { name, .. } => format!("shortcuts.run:{name}"),
        Request::ClipboardWrite { .. } => "clipboard.write".into(),
        Request::OpenUrl { host, .. } => format!("open.url:{host}"),
        Request::OpenFile { .. } => "open.file".into(),
        Request::OpenApp { name } => format!("open.app:{}", name.to_lowercase()),
        // Reads and notifications are never asked about.
        _ => String::new(),
    }
}

/// The facts of an ACT request for the approval window, in full: what
/// will be sent or created, not a description of it.
fn details_of(req: &Request) -> Vec<(String, String)> {
    let kv = |k: &str, v: &str| (k.to_string(), v.to_string());
    match req {
        Request::MessagesSend { to, text, service } => {
            vec![kv("to", to), kv("service", service.label()), kv("text", text)]
        }
        Request::CalendarCreate { title, start, end, calendar, location, notes, all_day } => {
            let (s, e) = (start.local(), end.local());
            let when = if *all_day {
                format!("{:04}-{:02}-{:02} .. {:04}-{:02}-{:02}", s.year, s.month, s.day, e.year, e.month, e.day)
            } else {
                format!("{} .. {}", s.display(), e.display())
            };
            let mut d = vec![kv("title", title), kv("when", &when)];
            if let Some(c) = calendar {
                d.push(kv("calendar", c));
            }
            if !location.is_empty() {
                d.push(kv("location", location));
            }
            if !notes.is_empty() {
                d.push(kv("notes", notes));
            }
            d
        }
        Request::RemindersCreate { title, due, list, notes } => {
            let mut d = vec![kv("title", title)];
            if let Some(due) = due {
                d.push(kv("when", &due.local().display()));
            }
            if let Some(l) = list {
                d.push(kv("list", l));
            }
            if !notes.is_empty() {
                d.push(kv("notes", notes));
            }
            d
        }
        Request::RemindersComplete { id } => vec![kv("id", id)],
        Request::NotesCreate { title, text, folder } => {
            let mut d = vec![kv("title", title)];
            if let Some(f) = folder {
                d.push(kv("folder", f));
            }
            if !text.is_empty() {
                d.push(kv("text", text));
            }
            d
        }
        Request::ShortcutsRun { name, input } => {
            let mut d = vec![kv("name", name)];
            if !input.is_empty() {
                d.push(kv("input", input));
            }
            d
        }
        Request::ClipboardWrite { text } => vec![kv("text", text)],
        Request::OpenUrl { url, .. } => vec![kv("url", url)],
        Request::OpenFile { path } => vec![kv("path", &path.to_string_lossy())],
        Request::OpenApp { name } => vec![kv("name", name)],
        _ => Vec::new(),
    }
}

/// What the journal may name for a call: never content. A site, a file
/// path, an app or a shortcut name where the action has one; nothing for
/// messages, contacts, events, reminders and notes.
fn journal_target(req: &Request) -> String {
    match req {
        Request::OpenUrl { host, .. } => host.clone(),
        Request::OpenFile { path } => path.to_string_lossy().to_string(),
        Request::OpenApp { name } => name.clone(),
        Request::ShortcutsRun { name, .. } => name.clone(),
        _ => String::new(),
    }
}

/// Serve one `apps.<app>.<action>` rpc call. The ONE gate (see the top
/// of this file). The error is the text the synth gets.
pub async fn handle(shared: &Arc<Shared>, method: &str, params: &Value) -> Result<Value, String> {
    let (app, action) = method
        .strip_prefix("apps.")
        .and_then(|m| m.split_once('.'))
        .ok_or_else(|| format!("denied: unknown method {method:?}"))?;
    let Some(spec) = find(app, action) else {
        return Err(format!("denied: unknown method {method:?}"));
    };
    let label = format!("{app}.{action}");
    let record = |target: String, outcome: String, count: Option<u64>| {
        let mut e = Entry::new(CAP, &label, target, outcome);
        e.count = count;
        shared.journal().record(e);
    };
    let refuse = |why: AppError| -> String {
        let text = why.to_string();
        // Malformed calls are the synth's mistakes, not activity on the
        // owner's computer: no journal line for them.
        if !matches!(why, AppError::BadRequest(_)) {
            record(String::new(), text.clone(), None);
        }
        text
    };

    if shared.paused() {
        return Err(refuse(AppError::Denied("apps are paused by the owner".into())));
    }
    let settings: Settings = shared.settings();
    if !settings.apps.enabled {
        return Err(refuse(AppError::Denied("apps are turned off on this computer".into())));
    }
    let backend = shared.apps.backend.clone();
    // Not on this system (Calendar on Windows) reads the same as an
    // unknown app would to an owner: it is simply not here.
    let offered = backend.offered();
    let has = offered.iter().any(|o| o.app == app && o.actions.contains(&action));
    if !has {
        return Err(refuse(AppError::Unavailable(format!(
            "{} ({action}) is not available on this computer",
            app_title(app)
        ))));
    }
    if !settings.apps.app_on(app) {
        return Err(refuse(AppError::Denied(format!(
            "{} is turned off on this computer; only the owner can turn it on in the app",
            app_title(app)
        ))));
    }

    let mut request = Request::parse(app, action, params).map_err(refuse)?;
    // A file is opened only inside the folders the owner shared, checked
    // after symlinks are resolved, and only if it is a document.
    if let Request::OpenFile { path } = &mut request {
        *path = resolve_openable(path, &settings).await.map_err(refuse)?;
    }
    let reason = params.get("reason").and_then(Value::as_str).unwrap_or("");

    if let Decision::Ask { scope, remember } = decide(settings.apps.mode, spec.risk, &request) {
        let granted = remember != Remember::Never && shared.grants().allows(CAP, &scope);
        if !granted {
            let ask = ApprovalRequest::new(CAP, &scope, reason, format!("let the synth do {label}"))
                .with_details(details_of(&request))
                .with_remember(remember);
            let answer = shared.ask_scoped(CAP, &scope, ask).await;
            if !answer.allow {
                record(journal_target(&request), "denied: owner".into(), None);
                return Err("denied: the owner did not allow it".into());
            }
        }
    }

    if matches!(request, Request::NotifyShow { .. }) && !shared.apps.notify_allowed() {
        return Err(refuse(AppError::Denied(format!(
            "too many notifications: at most {NOTIFY_PER_MINUTE} a minute"
        ))));
    }

    let target = journal_target(&request);
    let synth_name = shared
        .synth
        .read()
        .unwrap()
        .as_ref()
        .map(|s| s.synth_name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "SNTH".to_string());
    let limit = if matches!(request, Request::ShortcutsRun { .. }) { SHORTCUT_TIMEOUT } else { CALL_TIMEOUT };
    let call = Call { request: request.clone(), synth_name };
    let result = match tokio::time::timeout(limit, backend.call(call)).await {
        Ok(r) => r,
        Err(_) => Err(AppError::Unavailable(format!(
            "{} did not answer in {} s",
            app_title(app),
            limit.as_secs()
        ))),
    };
    match result {
        Ok(value) => {
            let (value, count) = bound(&request, value);
            record(target, "ok".into(), count);
            Ok(value)
        }
        Err(why) => {
            let text = why.to_string();
            record(target, failure_outcome(&why), None);
            Err(text)
        }
    }
}

/// The journal outcome of a failure: its label, never its details (an
/// error text can quote what the app said).
fn failure_outcome(why: &AppError) -> String {
    match why {
        AppError::Denied(m) => format!("denied: {m}"),
        AppError::Unavailable(_) => "failed: unavailable".into(),
        AppError::NotFound(_) => "failed: not found".into(),
        AppError::BadRequest(_) => "failed: bad request".into(),
        AppError::Failed(_) => "failed".into(),
    }
}

/// `open.file`: the path must sit inside a shared folder (after symlink
/// resolution, the same check `files.read` uses) and be a document.
async fn resolve_openable(path: &std::path::Path, settings: &Settings) -> Result<std::path::PathBuf, AppError> {
    if !settings.files.enabled || settings.files.roots.is_empty() {
        return Err(AppError::Denied(
            "no folder is shared on this computer, and only files in shared folders can be opened".into(),
        ));
    }
    let roots: &[Root] = &settings.files.roots;
    let real = crate::files::resolve_read(&path.to_string_lossy(), roots)
        .await
        .map_err(|e| {
            if let Some(m) = e.strip_prefix("not found: ") {
                AppError::NotFound(m.to_string())
            } else {
                AppError::Denied(e.strip_prefix("denied: ").unwrap_or(&e).to_string())
            }
        })?;
    let md = tokio::fs::metadata(&real)
        .await
        .map_err(|_| AppError::NotFound(path.to_string_lossy().to_string()))?;
    request::openable(&real, md.is_dir())?;
    #[cfg(unix)]
    if md.is_file() {
        use std::os::unix::fs::PermissionsExt;
        if md.permissions().mode() & 0o111 != 0 {
            return Err(AppError::Denied(
                "this file is marked as a program and is not opened".into(),
            ));
        }
    }
    Ok(real)
}

fn cap_str(v: &mut Value, key: &str, max: usize) -> bool {
    if let Some(s) = v.get(key).and_then(Value::as_str) {
        let (short, was_cut) = request::cut(s, max);
        if was_cut {
            v[key] = Value::String(short);
            return true;
        }
    }
    false
}

fn cap_list(v: &mut Value, key: &str, max: usize) -> (usize, bool) {
    match v.get_mut(key).and_then(Value::as_array_mut) {
        Some(items) => {
            let was_cut = items.len() > max;
            items.truncate(max);
            (items.len(), was_cut)
        }
        None => (0, false),
    }
}

fn mark_truncated(v: &mut Value, was_cut: bool) {
    if was_cut || v.get("truncated").and_then(Value::as_bool) == Some(true) {
        v["truncated"] = Value::Bool(true);
    } else if v.is_object() {
        v["truncated"] = Value::Bool(false);
    }
}

fn digits(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

/// Does this contact answer the query? By name, organization or email
/// (case-insensitive substring), or by the digits of a phone number.
fn contact_matches(contact: &Value, query: &str) -> bool {
    let q = query.to_lowercase();
    let field = |k: &str| contact.get(k).and_then(Value::as_str).unwrap_or("").to_lowercase();
    let list = |k: &str| -> Vec<String> {
        contact
            .get(k)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default()
    };
    if field("name").contains(&q) || field("organization").contains(&q) {
        return true;
    }
    if list("emails").iter().any(|e| e.to_lowercase().contains(&q)) {
        return true;
    }
    let qd = digits(query);
    qd.len() >= request::CONTACT_DIGITS_MIN && list("phones").iter().any(|p| digits(p).contains(&qd))
}

/// Bound and filter a backend's result to what was asked, whatever the
/// backend returned: list lengths, text lengths, and for a contact search
/// only real matches. Returns the value and the count for the journal.
pub fn bound(req: &Request, mut v: Value) -> (Value, Option<u64>) {
    match req {
        Request::MessagesRecent { limit, .. } => {
            let (n, was_cut) = cap_list(&mut v, "messages", (*limit).min(request::MESSAGES_MAX));
            if let Some(items) = v.get_mut("messages").and_then(Value::as_array_mut) {
                for m in items {
                    cap_str(m, "text", request::MESSAGE_TEXT_MAX);
                    // No attachments in this version: a count at most,
                    // never a path or a name.
                    if let Some(o) = m.as_object_mut() {
                        o.retain(|k, _| matches!(k.as_str(), "time" | "from" | "chat" | "chat_name" | "text" | "service" | "attachments"));
                    }
                }
            }
            mark_truncated(&mut v, was_cut);
            (v, Some(n as u64))
        }
        Request::ContactsSearch { query, limit } => {
            let mut was_cut = false;
            if let Some(items) = v.get_mut("contacts").and_then(Value::as_array_mut) {
                items.retain(|c| contact_matches(c, query));
                for c in items.iter_mut() {
                    cap_str(c, "name", request::NAME_MAX);
                    cap_str(c, "organization", request::NAME_MAX);
                    for key in ["phones", "emails"] {
                        cap_list(c, key, 10);
                    }
                    if let Some(o) = c.as_object_mut() {
                        o.retain(|k, _| matches!(k.as_str(), "name" | "organization" | "phones" | "emails"));
                    }
                }
                let max = (*limit).min(request::CONTACTS_MAX);
                was_cut = items.len() > max;
                items.truncate(max);
            }
            let n = v.get("contacts").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
            mark_truncated(&mut v, was_cut);
            (v, Some(n as u64))
        }
        Request::CalendarList { limit, .. } => {
            let (n, was_cut) = cap_list(&mut v, "events", (*limit).min(request::EVENTS_MAX));
            if let Some(items) = v.get_mut("events").and_then(Value::as_array_mut) {
                for e in items {
                    cap_str(e, "title", request::TITLE_MAX);
                    cap_str(e, "location", request::TITLE_MAX);
                    cap_str(e, "notes", request::BODY_MAX);
                }
            }
            mark_truncated(&mut v, was_cut);
            (v, Some(n as u64))
        }
        Request::RemindersList { limit, .. } => {
            let (n, was_cut) = cap_list(&mut v, "reminders", (*limit).min(request::REMINDERS_MAX));
            if let Some(items) = v.get_mut("reminders").and_then(Value::as_array_mut) {
                for r in items {
                    cap_str(r, "title", request::TITLE_MAX);
                    cap_str(r, "notes", request::BODY_MAX);
                }
            }
            mark_truncated(&mut v, was_cut);
            (v, Some(n as u64))
        }
        Request::NotesSearch { limit, .. } => {
            let (n, was_cut) = cap_list(&mut v, "notes", (*limit).min(request::NOTES_MAX));
            if let Some(items) = v.get_mut("notes").and_then(Value::as_array_mut) {
                for note in items {
                    cap_str(note, "title", request::TITLE_MAX);
                    cap_str(note, "snippet", 200);
                }
            }
            mark_truncated(&mut v, was_cut);
            (v, Some(n as u64))
        }
        Request::NotesRead { .. } => {
            cap_str(&mut v, "title", request::TITLE_MAX);
            let was_cut = cap_str(&mut v, "text", request::NOTE_TEXT_MAX);
            mark_truncated(&mut v, was_cut);
            (v, Some(1))
        }
        Request::ShortcutsList => {
            let (n, was_cut) = cap_list(&mut v, "shortcuts", request::SHORTCUTS_MAX);
            mark_truncated(&mut v, was_cut);
            (v, Some(n as u64))
        }
        Request::ShortcutsRun { .. } => {
            let was_cut = cap_str(&mut v, "output", request::SHORTCUT_OUTPUT_MAX);
            mark_truncated(&mut v, was_cut);
            (v, None)
        }
        Request::ClipboardRead => {
            let was_cut = cap_str(&mut v, "text", request::CLIPBOARD_MAX);
            mark_truncated(&mut v, was_cut);
            (v, None)
        }
        _ => (v, None),
    }
}

#[cfg(test)]
mod tests;
