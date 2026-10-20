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

/// The platform the owner's account lives on (sign-in, the app's API).
/// Fixed at build time (`SNTH_PLATFORM_URL=... npm run tauri build`), with
/// this default.
pub const PLATFORM_URL: &str = match option_env!("SNTH_PLATFORM_URL") {
    Some(u) => u,
    None => "https://platform.snth.ai",
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
    /// The last state and detail each link reported (only a change is
    /// news for the UI).
    pub link_detail: Mutex<std::collections::HashMap<String, (LinkState, String)>>,
    /// A code from a `snth://pair` link, waiting for the UI to collect it.
    pub pair_code: Mutex<Option<String>>,
    /// Wakes the helper supervisor: the owner clicked "try again now".
    pub helper_retry: Notify,
    /// When the next automatic helper install attempt is due (unix
    /// seconds), while one is waiting.
    pub helper_retry_at: Mutex<Option<i64>>,
    /// Wakes the link supervisor after a new pairing: it starts a loop
    /// for every link that has none.
    pub relink: Notify,
    /// Wakes the account sync (the window came up).
    pub account_wake: Notify,
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
    /// The Safari extension that ships inside this app (macOS): whether
    /// this copy carries it, whether the owner switched it on.
    pub safari: crate::safari::Safari,
    /// The stop shortcut as registered now (only it is released when it
    /// changes: the island's keys are registered next to it).
    pub stop_shortcut: Mutex<Option<tauri_plugin_global_shortcut::Shortcut>>,
    /// The island: its window, its keys, the message on its way.
    pub island: crate::island::Island,
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
    /// The synth the answer was given to, and its name.
    pub synth_id: String,
    pub synth_name: Option<String>,
    pub capability: String,
    pub scope: String,
    /// 0 = always, -1 = until the app quits, else the unix second it ends.
    pub until: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingDto {
    pub synth_id: String,
    pub synth_url: String,
    /// The synth's name when the platform said it.
    pub synth_name: Option<String>,
    /// A code redeemed through the signed-in account: did the platform
    /// tie the synth to the account (None: the code went to the hub).
    pub linked_to_account: Option<bool>,
    /// Why not ("synth_linked_elsewhere").
    pub reason: Option<String>,
}

impl PendingDto {
    pub fn of(p: connect_core::Pending) -> PendingDto {
        PendingDto {
            synth_name: Some(p.synth_name).filter(|n| !n.is_empty()),
            synth_id: p.synth_id,
            synth_url: p.synth_url,
            linked_to_account: p.linked_to_account,
            reason: p.reason,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PrefsDto {
    pub onboarded: bool,
    pub language: Option<String>,
    pub autostart: bool,
    pub stop_shortcut: String,
    pub stop_shortcut_active: bool,
    /// Developer mode: the developer section (the command line) is shown.
    pub developer: bool,
}

/// The Island screen.
#[derive(Debug, Clone, Serialize)]
pub struct IslandDto {
    /// The island can be shown on this system (macOS, Windows).
    pub available: bool,
    #[serde(flatten)]
    pub core: connect_core::island::IslandView,
    /// This system's default keys (for "reset").
    pub defaults: connect_core::island::Hotkeys,
    /// The keys are registered with the system right now.
    pub keys_active: bool,
    /// A key the system would not give ("taken", "bad"), by key.
    pub key_problems: std::collections::HashMap<connect_core::island::HotkeyKind, String>,
    pub mic: crate::island::audio::MicPermission,
    pub stt: SttDto,
    /// Wake by name.
    pub wake: crate::island::wake::WakeDto,
}

/// On-device speech-to-text on the Island screen (the switch itself is
/// in the settings: `stt`, `stt_offer`).
#[derive(Debug, Clone, Serialize)]
pub struct SttDto {
    /// The engine is in this build (Apple silicon).
    pub available: bool,
    pub model: crate::island::stt::model::Status,
    /// The download, in bytes.
    pub size: u64,
}

/// The models' status as Settings shows it: the speech model's, with the
/// tone's emotion model counted in while the tone switch is on (its
/// download continues the speech model's progress; a tone model that is
/// missing or failed leaves the speech model's status alone, since the
/// text works without it).
fn stt_dto(ctx: &Ctx) -> SttDto {
    use crate::island::stt::model::{self, Status};
    let s = ctx.core.island();
    let stt = ctx.island.stt.models.status();
    let stt_size = model::total_size();
    let tone_on = s.stt_tone && ctx.island.tone.available();
    if !tone_on {
        return SttDto { available: crate::island::stt::available(), model: stt, size: stt_size };
    }
    let tone = ctx.island.tone.models.status();
    let tone_size = model::tone_size();
    let size = stt_size + tone_size;
    let model = match (stt, tone) {
        (Status::Downloading { done, .. }, Status::Ready) => Status::Downloading { done: done + tone_size, total: size },
        (Status::Downloading { done, .. }, _) => Status::Downloading { done, total: size },
        (Status::Ready, Status::Downloading { done, .. }) => Status::Downloading { done: stt_size + done, total: size },
        (Status::Ready, Status::Verifying) => Status::Verifying,
        (other, _) => other,
    };
    let size = if ctx.island.tone.models.ready() || matches!(model, Status::Downloading { .. }) { size } else { stt_size };
    SttDto { available: crate::island::stt::available(), model, size }
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
    /// The shell the synth's commands run in here: "pwsh", "powershell",
    /// "zsh", "bash", "sh"...
    pub os_shell: String,
}

/// Everything the UI renders from, in one object.
#[derive(Debug, Clone, Serialize)]
pub struct UiState {
    pub core: Snapshot,
    /// Why the link is not up, in the core's words (diagnostics only).
    pub link_detail: String,
    /// Every link was refused by its synth: the owner has to link again
    /// (each link says so itself in `core.links`).
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
    /// Which of the "how it looks" settings the helper follows on this
    /// computer; the UI hides the rest.
    pub look_support: connect_core::cua_look::Support,
    /// The Logins page: the browsers and what the synth's vault holds.
    pub logins: crate::logins::LoginsState,
    /// The platform account: signed in or not, the sign-in in progress,
    /// the account's synths.
    pub account: connect_core::account::AccountView,
    /// The island (the Island screen).
    pub island: IslandDto,
    pub prefs: PrefsDto,
    pub app: AppInfo,
}

pub async fn build_state(ctx: &Ctx) -> UiState {
    let core = ctx.core.snapshot().await;
    // Why the links are not up, one line per link that is not.
    let link_detail = core
        .links
        .iter()
        .filter(|l| l.state != LinkState::Linked && !l.detail.is_empty())
        .map(|l| format!("{}: {}", l.synth_id, l.detail))
        .collect::<Vec<_>>()
        .join("; ");
    let needs_repair = !core.links.is_empty() && core.links.iter().all(|l| l.state == LinkState::NeedsRepair);
    let prefs = ctx.prefs.lock().unwrap().clone();
    let names: std::collections::HashMap<String, Option<String>> =
        core.links.iter().map(|l| (l.synth_id.clone(), l.synth_name.clone())).collect();
    UiState {
        link_detail,
        needs_repair,
        pending: ctx.core.pending().map(PendingDto::of),
        helper: ctx.core.cua_helper_state(),
        helper_retry_at: *ctx.helper_retry_at.lock().unwrap(),
        update: ctx.updates.status(prefs.auto_update),
        grants: ctx
            .core
            .grant_list()
            .into_iter()
            .map(|g| GrantDto {
                synth_name: names.get(&g.synth_id).cloned().flatten(),
                synth_id: g.synth_id,
                capability: g.capability,
                scope: g.scope,
                until: g.until,
            })
            .collect(),
        apps: ctx.core.apps_views(),
        look_support: ctx.core.look_support(),
        logins: crate::logins::state(ctx),
        account: ctx.core.account_view(),
        island: IslandDto {
            available: crate::island::supported_os(),
            core: ctx.core.island_view(),
            defaults: connect_core::island::Hotkeys::default(),
            keys_active: ctx.island.keys_active(),
            key_problems: ctx.island.problems(),
            mic: crate::island::audio::permission(),
            stt: stt_dto(ctx),
            wake: crate::island::wake::dto(ctx),
        },
        prefs: PrefsDto {
            onboarded: prefs.onboarded,
            language: prefs.language,
            autostart: prefs.autostart,
            stop_shortcut: prefs.stop_shortcut,
            stop_shortcut_active: *ctx.shortcut_active.lock().unwrap(),
            developer: prefs.developer,
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
            os_shell: connect_core::shell::os_shell().label.clone(),
        },
        core,
    }
}

/// Tell the UI to read the state again (and the tray to follow).
pub fn emit_state(ctx: &Ctx) {
    crate::tray::refresh(ctx);
    crate::ui::emit(&ctx.app, crate::ui::UiEvent::State);
}

/// Which preset, if any, these settings are exactly (so the UI can show
/// "Careful" again after the owner undoes a hand edit). How computer
/// control looks is not part of a preset: a cursor colour does not make
/// the settings "custom". Neither is the owner's choice of apps, which a
/// preset keeps as it keeps the shared folders, nor the experiment.
pub fn matching_preset(s: &Settings) -> Preset {
    for p in [Preset::Careful, Preset::Balanced, Preset::Full] {
        let mut probe = Settings::from_preset(p, s.files.roots.clone());
        probe.preset = s.preset;
        probe.computer.look = s.computer.look.clone();
        probe.computer.apps = s.computer.apps.clone();
        probe.computer.experiment = s.computer.experiment;
        // The command line is off in every preset: on, the settings are
        // custom; its mode alone (while off) does not count.
        probe.shell.mode = s.shell.mode;
        if &probe == s {
            return p;
        }
    }
    Preset::Custom
}
