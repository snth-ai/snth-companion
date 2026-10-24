//! One link per synth.
//!
//! A computer can be linked to several synths at once (every synth of the
//! owner's platform account, plus any linked with a code from a synth's
//! bot). Everything the computer offers is shared (the settings, the
//! journal, computer control, the browser link, the approvals); what
//! belongs to one link lives here, in its `LinkSlot`: who the synth is,
//! where it is, how the link stands, and the control stream's requests.
//! `Shared::links` holds the slots; each slot has its own reconnect loop
//! (`link::run_forever`).
//!
//! Work a synth asks for over its link runs inside `PEER` (a task-local
//! set by the link for every stream it serves), so the policy code can
//! tell which synth asks without passing the slot through every handler:
//! grants are per synth, the journal and the approval window name the
//! synth, and computer control is held by one synth at a time.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use serde::Serialize;
use tokio::sync::Notify;

use crate::core::LinkState;
use crate::requests::LinkRequests;
use crate::settings::{LinkSource, StoredIdentity};
use crate::wire::HelloAck;

/// The keychain key a synth's token is stored under.
pub fn token_key(synth_id: &str) -> String {
    format!("{}:{synth_id}", crate::core::TOKEN_KEY)
}

tokio::task_local! {
    /// The link a piece of work came in on (see the module comment).
    pub(crate) static PEER: Arc<LinkSlot>;
}

/// The link the current work came in on, if any.
pub(crate) fn current() -> Option<Arc<LinkSlot>> {
    PEER.try_with(|slot| slot.clone()).ok()
}

/// Everything of one link.
pub struct LinkSlot {
    /// The synth's id. Empty only for a link the CLI was handed a token and
    /// an address for (`adhoc`), until its first hello_ack.
    pub synth_id: String,
    /// The synth's base url (https://...).
    pub(crate) url: RwLock<String>,
    pub(crate) source: RwLock<LinkSource>,
    /// The platform's hub id for an account link (re-mint), else empty.
    pub(crate) hub_id: RwLock<String>,
    /// The name stored with the pairing (from the platform or the last
    /// hello_ack), for while the link is down.
    pub(crate) stored_name: RwLock<String>,
    /// Where the token is in the SecretStore.
    pub(crate) token_key: String,
    /// Not stored: the CLI's link from `--url` + `--token`.
    pub(crate) adhoc: bool,
    /// The token of an app before 0.1.13 could not be moved to this
    /// synth's key yet: it is read from the bare `pairing_token` key until
    /// a move succeeds (`Core::new` retries at every start).
    pub(crate) legacy_token: AtomicBool,
    /// What the synth said in its last hello_ack.
    pub(crate) synth: RwLock<Option<HelloAck>>,
    pub(crate) state: RwLock<LinkState>,
    /// Why the link is not up (the core's words, for diagnostics).
    pub(crate) detail: Mutex<String>,
    /// Unpair asked the loop to stop for good.
    pub(crate) stopping: AtomicBool,
    /// A reconnect loop runs this slot (at most one does).
    pub(crate) running: AtomicBool,
    /// Wakes a loop that waits (after the synth refused the token): a new
    /// token is in place, or the slot is going away.
    pub(crate) wake: Notify,
    pub(crate) unpair_req: Notify,
    pub(crate) unpair_ack: Notify,
    /// What the app asks this synth over the control stream.
    pub(crate) requests: LinkRequests,
    /// What this synth offers of `observe` (observe.rs).
    pub(crate) observe: Mutex<crate::observe::LinkObserve>,
}

impl LinkSlot {
    pub(crate) fn from_identity(id: &StoredIdentity) -> LinkSlot {
        LinkSlot::new(
            id.synth_id.clone(),
            id.synth_url.clone(),
            id.source,
            id.hub_id.clone(),
            id.synth_name.clone(),
            token_key(&id.synth_id),
            false,
        )
    }

    pub(crate) fn new(
        synth_id: String,
        url: String,
        source: LinkSource,
        hub_id: String,
        name: String,
        token_key: String,
        adhoc: bool,
    ) -> LinkSlot {
        LinkSlot {
            synth_id,
            url: RwLock::new(url),
            source: RwLock::new(source),
            hub_id: RwLock::new(hub_id),
            stored_name: RwLock::new(name),
            token_key,
            adhoc,
            legacy_token: AtomicBool::new(false),
            synth: RwLock::new(None),
            state: RwLock::new(LinkState::Connecting),
            detail: Mutex::new(String::new()),
            stopping: AtomicBool::new(false),
            running: AtomicBool::new(false),
            wake: Notify::new(),
            unpair_req: Notify::new(),
            unpair_ack: Notify::new(),
            requests: LinkRequests::default(),
            observe: Mutex::new(crate::observe::LinkObserve::default()),
        }
    }

    pub fn source(&self) -> LinkSource {
        *self.source.read().unwrap()
    }

    pub fn state(&self) -> LinkState {
        *self.state.read().unwrap()
    }

    pub fn hub_id(&self) -> String {
        self.hub_id.read().unwrap().clone()
    }

    pub fn url(&self) -> String {
        self.url.read().unwrap().clone()
    }

    pub(crate) fn stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }

    /// The name to show: from the synth's last hello_ack, else the one
    /// stored with the pairing, else the id.
    pub fn display_name(&self) -> String {
        if let Some(ack) = self.synth.read().unwrap().as_ref() {
            if !ack.synth_name.is_empty() {
                return ack.synth_name.clone();
            }
        }
        let stored = self.stored_name.read().unwrap().clone();
        if !stored.is_empty() {
            return stored;
        }
        if self.synth_id.is_empty() {
            if let Some(ack) = self.synth.read().unwrap().as_ref() {
                return ack.synth_id.clone();
            }
        }
        self.synth_id.clone()
    }

    /// The name, when one is known (not just the id).
    fn known_name(&self) -> Option<String> {
        if let Some(ack) = self.synth.read().unwrap().as_ref() {
            if !ack.synth_name.is_empty() {
                return Some(ack.synth_name.clone());
            }
        }
        Some(self.stored_name.read().unwrap().clone()).filter(|n| !n.is_empty())
    }

    /// The synth's WebSocket endpoint.
    pub fn ws_url(&self) -> String {
        crate::pairing::ws_url_of(&self.url())
    }

    pub(crate) fn detail(&self) -> String {
        self.detail.lock().unwrap().clone()
    }

    /// The picture of this link for the UI. `paused`: the owner paused
    /// the computer (every link reads as paused then).
    pub fn brief(&self, paused: bool) -> LinkBrief {
        let ack = self.synth.read().unwrap().clone();
        let state = self.state();
        LinkBrief {
            synth_id: if self.synth_id.is_empty() {
                ack.as_ref().map(|a| a.synth_id.clone()).unwrap_or_default()
            } else {
                self.synth_id.clone()
            },
            synth_name: self.known_name(),
            synth_url: self.url(),
            synth_version: ack.map(|a| a.synth_version).filter(|v| !v.is_empty()),
            // A link that needs a new token stays so while paused: the
            // owner has something to do either way.
            state: if paused && state != LinkState::NeedsRepair { LinkState::Paused } else { state },
            source: self.source(),
            detail: self.detail(),
            hub_id: Some(self.hub_id()).filter(|h| !h.is_empty()),
        }
    }
}

/// One link, as the UI shows it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LinkBrief {
    pub synth_id: String,
    pub synth_name: Option<String>,
    pub synth_url: String,
    pub synth_version: Option<String>,
    pub state: LinkState,
    pub source: LinkSource,
    /// Why the link is not up, in the core's words (diagnostics).
    pub detail: String,
    pub hub_id: Option<String>,
}

#[cfg(test)]
mod tests;
