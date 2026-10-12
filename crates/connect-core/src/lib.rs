//! SNTH Connect core.
//!
//! A library, never a UI. It keeps ONE outbound WebSocket to the owner's
//! synth and gives it five capabilities under the owner's policy: internet
//! through this computer (`net`), browser cookies per domain (`cookies`),
//! computer use through Cua Driver (`computer`), files in chosen folders
//! (`files`), and the owner's own apps (`apps`). The protocol is v1 as written in connect.md; the
//! synth side (Go) is the peer.
//!
//! A host (the Tauri shell, or the `connectd` CLI) builds one [`Core`]
//! with an [`Approver`], a [`CookieSource`], a [`SecretStore`] and a
//! [`HubClient`], then drives it with commands and watches its events.

#[macro_export]
macro_rules! logln {
    ($($arg:tt)*) => {{
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = now.as_secs() % 86_400;
        eprintln!(
            "{:02}:{:02}:{:02}.{:03}Z [connect-core] {}",
            secs / 3600,
            (secs / 60) % 60,
            secs % 60,
            now.subsec_millis(),
            format_args!($($arg)*)
        );
    }};
}

pub mod apps;
pub mod cookies;
pub mod core;
pub mod cua;
pub mod cua_install;
pub mod files;
pub mod grant;
pub mod journal;
pub mod link;
pub mod mcp;
pub mod pairing;
pub mod presence;
pub mod proc;
pub mod proto;
pub mod rpc;
pub mod secret;
pub mod settings;
pub mod tcc;
pub mod tcp;
pub mod wire;
pub mod ws_io;

pub use crate::core::{
    Core, CoreConfig, CoreEvent, CuaAttach, LinkBrief, LinkState, Shared, Snapshot,
};
pub use cua_install::{FailCode, InstallState};
pub use cookies::{CookieError, CookieRead, CookieSource, KeySource, MultiReader, WireCookie};
pub use grant::{Approval, ApprovalRequest, Approver, Duration_, GrantStore};
pub use journal::{Entry, Journal};
pub use pairing::{ClaimError, ClaimResult, HubClient, Pending, UreqHubClient};
pub use proto::Refusal;
pub use secret::{FileSecretStore, KeyringSecretStore, MemSecretStore, SecretStore};
pub use settings::{Preset, Root, Settings};
