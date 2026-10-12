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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerSettings {
    pub enabled: bool,
    pub mode: ComputerMode,
    pub foreground: Foreground,
    /// Idle minutes required in `away` mode and for the `idle` foreground
    /// gate.
    #[serde(default = "default_away_idle")]
    pub away_idle_minutes: u64,
    /// Apps the synth may not drive, as far as a call's arguments name
    /// them (bundle id or app name).
    #[serde(default)]
    pub app_blocklist: Vec<String>,
    /// Cua tools the synth may not call at all.
    #[serde(default)]
    pub tool_denylist: Vec<String>,
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
                tool_denylist: Vec::new(),
            },
            files: FilesSettings {
                enabled: false,
                roots: Vec::new(),
                ask_on_write: false,
            },
            apps: AppsSettings::default(),
            preset: Preset::Custom,
        }
    }
}

impl Settings {
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
                    tool_denylist: Vec::new(),
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
                    tool_denylist: Vec::new(),
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
                    tool_denylist: Vec::new(),
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

/// The confirmed pairing, minus the token (that lives in the SecretStore).
/// Kept on disk so a restart can reopen the link without pairing again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredIdentity {
    pub synth_id: String,
    pub synth_url: String,
    /// The friendly name from the last `hello_ack`, for the status line
    /// while the link is down. Empty until the first link.
    #[serde(default)]
    pub synth_name: String,
}

/// A grant that outlives the process: "always" (`until` = 0) or until a
/// unix second. Session grants are never written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredGrant {
    pub capability: String,
    pub scope: String,
    pub until: i64,
}

/// What is actually written to disk: the settings plus the monotonic rev,
/// and the small amount of state that must survive a restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Persisted {
    pub rev: u64,
    pub settings: Settings,
    /// Random, generated once, kept across restarts (connect.md device_id).
    pub device_id: String,
    /// The confirmed pairing (no token). None = not paired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<StoredIdentity>,
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
}

impl Persisted {
    pub fn fresh(device_id: String) -> Persisted {
        Persisted {
            rev: 0,
            settings: Settings::default(),
            device_id,
            identity: None,
            grants: Vec::new(),
            egress_month: String::new(),
            egress_bytes: 0,
        }
    }
}

/// Load the persisted settings from `path`. A missing file yields a fresh
/// record with a new device id (not written until the first save).
pub fn load(path: &Path, new_device_id: impl FnOnce() -> String) -> io::Result<Persisted> {
    match std::fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
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
    fn preset_keeps_roots() {
        let roots = vec![Root {
            path: PathBuf::from("/tmp/x"),
            write: true,
        }];
        let s = Settings::from_preset(Preset::Full, roots.clone());
        assert_eq!(s.files.roots, roots);
    }
}
