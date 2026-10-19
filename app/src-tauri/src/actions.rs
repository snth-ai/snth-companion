//! Things that happen from more than one place (a command, the tray, the
//! stop shortcut, a deep link).

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use connect_core::pairing::{parse_deep_link, DeepLink};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

use crate::state::Ctx;
use crate::ui::{self, UiEvent};
use crate::{tray, windows};

/// Stop everything: refuse what is waiting, close the synth's open work,
/// and remember the pause so a relaunch stays paused.
pub async fn pause(ctx: &Arc<Ctx>) {
    for id in ctx.approvals.deny_all() {
        ui::emit(&ctx.app, UiEvent::ApprovalClosed { id });
    }
    windows::hide_approval(&ctx.app);
    ctx.core.pause().await;
    ctx.update_prefs(|p| p.paused = true);
    tray::refresh(ctx);
    ui::emit(&ctx.app, UiEvent::State);
}

/// Resume. Refused while onboarding is unfinished: until then the core is
/// held paused on purpose (see `pair_confirm`).
pub fn resume(ctx: &Arc<Ctx>) {
    if !ctx.prefs.lock().unwrap().onboarded {
        return;
    }
    ctx.core.resume();
    ctx.update_prefs(|p| p.paused = false);
    tray::refresh(ctx);
    ui::emit(&ctx.app, UiEvent::State);
}

/// Quit for real (the tray's Quit, Cmd+Q): stop the link, stop our own
/// helper daemon if one is running, write what is batched, exit. A
/// downloaded update is put in place on the way out.
pub async fn quit(ctx: &Arc<Ctx>) {
    stop_everything(ctx).await;
    crate::updates::apply_at_quit(ctx).await;
    ctx.app.exit(0);
}

/// Leave without the quit-time extras: used right after an update was
/// installed, when a relaunch is already scheduled.
pub async fn exit(ctx: &Arc<Ctx>) {
    stop_everything(ctx).await;
    ctx.app.exit(0);
}

async fn stop_everything(ctx: &Arc<Ctx>) {
    let _ = ctx.shutdown.send(true);
    ctx.core.cua_stop_install();
    ctx.approvals.deny_all();
    // `pause` is what stops the private Cua daemon. The pause itself is
    // not remembered: only the owner's own pause survives a relaunch.
    let _ = tokio::time::timeout(Duration::from_secs(4), ctx.core.pause()).await;
    ctx.core.flush();
}

/// A `snth://` link. Any web page can open one, so:
///
/// - `snth://pair?code=XXXXXX` only brings the window up and leaves the
///   code for the UI to collect (`ui_ready`). The UI then asks who the
///   code belongs to and shows the confirmation screen; the token is kept
///   and the link opened only after the owner clicks Connect there.
/// - `snth://login?code=...&state=...` finishes a sign-in only when its
///   state is the one this app is waiting for (the core checks it and
///   journals any other); then the window comes up with the account's
///   synths to confirm. Nothing is linked by the link itself.
pub fn handle_deep_link(ctx: &Arc<Ctx>, url: &str) {
    match parse_deep_link(url) {
        Some(DeepLink::Pair(code)) => {
            *ctx.pair_code.lock().unwrap() = Some(code);
            windows::show_main(&ctx.app);
            ui::emit(&ctx.app, UiEvent::PairLink);
        }
        Some(DeepLink::Login { code, state }) => {
            let ctx = ctx.clone();
            tauri::async_runtime::spawn(async move {
                match ctx.core.login_complete(code, &state).await {
                    // A link this app did not ask for: nothing to show.
                    Err(e) if e == "state_mismatch" => {}
                    _ => {
                        windows::show_main(&ctx.app);
                        crate::state::emit_state(&ctx);
                    }
                }
            });
        }
        None => {}
    }
}

/// Register the stop shortcut with the OS. An empty string turns it off.
/// Err carries a short reason (another app owns the combination, or the
/// text is not a shortcut).
pub fn set_stop_shortcut(ctx: &Arc<Ctx>, accelerator: &str) -> Result<(), String> {
    let gs = ctx.app.global_shortcut();
    // Only the stop shortcut: the island's keys are registered next to it.
    if let Some(old) = ctx.stop_shortcut.lock().unwrap().take() {
        let _ = gs.unregister(old);
    }
    *ctx.shortcut_active.lock().unwrap() = false;
    if accelerator.trim().is_empty() {
        return Ok(());
    }
    let shortcut = Shortcut::from_str(accelerator).map_err(|e| format!("bad_shortcut: {e}"))?;
    // One of the island's keys already: not a second time.
    let island = ctx.core.island().hotkeys;
    let mine = connect_core::island::canonical(accelerator);
    if connect_core::island::HotkeyKind::ALL.iter().any(|k| connect_core::island::canonical(island.get(*k)) == mine)
        && ctx.island.keys_active()
    {
        return Err("shortcut_unavailable: an island key".into());
    }
    gs.register(shortcut)
        .map_err(|e| format!("shortcut_unavailable: {e}"))?;
    *ctx.stop_shortcut.lock().unwrap() = Some(shortcut);
    *ctx.shortcut_active.lock().unwrap() = true;
    Ok(())
}

/// Is this the stop shortcut?
pub fn is_stop_shortcut(ctx: &Ctx, id: u32) -> bool {
    ctx.stop_shortcut.lock().unwrap().is_some_and(|s| s.id() == id)
}
