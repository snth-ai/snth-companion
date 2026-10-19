//! The shell's side of the platform account: when the account's synths
//! are read again, and what the owner is told about it.
//!
//! The core does the work (`connect_core::account`). The shell reads the
//! account at start, every 10 minutes, and when the window comes up (at
//! most once a minute for that), shows a notification for each synth the
//! computer was linked to by the sync, and starts or ends the onboarding
//! when the first link comes or the last one goes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use connect_core::account::SYNC_EVERY;
use tauri_plugin_notification::NotificationExt;
use tokio::sync::watch;

use crate::state::{emit_state, Ctx};
use crate::{commands, i18n};

/// The window coming up reads the account at most this often.
const FOCUS_GAP: Duration = Duration::from_secs(60);

/// Read the account now and act on what changed.
pub async fn sync_now(ctx: &Arc<Ctx>) {
    let before = ctx.core.link_ids().len();
    let report = ctx.core.account_sync().await;
    if !report.added.is_empty() {
        ctx.relink.notify_one();
        if before == 0 {
            commands::begin_onboarding(ctx).await;
        }
        let lang = ctx.lang();
        for name in &report.added {
            let _ = ctx
                .app
                .notification()
                .builder()
                .title(i18n::connected_title(lang))
                .body(i18n::connected_body(lang, name))
                .show();
        }
    }
    if before > 0 && ctx.core.link_ids().is_empty() {
        commands::after_last_link(ctx);
    }
    emit_state(ctx);
}

/// The sync loop: at start, every `SYNC_EVERY`, and when the window comes
/// up (`ctx.account_wake`).
pub fn spawn_sync(ctx: Arc<Ctx>, mut shutdown: watch::Receiver<bool>) {
    tauri::async_runtime::spawn(async move {
        loop {
            sync_now(&ctx).await;
            let last = Instant::now();
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(SYNC_EVERY) => break,
                    _ = ctx.account_wake.notified() => {
                        if last.elapsed() >= FOCUS_GAP {
                            break;
                        }
                    }
                    _ = shutdown.changed() => return,
                }
            }
        }
    });
}
