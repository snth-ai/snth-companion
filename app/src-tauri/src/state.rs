//! The shell's shared context: the one `Core`, the approval queue, the
//! shell's own preferences, and the small bits of state the UI asks for.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use connect_core::{Core, InstallState, LinkState, Preset, Settings, Snapshot};
use serde::Serialize;
use tauri::menu::MenuItem;
use tauri::{AppHandle, Wry};
use tokio::sync::{watch, Notify};

use crate::approvals::Approvals;
use crate::updates::{UpdateStatus, Updates};
use crate::i18n::Lang;
use crate::prefs::{Prefs, PrefsFile};

/// The hub that pairs a code with a synth. Fixed at build time
/// (`SNTH_HUB_URL=... npm run tauri build`), with this default.
pub const HUB_URL: &str = match option_env!("SNTH_HUB_URL") {
    Some(u) => u,
    None => "https://hub.snth.ai",
};

/// The updater key placeholder in tauri.conf.json. While it is there the
/// updater plugin is not loaded and "check for updates" says so.
pub const UPDATER_KEY_PLACEHOLDER: &str = "REPLACE_WITH_TAURI_UPDATER_PUBLIC_KEY";

pub struct TrayItems {
    pub status: MenuItem<Wry>,
    pub toggle: MenuItem<Wry>,
    pub open: MenuItem<Wry>,
    pub quit: MenuItem<Wry>,
}

pub struct Ctx {
    pub app: AppHandle,
    pub core: Core,
    pub approvals: Arc<Approvals>,
    pub updates: Updates,
    pub prefs: Mutex<Prefs>,
    pub prefs_file: PrefsFile,
    /// Folders the owner picked in the native dialog during this run, plus
    /// the suggested ones. The webview can only add a folder from here.
    pub offered_roots: Mutex<HashSet<PathBuf>>,
    /// The last link detail the core reported (why it is reconnecting).
    pub link_detail: Mutex<(Option<LinkState>, String)>,
    /// A code from a `snth://pair` link, waiting for the UI to collect it.
    pub pair_code: Mutex<Option<String>>,
    /// Wakes the helper supervisor: the owner clicked "try again now".
    pub helper_retry: Notify,
    /// When the next automatic helper install attempt is due (unix
    /// seconds), while one is waiting.
    pub helper_retry_at: Mutex<Option<i64>>,
    /// Wakes the link supervisor after a new pairing.
    pub relink: Notify,
    pub shutdown: watch::Sender<bool>,
    pub tray: Mutex<Option<TrayItems>>,
    pub device_name: String,
    pub secret_store: &'static str,
    pub cua_bin: String,
    pub updater_ready: bool,
    /// The stop shortcut is registered with the OS right now.
    pub shortcut_active: Mutex<bool>,
    /// The app's data folder (the journal, the helper, the browser link).
    pub data_dir: PathBuf,
    /// Why the browser link is not being served, when it is not.
    pub browser_link: Mutex<Option<String>>,
    /// What the synth's vault holds, as last asked (Logins page).
    pub vault: crate::logins::VaultCache,
}

impl Ctx {
    /// The language for the tray and notifications: the owner's choice,
    /// else what the UI last resolved from the OS, else English.
    pub fn lang(&self) -> Lang {
        let p = self.prefs.lock().unwrap();
        p.language
            .as_deref()
            .or(p.last_language.as_deref())
            .map(Lang::parse)
            .unwrap_or(Lang::En)
    }

    /// Change the preferences and write them.
    pub fn update_prefs(&self, f: impl FnOnce(&mut Prefs)) {
        let mut p = self.prefs.lock().unwrap();
        f(&mut p);
        self.prefs_file.save(&p);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GrantDto {
    pub capability: String,
    pub scope: String,
    /// 0 = always, -1 = until the app quits, else the unix second it ends.
    pub until: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingDto {
    pub synth_id: String,
    pub synth_url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrefsDto {
    pub onboarded: bool,
    pub language: Option<String>,
    pub autostart: bool,
    pub stop_shortcut: String,
    pub stop_shortcut_active: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppInfo {
    pub version: String,
    pub device_name: String,
    pub os: &'static str,
    pub arch: &'static str,
    pub debug_build: bool,
    pub hub_url: &'static str,
    pub updater_ready: bool,
    /// Can the shell tell a metered connection on this system? Not yet:
    /// nothing calls `Core::set_metered`, so the UI shows the "skip on
    /// metered connections" switch as unavailable instead of pretending.
    pub metered_detection: bool,
    pub secret_store: &'static str,
    pub cua_pinned_version: &'static str,
    pub cua_path: String,
}

/// Everything the UI renders from, in one object.
#[derive(Debug, Clone, Serialize)]
pub struct UiState {
    pub core: Snapshot,
    /// Why the link is not up, in the core's words (diagnostics only).
    pub link_detail: String,
    /// The synth refused our key: the owner has to unlink and pair again.
    pub needs_repair: bool,
    /// A claimed code waiting for the owner's confirmation.
    pub pending: Option<PendingDto>,
    /// The helper for computer control: checking, downloading, ready,
    /// failed. The app installs it by itself.
    pub helper: InstallState,
    /// After a failed install: when the app tries again by itself.
    pub helper_retry_at: Option<i64>,
    /// Updates of the app itself: phase, the setting, what blocks them.
    pub update: UpdateStatus,
    pub grants: Vec<GrantDto>,
    /// The owner's own apps this computer offers (no Mac app on
    /// Windows), with the owner's switches.
    pub apps: Vec<connect_core::apps::AppView>,
    /// The Logins page: the browsers and what the synth's vault holds.
    pub logins: crate::logins::LoginsState,
    pub prefs: PrefsDto,
    pub app: AppInfo,
}

pub async fn build_state(ctx: &Ctx) -> UiState {
    let core = ctx.core.snapshot().await;
    let (_, link_detail) = ctx.link_detail.lock().unwrap().clone();
    let needs_repair = core.paired
        && core.link_state != LinkState::Linked
        && link_detail.contains("re-pair needed");
    let prefs = ctx.prefs.lock().unwrap().clone();
    UiState {
        link_detail,
        needs_repair,
        pending: ctx.core.pending().map(|p| PendingDto {
            synth_id: p.synth_id,
            synth_url: p.synth_url,
        }),
        helper: ctx.core.cua_helper_state(),
        helper_retry_at: *ctx.helper_retry_at.lock().unwrap(),
        update: ctx.updates.status(prefs.auto_update),
        grants: ctx
            .core
            .grant_list()
            .into_iter()
            .map(|(capability, scope, until)| GrantDto { capability, scope, until })
            .collect(),
        apps: ctx.core.apps_views(),
        logins: crate::logins::state(ctx),
        prefs: PrefsDto {
            onboarded: prefs.onboarded,
            language: prefs.language,
            autostart: prefs.autostart,
            stop_shortcut: prefs.stop_shortcut,
            stop_shortcut_active: *ctx.shortcut_active.lock().unwrap(),
        },
        app: AppInfo {
            version: ctx.app.package_info().version.to_string(),
            device_name: ctx.device_name.clone(),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            debug_build: cfg!(debug_assertions),
            hub_url: HUB_URL,
            updater_ready: ctx.updater_ready,
            metered_detection: false,
            secret_store: ctx.secret_store,
            cua_pinned_version: connect_core::cua::PINNED_VERSION,
            cua_path: ctx.cua_bin.clone(),
        },
        core,
    }
}

/// Which preset, if any, these settings are exactly (so the UI can show
/// "Careful" again after the owner undoes a hand edit).
pub fn matching_preset(s: &Settings) -> Preset {
    for p in [Preset::Careful, Preset::Balanced, Preset::Full] {
        let mut probe = Settings::from_preset(p, s.files.roots.clone());
        probe.preset = s.preset;
        if &probe == s {
            return p;
        }
    }
    Preset::Custom
}
