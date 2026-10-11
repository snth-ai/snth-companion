//! Things that happen from more than one place (a command, the tray, the
//! stop shortcut, a deep link).

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use connect_core::pairing::parse_deep_link;
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

/// Quit for real (the tray's Quit): stop the link, stop our own helper
/// daemon if one is running, write what is batched, exit.
pub async fn quit(ctx: &Arc<Ctx>) {
    let _ = ctx.shutdown.send(true);
    ctx.approvals.deny_all();
    // `pause` is what stops the private Cua daemon. The pause itself is
    // not remembered: only the owner's own pause survives a relaunch.
    let _ = tokio::time::timeout(Duration::from_secs(4), ctx.core.pause()).await;
    ctx.core.flush();
    ctx.app.exit(0);
}

/// `snth://pair?code=XXXXXX`. Any web page can open such a link, so this
/// only brings the window up and leaves the code for the UI to collect
/// (`ui_ready`). The UI then asks the hub who the code belongs to and
/// shows the confirmation screen; the token is kept and the link opened
/// only after the owner clicks Connect there.
pub fn handle_deep_link(ctx: &Arc<Ctx>, url: &str) {
    let Some(code) = parse_deep_link(url) else {
        return;
    };
    *ctx.pair_code.lock().unwrap() = Some(code);
    windows::show_main(&ctx.app);
    ui::emit(&ctx.app, UiEvent::PairLink);
}

/// Register the stop shortcut with the OS. An empty string turns it off.
/// Err carries a short reason (another app owns the combination, or the
/// text is not a shortcut).
pub fn set_stop_shortcut(ctx: &Arc<Ctx>, accelerator: &str) -> Result<(), String> {
    let gs = ctx.app.global_shortcut();
    let _ = gs.unregister_all();
    *ctx.shortcut_active.lock().unwrap() = false;
    if accelerator.trim().is_empty() {
        return Ok(());
    }
    let shortcut = Shortcut::from_str(accelerator).map_err(|e| format!("bad_shortcut: {e}"))?;
    gs.register(shortcut)
        .map_err(|e| format!("shortcut_unavailable: {e}"))?;
    *ctx.shortcut_active.lock().unwrap() = true;
    Ok(())
}
