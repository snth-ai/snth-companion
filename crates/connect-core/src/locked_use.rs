//! Preliminary macOS Locked use diagnostics.
//!
//! A native plug-in is built separately under app/macos/locked-use. Its
//! authenticated broker, protected desktop and relock lifecycle are not
//! integrated yet. Installation diagnostics
//! are not evidence that an active task is authorized to unlock the Mac.

use serde::Serialize;
#[cfg(target_os = "macos")]
use std::path::Path;

/// Reverse-DNS id used by the authorization right and plug-in bundle.
pub const AUTH_RIGHT: &str = "ai.snth.connect.LockedUse.AuthorizationPlugin.remote";
pub const PLUGIN_BUNDLE: &str = "SNTHConnectAuthorizationPlugin.bundle";
pub const PLUGIN_PATH: &str =
    "/Library/Security/SecurityAgentPlugins/SNTHConnectAuthorizationPlugin.bundle";

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub supported: bool,
    pub installed: bool,
    pub registered: bool,
    pub detail: String,
}

/// Read the local plug-in state without changing the authorization database.
pub fn status() -> Status {
    #[cfg(target_os = "macos")]
    {
        let installed = Path::new(PLUGIN_PATH).is_dir();
        let registered = authorization_rule_contains(AUTH_RIGHT);
        let detail = match (installed, registered) {
            (true, true) => "authorization plug-in is installed and registered".into(),
            (true, false) => "authorization plug-in is installed but not registered".into(),
            (false, true) => "authorization right is registered but the plug-in is missing".into(),
            (false, false) => "authorization plug-in is not installed".into(),
        };
        Status {
            supported: true,
            installed,
            registered,
            detail,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Status {
            supported: false,
            installed: false,
            registered: false,
            detail: "locked use is available only on macOS".into(),
        }
    }
}

/// Refuse locked turns until the native authorization lifecycle is implemented.
/// A bundle directory and an authorization-db entry cannot authorize a task.
pub fn authorize_active_turn() -> Result<(), String> {
    Err("locked use unavailable: native unlock is not implemented in this build".into())
}

#[cfg(target_os = "macos")]
fn authorization_rule_contains(needle: &str) -> bool {
    crate::proc::std_command("/usr/bin/security")
        .args(["authorizationdb", "read", "system.login.screensaver"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).contains(needle))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locked_turn_is_refused_until_native_authorization_exists() {
        // This must stay denied even on a machine with a matching bundle/rule.
        assert!(authorize_active_turn().is_err());
    }
}
