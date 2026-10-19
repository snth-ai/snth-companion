//! The control-stream wire shapes (protocol v1, connect.md section
//! "Control stream"). The app sends hello / presence / settings / unpair;
//! the synth answers hello_ack / unpair_ack / error.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::settings::{NetMode, Settings};

pub const PROTO: u32 = 1;

/// Build the capabilities object exactly as connect.md lists it.
/// `cua_version` and `cua_status` describe computer use.
///
/// `apps` is the fifth capability, added after the first release: a synth
/// that does not know it ignores the key. It lists only the apps this
/// computer has (never a Mac app on Windows), each with the owner's
/// switch and the actions available here.
pub fn capabilities_value(
    s: &Settings,
    cua_version: &str,
    cua_status: &str,
    apps: &dyn crate::apps::Backend,
) -> Value {
    let net_mode = match s.net.mode {
        NetMode::All => "all",
        NetMode::Ask => "ask",
        NetMode::List => "list",
    };
    let computer_mode = match s.computer.mode {
        crate::settings::ComputerMode::Ask => "ask",
        crate::settings::ComputerMode::Auto => "auto",
        crate::settings::ComputerMode::Away => "away",
    };
    let foreground = match s.computer.foreground {
        crate::settings::Foreground::Never => "never",
        crate::settings::Foreground::Idle => "idle",
        crate::settings::Foreground::Ask => "ask",
        crate::settings::Foreground::Always => "always",
    };
    let experiment = match s.computer.experiment {
        crate::settings::Experiment::Default => "default",
        crate::settings::Experiment::On => "on",
        crate::settings::Experiment::Off => "off",
    };
    let roots: Vec<Value> = s
        .files
        .roots
        .iter()
        .map(|r| json!({"path": r.path.to_string_lossy(), "write": r.write}))
        .collect();
    json!({
        "net": {"enabled": s.net.enabled, "mode": net_mode},
        "cookies": {"enabled": s.cookies.enabled},
        "computer": {
            "enabled": s.computer.enabled,
            "mode": computer_mode,
            "foreground": foreground,
            "experiment": experiment,
            "cua_version": cua_version,
            "status": cua_status,
        },
        // max_request_bytes: the longest rpc request line this app reads
        // (added after the first release; a synth that does not know it
        // sends 512 KiB chunks, one that does sizes them to it).
        "files": {"enabled": s.files.enabled, "roots": roots, "max_request_bytes": crate::rpc::MAX_REQUEST_LINE},
        "apps": crate::apps::wire_value(apps, &s.apps),
    })
}

/// The hello message.
pub fn hello(
    device_id: &str,
    device_name: &str,
    app_version: &str,
    settings_rev: u64,
    caps: Value,
) -> Value {
    json!({
        "type": "hello",
        "proto": PROTO,
        "device_id": device_id,
        "device_name": device_name,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "app_version": app_version,
        "settings_rev": settings_rev,
        "capabilities": caps,
    })
}

pub fn presence(idle_seconds: u64, screen_locked: bool, paused: bool, metered: bool) -> Value {
    json!({
        "type": "presence",
        "idle_seconds": idle_seconds,
        "screen_locked": screen_locked,
        "paused": paused,
        "metered": metered,
    })
}

pub fn settings_msg(settings_rev: u64, caps: Value) -> Value {
    json!({"type": "settings", "settings_rev": settings_rev, "capabilities": caps})
}

/// The synth's answer to hello.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct HelloAck {
    #[serde(default)]
    pub synth_id: String,
    #[serde(default)]
    pub synth_name: String,
    #[serde(default)]
    pub synth_version: String,
    /// What this synth supports beyond the first release of protocol v1
    /// ("requests", "vault"). An older synth sends none.
    #[serde(default)]
    pub features: Vec<String>,
}

impl HelloAck {
    pub fn has(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }
}

/// device.info result (connect.md rpc).
pub fn device_info(device_id: &str, device_name: &str, app_version: &str, cua_version: &str) -> Value {
    json!({
        "device_id": device_id,
        "device_name": device_name,
        "os": std::env::consts::OS,
        "os_version": os_version(),
        "arch": std::env::consts::ARCH,
        "app_version": app_version,
        "cua_version": cua_version,
        "hostname": hostname(),
    })
}

fn os_version() -> String {
    #[cfg(target_os = "macos")]
    {
        crate::proc::std_command("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }
    #[cfg(not(target_os = "macos"))]
    {
        String::new()
    }
}

pub fn hostname() -> String {
    // Windows has the name in the environment: no process needed.
    if cfg!(windows) {
        if let Some(name) = std::env::var("COMPUTERNAME").ok().filter(|n| !n.is_empty()) {
            return name;
        }
    }
    crate::proc::std_command("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .unwrap_or_else(|| "unknown".into())
}
