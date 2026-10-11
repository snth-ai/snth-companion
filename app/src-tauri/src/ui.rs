//! The one event channel to the web UI, plus system notifications.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::approvals::ApprovalDto;
use crate::i18n;
use crate::state::Ctx;

/// Every window listens to this single event name.
pub const CHANNEL: &str = "connect:event";

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiEvent {
    /// Something in the state changed (link, settings, pairing, pause,
    /// grants, preferences): fetch the state again.
    State,
    /// One new line in the activity journal.
    Journal { entry: connect_core::Entry },
    /// The core asks the owner something.
    Approval { request: ApprovalDto },
    /// A request left the queue (answered, timed out, refused by pause).
    ApprovalClosed { id: u64 },
    /// `snth://pair?code=...` was opened: collect the code with
    /// `ui_ready` and show the confirmation screen. Nothing is linked
    /// until the owner clicks there.
    PairLink,
}

pub fn emit(app: &AppHandle, event: UiEvent) {
    let _ = app.emit(CHANNEL, event);
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

pub fn notify_approval(app: &AppHandle, dto: &ApprovalDto) {
    let Some(ctx) = app.try_state::<std::sync::Arc<Ctx>>() else {
        return;
    };
    let lang = ctx.lang();
    let brief = ctx.core.link_brief();
    let synth = brief
        .synth_name
        .or(brief.synth_id)
        .unwrap_or_else(|| "SNTH".to_string());
    notify(
        app,
        &i18n::approval_title(lang, &synth),
        &i18n::approval_body(lang, &dto.capability, &dto.scope),
    );
}

pub fn notify_paused(app: &AppHandle) {
    let Some(ctx) = app.try_state::<std::sync::Arc<Ctx>>() else {
        return;
    };
    let lang = ctx.lang();
    notify(app, i18n::paused_title(lang), i18n::paused_body(lang));
}
