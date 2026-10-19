//! The approval queue: the core asks through `Approver`, the approval
//! window answers through a command, and this map joins the two.
//!
//! No answer is a refusal. A request leaves the queue when the owner
//! answers, when the core's 55 s window runs out, when the owner pauses,
//! or when the approval window is closed (deny, the safe default).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use connect_core::grant::{sanitize_reason, ApprovalFuture, APPROVAL_TIMEOUT};
use connect_core::{Approval, ApprovalRequest, Approver};
use serde::Serialize;
use tauri::AppHandle;
use tokio::sync::oneshot;

use crate::{ui, windows};

/// Longest scope text shown to the owner. For `permission.request` the
/// scope is the synth's own text, so it is cleaned and capped like the
/// reason is.
const MAX_SCOPE: usize = 200;

/// The longest single fact shown in the window (a message text, the
/// input of a shortcut). The core bounds these far below this already.
const MAX_DETAIL: usize = 20_000;

/// One fact of the action the owner is asked about ("to", "text",
/// "title", "when"...). The value is what will actually be sent or
/// created, shown in full.
#[derive(Debug, Clone, Serialize)]
pub struct DetailDto {
    pub key: String,
    pub value: String,
}

/// One pending request, as the approval window sees it.
#[derive(Debug, Clone, Serialize)]
pub struct ApprovalDto {
    pub id: u64,
    /// "net", "cookies", "computer", "files", "apps", or whatever the
    /// synth named in a `permission.request`.
    pub capability: String,
    /// A site, "task", "foreground:<tool>", a file path, or for apps the
    /// kind of action ("calendar.create", "messages.send:<recipient>").
    pub scope: String,
    /// App-authored English line from the core; the UI words its own from
    /// capability + scope and falls back to this.
    pub summary: String,
    /// The synth's own words. Untrusted; shown as "the synth says".
    pub reason: String,
    /// The facts of the action itself, from the core (empty for most
    /// requests).
    pub details: Vec<DetailDto>,
    /// What the owner may ask to be remembered: "any", "always" (only
    /// "always", for this exact scope) or "never" (this time only).
    pub remember: &'static str,
    pub asked_at_ms: u64,
    pub expires_at_ms: u64,
    /// The synth that asks (its name), when a synth asks.
    pub synth: Option<String>,
}

struct Pending {
    dto: ApprovalDto,
    tx: oneshot::Sender<Approval>,
}

#[derive(Default)]
pub struct Approvals {
    next: AtomicU64,
    pending: Mutex<BTreeMap<u64, Pending>>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Approvals {
    fn push(&self, req: ApprovalRequest, tx: oneshot::Sender<Approval>) -> ApprovalDto {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let now = now_ms();
        let mut scope = sanitize_reason(&req.scope);
        if scope.len() > MAX_SCOPE {
            let mut cut = MAX_SCOPE;
            while !scope.is_char_boundary(cut) {
                cut -= 1;
            }
            scope.truncate(cut);
        }
        let dto = ApprovalDto {
            id,
            capability: sanitize_reason(&req.capability),
            scope,
            summary: req.summary,
            reason: req.reason,
            details: req
                .details
                .into_iter()
                .map(|(key, value)| DetailDto {
                    key: sanitize_reason(&key),
                    value: value.chars().take(MAX_DETAIL).collect(),
                })
                .collect(),
            remember: req.remember.label(),
            synth: req.synth.map(|n| sanitize_reason(&n)).filter(|n| !n.is_empty()),
            asked_at_ms: now,
            expires_at_ms: now + APPROVAL_TIMEOUT.as_millis() as u64,
        };
        self.pending
            .lock()
            .unwrap()
            .insert(id, Pending { dto: dto.clone(), tx });
        dto
    }

    /// The queue, oldest first.
    pub fn list(&self) -> Vec<ApprovalDto> {
        self.pending
            .lock()
            .unwrap()
            .values()
            .map(|p| p.dto.clone())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.lock().unwrap().is_empty()
    }

    /// Resolve one request. False when it is no longer waiting (it timed
    /// out or was answered already).
    pub fn answer(&self, id: u64, approval: Approval) -> bool {
        match self.pending.lock().unwrap().remove(&id) {
            Some(p) => {
                let _ = p.tx.send(approval);
                true
            }
            None => false,
        }
    }

    /// Refuse everything that is waiting. Returns the ids refused.
    pub fn deny_all(&self) -> Vec<u64> {
        let drained: Vec<(u64, Pending)> = {
            let mut map = self.pending.lock().unwrap();
            std::mem::take(&mut *map).into_iter().collect()
        };
        drained
            .into_iter()
            .map(|(id, p)| {
                let _ = p.tx.send(Approval::deny());
                id
            })
            .collect()
    }

    fn drop_expired(&self, id: u64) -> bool {
        self.pending.lock().unwrap().remove(&id).is_some()
    }
}

/// The `Approver` the core holds.
pub struct UiApprover {
    pub app: AppHandle,
    pub approvals: Arc<Approvals>,
}

impl Approver for UiApprover {
    fn ask(&self, req: ApprovalRequest) -> ApprovalFuture {
        let (tx, rx) = oneshot::channel();
        let dto = self.approvals.push(req, tx);
        let id = dto.id;
        let app = self.app.clone();
        let approvals = self.approvals.clone();

        // Show it: the window, the event for an already open window, and a
        // system notification in case the owner is looking elsewhere. Off
        // this task, so the core's stream handler is not held up by it.
        let app_show = app.clone();
        tauri::async_runtime::spawn(async move {
            ui::emit(&app_show, ui::UiEvent::Approval { request: dto.clone() });
            windows::open_approval(&app_show);
            ui::notify_approval(&app_show, &dto);
        });

        Box::pin(async move {
            match tokio::time::timeout(APPROVAL_TIMEOUT, rx).await {
                Ok(Ok(approval)) => approval,
                // No answer in time (or the sender is gone): refuse, and
                // take the request off the owner's screen.
                _ => {
                    if approvals.drop_expired(id) {
                        ui::emit(&app, ui::UiEvent::ApprovalClosed { id });
                        if approvals.is_empty() {
                            windows::hide_approval(&app);
                        }
                    }
                    Approval::deny()
                }
            }
        })
    }
}
