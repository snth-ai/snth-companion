//! Grants and the approver seam.
//!
//! A grant = (capability, scope, duration). The core asks the owner
//! through an `Approver` the UI implements; the UI answers. Every ask is
//! bounded at 55 s by the core (the synth's hard limit is 60 s, so the
//! app must answer first). No answer = a refusal labeled
//! "denied: no answer from the user". A grant the owner gives after the
//! timeout is still recorded, so it applies to the next attempt
//! (connect.md, the 60 s rule).
//!
//! The reason text comes from the synth and is UNTRUSTED: it is stripped
//! of control characters, length-capped, and handed to the approver
//! flagged as such so the UI can show it as "the synth says: ...".

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The window the app has to answer any single ask. Below the synth's
/// 60 s hard limit so the refusal reaches the synth as our own label, not
/// as a dropped stream.
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(55);

/// Longest untrusted reason shown to the owner.
const MAX_REASON: usize = 400;

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// How long a grant lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Duration_ {
    /// This one attempt only.
    Once,
    /// Until the app restarts.
    Session,
    /// Until this unix second.
    Until(i64),
    /// Forever (persisted).
    Always,
}

/// The owner's answer to one ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Approval {
    pub allow: bool,
    pub duration: Duration_,
}

impl Approval {
    pub fn deny() -> Approval {
        Approval {
            allow: false,
            duration: Duration_::Once,
        }
    }
    pub fn once() -> Approval {
        Approval {
            allow: true,
            duration: Duration_::Once,
        }
    }
    pub fn always() -> Approval {
        Approval {
            allow: true,
            duration: Duration_::Always,
        }
    }
}

/// What the owner is being asked to allow.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    /// "net", "cookies", "computer" or "files".
    pub capability: String,
    /// A registrable domain, a folder, a tool, "task", ...
    pub scope: String,
    /// The synth's reason, already sanitized. ALWAYS from the synth and
    /// never to be trusted; show it as "the synth says: ...".
    pub reason: String,
    /// A short, app-authored line (not from the synth) describing the
    /// concrete action, e.g. "read cookies for youtube.com".
    pub summary: String,
}

impl ApprovalRequest {
    pub fn new(
        capability: impl Into<String>,
        scope: impl Into<String>,
        reason: &str,
        summary: impl Into<String>,
    ) -> ApprovalRequest {
        ApprovalRequest {
            capability: capability.into(),
            scope: scope.into(),
            reason: sanitize_reason(reason),
            summary: summary.into(),
        }
    }
}

/// Strip control characters and cap the length of synth-supplied text.
pub fn sanitize_reason(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(MAX_REASON));
    for c in s.chars() {
        if out.len() >= MAX_REASON {
            break;
        }
        if c.is_control() {
            if c == '\n' || c == '\t' {
                out.push(' ');
            }
            continue;
        }
        out.push(c);
    }
    out.trim().to_string()
}

/// The future an approver returns.
pub type ApprovalFuture = Pin<Box<dyn Future<Output = Approval> + Send>>;

/// Implemented by the UI. The core calls `ask`; the UI shows a window and
/// resolves the future when the owner decides. The core bounds the wait
/// itself, so an approver may take as long as it likes.
pub trait Approver: Send + Sync {
    fn ask(&self, req: ApprovalRequest) -> ApprovalFuture;
}

/// One remembered grant.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Grant {
    capability: String,
    scope: String,
    /// None = session (dropped on restart); Some(0) = forever;
    /// Some(unix) = until that second.
    until: Option<i64>,
}

/// In-memory store of session, until-time and always grants. The caller
/// persists the "always" ones through settings (e.g. net domains) where
/// that makes sense; this store is the fast path checked per request.
#[derive(Default)]
pub struct GrantStore {
    grants: Mutex<Vec<Grant>>,
}

impl GrantStore {
    pub fn new() -> GrantStore {
        GrantStore::default()
    }

    /// Record an approval for (capability, scope). `Once` records nothing.
    pub fn record(&self, capability: &str, scope: &str, approval: Approval) {
        if !approval.allow {
            return;
        }
        let until = match approval.duration {
            Duration_::Once => return,
            Duration_::Session => None,
            Duration_::Always => Some(0),
            Duration_::Until(t) => Some(t),
        };
        let mut g = self.grants.lock().unwrap();
        g.retain(|x| !(x.capability == capability && x.scope == scope));
        g.push(Grant {
            capability: capability.to_string(),
            scope: scope.to_string(),
            until,
        });
    }

    /// Is there a live grant for (capability, scope) right now?
    pub fn allows(&self, capability: &str, scope: &str) -> bool {
        let now = now_unix();
        let mut g = self.grants.lock().unwrap();
        g.retain(|x| match x.until {
            Some(0) | None => true,
            Some(t) => t > now,
        });
        g.iter().any(|x| {
            x.capability == capability
                && x.scope == scope
                && match x.until {
                    Some(0) | None => true,
                    Some(t) => t > now,
                }
        })
    }

    /// Drop a grant (the owner revoked it in the UI).
    pub fn revoke(&self, capability: &str, scope: &str) {
        let mut g = self.grants.lock().unwrap();
        g.retain(|x| !(x.capability == capability && x.scope == scope));
    }

    /// Put back a grant read from disk at startup (`until`: 0 = forever,
    /// otherwise the unix second it ends). An expired one is skipped.
    pub fn restore(&self, capability: &str, scope: &str, until: i64) {
        if until != 0 && until <= now_unix() {
            return;
        }
        let mut g = self.grants.lock().unwrap();
        g.retain(|x| !(x.capability == capability && x.scope == scope));
        g.push(Grant {
            capability: capability.to_string(),
            scope: scope.to_string(),
            until: Some(until),
        });
    }

    /// Forget everything (unpair).
    pub fn clear(&self) {
        self.grants.lock().unwrap().clear();
    }

    /// Snapshot for the UI / status: (capability, scope, until_unix) where
    /// until_unix 0 = forever, -1 = session.
    pub fn list(&self) -> Vec<(String, String, i64)> {
        let now = now_unix();
        let g = self.grants.lock().unwrap();
        g.iter()
            .filter(|x| match x.until {
                Some(0) | None => true,
                Some(t) => t > now,
            })
            .map(|x| {
                let u = x.until.unwrap_or(-1);
                (x.capability.clone(), x.scope.clone(), u)
            })
            .collect()
    }
}

/// Resolve an ask to an until-time for the synth's `permission.request`
/// (0 = one time or session, else the unix second the grant ends).
pub fn until_unix(approval: Approval) -> i64 {
    match approval.duration {
        Duration_::Once | Duration_::Session => 0,
        Duration_::Always => 0,
        Duration_::Until(t) => t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_controls_and_caps() {
        let r = sanitize_reason("hi\x07there\nyou");
        assert_eq!(r, "hithere you");
        let long = "x".repeat(1000);
        assert_eq!(sanitize_reason(&long).len(), MAX_REASON);
    }

    #[test]
    fn once_records_nothing() {
        let s = GrantStore::new();
        s.record("net", "example.com", Approval::once());
        assert!(!s.allows("net", "example.com"));
    }

    #[test]
    fn always_and_session_are_remembered() {
        let s = GrantStore::new();
        s.record("net", "a.com", Approval::always());
        s.record(
            "cookies",
            "b.com",
            Approval {
                allow: true,
                duration: Duration_::Session,
            },
        );
        assert!(s.allows("net", "a.com"));
        assert!(s.allows("cookies", "b.com"));
        assert!(!s.allows("net", "other.com"));
    }

    #[test]
    fn until_time_expires() {
        let s = GrantStore::new();
        s.record(
            "files",
            "/x",
            Approval {
                allow: true,
                duration: Duration_::Until(now_unix() - 1),
            },
        );
        assert!(!s.allows("files", "/x"));
        s.record(
            "files",
            "/y",
            Approval {
                allow: true,
                duration: Duration_::Until(now_unix() + 100),
            },
        );
        assert!(s.allows("files", "/y"));
    }

    #[test]
    fn restore_skips_expired() {
        let s = GrantStore::new();
        s.restore("cookies", "a.com", 0);
        s.restore("cookies", "b.com", now_unix() + 100);
        s.restore("cookies", "c.com", now_unix() - 100);
        assert!(s.allows("cookies", "a.com"));
        assert!(s.allows("cookies", "b.com"));
        assert!(!s.allows("cookies", "c.com"));
        assert_eq!(s.list().len(), 2);
    }

    #[test]
    fn revoke_and_clear() {
        let s = GrantStore::new();
        s.record("net", "a.com", Approval::always());
        s.record("net", "b.com", Approval::always());
        s.revoke("net", "a.com");
        assert!(!s.allows("net", "a.com"));
        assert!(s.allows("net", "b.com"));
        s.clear();
        assert!(!s.allows("net", "b.com"));
    }
}
