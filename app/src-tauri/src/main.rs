//! SNTH Connect: the desktop shell. A tray app that holds one
//! `connect_core::Core`, shows its state in a small window, and asks the
//! owner before the synth does anything the settings say to ask about.

// No console window next to the app on Windows (release builds).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod account;
mod actions;
mod approvals;
mod browsers;
mod commands;
mod crashlog;
mod i18n;
mod logins;
#[cfg(target_os = "macos")]
mod menu;
mod prefs;
mod safari;
mod state;
mod tray;
mod ui;
mod updates;
mod windows;

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use connect_core::{
    Core, CoreConfig, CoreEvent, CuaAttach, FailCode, InstallState, SecretStore, UreqHubClient,
};
use tauri::{AppHandle, Manager, RunEvent, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_global_shortcut::ShortcutState;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{watch, Notify};

use crate::approvals::{Approvals, UiApprover};
use crate::prefs::PrefsFile;
use crate::state::{Ctx, PLATFORM_URL, UPDATER_KEY_PLACEHOLDER};
use crate::ui::UiEvent;

/// The flag the autostart entry passes, so a launch at login stays in the
/// tray instead of opening the window.
const HIDDEN_FLAG: &str = "--hidden";

/// The pairing token goes to the OS keychain in a release build. A debug
/// build is unsigned, and an unsigned binary reading the keychain makes
/// macOS ask for the login password on every rebuild, so debug builds keep
/// the token in a 0600 file in the config dir instead.
#[cfg(not(debug_assertions))]
fn secret_store(_config_dir: &Path) -> (Arc<dyn SecretStore>, &'static str) {
    (
        Arc::new(connect_core::KeyringSecretStore::new("ai.snth.connect")),
        "system keychain",
    )
}

#[cfg(debug_assertions)]
fn secret_store(config_dir: &Path) -> (Arc<dyn SecretStore>, &'static str) {
    (
        Arc::new(connect_core::FileSecretStore::new(config_dir.join("secrets.json"))),
        "file (debug build)",
    )
}

/// Is a real updater public key configured (not the placeholder)?
fn updater_configured(config: &tauri::Config) -> bool {
    config
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("pubkey"))
        .and_then(|k| k.as_str())
        .is_some_and(|k| !k.trim().is_empty() && k != UPDATER_KEY_PLACEHOLDER)
}

fn setup(app: &mut tauri::App, updater_ready: bool) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    let config_dir = app.path().app_config_dir()?;
    let data_dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&config_dir)?;
    std::fs::create_dir_all(&data_dir)?;

    let prefs_file = PrefsFile::new(config_dir.join("shell.json"));
    let mut prefs = prefs_file.load();
    let was_paused = prefs.paused;
    let stop_shortcut = prefs.stop_shortcut.clone();
    let onboarded = prefs.onboarded;

    let approvals = Arc::new(Approvals::default());
    let (secret, secret_kind) = secret_store(&config_dir);
    let core = Core::new(CoreConfig {
        settings_path: Some(config_dir.join("settings.json")),
        journal_path: Some(data_dir.join("journal.jsonl")),
        approver: Arc::new(UiApprover { app: handle.clone(), approvals: approvals.clone() }),
        cookies: Arc::new(browsers::system_reader()),
        // The owner's own apps on this system. Notifications go out as
        // this app (its name and icon), like the app's own.
        apps: connect_core::apps::system_backend(Some(Arc::new(ui::AppNotifier { app: handle.clone() }))),
        secret,
        hub: Arc::new(UreqHubClient),
        app_version: app.package_info().version.to_string(),
        // The helper for computer control is the app's own pinned copy in
        // its data dir, installed by the app itself (see spawn_helper).
        // A driver the owner installed for themselves is never used,
        // changed or stopped, and neither is its daemon.
        cua_bin: String::new(),
        cua_private_root: Some(data_dir.join("cua")),
        cua_attach: CuaAttach::Private,
    });
    // The owner's platform account: sign-in and the app's API.
    core.set_platform(Arc::new(connect_core::account::UreqPlatformClient), PLATFORM_URL);

    // The first run of a new version: one line in the journal. And an
    // update restart leaves word whether to come back hidden.
    let version = app.package_info().version.to_string();
    let updated_from = updates::note_version(&mut prefs, &version, core.shared().journal());
    let resume_hidden = prefs.resume_hidden.take();
    prefs_file.save(&prefs);

    let (shutdown, shutdown_rx) = watch::channel(false);
    let ctx = Arc::new(Ctx {
        app: handle.clone(),
        core: core.clone(),
        approvals,
        updates: updates::Updates::new(updater_ready, updated_from),
        prefs: Mutex::new(prefs),
        prefs_file,
        offered_roots: Mutex::new(HashSet::new()),
        link_detail: Mutex::new(std::collections::HashMap::new()),
        pair_code: Mutex::new(None),
        helper_retry: Notify::new(),
        helper_retry_at: Mutex::new(None),
        relink: Notify::new(),
        account_wake: Notify::new(),
        shutdown,
        tray: Mutex::new(None),
        device_name: core.shared().device_name(),
        secret_store: secret_kind,
        cua_bin: core.cua_bin(),
        updater_ready,
        shortcut_active: Mutex::new(false),
        data_dir: data_dir.clone(),
        browser_link: Mutex::new(None),
        vault: logins::VaultCache::default(),
        safari: safari::Safari::detect(|exe| logins::stays_put(exe, false)),
    });
    app.manage(ctx.clone());

    // Stay paused if the owner left the app paused, and while onboarding
    // is unfinished (nothing may run before the owner finished choosing).
    if core.is_paired() && (was_paused || !onboarded) {
        tauri::async_runtime::block_on(core.pause());
    }

    // Subscribe before the link starts so no event is missed.
    spawn_event_forwarder(ctx.clone());
    spawn_link(ctx.clone(), shutdown_rx.clone());
    spawn_helper(ctx.clone(), shutdown_rx.clone());
    // The SNTH Vault extension: serve its helper, and tell the installed
    // browsers where that helper is (only while linked to a synth; this
    // also follows the app after an update or a move).
    logins::spawn_link(ctx.clone(), shutdown_rx.clone());
    // Safari's own way in (macOS, when this copy of the app carries the
    // Safari extension).
    logins::spawn_safari_door(ctx.clone(), shutdown_rx.clone());
    logins::sync(&ctx);
    // The account's synths: at start, every 10 minutes, when the window
    // comes up.
    account::spawn_sync(ctx.clone(), shutdown_rx.clone());
    updates::spawn(ctx.clone(), shutdown_rx);

    tray::build(&handle, &ctx)?;
    #[cfg(target_os = "macos")]
    menu::install(&handle)?;

    // Launched at login, or back from an update that happened while the
    // window was hidden: stay in the tray, unless there is something the
    // owner has to do first (pair or finish onboarding).
    let asked_hidden = resume_hidden.unwrap_or_else(|| std::env::args().any(|a| a == HIDDEN_FLAG));
    let hidden = asked_hidden && onboarded && core.is_paired();
    ctx.updates.started_hidden.store(hidden, Ordering::Release);
    ctx.updates.window_was_shown.store(!hidden, Ordering::Release);
    windows::create_main(&handle, !hidden)?;
    #[cfg(target_os = "macos")]
    {
        if hidden {
            let _ = handle.set_activation_policy(tauri::ActivationPolicy::Accessory);
        }
    }

    // The stop shortcut. Another app may own the combination; the General
    // screen shows that and lets the owner pick another.
    let _ = actions::set_stop_shortcut(&ctx, &stop_shortcut);

    // snth://pair?code=...
    #[cfg(any(windows, target_os = "linux"))]
    let _ = app.deep_link().register_all();
    if let Ok(Some(urls)) = app.deep_link().get_current() {
        for url in urls {
            actions::handle_deep_link(&ctx, url.as_str());
        }
    }
    let ctx_links = ctx.clone();
    app.deep_link().on_open_url(move |event| {
        for url in event.urls() {
            actions::handle_deep_link(&ctx_links, url.as_str());
        }
    });

    Ok(())
}

/// The link supervisor: one reconnect loop per link (per synth). A loop
/// ends when the app shuts down or its link is removed. After a new
/// pairing (`relink`) a loop is started for every link that has none
/// (`run_slot` returns at once for a link whose loop runs already).
fn spawn_link(ctx: Arc<Ctx>, mut shutdown: watch::Receiver<bool>) {
    tauri::async_runtime::spawn(async move {
        loop {
            for synth_id in ctx.core.link_ids() {
                let (core, stop) = (ctx.core.clone(), shutdown.clone());
                tauri::async_runtime::spawn(async move { core.run_slot(&synth_id, stop).await });
            }
            tokio::select! {
                _ = ctx.relink.notified() => {}
                _ = shutdown.changed() => break,
            }
        }
    });
}

/// How long to wait before trying the helper install again: 30 s, 2 min,
/// 10 min, 30 min, then every hour. A click on "try again" skips the wait.
const HELPER_BACKOFF_SECS: [u64; 5] = [30, 120, 600, 1800, 3600];

/// The helper for computer control. Checked at every launch, in the
/// background: installed when missing, replaced when this app version pins
/// another one. The window and the link never wait for it. A failure
/// (offline, a broken download) is retried with a growing pause, and again
/// at the next launch.
fn spawn_helper(ctx: Arc<Ctx>, mut shutdown: watch::Receiver<bool>) {
    // Every state goes to the UI as it happens (the progress line).
    if let Some(mut states) = ctx.core.subscribe_cua_helper() {
        let ctx = ctx.clone();
        tauri::async_runtime::spawn(async move {
            while states.changed().await.is_ok() {
                let state = states.borrow().clone();
                let settled = matches!(state, InstallState::Ready { .. } | InstallState::Failed { .. });
                ui::emit(&ctx.app, UiEvent::Helper { state });
                if settled {
                    ui::emit(&ctx.app, UiEvent::State);
                }
            }
        });
    }

    tauri::async_runtime::spawn(async move {
        let mut attempt = 0usize;
        loop {
            *ctx.helper_retry_at.lock().unwrap() = None;
            match ctx.core.cua_ensure().await {
                InstallState::Ready { .. } => break,
                // Retrying cannot help these.
                InstallState::Failed { code: FailCode::Unsupported | FailCode::Cancelled, .. } => break,
                _ => {}
            }
            let wait = HELPER_BACKOFF_SECS[attempt.min(HELPER_BACKOFF_SECS.len() - 1)];
            attempt += 1;
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            *ctx.helper_retry_at.lock().unwrap() = Some(now + wait as i64);
            ui::emit(&ctx.app, UiEvent::State);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
                // The owner clicked "try again now".
                _ = ctx.helper_retry.notified() => attempt = 0,
                _ = shutdown.changed() => break,
            }
        }
    });
}

/// Core events and journal entries -> the UI's one event channel, plus the
/// tray label.
fn spawn_event_forwarder(ctx: Arc<Ctx>) {
    let mut events = ctx.core.subscribe();
    let mut journal = ctx.core.subscribe_journal();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::select! {
                ev = events.recv() => match ev {
                    Ok(CoreEvent::Link { synth_id, state, detail }) => {
                        // A link without a token repeats "no pairing" every
                        // two seconds; only a change is news.
                        let changed = {
                            let mut last = ctx.link_detail.lock().unwrap();
                            let next = (state, detail);
                            let changed = last.get(&synth_id) != Some(&next);
                            last.insert(synth_id, next);
                            changed
                        };
                        if changed {
                            tray::refresh(&ctx);
                            ui::emit(&ctx.app, UiEvent::State);
                        }
                    }
                    // Linked or unlinked: the browsers learn or forget
                    // where the helper is, and what the vault held is no
                    // longer known.
                    Ok(event @ (CoreEvent::Paired { .. } | CoreEvent::Unpaired { .. })) => {
                        // A new link gets its loop.
                        if matches!(event, CoreEvent::Paired { .. }) {
                            ctx.relink.notify_one();
                        }
                        logins::sync(&ctx);
                        ctx.vault.set(logins::VaultView::Unknown);
                        tray::refresh(&ctx);
                        ui::emit(&ctx.app, UiEvent::State);
                    }
                    Ok(_) | Err(RecvError::Lagged(_)) => {
                        tray::refresh(&ctx);
                        ui::emit(&ctx.app, UiEvent::State);
                    }
                    Err(RecvError::Closed) => break,
                },
                entry = journal.recv() => match entry {
                    Ok(entry) => ui::emit(&ctx.app, UiEvent::Journal { entry }),
                    Err(RecvError::Lagged(_)) => ui::emit(&ctx.app, UiEvent::State),
                    Err(RecvError::Closed) => break,
                },
            }
        }
    });
}

fn ctx_of(app: &AppHandle) -> Option<Arc<Ctx>> {
    app.try_state::<Arc<Ctx>>().map(|s| s.inner().clone())
}

fn main() {
    let context = tauri::generate_context!();
    // Before anything else runs: a panic from here on is written down
    // (crashlog.rs says where), and so is stderr when it goes nowhere.
    crashlog::install(&context.package_info().version.to_string());
    let updater_ready = updater_configured(context.config());

    let mut builder = tauri::Builder::default()
        // Must be first: a second launch (or a deep link on Windows and
        // Linux) lands in the running instance.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            windows::show_main(app);
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![HIDDEN_FLAG]),
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    // The only shortcut the app registers is "stop".
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    let Some(ctx) = ctx_of(app) else { return };
                    tauri::async_runtime::spawn(async move {
                        if !ctx.core.link_brief().paused {
                            actions::pause(&ctx).await;
                            ui::notify_paused(&ctx.app);
                        }
                    });
                })
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init());

    // The updater verifies downloads against a public key from
    // tauri.conf.json. Until the owner puts a real key there, the plugin
    // stays out and "check for updates" reports that it is not set up.
    if updater_ready {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }

    let app = builder
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::get_journal,
            commands::ui_ready,
            commands::pair_begin,
            commands::pair_confirm,
            commands::pair_cancel,
            commands::unpair,
            commands::unpair_synth,
            commands::reconnect_synth,
            commands::login_cancel,
            commands::account_connect,
            commands::account_sync,
            commands::sign_out,
            commands::finish_onboarding,
            commands::apply_preset,
            commands::update_settings,
            commands::suggested_roots,
            commands::add_suggested_root,
            commands::pick_folder,
            commands::remove_root,
            commands::set_root_write,
            commands::revoke_grant,
            commands::pause,
            commands::resume,
            commands::approvals_pending,
            commands::approval_answer,
            commands::cua_check,
            commands::cua_grant_permissions,
            commands::cua_install,
            commands::computer_apps,
            commands::cua_health_check,
            commands::apps_check,
            commands::apps_request_permission,
            commands::logins_refresh,
            commands::safari_check,
            commands::vault_forget,
            commands::browser_link,
            commands::browser_unlink,
            commands::browser_forget,
            commands::set_language,
            commands::set_autostart,
            commands::set_stop_shortcut,
            commands::check_update,
            commands::install_update,
            commands::set_auto_update,
            commands::set_developer,
            commands::diagnostics,
            commands::open_external,
        ])
        .on_window_event(|window, event| {
            // The main window came up: a good moment to read the account.
            if let WindowEvent::Focused(true) = event {
                if window.label() != windows::APPROVAL {
                    if let Some(ctx) = ctx_of(window.app_handle()) {
                        ctx.account_wake.notify_one();
                    }
                }
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                // Neither window is ever destroyed by its close button.
                api.prevent_close();
                let app = window.app_handle();
                match window.label() {
                    windows::APPROVAL => {
                        // Closing the request window refuses what is in it.
                        if let Some(ctx) = ctx_of(app) {
                            for id in ctx.approvals.deny_all() {
                                ui::emit(app, UiEvent::ApprovalClosed { id });
                            }
                        }
                        windows::hide_approval(app);
                    }
                    _ => windows::hide_main(app),
                }
            }
        })
        .setup(move |app| setup(app, updater_ready))
        .build(context)
        .expect("failed to start SNTH Connect");

    app.run(|app, event| match event {
        // Closing the last window must not quit a tray app. A real quit
        // (the tray's Quit, an update restart) carries an exit code.
        RunEvent::ExitRequested { api, code, .. } => {
            if code.is_none() {
                api.prevent_exit();
            }
        }
        // macOS: the dock icon was clicked.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => windows::show_main(app),
        _ => {
            let _ = app;
        }
    });
}
