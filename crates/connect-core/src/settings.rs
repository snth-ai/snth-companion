//! Persisted settings and the three onboarding presets.
//!
//! The settings are the owner's policy for the five capabilities. They
//! live in the OS config dir, are written atomically, and carry a
//! `settings_rev` that grows by one on every change (the synth uses it to
//! decide when to re-read the MCP catalog, connect.md).
//!
//! Fresh install = every capability off. Onboarding applies one of the
//! presets (careful / balanced / full), after which the owner tunes each
//! setting by hand. The synth has no message that changes any of this; it
//! only receives the summary (see `wire.rs`).

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Net egress mode (connect.md: net.mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetMode {
    /// No questions: any public host may be dialed.
    All,
    /// Ask the owner on each new registrable domain.
    Ask,
    /// Only the domains on the list.
    List,
}

/// How the app decides to ask before handing the synth cookies. Only
/// `enabled` reaches the wire; the mode is the app's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CookiesMode {
    /// Ask every time, even for a domain granted before.
    Ask,
    /// Ask once per domain, remember an "always" grant.
    AskNew,
    /// No questions.
    Auto,
}

/// Computer-use mode (connect.md: computer.mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComputerMode {
    /// Ask the owner per task (per working mcp stream).
    Ask,
    /// No questions.
    Auto,
    /// Calls pass only while the owner is idle (away-idle minutes).
    Away,
}

/// Experiment (connect.md: computer.experiment): the owner's choice of the
/// new way for the synth to drive this computer. The synth decides what it
/// means; it is honoured only where the synth already has the new way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Experiment {
    /// As the synth decides.
    #[default]
    Default,
    /// Use the new way where the synth has it.
    On,
    /// Never the new way: the synth drives the computer the old way.
    Off,
}

/// Foreground-delivery gate (connect.md: computer.foreground). Applies to
/// a tools/call that asks the driver to front a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Foreground {
    /// Foreground delivery is always refused.
    Never,
    /// Allowed only while the owner is idle.
    Idle,
    /// Ask the owner each time.
    Ask,
    /// Always allowed.
    Always,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetSettings {
    pub enabled: bool,
    pub mode: NetMode,
    /// Registrable domains: the allowlist in `list` mode, and the
    /// remembered "always" answers in `ask` mode.
    #[serde(default)]
    pub domains: Vec<String>,
    /// Off by default: private, loopback and link-local targets stay
    /// refused (the TZ local-network toggle).
    #[serde(default)]
    pub allow_local_network: bool,
    /// Monthly egress cap in bytes, 0 = unlimited.
    #[serde(default)]
    pub monthly_cap_bytes: u64,
    /// Refuse egress while the connection is metered.
    #[serde(default)]
    pub skip_on_metered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CookiesSettings {
    pub enabled: bool,
    pub mode: CookiesMode,
}

/// The colour of the synth's cursor. The driver has no colour setting:
/// its own cursor (`Auto`) is coloured by the driver, one colour per task,
/// and every fixed colour is a cursor theme the app ships and installs
/// into its own driver home (see `cua_look.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorColor {
    /// The driver's own cursor: it picks a colour for each task.
    #[default]
    Auto,
    Sky,
    Violet,
    Rose,
    Green,
    Amber,
    Red,
    Graphite,
}

/// How fast the synth's cursor moves. The synth's action waits for the
/// cursor to arrive, so this is also a little of how fast it works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorSpeed {
    /// Every move takes a quarter of a second.
    Fast,
    /// The driver's own timing: the further, the longer.
    #[default]
    Normal,
    /// Every move takes a second and a half, with a pause after a click.
    Relaxed,
}

/// Whether the cursor's own animation plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReducedMotion {
    /// Follow this computer's "reduce motion" setting. The app reads it:
    /// the driver's own `auto` does not look at the system.
    #[default]
    System,
    /// Still pictures only.
    Always,
    /// Always animated.
    Never,
}

/// The choices the owner has for "hide the cursor when idle", in seconds.
/// 0 = never. The driver takes at most 60 s.
pub const IDLE_HIDE_CHOICES: [u32; 4] = [5, 15, 60, 0];

/// The choices for the largest screenshot the synth gets (long edge, in
/// pixels). 0 = no rule of the owner's: the driver and the synth decide,
/// as they did before this setting existed.
pub const SCREENSHOT_CHOICES: [u32; 4] = [0, 1024, 1568, 2048];

/// How computer control looks on the owner's screen. The app's own: no
/// part of it is told to the synth, and the synth cannot change it (the
/// mcp pipe refuses its attempts, see `cua_look::gate`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LookSettings {
    /// Show the synth's cursor at all.
    #[serde(default = "yes")]
    pub cursor: bool,
    #[serde(default)]
    pub color: CursorColor,
    #[serde(default)]
    pub speed: CursorSpeed,
    /// Seconds without activity before the cursor hides; 0 = never.
    #[serde(default = "default_idle_hide")]
    pub idle_hide_seconds: u32,
    #[serde(default)]
    pub reduced_motion: ReducedMotion,
    /// The largest screenshot the synth gets, long edge in pixels; 0 = no
    /// rule (see `SCREENSHOT_CHOICES`).
    #[serde(default)]
    pub screenshot_max: u32,
    /// The driver's experimental preview window (macOS only).
    #[serde(default)]
    pub preview_window: bool,
}

fn yes() -> bool {
    true
}

fn default_idle_hide() -> u32 {
    15
}

impl Default for LookSettings {
    /// The driver's own behaviour, unchanged.
    fn default() -> Self {
        LookSettings {
            cursor: true,
            color: CursorColor::Auto,
            speed: CursorSpeed::Normal,
            idle_hide_seconds: default_idle_hide(),
            reduced_motion: ReducedMotion::System,
            screenshot_max: 0,
            preview_window: false,
        }
    }
}

impl LookSettings {
    /// Bring values that came from outside (the webview, an edited file)
    /// to ones the app offers.
    pub fn normalize(&mut self) {
        if !IDLE_HIDE_CHOICES.contains(&self.idle_hide_seconds) {
            self.idle_hide_seconds = default_idle_hide();
        }
        if !SCREENSHOT_CHOICES.contains(&self.screenshot_max) {
            self.screenshot_max = 0;
        }
    }
}

/// Which apps the synth may use through computer control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppsPolicy {
    /// Every app on this computer.
    #[default]
    All,
    /// Every app except the ones on the `except` list. The driver has no
    /// such mode (its manifest only allows): the app's own pipe resolves
    /// every call to an app and refuses the listed ones (`cua_apps.rs`).
    Except,
    /// Only the apps on the `only` list. Enforced by the driver itself:
    /// our daemon runs in bounded mode with a capability manifest.
    Only,
}

/// One app the owner picked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppRef {
    /// What the driver knows the app by: the bundle id on macOS
    /// (`com.apple.Safari`), the path of the program on Windows and Linux.
    pub id: String,
    /// The app's name as the driver listed it, for the owner to read.
    #[serde(default)]
    pub name: String,
    /// A word the owner typed by hand before the app list existed (the old
    /// "blocked apps"): it matches any app whose name or id contains it.
    /// Only ever on the `except` list.
    #[serde(default, skip_serializing_if = "is_false")]
    pub loose: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// How many apps one list holds at most.
pub const MAX_APPS: usize = 200;

/// The owner's choice of apps for computer control. Both lists are kept,
/// so switching between the choices loses nothing.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AppScope {
    #[serde(default)]
    pub policy: AppsPolicy,
    /// For `Except`: the apps the synth may not use.
    #[serde(default)]
    pub except: Vec<AppRef>,
    /// For `Only`: the apps the synth may use.
    #[serde(default)]
    pub only: Vec<AppRef>,
}

impl AppScope {
    /// The list the current choice goes by (empty for `All`).
    pub fn active(&self) -> &[AppRef] {
        match self.policy {
            AppsPolicy::All => &[],
            AppsPolicy::Except => &self.except,
            AppsPolicy::Only => &self.only,
        }
    }

    /// Bring lists that came from outside (the webview, an edited file) to
    /// shape: no empty or oversized entries, no control characters, no
    /// entry twice, a hand-typed word only among the exceptions.
    pub fn normalize(&mut self) {
        fn clean(list: &mut Vec<AppRef>, loose_allowed: bool) {
            let mut seen = std::collections::BTreeSet::new();
            let mut out = Vec::new();
            for mut app in std::mem::take(list) {
                app.id = plain_line(&app.id, 1024);
                app.name = plain_line(&app.name, 120);
                if app.id.is_empty() || (app.loose && !loose_allowed) {
                    continue;
                }
                if app.name.is_empty() {
                    app.name = app.id.clone();
                }
                if seen.insert((app.id.to_lowercase(), app.loose)) && out.len() < MAX_APPS {
                    out.push(app);
                }
            }
            *list = out;
        }
        clean(&mut self.except, true);
        clean(&mut self.only, false);
    }
}

/// One line of plain text: no control characters, trimmed, cut to `max`
/// characters.
fn plain_line(text: &str, max: usize) -> String {
    text.chars().filter(|c| !c.is_control()).collect::<String>().trim().chars().take(max).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerSettings {
    pub enabled: bool,
    pub mode: ComputerMode,
    pub foreground: Foreground,
    /// Idle minutes required in `away` mode and for the `idle` foreground
    /// gate.
    #[serde(default = "default_away_idle")]
    pub away_idle_minutes: u64,
    /// The setting this app had before `apps`: words the owner typed, each
    /// refusing a call that NAMED such an app. Read from an old settings
    /// file and moved into `apps.except` (`Settings::migrate`); never
    /// written again.
    #[serde(default, skip_serializing)]
    pub app_blocklist: Vec<String>,
    /// Which apps the synth may use. Absent in a settings file written
    /// before it existed: every app.
    #[serde(default)]
    pub apps: AppScope,
    /// Cua tools the synth may not call at all.
    #[serde(default)]
    pub tool_denylist: Vec<String>,
    /// How it looks on the screen: the cursor, its speed, screenshots.
    /// Absent in a settings file written before it existed: the defaults.
    #[serde(default)]
    pub look: LookSettings,
    /// The owner's choice of the experimental way (sent to the synth).
    /// Absent in a settings file written before it existed: default.
    #[serde(default)]
    pub experiment: Experiment,
}

fn default_away_idle() -> u64 {
    5
}

/// One folder the owner shared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Root {
    pub path: PathBuf,
    /// false = read only, true = read and write.
    pub write: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesSettings {
    pub enabled: bool,
    pub roots: Vec<Root>,
    /// Hold a `files.write` for owner approval (the careful preset).
    #[serde(default)]
    pub ask_on_write: bool,
}

/// When the app asks before the synth CHANGES or SENDS something in one of
/// the owner's apps (connect.md: apps.mode). Reading never asks: the
/// per-app switch is the consent for reading.
///
/// Sending a message as the owner is outside this setting: it asks every
/// time in every mode, unless the owner said "always" for that one
/// recipient (see `apps::gate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppsMode {
    /// Ask before every change, every time. Nothing is remembered.
    Ask,
    /// Ask before a change; the owner may let the app remember the answer
    /// for that kind of change.
    AskNew,
    /// No questions for changes.
    Auto,
}

/// The owner's own apps on this computer (Calendar, Notes, the
/// clipboard...). Which apps exist depends on the system: see
/// `apps::Backend::offered`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppsSettings {
    pub enabled: bool,
    pub mode: AppsMode,
    /// Per-app switches the owner moved away from the default. An app
    /// that is not listed has its default: on, except Messages (off until
    /// the owner switches it on by hand). See `AppsSettings::app_on`.
    #[serde(default)]
    pub apps: std::collections::BTreeMap<String, bool>,
}

impl Default for AppsSettings {
    fn default() -> Self {
        AppsSettings {
            enabled: false,
            mode: AppsMode::Ask,
            apps: std::collections::BTreeMap::new(),
        }
    }
}

impl AppsSettings {
    /// Is this app on by default once the owner enabled apps? Everything
    /// except Messages: sending and reading messages as the owner is the
    /// one the owner has to switch on separately.
    pub fn default_on(app: &str) -> bool {
        app != "messages"
    }

    /// The owner's switch for one app (the master switch is separate).
    pub fn app_on(&self, app: &str) -> bool {
        self.apps.get(app).copied().unwrap_or_else(|| Self::default_on(app))
    }

    /// Drop entries that only repeat the default, and entries for apps
    /// this build does not know, so equal policies compare equal.
    pub fn normalize(&mut self, known: &[&str]) {
        self.apps
            .retain(|app, on| known.contains(&app.as_str()) && *on != Self::default_on(app));
    }
}

/// When the app asks before the synth runs a command (connect.md:
/// shell.mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShellMode {
    /// Every command asks, with the whole command and its folder on the
    /// screen. Only "always for this exact command" can be remembered.
    #[default]
    Ask,
    /// No questions.
    Auto,
}

/// The command line on this computer (`shell`): full access by
/// definition. A command runs as the owner, with everything the owner can
/// reach; the shared folders do not limit it. Off in every preset and on
/// a fresh install; the switch lives in the developer section, which the
/// owner turns on by hand.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ShellSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub mode: ShellMode,
}

/// Which preset the settings came from, or `custom` after a hand edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    Careful,
    Balanced,
    Full,
    Custom,
}

/// The whole owner policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub net: NetSettings,
    pub cookies: CookiesSettings,
    pub computer: ComputerSettings,
    pub files: FilesSettings,
    /// Absent in a settings file written before apps existed: off.
    #[serde(default)]
    pub apps: AppsSettings,
    /// The command line. Absent in a settings file written before it
    /// existed: off.
    #[serde(default)]
    pub shell: ShellSettings,
    pub preset: Preset,
}

impl Default for Settings {
    /// Fresh install: everything off until onboarding applies a preset.
    fn default() -> Self {
        Settings {
            net: NetSettings {
                enabled: false,
                mode: NetMode::Ask,
                domains: Vec::new(),
                allow_local_network: false,
                monthly_cap_bytes: 0,
                skip_on_metered: false,
            },
            cookies: CookiesSettings {
                enabled: false,
                mode: CookiesMode::Ask,
            },
            computer: ComputerSettings {
                enabled: false,
                mode: ComputerMode::Ask,
                foreground: Foreground::Never,
                away_idle_minutes: 5,
                app_blocklist: Vec::new(),
                apps: AppScope::default(),
                tool_denylist: Vec::new(),
                look: LookSettings::default(),
                experiment: Experiment::Default,
            },
            files: FilesSettings {
                enabled: false,
                roots: Vec::new(),
                ask_on_write: false,
            },
            apps: AppsSettings::default(),
            shell: ShellSettings::default(),
            preset: Preset::Custom,
        }
    }
}

impl Settings {
    /// Bring a settings file of an older version of the app to this one.
    /// Idempotent. True when something was moved.
    ///
    /// The old "blocked apps" (`computer.app_blocklist`, words typed by
    /// hand) become "every app except these", each word kept as the owner
    /// wrote it. Nothing the owner chose is dropped: where the old setting
    /// only refused a call that named such an app, the new one also
    /// refuses a call that reaches it by its window or process.
    pub fn migrate(&mut self) -> bool {
        let old = std::mem::take(&mut self.computer.app_blocklist);
        if old.is_empty() {
            return false;
        }
        let scope = &mut self.computer.apps;
        for word in old {
            let word = plain_line(&word, 120);
            if word.is_empty() {
                continue;
            }
            let known = scope
                .except
                .iter()
                .any(|a| a.id.eq_ignore_ascii_case(&word) || a.name.eq_ignore_ascii_case(&word));
            if !known {
                scope.except.push(AppRef { id: word.clone(), name: word, loose: true });
            }
        }
        // The old list was in force whatever else was set. Keep it so,
        // unless the owner has already chosen "only these" (stricter).
        if scope.policy == AppsPolicy::All && !scope.except.is_empty() {
            scope.policy = AppsPolicy::Except;
        }
        scope.normalize();
        true
    }

    /// Settings a preset applies. Roots are NOT touched: the owner picks
    /// folders separately (onboarding offers Desktop and Downloads).
    #[allow(clippy::field_reassign_with_default)]
    pub fn from_preset(preset: Preset, keep_roots: Vec<Root>) -> Settings {
        let mut s = Settings::default();
        s.preset = preset;
        s.files.roots = keep_roots;
        match preset {
            Preset::Careful => {
                s.net = NetSettings {
                    enabled: true,
                    mode: NetMode::Ask,
                    domains: Vec::new(),
                    allow_local_network: false,
                    monthly_cap_bytes: 0,
                    skip_on_metered: true,
                };
                s.cookies = CookiesSettings {
                    enabled: true,
                    mode: CookiesMode::Ask,
                };
                s.computer = ComputerSettings {
                    enabled: true,
                    mode: ComputerMode::Ask,
                    foreground: Foreground::Never,
                    away_idle_minutes: 5,
                    app_blocklist: Vec::new(),
                    apps: AppScope::default(),
                    tool_denylist: Vec::new(),
                    look: LookSettings::default(),
                    experiment: Experiment::Default,
                };
                s.files.enabled = true;
                s.files.ask_on_write = true;
                s.apps.enabled = true;
                s.apps.mode = AppsMode::Ask;
            }
            Preset::Balanced => {
                s.net = NetSettings {
                    enabled: true,
                    mode: NetMode::All,
                    domains: Vec::new(),
                    allow_local_network: false,
                    monthly_cap_bytes: 0,
                    skip_on_metered: false,
                };
                s.cookies = CookiesSettings {
                    enabled: true,
                    mode: CookiesMode::AskNew,
                };
                s.computer = ComputerSettings {
                    enabled: true,
                    mode: ComputerMode::Auto,
                    foreground: Foreground::Idle,
                    away_idle_minutes: 5,
                    app_blocklist: Vec::new(),
                    apps: AppScope::default(),
                    tool_denylist: Vec::new(),
                    look: LookSettings::default(),
                    experiment: Experiment::Default,
                };
                s.files.enabled = true;
                s.files.ask_on_write = false;
                s.apps.enabled = true;
                s.apps.mode = AppsMode::AskNew;
            }
            Preset::Full => {
                s.net = NetSettings {
                    enabled: true,
                    mode: NetMode::All,
                    domains: Vec::new(),
                    allow_local_network: false,
                    monthly_cap_bytes: 0,
                    skip_on_metered: false,
                };
                s.cookies = CookiesSettings {
                    enabled: true,
                    mode: CookiesMode::Auto,
                };
                s.computer = ComputerSettings {
                    enabled: true,
                    mode: ComputerMode::Auto,
                    foreground: Foreground::Always,
                    away_idle_minutes: 5,
                    app_blocklist: Vec::new(),
                    apps: AppScope::default(),
                    tool_denylist: Vec::new(),
                    look: LookSettings::default(),
                    experiment: Experiment::Default,
                };
                s.files.enabled = true;
                s.files.ask_on_write = false;
                s.apps.enabled = true;
                s.apps.mode = AppsMode::Auto;
            }
            Preset::Custom => {}
        }
        s
    }
}

/// How a link's token was obtained: with a code from the synth's bot, or
/// through the owner's platform account (`account.rs`). Only an account
/// link is dropped when the account signs out, and only an account link
/// can get a fresh token by itself after the synth refused the old one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkSource {
    #[default]
    Code,
    Account,
}

/// One confirmed pairing, minus the token (that lives in the SecretStore
/// under `pairing_token:<synth_id>`). Kept on disk so a restart can reopen
/// the link without pairing again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredIdentity {
    pub synth_id: String,
    pub synth_url: String,
    /// The friendly name from the last `hello_ack` (or the platform), for
    /// the status line while the link is down. Empty until known.
    #[serde(default)]
    pub synth_name: String,
    #[serde(default)]
    pub source: LinkSource,
    /// The hub the synth lives on, as the platform names it (account
    /// links only): needed to ask the platform for a fresh token.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hub_id: String,
}

/// A grant that outlives the process: "always" (`until` = 0) or until a
/// unix second. Session grants are never written. Each grant belongs to
/// one synth: an answer given to one synth is never an answer to another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredGrant {
    /// The synth the grant was given to. Empty in a file of an app before
    /// 0.1.13 (one synth only): the migration gives it to that synth.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub synth_id: String,
    pub capability: String,
    pub scope: String,
    pub until: i64,
}

/// The owner's platform account this computer is signed in with (the
/// token itself is in the SecretStore under `account_token`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StoredAccount {
    pub id: String,
    pub email: String,
    #[serde(default)]
    pub name: String,
    /// The owner clicked "Connect" on the consent screen on this
    /// computer: from then on a new synth of the account is connected by
    /// the sync without asking again.
    #[serde(default)]
    pub consented: bool,
    /// Synths of the account the owner disconnected on this computer: the
    /// sync does not connect them again by itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
}

/// A browser whose SNTH Vault extension has talked to this app. Kept so the
/// Logins page can show it while the browser is closed, and so only a
/// browser the owner linked here is ever asked for a site's cookies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredBrowser {
    /// A random id the extension made for its own profile.
    pub install: String,
    /// "chrome", "edge", "brave", "chromium" or "browser".
    pub browser: String,
    /// The owner linked it in this app and it holds a vault pairing of
    /// this synth.
    #[serde(default)]
    pub linked: bool,
    /// The vault's id of that pairing ("dev_..."), for unlinking.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub device_id: String,
    /// The synth whose vault the browser is linked to. Empty for a browser
    /// that is not linked (and in a file of an app before 0.1.13, where
    /// the migration gives a linked browser to the one synth there was).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub synth_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ext_version: String,
    /// Unix seconds of its last hello.
    #[serde(default)]
    pub last_seen: i64,
}

/// What is actually written to disk: the settings plus the monotonic rev,
/// and the small amount of state that must survive a restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Persisted {
    pub rev: u64,
    pub settings: Settings,
    /// Random, generated once, kept across restarts (connect.md device_id).
    pub device_id: String,
    /// The confirmed pairings (no tokens), one per synth, in the order
    /// they were made.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub identities: Vec<StoredIdentity>,
    /// The one pairing of an app before 0.1.13. Read, never written:
    /// `migrate_links` moves it into `identities`.
    #[serde(default, skip_serializing)]
    pub identity: Option<StoredIdentity>,
    /// The platform account this computer is signed in with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<StoredAccount>,
    /// "Always" and until-time grants. Not part of `settings_rev`: the
    /// synth never sees them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<StoredGrant>,
    /// Monthly egress accounting (`YYYY-MM` and the bytes used in it), so
    /// the monthly cap survives a restart.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub egress_month: String,
    #[serde(default)]
    pub egress_bytes: u64,
    /// Browsers whose extension has talked to this app (see
    /// `StoredBrowser`). Not part of `settings_rev`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub browsers: Vec<StoredBrowser>,
}

impl Persisted {
    /// Bring the pairing of an app before 0.1.13 (one synth: `identity`,
    /// grants and linked browsers without a synth) to the shape of this
    /// one (a list of pairings, everything tagged with its synth).
    /// Idempotent. True when something was moved. The token's key in the
    /// SecretStore moves separately (`Core::new`), since only the core
    /// holds the store.
    pub fn migrate_links(&mut self) -> bool {
        // The old field stays in memory (it is never written) so the core
        // can move the token of that pairing too.
        let Some(old) = self.identity.clone() else {
            return false;
        };
        let id = old.synth_id.clone();
        if !self.identities.iter().any(|i| i.synth_id == id) {
            self.identities.insert(0, old);
        }
        for g in self.grants.iter_mut().filter(|g| g.synth_id.is_empty()) {
            g.synth_id = id.clone();
        }
        for b in self.browsers.iter_mut().filter(|b| b.linked && b.synth_id.is_empty()) {
            b.synth_id = id.clone();
        }
        true
    }

    /// The stored pairing with this synth.
    pub fn identity_of(&self, synth_id: &str) -> Option<&StoredIdentity> {
        self.identities.iter().find(|i| i.synth_id == synth_id)
    }

    pub fn fresh(device_id: String) -> Persisted {
        Persisted {
            rev: 0,
            settings: Settings::default(),
            device_id,
            identities: Vec::new(),
            identity: None,
            account: None,
            grants: Vec::new(),
            egress_month: String::new(),
            egress_bytes: 0,
            browsers: Vec::new(),
        }
    }
}

/// Load the persisted settings from `path`. A missing file yields a fresh
/// record with a new device id (not written until the first save).
pub fn load(path: &Path, new_device_id: impl FnOnce() -> String) -> io::Result<Persisted> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let mut p: Persisted = serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            // A file of an older version: move what changed its place. Not
            // a change the owner made, so the rev stays; the next save
            // writes the new shape.
            p.settings.migrate();
            // One synth before 0.1.13, a list of them now.
            p.migrate_links();
            Ok(p)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Persisted::fresh(new_device_id())),
        Err(e) => Err(e),
    }
}

/// Write `p` to `path` atomically: write a sibling temp file, fsync it,
/// rename over the target. A crash leaves either the old file or the new
/// one, never a half-written one.
pub fn save(path: &Path, p: &Persisted) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let bytes = serde_json::to_vec_pretty(p).map_err(io::Error::other)?;
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    restrict(&tmp);
    std::fs::rename(&tmp, path)
}

/// 0600 on Unix; a no-op elsewhere.
fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_install_is_all_off() {
        let s = Settings::default();
        assert!(!s.net.enabled);
        assert!(!s.cookies.enabled);
        assert!(!s.computer.enabled);
        assert!(!s.files.enabled);
        assert!(!s.apps.enabled);
    }

    #[test]
    fn apps_presets_and_the_messages_default() {
        let careful = Settings::from_preset(Preset::Careful, vec![]);
        assert!(careful.apps.enabled);
        assert_eq!(careful.apps.mode, AppsMode::Ask);
        let balanced = Settings::from_preset(Preset::Balanced, vec![]);
        assert_eq!(balanced.apps.mode, AppsMode::AskNew);
        let full = Settings::from_preset(Preset::Full, vec![]);
        assert_eq!(full.apps.mode, AppsMode::Auto);
        // Every app is on after enabling, except Messages: in every
        // preset, the full one included.
        for s in [&careful, &balanced, &full] {
            assert!(s.apps.app_on("calendar"));
            assert!(s.apps.app_on("clipboard"));
            assert!(!s.apps.app_on("messages"), "Messages must be switched on by hand");
        }
    }

    #[test]
    fn apps_switches_normalize_to_the_default() {
        let mut a = AppsSettings::default();
        a.apps.insert("calendar".into(), true); // same as the default
        a.apps.insert("messages".into(), false); // same as the default
        a.apps.insert("notes".into(), false);
        a.apps.insert("teleporter".into(), true); // not an app we know
        a.normalize(&["calendar", "messages", "notes"]);
        assert_eq!(a.apps.len(), 1);
        assert!(!a.app_on("notes"));
        assert!(a.app_on("calendar"));
    }

    #[test]
    fn a_settings_file_written_before_apps_still_loads() {
        // settings.json of app 0.1.1 has no "apps" key at all.
        let mut old = serde_json::to_value(Settings::from_preset(Preset::Balanced, vec![])).unwrap();
        old.as_object_mut().unwrap().remove("apps");
        let s: Settings = serde_json::from_value(old).unwrap();
        assert!(!s.apps.enabled, "apps stay off until the owner turns them on");
        assert!(s.net.enabled);
    }

    #[test]
    fn the_command_line_is_off_in_every_preset_and_in_an_old_file() {
        for p in [Preset::Careful, Preset::Balanced, Preset::Full, Preset::Custom] {
            let s = Settings::from_preset(p, vec![]);
            assert!(!s.shell.enabled, "{p:?} turns the command line on");
            assert_eq!(s.shell.mode, ShellMode::Ask);
        }
        assert!(!Settings::default().shell.enabled);
        // settings.json of app 0.1.11 has no "shell" key at all.
        let mut old = serde_json::to_value(Settings::from_preset(Preset::Full, vec![])).unwrap();
        old.as_object_mut().unwrap().remove("shell");
        let s: Settings = serde_json::from_value(old).unwrap();
        assert!(!s.shell.enabled);
        assert_eq!(s.shell.mode, ShellMode::Ask);
        // Stored in plain words.
        assert_eq!(serde_json::to_value(ShellMode::Auto).unwrap(), serde_json::json!("auto"));
        let partial: ShellSettings = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert_eq!(partial.mode, ShellMode::Ask);
    }

    #[test]
    fn presets_match_the_tz_table() {
        let careful = Settings::from_preset(Preset::Careful, vec![]);
        assert_eq!(careful.net.mode, NetMode::Ask);
        assert_eq!(careful.cookies.mode, CookiesMode::Ask);
        assert_eq!(careful.computer.mode, ComputerMode::Ask);
        assert_eq!(careful.computer.foreground, Foreground::Never);
        assert!(careful.files.ask_on_write);

        let balanced = Settings::from_preset(Preset::Balanced, vec![]);
        assert_eq!(balanced.net.mode, NetMode::All);
        assert_eq!(balanced.cookies.mode, CookiesMode::AskNew);
        assert_eq!(balanced.computer.mode, ComputerMode::Auto);
        assert_eq!(balanced.computer.foreground, Foreground::Idle);
        assert!(!balanced.files.ask_on_write);

        let full = Settings::from_preset(Preset::Full, vec![]);
        assert_eq!(full.cookies.mode, CookiesMode::Auto);
        assert_eq!(full.computer.foreground, Foreground::Always);
    }

    #[test]
    fn roundtrip_atomic_save_load() {
        let dir = std::env::temp_dir().join(format!("connect-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");
        let mut p = Persisted::fresh("dev-abc".into());
        p.rev = 7;
        p.settings = Settings::from_preset(Preset::Balanced, vec![]);
        save(&path, &p).unwrap();
        let back = load(&path, || "should-not-be-called".into()).unwrap();
        assert_eq!(back.rev, 7);
        assert_eq!(back.device_id, "dev-abc");
        assert_eq!(back.settings, p.settings);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_written_before_the_extra_state_still_loads() {
        // settings.json from the first P1 build has only rev / settings /
        // device_id; the newer fields must default.
        let old = serde_json::json!({
            "rev": 3,
            "settings": Settings::default(),
            "device_id": "dev-old",
        });
        let p: Persisted = serde_json::from_value(old).unwrap();
        assert_eq!(p.rev, 3);
        assert!(p.identity.is_none());
        assert!(p.grants.is_empty());
        assert_eq!(p.egress_bytes, 0);
    }

    #[test]
    fn every_preset_has_the_same_look_and_it_is_the_drivers_own() {
        let look = LookSettings::default();
        assert!(look.cursor);
        assert_eq!(look.color, CursorColor::Auto);
        assert_eq!(look.speed, CursorSpeed::Normal);
        assert_eq!(look.idle_hide_seconds, 15);
        assert_eq!(look.reduced_motion, ReducedMotion::System);
        assert_eq!(look.screenshot_max, 0);
        assert!(!look.preview_window);
        for p in [Preset::Careful, Preset::Balanced, Preset::Full, Preset::Custom] {
            assert_eq!(Settings::from_preset(p, vec![]).computer.look, look);
        }
    }

    #[test]
    fn a_settings_file_written_before_the_experiment_still_loads() {
        // settings.json of app 0.1.10 has no "experiment" under "computer".
        let mut old = serde_json::to_value(Settings::from_preset(Preset::Balanced, vec![])).unwrap();
        old["computer"].as_object_mut().unwrap().remove("experiment");
        let s: Settings = serde_json::from_value(old).unwrap();
        assert_eq!(s.computer.experiment, Experiment::Default);
        assert_eq!(s.computer.mode, ComputerMode::Auto);
        assert_eq!(Settings::default().computer.experiment, Experiment::Default);
        // Stored in plain words.
        for (e, word) in [(Experiment::Default, "default"), (Experiment::On, "on"), (Experiment::Off, "off")] {
            assert_eq!(serde_json::to_value(e).unwrap(), serde_json::json!(word));
            assert_eq!(serde_json::from_value::<Experiment>(serde_json::json!(word)).unwrap(), e);
        }
    }

    #[test]
    fn a_settings_file_written_before_the_look_still_loads() {
        // settings.json of app 0.1.4 has no "look" under "computer".
        let mut old = serde_json::to_value(Settings::from_preset(Preset::Balanced, vec![])).unwrap();
        old["computer"].as_object_mut().unwrap().remove("look");
        let s: Settings = serde_json::from_value(old).unwrap();
        assert_eq!(s.computer.look, LookSettings::default());
        assert_eq!(s.computer.mode, ComputerMode::Auto);
        // A look with one key only: the rest are the defaults.
        let partial: LookSettings = serde_json::from_str(r#"{"color":"rose"}"#).unwrap();
        assert_eq!(partial.color, CursorColor::Rose);
        assert!(partial.cursor);
        assert_eq!(partial.idle_hide_seconds, 15);
    }

    #[test]
    fn the_look_is_stored_in_plain_words() {
        let look = LookSettings {
            cursor: false,
            color: CursorColor::Graphite,
            speed: CursorSpeed::Relaxed,
            idle_hide_seconds: 0,
            reduced_motion: ReducedMotion::Always,
            screenshot_max: 1024,
            preview_window: true,
        };
        let v = serde_json::to_value(&look).unwrap();
        assert_eq!(v["color"], "graphite");
        assert_eq!(v["speed"], "relaxed");
        assert_eq!(v["reduced_motion"], "always");
        let back: LookSettings = serde_json::from_value(v).unwrap();
        assert_eq!(back, look);
    }

    #[test]
    fn a_look_from_outside_is_brought_to_the_offered_values() {
        let mut look = LookSettings { idle_hide_seconds: 7, screenshot_max: 99_999, ..LookSettings::default() };
        look.normalize();
        assert_eq!(look.idle_hide_seconds, 15);
        assert_eq!(look.screenshot_max, 0);
        let mut kept = LookSettings { idle_hide_seconds: 0, screenshot_max: 2048, ..LookSettings::default() };
        kept.normalize();
        assert_eq!(kept.idle_hide_seconds, 0);
        assert_eq!(kept.screenshot_max, 2048);
    }

    #[test]
    fn every_preset_lets_the_synth_use_every_app() {
        for p in [Preset::Careful, Preset::Balanced, Preset::Full, Preset::Custom] {
            let s = Settings::from_preset(p, vec![]);
            assert_eq!(s.computer.apps, AppScope::default());
            assert_eq!(s.computer.apps.policy, AppsPolicy::All);
            assert!(s.computer.apps.active().is_empty());
        }
    }

    #[test]
    fn the_old_blocked_apps_become_every_app_except_these() {
        // settings.json of app 0.1.5: "app_blocklist" under "computer",
        // no "apps" there.
        let mut old = serde_json::to_value(Settings::from_preset(Preset::Balanced, vec![])).unwrap();
        let computer = old["computer"].as_object_mut().unwrap();
        computer.remove("apps");
        computer.insert("app_blocklist".into(), serde_json::json!(["Mail", "com.agilebits.onepassword7", "  ", "mail"]));
        let mut s: Settings = serde_json::from_value(old).unwrap();
        assert_eq!(s.computer.apps, AppScope::default(), "nothing is chosen until the migration ran");
        assert!(s.migrate());
        let scope = &s.computer.apps;
        assert_eq!(scope.policy, AppsPolicy::Except);
        assert_eq!(
            scope.except,
            vec![
                AppRef { id: "Mail".into(), name: "Mail".into(), loose: true },
                AppRef { id: "com.agilebits.onepassword7".into(), name: "com.agilebits.onepassword7".into(), loose: true },
            ]
        );
        assert!(scope.only.is_empty());
        assert!(s.computer.app_blocklist.is_empty());
        // Once is enough, and the old key is never written again.
        assert!(!s.migrate());
        let written = serde_json::to_value(&s).unwrap();
        assert!(written["computer"].get("app_blocklist").is_none());
        assert_eq!(written["computer"]["apps"]["policy"], "except");
        assert_eq!(written["computer"]["apps"]["except"][0], serde_json::json!({"id": "Mail", "name": "Mail", "loose": true}));
        let back: Settings = serde_json::from_value(written).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn the_migration_runs_when_the_file_is_loaded() {
        let dir = std::env::temp_dir().join(format!("connect-settings-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let mut p = serde_json::to_value(Persisted::fresh("dev-old".into())).unwrap();
        p["rev"] = serde_json::json!(4);
        let computer = p["settings"]["computer"].as_object_mut().unwrap();
        computer.remove("apps");
        computer.insert("app_blocklist".into(), serde_json::json!(["Keychain Access"]));
        std::fs::write(&path, serde_json::to_vec(&p).unwrap()).unwrap();
        let back = load(&path, || "no".into()).unwrap();
        assert_eq!(back.rev, 4, "a migration is not a change the owner made");
        assert_eq!(back.settings.computer.apps.policy, AppsPolicy::Except);
        assert_eq!(back.settings.computer.apps.except[0].id, "Keychain Access");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_owner_who_already_chose_only_these_keeps_that_choice() {
        let mut s = Settings::from_preset(Preset::Full, vec![]);
        s.computer.apps.policy = AppsPolicy::Only;
        s.computer.apps.only = vec![AppRef { id: "com.apple.Safari".into(), name: "Safari".into(), loose: false }];
        s.computer.app_blocklist = vec!["Mail".into()];
        assert!(s.migrate());
        assert_eq!(s.computer.apps.policy, AppsPolicy::Only);
        assert_eq!(s.computer.apps.except.len(), 1, "the words are kept for when the owner switches");
    }

    #[test]
    fn app_lists_from_outside_are_brought_to_shape() {
        let app = |id: &str, name: &str, loose: bool| AppRef { id: id.into(), name: name.into(), loose };
        let mut scope = AppScope {
            policy: AppsPolicy::Only,
            except: vec![app("mail", "", true), app("com.apple.mail", "Mail", false), app("COM.APPLE.MAIL", "Mail again", false), app("", "nameless", false)],
            only: vec![app("com.apple.Safari", "Safa\nri\u{7}", false), app("typed", "typed", true), app(" com.apple.Notes ", "", false)],
        };
        scope.normalize();
        assert_eq!(scope.except, vec![app("mail", "mail", true), app("com.apple.mail", "Mail", false)]);
        // A hand-typed word cannot be on the "only these" list: the driver
        // needs the app's exact identity there.
        assert_eq!(scope.only, vec![app("com.apple.Safari", "Safari", false), app("com.apple.Notes", "com.apple.Notes", false)]);
        assert_eq!(scope.active().len(), 2);
        let mut many = AppScope { policy: AppsPolicy::Except, except: (0..500).map(|i| app(&format!("app{i}"), "x", false)).collect(), only: vec![] };
        many.normalize();
        assert_eq!(many.except.len(), MAX_APPS);
    }

    #[test]
    fn a_settings_file_written_before_the_app_choice_still_loads() {
        let mut old = serde_json::to_value(Settings::from_preset(Preset::Balanced, vec![])).unwrap();
        old["computer"].as_object_mut().unwrap().remove("apps");
        let mut s: Settings = serde_json::from_value(old).unwrap();
        assert!(!s.migrate());
        assert_eq!(s.computer.apps, AppScope::default());
    }

    #[test]
    fn preset_keeps_roots() {
        let roots = vec![Root {
            path: PathBuf::from("/tmp/x"),
            write: true,
        }];
        let s = Settings::from_preset(Preset::Full, roots.clone());
        assert_eq!(s.files.roots, roots);
    }
}
