//! The commands the web UI may call. Each one is also listed in
//! `build.rs` and allowed per window in `capabilities/*.json`.
//!
//! Errors are short codes (`code_not_found`, `already_linked`, ...) the UI
//! turns into sentences; anything after a colon is detail for diagnostics.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use connect_core::pairing::normalize_code;
use connect_core::{Approval, Core, Duration_, Entry, InstallState, Preset, Settings};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::DialogExt;

use crate::i18n::Lang;
use crate::state::{build_state, matching_preset, Ctx, PendingDto, UiState, HUB_URL};
use crate::ui::{self, UiEvent};
use crate::{actions, logins, tray, updates, windows};

type Cx<'a> = State<'a, Arc<Ctx>>;

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// --- state ---

#[tauri::command]
pub async fn get_state(ctx: Cx<'_>) -> Result<UiState, String> {
    Ok(build_state(&ctx).await)
}

/// The recent activity, oldest first.
#[tauri::command]
pub fn get_journal(ctx: Cx<'_>) -> Vec<Entry> {
    ctx.core.recent_journal()
}

#[derive(Serialize)]
pub struct Ready {
    /// A code from a `snth://pair` link that has not been shown yet.
    pair_code: Option<String>,
}

/// A window finished loading. `language` is what the UI resolved to (the
/// owner's choice or the OS locale); the tray follows it.
#[tauri::command]
pub fn ui_ready(ctx: Cx<'_>, language: String) -> Ready {
    let lang = Lang::parse(&language).code().to_string();
    ctx.update_prefs(|p| p.last_language = Some(lang));
    tray::refresh(&ctx);
    Ready { pair_code: ctx.pair_code.lock().unwrap().take() }
}

// --- pairing ---

fn hub_for(override_url: Option<String>) -> Result<String, String> {
    let Some(url) = override_url.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()) else {
        return Ok(HUB_URL.to_string());
    };
    let local = url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost");
    if url.starts_with("https://") || (cfg!(debug_assertions) && local) {
        Ok(url)
    } else {
        Err("bad_hub_url".into())
    }
}

/// Ask the hub who a 6-digit code belongs to. Nothing is stored and no
/// link is opened: the answer is held in memory until `pair_confirm`.
#[tauri::command]
pub async fn pair_begin(
    ctx: Cx<'_>,
    code: String,
    hub_url: Option<String>,
) -> Result<PendingDto, String> {
    if ctx.core.is_paired() {
        return Err("already_linked".into());
    }
    let code = normalize_code(&code).ok_or("bad_code")?;
    let hub = hub_for(hub_url)?;
    match ctx.core.pair_begin(hub, code).await {
        Ok(p) => Ok(PendingDto { synth_id: p.synth_id, synth_url: p.synth_url }),
        Err(e) => {
            let code = if e.contains("not found") {
                "code_not_found"
            } else if e.contains("expired") {
                "code_expired"
            } else if e.contains("six digits") {
                "bad_code"
            } else if e.contains("could not reach hub") {
                "hub_unreachable"
            } else {
                "hub_error"
            };
            Err(format!("{code}: {e}"))
        }
    }
}

/// The owner saw the synth's identity and clicked Connect. Only now is the
/// token stored and the link allowed. Every capability starts off, and the
/// core is held paused until onboarding is finished (`finish_onboarding`),
/// so nothing the owner picks halfway through takes effect early.
#[tauri::command]
pub async fn pair_confirm(ctx: Cx<'_>) -> Result<UiState, String> {
    ctx.core.pair_confirm()?;
    ctx.core.mutate_settings(|s| *s = Settings::default());
    ctx.update_prefs(|p| {
        p.onboarded = false;
        p.paused = false;
    });
    ctx.core.pause().await;
    ctx.relink.notify_one();
    tray::refresh(&ctx);
    Ok(build_state(&ctx).await)
}

#[tauri::command]
pub fn pair_cancel(ctx: Cx<'_>) {
    ctx.core.pair_cancel();
}

/// Unlink this computer: tell the synth, erase the token and every
/// remembered permission, turn every capability off. The activity journal
/// stays on this computer.
#[tauri::command]
pub async fn unpair(ctx: Cx<'_>) -> Result<UiState, String> {
    for id in ctx.approvals.deny_all() {
        ui::emit(&ctx.app, UiEvent::ApprovalClosed { id });
    }
    windows::hide_approval(&ctx.app);
    ctx.core.unpair().await;
    ctx.core.mutate_settings(|s| *s = Settings::default());
    if ctx.core.link_brief().paused {
        ctx.core.resume();
    }
    ctx.update_prefs(|p| {
        p.onboarded = false;
        p.paused = false;
    });
    *ctx.link_detail.lock().unwrap() = (None, String::new());
    tray::refresh(&ctx);
    Ok(build_state(&ctx).await)
}

/// Onboarding is done: what the owner chose starts to apply now.
/// `autostart` is their answer on the last step.
#[tauri::command]
pub async fn finish_onboarding(ctx: Cx<'_>, autostart: bool) -> Result<UiState, String> {
    if !ctx.core.is_paired() {
        return Err("not_linked".into());
    }
    ctx.update_prefs(|p| {
        p.onboarded = true;
        p.paused = false;
    });
    ctx.core.resume();
    apply_autostart(&ctx, autostart);
    tray::refresh(&ctx);
    Ok(build_state(&ctx).await)
}

// --- settings ---

#[tauri::command]
pub async fn apply_preset(ctx: Cx<'_>, preset: String) -> Result<UiState, String> {
    let preset = match preset.as_str() {
        "careful" => Preset::Careful,
        "balanced" => Preset::Balanced,
        "full" => Preset::Full,
        _ => return Err("bad_preset".into()),
    };
    ctx.core.apply_preset(preset);
    Ok(build_state(&ctx).await)
}

/// A site as the owner typed it -> the registrable domain the core matches
/// on ("https://Mail.Google.com/inbox" -> "google.com"). None when nothing
/// usable is left.
fn clean_domain(raw: &str) -> Option<String> {
    let mut d = raw.trim().to_ascii_lowercase();
    if let Some((_, rest)) = d.split_once("://") {
        d = rest.to_string();
    }
    let d = d.split(['/', '?', '#', ':']).next().unwrap_or("");
    let d = d.trim_start_matches("*.").trim_matches('.');
    let ok = !d.is_empty()
        && d.len() <= 253
        && d.contains('.')
        && d.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || !c.is_ascii());
    ok.then(|| connect_core::core::registrable_domain(d))
}

fn clean_list(items: Vec<String>, max: usize, f: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        if let Some(v) = f(&item) {
            if !out.contains(&v) && out.len() < max {
                out.push(v);
            }
        }
    }
    out
}

/// Replace the settings with what the UI edited. Shared folders are NOT
/// taken from the webview: they change only through the folder commands
/// below, which accept nothing but a folder the owner picked.
#[tauri::command]
pub async fn update_settings(ctx: Cx<'_>, settings: Settings) -> Result<UiState, String> {
    let mut next = settings;
    next.net.domains = clean_list(next.net.domains, 500, clean_domain);
    // The owner's choice of apps: bounded lists of plain entries. The old
    // blocked-apps words can only come from an old settings file.
    next.computer.app_blocklist.clear();
    next.computer.apps.normalize();
    next.computer.away_idle_minutes = next.computer.away_idle_minutes.clamp(1, 240);
    // Only the choices the app offers for how computer control looks.
    next.computer.look.normalize();
    // Only apps this build knows, and only the switches that differ from
    // the default, so equal choices compare equal to a preset.
    next.apps.normalize(connect_core::apps::APPS);
    ctx.core.mutate_settings(move |s| {
        next.files.roots = s.files.roots.clone();
        next.computer.tool_denylist = s.computer.tool_denylist.clone();
        next.preset = s.preset;
        next.preset = matching_preset(&next);
        *s = next;
    });
    Ok(build_state(&ctx).await)
}

#[derive(Serialize)]
pub struct RootDto {
    path: String,
    /// "desktop" / "downloads", so the UI can name it in its own language.
    kind: &'static str,
}

/// The folders onboarding offers: Desktop and Downloads, when they exist.
#[tauri::command]
pub fn suggested_roots(ctx: Cx<'_>) -> Vec<RootDto> {
    let roots = Core::default_roots();
    let mut offered = ctx.offered_roots.lock().unwrap();
    roots
        .into_iter()
        .map(|r| {
            offered.insert(r.path.clone());
            let kind = if r.path.file_name().and_then(|n| n.to_str()) == Some("Downloads") {
                "downloads"
            } else {
                "desktop"
            };
            RootDto { path: r.path.to_string_lossy().to_string(), kind }
        })
        .collect()
}

/// Share one of the suggested folders, or one picked earlier in this run.
#[tauri::command]
pub async fn add_suggested_root(ctx: Cx<'_>, path: String, write: bool) -> Result<UiState, String> {
    let path = PathBuf::from(path);
    if !ctx.offered_roots.lock().unwrap().contains(&path) {
        return Err("folder_not_offered".into());
    }
    add_root_keeping_switch(&ctx.core, path, write);
    Ok(build_state(&ctx).await)
}

/// `Core::add_root` also turns file access on. Adding a folder while the
/// owner has file access switched off must not switch it back on.
fn add_root_keeping_switch(core: &Core, path: PathBuf, write: bool) {
    let was_enabled = core.settings().files.enabled;
    core.add_root(path, write);
    if !was_enabled {
        core.mutate_settings(|s| s.files.enabled = false);
    }
}

/// Open the native folder picker and share what the owner picks. The
/// webview never supplies the path. Ok(None) = the owner cancelled.
#[tauri::command]
pub async fn pick_folder(
    app: AppHandle,
    ctx: Cx<'_>,
    write: bool,
) -> Result<Option<UiState>, String> {
    let picked = tauri::async_runtime::spawn_blocking(move || app.dialog().file().blocking_pick_folder())
        .await
        .map_err(|e| format!("dialog: {e}"))?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|e| format!("dialog: {e}"))?;
    if !path.is_dir() {
        return Err("not_a_folder".into());
    }
    ctx.offered_roots.lock().unwrap().insert(path.clone());
    add_root_keeping_switch(&ctx.core, path, write);
    Ok(Some(build_state(&ctx).await))
}

#[tauri::command]
pub async fn remove_root(ctx: Cx<'_>, path: String) -> Result<UiState, String> {
    ctx.core.remove_root(Path::new(&path));
    Ok(build_state(&ctx).await)
}

/// Switch a shared folder between "read only" and "read and write".
#[tauri::command]
pub async fn set_root_write(ctx: Cx<'_>, path: String, write: bool) -> Result<UiState, String> {
    let path = PathBuf::from(path);
    ctx.core.mutate_settings(|s| {
        if let Some(r) = s.files.roots.iter_mut().find(|r| r.path == path) {
            r.write = write;
        }
    });
    Ok(build_state(&ctx).await)
}

#[tauri::command]
pub async fn revoke_grant(ctx: Cx<'_>, capability: String, scope: String) -> Result<UiState, String> {
    ctx.core.revoke(&capability, &scope);
    Ok(build_state(&ctx).await)
}

// --- pause ---

#[tauri::command]
pub async fn pause(ctx: Cx<'_>) -> Result<UiState, String> {
    actions::pause(&ctx).await;
    Ok(build_state(&ctx).await)
}

#[tauri::command]
pub async fn resume(ctx: Cx<'_>) -> Result<UiState, String> {
    actions::resume(&ctx);
    Ok(build_state(&ctx).await)
}

// --- approvals ---

#[tauri::command]
pub fn approvals_pending(ctx: Cx<'_>) -> Vec<crate::approvals::ApprovalDto> {
    ctx.approvals.list()
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerDuration {
    Once,
    Session,
    Hours,
    Always,
}

/// The owner's answer to one request. `hours` is used with `duration:
/// "hours"`. False = the request was no longer waiting.
#[tauri::command]
pub fn approval_answer(
    ctx: Cx<'_>,
    id: u64,
    allow: bool,
    duration: AnswerDuration,
    hours: Option<u32>,
) -> bool {
    let approval = if !allow {
        Approval::deny()
    } else {
        let duration = match duration {
            AnswerDuration::Once => Duration_::Once,
            AnswerDuration::Session => Duration_::Session,
            AnswerDuration::Always => Duration_::Always,
            AnswerDuration::Hours => {
                let h = i64::from(hours.unwrap_or(1).clamp(1, 24 * 30));
                Duration_::Until(now_unix() + h * 3600)
            }
        };
        Approval { allow: true, duration }
    };
    let answered = ctx.approvals.answer(id, approval);
    ui::emit(&ctx.app, UiEvent::ApprovalClosed { id });
    if ctx.approvals.is_empty() {
        windows::hide_approval(&ctx.app);
    }
    // A remembered answer shows up in the grant lists.
    ui::emit(&ctx.app, UiEvent::State);
    answered
}

// --- computer control helper (Cua Driver) ---

#[derive(Serialize)]
pub struct CuaDto {
    /// Checking, downloading, installing, verifying, ready or failed.
    helper: InstallState,
    /// After a failure: when the app tries again by itself (unix seconds).
    retry_at: Option<i64>,
    pinned_version: &'static str,
    /// macOS only (None elsewhere): what the system allows THIS app. The
    /// embedded helper works under the app's permissions.
    accessibility: Option<bool>,
    screen_recording: Option<bool>,
}

/// Where the helper stands and whether the app has the two macOS
/// permissions. Read-only and cheap: no process is started, nothing is
/// shown.
#[tauri::command]
pub fn cua_check(ctx: Cx<'_>) -> CuaDto {
    let permissions = ctx.core.computer_permissions();
    CuaDto {
        helper: ctx.core.cua_helper_state(),
        retry_at: *ctx.helper_retry_at.lock().unwrap(),
        pinned_version: connect_core::cua::PINNED_VERSION,
        accessibility: permissions.map(|p| p.accessibility),
        screen_recording: permissions.map(|p| p.screen_recording),
    }
}

/// Run a helper command without waiting for it. Only ever called with a
/// fixed argv from this file; nothing from the webview reaches it.
fn run_detached(argv: Vec<String>) -> Result<(), String> {
    let (bin, args) = argv.split_first().ok_or("empty command")?;
    let mut child = connect_core::proc::std_command(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("helper_failed: {e}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Ask macOS for the two permissions, as this app (its own dialogs come
/// up). Only ever from the owner's click.
#[tauri::command]
pub fn cua_grant_permissions(app: AppHandle, ctx: Cx<'_>) -> Result<(), String> {
    let core = ctx.core.clone();
    app.run_on_main_thread(move || core.request_computer_permissions())
        .map_err(|e| format!("helper_failed: {e}"))
}

/// "Try again now": skip the pause before the next automatic attempt to
/// install the helper. The install itself always runs in the background.
#[tauri::command]
pub fn cua_install(ctx: Cx<'_>) {
    ctx.helper_retry.notify_one();
}

#[derive(Serialize)]
pub struct ComputerAppsDto {
    /// Every app the helper lists on this computer, running ones first.
    apps: Vec<connect_core::cua_apps::AppChoice>,
    /// What a list of exceptions starts with: the password managers and
    /// the system's own security settings that this computer has.
    suggested: Vec<connect_core::AppRef>,
}

/// The owner's picker for "which apps": read now from the helper, by the
/// app itself. Takes a few seconds on a computer with many apps. Nothing
/// is started on the screen and the synth is not involved.
#[tauri::command]
pub async fn computer_apps(ctx: Cx<'_>) -> Result<ComputerAppsDto, String> {
    let apps = ctx.core.computer_apps().await.map_err(|e| {
        if e == "not_installed" {
            e
        } else {
            format!("helper_failed: {}", without_home(&e))
        }
    })?;
    let suggested = connect_core::cua_apps::default_exclusions(&apps);
    Ok(ComputerAppsDto { apps, suggested })
}

/// "Check the helper": the driver's own health report from the app's own
/// daemon. Read-only; raises no system prompt; nothing leaves the
/// computer.
#[tauri::command]
pub async fn cua_health_check(ctx: Cx<'_>) -> Result<connect_core::cua_health::Report, String> {
    Ok(ctx.core.cua_health().await)
}

// --- the owner's own apps ---

#[derive(Serialize)]
pub struct AppStatusDto {
    /// "calendar", "messages", "clipboard"...
    id: &'static str,
    /// The owner's switch for this app.
    on: bool,
    actions: Vec<&'static str>,
    /// The system permissions this app needs and where they stand. Empty
    /// = none needed.
    permissions: Vec<connect_core::apps::Permission>,
}

#[derive(Serialize)]
pub struct AppsDto {
    /// Only the apps this computer has: no Mac app on Windows.
    apps: Vec<AppStatusDto>,
}

/// What this computer offers and where each app's permission stands.
/// Read-only: nothing is shown and no app is started.
///
/// A permission is looked at only for an app the owner switched on (with
/// apps on at all). Looking is not free of traces: checking whether the
/// Messages history can be read makes macOS list SNTH Connect under Full
/// Disk Access, which should not happen for someone who left Messages off.
#[tauri::command]
pub async fn apps_check(ctx: Cx<'_>) -> Result<AppsDto, String> {
    let core = ctx.core.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let enabled = core.settings().apps.enabled;
        AppsDto {
            apps: core
                .apps_views()
                .into_iter()
                .map(|v| AppStatusDto {
                    id: v.id,
                    on: v.on,
                    permissions: if enabled && v.on { core.apps_permissions(v.id) } else { Vec::new() },
                    actions: v.actions,
                })
                .collect(),
        }
    })
    .await
    .map_err(|e| format!("apps_check: {e}"))
}

/// Make macOS ask whether SNTH Connect may control one app (its own
/// window comes up; the app in question is opened for it). Only from the
/// owner's click. `app` must be one this computer offers.
#[tauri::command]
pub async fn apps_request_permission(ctx: Cx<'_>, app: String) -> Result<(), String> {
    let core = ctx.core.clone();
    if !core.apps_views().iter().any(|v| v.id == app) {
        return Err("unknown_app".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        core.apps_request_permission(&app, connect_core::apps::PermissionKind::Automation)
    })
    .await
    .map_err(|e| format!("helper_failed: {e}"))?
    .map_err(|e| format!("helper_failed: {e}"))
}

// --- logins: the synth's vault and the browsers ---

/// Ask the synth which sites its vault holds a login for (names and
/// dates only; there is no request that returns a login).
#[tauri::command]
pub async fn logins_refresh(ctx: Cx<'_>) -> Result<UiState, String> {
    ask_safari(&ctx).await;
    if ctx.core.is_paired() {
        match ctx.core.vault_list().await {
            Ok(list) => ctx.vault.ready(list),
            Err(e) => ctx.vault.set(logins::vault_failure(&e)),
        }
    } else {
        ctx.vault.set(logins::VaultView::Unknown);
    }
    Ok(build_state(&ctx).await)
}

/// Ask Safari whether the owner switched the app's extension on. Safari
/// answers within a moment or not at all; either way the page is not held
/// for more than two seconds. Nothing is asked when this copy of the app
/// carries no Safari extension.
async fn ask_safari(ctx: &std::sync::Arc<crate::state::Ctx>) {
    if !ctx.safari.usable() {
        return;
    }
    let ctx = ctx.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || ctx.safari.refresh()).await;
}

/// The Logins page asks again where Safari stands: while the owner is on
/// their way through Safari's settings, and when the window comes back.
#[tauri::command]
pub async fn safari_check(ctx: Cx<'_>) -> Result<UiState, String> {
    ask_safari(&ctx).await;
    Ok(build_state(&ctx).await)
}

/// Remove one saved login from the synth's vault.
#[tauri::command]
pub async fn vault_forget(ctx: Cx<'_>, site: String) -> Result<UiState, String> {
    ctx.core.vault_forget(&site).await.map_err(|e| format!("vault_failed: {e}"))?;
    ctx.vault.drop_site(&site.trim().to_ascii_lowercase());
    Ok(build_state(&ctx).await)
}

/// "Link this browser": the owner's click here is the confirmation. The
/// app gets a pairing code from the synth and hands it to that browser's
/// extension, which redeems it at the hub.
#[tauri::command]
pub async fn browser_link(ctx: Cx<'_>, install: String) -> Result<UiState, String> {
    ctx.core.browser_link(&install).await.map_err(|why| why.code().to_string())?;
    Ok(build_state(&ctx).await)
}

/// Unlink one browser: its extension forgets the pairing and every site,
/// and the synth removes the pairing at the vault.
#[tauri::command]
pub async fn browser_unlink(ctx: Cx<'_>, install: String) -> Result<UiState, String> {
    // The browser is unlinked here whatever the synth answers; an error
    // only means the vault could not be told right now.
    let told = ctx.core.browser_unlink(&install).await;
    ctx.vault.set(logins::VaultView::Unknown);
    match told {
        Ok(()) => Ok(build_state(&ctx).await),
        Err(e) if e == "unknown browser" => Err("unknown_browser".into()),
        Err(_) => Ok(build_state(&ctx).await),
    }
}

/// Take a browser that is gone off the list.
#[tauri::command]
pub async fn browser_forget(ctx: Cx<'_>, install: String) -> Result<UiState, String> {
    ctx.core.browser_forget(&install);
    Ok(build_state(&ctx).await)
}

// --- general ---

/// `language`: "en" / "ru", or None to follow the OS again. `resolved` is
/// what the UI shows now.
#[tauri::command]
pub async fn set_language(
    ctx: Cx<'_>,
    language: Option<String>,
    resolved: String,
) -> Result<UiState, String> {
    let chosen = language.map(|l| Lang::parse(&l).code().to_string());
    let resolved = Lang::parse(&resolved).code().to_string();
    ctx.update_prefs(|p| {
        p.language = chosen;
        p.last_language = Some(resolved);
    });
    tray::refresh(&ctx);
    // The other window (if open) re-reads the language from the state.
    ui::emit(&ctx.app, UiEvent::State);
    Ok(build_state(&ctx).await)
}

/// Sync the OS "launch at login" entry to the owner's choice.
fn apply_autostart(ctx: &Ctx, enabled: bool) {
    let auto = ctx.app.autolaunch();
    let result = if enabled { auto.enable() } else { auto.disable() };
    // Disabling an entry that does not exist is not a failure worth
    // showing; what matters is what the OS reports afterwards.
    let _ = result;
    let actual = auto.is_enabled().unwrap_or(false);
    ctx.update_prefs(|p| p.autostart = actual);
}

#[tauri::command]
pub async fn set_autostart(ctx: Cx<'_>, enabled: bool) -> Result<UiState, String> {
    apply_autostart(&ctx, enabled);
    let state = build_state(&ctx).await;
    if state.prefs.autostart != enabled {
        return Err("autostart_failed".into());
    }
    Ok(state)
}

/// Change the "stop everything" shortcut. On failure the old one is put
/// back and the error says why.
#[tauri::command]
pub async fn set_stop_shortcut(ctx: Cx<'_>, accelerator: String) -> Result<UiState, String> {
    let previous = ctx.prefs.lock().unwrap().stop_shortcut.clone();
    match actions::set_stop_shortcut(&ctx, &accelerator) {
        Ok(()) => {
            ctx.update_prefs(|p| p.stop_shortcut = accelerator.trim().to_string());
            Ok(build_state(&ctx).await)
        }
        Err(e) => {
            let _ = actions::set_stop_shortcut(&ctx, &previous);
            Err(e)
        }
    }
}

/// "Check now": ask the release feed. The answer (and every step after
/// it) is in the returned state and in `update` events.
#[tauri::command]
pub async fn check_update(ctx: Cx<'_>) -> Result<UiState, String> {
    updates::check_by_owner(&ctx).await;
    Ok(build_state(&ctx).await)
}

/// "Install and restart": download if needed, put the new version in
/// place and restart into it now. On success the app is gone before this
/// returns; an error means the current version keeps running.
#[tauri::command]
pub async fn install_update(ctx: Cx<'_>) -> Result<UiState, String> {
    let result = updates::install_by_owner(&ctx).await;
    let state = build_state(&ctx).await;
    result.map(|()| state)
}

/// The "Update automatically" switch.
#[tauri::command]
pub async fn set_auto_update(ctx: Cx<'_>, enabled: bool) -> Result<UiState, String> {
    ctx.update_prefs(|p| p.auto_update = enabled);
    if enabled {
        // An update that was only announced so far is fetched right away.
        let ctx = ctx.inner().clone();
        tauri::async_runtime::spawn(async move { updates::fetch_if_wanted(&ctx).await });
    }
    Ok(build_state(&ctx).await)
}

/// Plain-text facts for a support request. No token, no cookie data, no
/// folder paths, no site names.
#[tauri::command]
pub async fn diagnostics(ctx: Cx<'_>) -> Result<String, String> {
    let st = build_state(&ctx).await;
    let c = &st.core;
    let s = &c.settings;
    let host = c
        .synth_url
        .as_deref()
        .map(|u| u.trim_start_matches("https://").trim_start_matches("http://"))
        .unwrap_or("-");
    let lines = [
        format!("SNTH Connect {}{}", st.app.version, if st.app.debug_build { " (debug build)" } else { "" }),
        format!("system: {} {}", st.app.os, st.app.arch),
        format!("hub: {}", st.app.hub_url),
        format!("key storage: {}", st.app.secret_store),
        format!("updates: configured {}, automatic {}, blocked {:?}, state {:?}",
            st.update.configured, st.update.auto, st.update.blocked, st.update.phase),
        String::new(),
        format!("linked: {} (state {:?}, paused {})", c.paired, c.link_state, c.paused),
        format!("link detail: {}", if st.link_detail.is_empty() { "-" } else { &st.link_detail }),
        format!("synth: {} at {} (version {})", c.synth_id.as_deref().unwrap_or("-"), host, c.synth_version.as_deref().unwrap_or("-")),
        format!("settings revision: {}, preset {:?}", c.settings_rev, s.preset),
        String::new(),
        format!("internet: {} (mode {:?}, {} listed sites, local network {}, limit {} bytes, skip metered {})",
            on(s.net.enabled), s.net.mode, s.net.domains.len(), on(s.net.allow_local_network), s.net.monthly_cap_bytes, on(s.net.skip_on_metered)),
        format!("internet used in {}: {} bytes (metered now: {})", c.egress_month, c.egress_bytes, c.metered),
        format!("logins: sign-ins {} (mode {:?}); browser link {}; browsers: {}",
            on(s.cookies.enabled), s.cookies.mode,
            if st.logins.link_ready { "served" } else { "not served" },
            // The browser and where it stands, never a site name.
            if st.logins.browsers.is_empty() { "none".to_string() } else {
                st.logins.browsers.iter().map(|b| format!("{} {}{}", b.browser, b.state, if b.connected { " (connected)" } else { "" })).collect::<Vec<_>>().join(", ")
            }),
        // The Safari extension inside this copy of the app, and its door.
        format!("safari: {}", match ctx.safari.info() {
            None => "no extension in this copy of the app".to_string(),
            Some(info) => format!("extension {:?}{}; door {}",
                info.enabled,
                if info.standalone { ", the stand-alone SNTH Vault app is installed too" } else { "" },
                match ctx.safari.door.lock().unwrap().as_deref() {
                    None if ctx.safari.usable() => "served".to_string(),
                    None => "not opened".to_string(),
                    Some(why) => format!("not served ({})", without_home(why)),
                }),
        }),
        // How many apps, never which.
        format!("computer control: {} (mode {:?}, screen {:?}, away after {} min, apps {:?}: {} excepted, {} chosen)",
            on(s.computer.enabled), s.computer.mode, s.computer.foreground, s.computer.away_idle_minutes,
            s.computer.apps.policy, s.computer.apps.except.len(), s.computer.apps.only.len()),
        format!("files: {} ({} folders, ask before writing {})", on(s.files.enabled), s.files.roots.len(), on(s.files.ask_on_write)),
        format!("apps: {} (mode {:?}, on: {})", on(s.apps.enabled), s.apps.mode,
            ctx.core.apps_views().iter().filter(|v| v.on).map(|v| v.id).collect::<Vec<_>>().join(" ")),
        format!("remembered permissions: {}", st.grants.len()),
        String::new(),
        format!("Cua Driver: {} (status {}, expected {})",
            if c.cua_version.is_empty() { "not installed" } else { &c.cua_version }, c.cua_status, st.app.cua_pinned_version),
        format!("helper install: {}", match &st.helper {
            // The reason may name a path under the home folder: keep the
            // promise that no folder paths leave with the diagnostics.
            InstallState::Failed { code, detail } => format!("failed ({code:?}): {}", without_home(detail)),
            other => format!("{other:?}"),
        }),
        format!("stop shortcut: {} (active {})",
            if st.prefs.stop_shortcut.is_empty() { "none" } else { &st.prefs.stop_shortcut }, st.prefs.stop_shortcut_active),
        format!("launch at login: {}", on(st.prefs.autostart)),
        // How many panics were written down and when the last was; the
        // file itself stays on this computer (crashlog.rs).
        format!("crash log: {}", match crate::crashlog::summary() {
            None => "no panics written down".to_string(),
            Some((count, last)) => format!("{count} panic(s), the last at {last} (panic.log in the app's log folder)"),
        }),
    ];
    Ok(lines.join("\n"))
}

/// Replace the home folder in a message with `~`.
fn without_home(text: &str) -> String {
    match directories::BaseDirs::new() {
        Some(dirs) => text.replace(&*dirs.home_dir().to_string_lossy(), "~"),
        None => text.to_string(),
    }
}

fn on(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

/// Open one of a fixed set of places outside the app. The webview names
/// the place; it never supplies a URL.
#[tauri::command]
pub fn open_external(ctx: Cx<'_>, target: String) -> Result<(), String> {
    // Safari's settings, at the extension that ships in this app. Safari
    // is asked to show it; if that cannot be done, Safari is just opened.
    if target == "safari_extension" {
        if !ctx.safari.present() {
            return Err("unknown_target".into());
        }
        if ctx.safari.show_settings() {
            return Ok(());
        }
        return run_detached(vec!["/usr/bin/open".into(), "-a".into(), "Safari".into()]);
    }
    let extension = logins::extension_url(HUB_URL);
    let url = match target.as_str() {
        "privacy_accessibility" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        "privacy_screen" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
        "privacy_automation" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation"
        }
        "privacy_full_disk" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"
        }
        "helper" => "https://cua.ai/docs/cua-driver",
        // Where the SNTH Vault extension is installed from.
        "vault_extension" => extension.as_str(),
        "releases" => "https://github.com/snth-ai/snth-companion/releases/latest",
        _ => return Err("unknown_target".into()),
    };
    #[cfg(target_os = "macos")]
    let argv: Vec<String> = vec!["/usr/bin/open".into(), url.into()];
    #[cfg(all(unix, not(target_os = "macos")))]
    let argv: Vec<String> = vec!["xdg-open".into(), url.into()];
    #[cfg(windows)]
    let argv: Vec<String> = vec!["explorer.exe".into(), url.into()];
    run_detached(argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_are_cleaned() {
        assert_eq!(clean_domain(" https://WWW.YouTube.com/watch?v=1 ").as_deref(), Some("youtube.com"));
        assert_eq!(clean_domain("*.example.org").as_deref(), Some("example.org"));
        assert_eq!(clean_domain("mail.example.org:443").as_deref(), Some("example.org"));
        assert_eq!(clean_domain("news.bbc.co.uk").as_deref(), Some("bbc.co.uk"));
        assert_eq!(clean_domain("localhost"), None);
        assert_eq!(clean_domain("bad domain.com"), None);
        assert_eq!(clean_domain(""), None);
    }

    #[test]
    fn lists_are_deduped_and_capped() {
        let got = clean_list(
            vec!["a.com".into(), "A.com".into(), "b.com".into(), "c.com".into()],
            2,
            clean_domain,
        );
        assert_eq!(got, ["a.com", "b.com"]);
    }

    #[test]
    fn a_cursor_colour_does_not_make_the_settings_custom() {
        let mut s = Settings::from_preset(Preset::Balanced, vec![]);
        s.computer.look.color = connect_core::settings::CursorColor::Rose;
        s.computer.look.screenshot_max = 1024;
        assert!(matches!(matching_preset(&s), Preset::Balanced));
        // A policy change still does.
        s.computer.foreground = connect_core::settings::Foreground::Never;
        assert!(matches!(matching_preset(&s), Preset::Custom));
    }

    #[test]
    fn the_choice_of_apps_does_not_make_the_settings_custom() {
        use connect_core::{AppRef, AppsPolicy};
        let mut s = Settings::from_preset(Preset::Full, vec![]);
        s.computer.apps.policy = AppsPolicy::Only;
        s.computer.apps.only = vec![AppRef { id: "com.apple.Safari".into(), name: "Safari".into(), loose: false }];
        assert!(matches!(matching_preset(&s), Preset::Full));
        s.computer.mode = connect_core::settings::ComputerMode::Ask;
        assert!(matches!(matching_preset(&s), Preset::Custom));
    }

    #[test]
    fn the_experiment_does_not_make_the_settings_custom() {
        let mut s = Settings::from_preset(Preset::Careful, vec![]);
        s.computer.experiment = connect_core::settings::Experiment::Off;
        assert!(matches!(matching_preset(&s), Preset::Careful));
        s.computer.foreground = connect_core::settings::Foreground::Always;
        assert!(matches!(matching_preset(&s), Preset::Custom));
    }

    #[test]
    fn hub_override_must_be_https() {
        assert_eq!(hub_for(None).unwrap(), HUB_URL);
        assert_eq!(hub_for(Some("  ".into())).unwrap(), HUB_URL);
        assert_eq!(hub_for(Some("https://hub.example".into())).unwrap(), "https://hub.example");
        assert!(hub_for(Some("http://hub.example".into())).is_err());
        assert_eq!(hub_for(Some("http://127.0.0.1:9".into())).is_ok(), cfg!(debug_assertions));
    }
}
