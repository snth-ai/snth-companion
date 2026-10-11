//! Cua Driver: detection, our own private daemon, settings -> Cua policy,
//! and the gate predicates the mcp pipe uses.
//!
//! The app runs its OWN daemon instance on a private socket and never
//! touches the user's own daemon (findings 5.2). Our instance runs
//! embedded (`CUA_DRIVER_EMBEDDED=1`), which keeps it in the host app's
//! TCC responsibility chain and never prompts, with telemetry off.
//!
//! Install and `permissions grant` are built here as commands the UI
//! runs; this module never runs them. `permissions status` is read-only
//! and is run on request.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::process::{Child, Command};

use crate::settings::{ComputerMode, Foreground, Settings};

/// The Cua Driver version the app pins and installs.
pub const PINNED_VERSION: &str = "0.33.1";

/// Our host bundle id, passed as the embedded-mode advisory label.
pub const HOST_BUNDLE_ID: &str = "ai.snth.connect";

/// One-line status for the hello `computer.status` field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Ok,
    NotInstalled,
    NoPermissions,
    DaemonDown,
    Other(String),
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::Ok => "ok".into(),
            Status::NotInstalled => "not_installed".into(),
            Status::NoPermissions => "no_permissions".into(),
            Status::DaemonDown => "daemon_down".into(),
            Status::Other(s) => s.clone(),
        }
    }
}

/// What `detect` found.
#[derive(Debug, Clone)]
pub struct Detect {
    pub installed: bool,
    pub version: Option<String>,
    pub path: String,
}

/// The environment every driver process the app starts gets. Telemetry and
/// the driver's own update check are always off. With a private copy
/// (`home` set) the driver also keeps its state in the app's data dir
/// instead of `~/.cua-driver`, which belongs to a driver the owner may
/// have installed for themselves.
#[derive(Debug, Clone, Default)]
pub struct DriverEnv {
    pub home: Option<PathBuf>,
}

impl DriverEnv {
    pub fn apply(&self, cmd: &mut Command) {
        telemetry_off(cmd);
        cmd.env("CUA_DRIVER_RS_UPDATE_CHECK", "0");
        if let Some(home) = &self.home {
            cmd.env("CUA_DRIVER_RS_HOME", home);
        }
    }
}

/// Run `<bin> --version` to see whether the driver is installed and which
/// version. Read-only.
pub async fn detect(bin: &str, env: &DriverEnv) -> Detect {
    let mut cmd = Command::new(bin);
    env.apply(&mut cmd);
    let out = cmd
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout);
            // "cua-driver 0.33.1"
            let version = text.split_whitespace().nth(1).map(|s| s.to_string());
            Detect {
                installed: true,
                version,
                path: bin.to_string(),
            }
        }
        _ => Detect {
            installed: false,
            version: None,
            path: bin.to_string(),
        },
    }
}

/// The argv the UI runs to install or update to the pinned version. We do
/// NOT run it. `cua-driver update --apply` uses the canonical installer.
pub fn install_argv(bin: &str) -> Vec<String> {
    vec![bin.to_string(), "update".into(), "--apply".into()]
}

/// The argv the UI runs to request Accessibility + Screen Recording as the
/// host app. We do NOT run it (it raises OS dialogs on the owner's
/// screen).
pub fn permissions_grant_argv(bin: &str) -> Vec<String> {
    vec![bin.to_string(), "permissions".into(), "grant".into()]
}

/// The structured answer of `cua-driver permissions status --json`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Permissions {
    #[serde(default)]
    pub accessibility: bool,
    #[serde(default)]
    pub screen_recording: bool,
}

/// Read `permissions status --json` from a running daemon. Read-only, no
/// prompt. `socket` targets our private daemon when we run one.
pub async fn permissions_status(
    bin: &str,
    socket: Option<&str>,
    env: &DriverEnv,
) -> std::io::Result<Permissions> {
    let mut cmd = Command::new(bin);
    env.apply(&mut cmd);
    cmd.args(["permissions", "status", "--json"]);
    if let Some(s) = socket {
        cmd.args(["--socket", s]);
    }
    let out = cmd
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await?;
    serde_json::from_slice(&out.stdout)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Compile the shared-folder roots into a Cua capability manifest (YAML).
/// Files map to the manifest `resources.files` read/write dirs; the owner
/// app blocklist is NOT a manifest concept (deny-by-list), it is enforced
/// by our pipe filter, so the manifest here narrows file resources only.
pub fn compile_manifest(settings: &Settings) -> String {
    let mut y = String::from("version: 3\n");
    y.push_str("resources:\n  files:\n");
    let read: Vec<&PathBuf> = settings.files.roots.iter().map(|r| &r.path).collect();
    let write: Vec<&PathBuf> = settings
        .files
        .roots
        .iter()
        .filter(|r| r.write)
        .map(|r| &r.path)
        .collect();
    if read.is_empty() {
        y.push_str("    read: []\n");
    } else {
        y.push_str("    read:\n");
        for p in read {
            y.push_str(&format!("      - dir: {}\n        recursive: true\n", p.display()));
        }
    }
    if write.is_empty() {
        y.push_str("    write: []\n");
    } else {
        y.push_str("    write:\n");
        for p in write {
            y.push_str(&format!("      - dir: {}\n        recursive: true\n", p.display()));
        }
    }
    y
}

/// A private Cua daemon we started, owned by our process.
pub struct PrivateDaemon {
    child: Child,
    pub socket: String,
    pub pid_file: PathBuf,
    pub pid: Option<u32>,
    dir: PathBuf,
}

impl PrivateDaemon {
    /// Start `cua-driver serve --embedded` on a private socket in its own
    /// temp dir, telemetry off. Waits up to ~5 s for the socket to appear.
    ///
    /// SAFETY NOTE: embedded mode never prompts and never relaunches the
    /// standalone app; it stays in this process's TCC chain. It does not
    /// touch the user's own daemon (a different socket).
    pub async fn start(
        bin: &str,
        _settings: &Settings,
        env: &DriverEnv,
    ) -> std::io::Result<PrivateDaemon> {
        let dir = std::env::temp_dir().join(format!(
            "connect-cua-{}-{}",
            std::process::id(),
            fastish_suffix()
        ));
        std::fs::create_dir_all(&dir)?;
        let socket = dir.join("cua.sock");
        let pid_file = dir.join("cua.pid");
        let socket_s = socket.to_string_lossy().to_string();

        let mut cmd = Command::new(bin);
        cmd.args([
            "serve",
            "--embedded",
            "--host-bundle-id",
            HOST_BUNDLE_ID,
            "--socket",
            &socket_s,
            "--pid-file",
            &pid_file.to_string_lossy(),
        ]);
        env.apply(&mut cmd);
        cmd.env("CUA_DRIVER_EMBEDDED", "1")
            .env("CUA_DRIVER_HOST_BUNDLE_ID", HOST_BUNDLE_ID)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let child = cmd.spawn()?;
        let pid = child.id();

        // Wait for the socket (or the pid file) to show up.
        for _ in 0..50 {
            if socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(PrivateDaemon {
            child,
            socket: socket_s,
            pid_file,
            pid,
            dir,
        })
    }

    /// Stop OUR daemon by its own pid (kill_on_drop also covers a crash).
    pub async fn stop(mut self, bin: &str, env: &DriverEnv) {
        // Polite stop on our socket first.
        let mut cmd = Command::new(bin);
        env.apply(&mut cmd);
        let _ = cmd
            .args(["stop", "--socket", &self.socket])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await;
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(3), self.child.wait()).await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Telemetry off for our instance (findings 5: default-on otherwise).
fn telemetry_off(cmd: &mut Command) {
    cmd.env("DO_NOT_TRACK", "1")
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "0")
        .env("CUA_TELEMETRY_ENABLED", "0");
}

/// The argv to pipe one mcp stream into a daemon: `mcp --embedded --socket
/// <s>` for our daemon, or `mcp --socket <s>` when attaching to the user's
/// daemon (read-only testing / fallback).
pub fn mcp_argv(bin: &str, socket: &str, embedded: bool) -> Vec<String> {
    let mut v = vec![bin.to_string(), "mcp".into()];
    if embedded {
        v.push("--embedded".into());
    }
    v.push("--socket".into());
    v.push(socket.to_string());
    v
}

fn fastish_suffix() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0)
}

// --- gate predicates (pure; used by the mcp pipe filter) ---

/// Does this parsed JSON-RPC message ask for foreground delivery? Scans
/// `params.arguments` (and, defensively, any nested object) for
/// `"delivery_mode":"foreground"`.
pub fn wants_foreground(msg: &serde_json::Value) -> bool {
    fn scan(v: &serde_json::Value) -> bool {
        match v {
            serde_json::Value::Object(m) => {
                if m.get("delivery_mode").and_then(|x| x.as_str()) == Some("foreground") {
                    return true;
                }
                m.values().any(scan)
            }
            serde_json::Value::Array(a) => a.iter().any(scan),
            _ => false,
        }
    }
    scan(msg)
}

/// The tool name of a `tools/call`, if this is one.
pub fn tool_name(msg: &serde_json::Value) -> Option<&str> {
    if msg.get("method").and_then(|m| m.as_str()) != Some("tools/call") {
        return None;
    }
    msg.get("params").and_then(|p| p.get("name")).and_then(|n| n.as_str())
}

/// Any app identifier a `tools/call`'s arguments name (bundle id or app
/// name), lowercased, so the blocklist can be matched. Empty when the
/// call does not name an app.
pub fn named_apps(msg: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(args) = msg.get("params").and_then(|p| p.get("arguments")) {
        for key in ["bundle_id", "app", "app_name", "name"] {
            if let Some(v) = args.get(key).and_then(|x| x.as_str()) {
                // "name" is also the tool arg for launch_app's app name.
                out.push(v.to_ascii_lowercase());
            }
        }
    }
    out
}

/// Does `mode` + idle allow a call right now (the `away` gate)?
pub fn away_allows(mode: ComputerMode, away_idle_minutes: u64, idle_seconds: u64) -> bool {
    match mode {
        ComputerMode::Away => idle_seconds >= away_idle_minutes.saturating_mul(60),
        _ => true,
    }
}

/// Does the foreground setting + idle allow a foreground-delivery call
/// (the `ask` case is handled by the caller, which must consult the
/// approver)? Returns None when an ask is required.
pub fn foreground_decision(
    fg: Foreground,
    away_idle_minutes: u64,
    idle_seconds: u64,
) -> Option<bool> {
    match fg {
        Foreground::Never => Some(false),
        Foreground::Always => Some(true),
        Foreground::Idle => Some(idle_seconds >= away_idle_minutes.saturating_mul(60)),
        Foreground::Ask => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Preset, Root};

    #[test]
    fn foreground_scan() {
        let m: serde_json::Value = serde_json::from_str(
            r#"{"method":"tools/call","params":{"name":"click","arguments":{"delivery_mode":"foreground","window_id":3}}}"#,
        )
        .unwrap();
        assert!(wants_foreground(&m));
        let bg: serde_json::Value = serde_json::from_str(
            r#"{"method":"tools/call","params":{"name":"click","arguments":{"delivery_mode":"background"}}}"#,
        )
        .unwrap();
        assert!(!wants_foreground(&bg));
    }

    #[test]
    fn apps_and_tool() {
        let m: serde_json::Value = serde_json::from_str(
            r#"{"method":"tools/call","params":{"name":"launch_app","arguments":{"bundle_id":"com.apple.Mail"}}}"#,
        )
        .unwrap();
        assert_eq!(tool_name(&m), Some("launch_app"));
        assert_eq!(named_apps(&m), vec!["com.apple.mail".to_string()]);
    }

    #[test]
    fn gates() {
        assert!(!away_allows(ComputerMode::Away, 5, 60)); // 1 min < 5 min
        assert!(away_allows(ComputerMode::Away, 5, 600)); // 10 min
        assert!(away_allows(ComputerMode::Auto, 5, 0));

        assert_eq!(foreground_decision(Foreground::Never, 5, 10_000), Some(false));
        assert_eq!(foreground_decision(Foreground::Always, 5, 0), Some(true));
        assert_eq!(foreground_decision(Foreground::Idle, 5, 10), Some(false));
        assert_eq!(foreground_decision(Foreground::Idle, 5, 600), Some(true));
        assert_eq!(foreground_decision(Foreground::Ask, 5, 0), None);
    }

    #[test]
    fn manifest_lists_roots() {
        let mut s = Settings::from_preset(Preset::Balanced, vec![]);
        s.files.roots = vec![
            Root { path: PathBuf::from("/Users/x/Desktop"), write: false },
            Root { path: PathBuf::from("/Users/x/Downloads"), write: true },
        ];
        let y = compile_manifest(&s);
        assert!(y.contains("/Users/x/Desktop"));
        assert!(y.contains("/Users/x/Downloads"));
        assert!(y.contains("write:"));
    }

    #[test]
    fn argv_builders() {
        assert_eq!(install_argv("cua-driver"), ["cua-driver", "update", "--apply"]);
        assert_eq!(
            mcp_argv("cua-driver", "/tmp/s.sock", true),
            ["cua-driver", "mcp", "--embedded", "--socket", "/tmp/s.sock"]
        );
        assert_eq!(
            mcp_argv("cua-driver", "/tmp/s.sock", false),
            ["cua-driver", "mcp", "--socket", "/tmp/s.sock"]
        );
    }
}
