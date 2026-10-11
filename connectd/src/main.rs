//! connectd: the core of SNTH Connect (P0 spike, no UI).
//!
//! Keeps ONE outbound WebSocket to the user's synth and serves multiplexed
//! streams on it:
//!
//! - `tcp`: dial from this computer (the synth's proxy egress);
//! - `mcp`: computer use through the Cua Driver daemon on this desktop;
//! - `rpc`: read-only file access inside the folders the user shared;
//! - `control`: hello and presence (opened by this side).
//!
//! No inbound port is opened on this computer. Everything the synth asks
//! for is checked here; a refusal always goes back with a labeled reason.

mod files;
mod link;
mod mcp;
mod proto;
mod tcp;
mod ws_io;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use link::LinkEnd;

/// Timestamped line on stderr.
#[macro_export]
macro_rules! logln {
    ($($arg:tt)*) => {{
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = now.as_secs() % 86_400;
        eprintln!(
            "{:02}:{:02}:{:02}.{:03}Z [connectd] {}",
            secs / 3600,
            (secs / 60) % 60,
            secs % 60,
            now.subsec_millis(),
            format_args!($($arg)*)
        );
    }};
}

pub struct Config {
    pub url: String,
    pub token: String,
    /// Local network access: allow dialing private, loopback and
    /// link-local addresses.
    pub allow_private: bool,
    /// Shared folders, canonical. Empty = files are off.
    pub roots: Vec<PathBuf>,
    pub device_id: String,
    pub hostname: String,
    pub cua_bin: String,
    pub no_computer: bool,
    /// When set, only these MCP tools may be called through the link.
    pub mcp_tools: Option<Vec<String>>,
}

const USAGE: &str = "\
connectd: SNTH Connect core (P0 spike)

USAGE:
  connectd run --url <ws[s]://host/api/connect/ws> --token <token> [options]

OPTIONS:
  --url <url>          Synth endpoint. ws:// is accepted for loopback hosts only.
  --token <token>      Pairing token. Also read from CONNECTD_TOKEN (preferred:
                       an argument is visible in the process list).
  --allow-private      Local network access: let the synth reach private,
                       loopback and link-local addresses through this computer.
  --root <dir>         Share a folder with the synth (read-only). Repeatable.
                       No --root = files are off.
  --device-id <id>     Device id sent in hello (default: derived from hostname).
  --cua-bin <path>     Cua Driver binary (default: cua-driver from PATH).
  --no-computer        Turn computer use off.
  --mcp-tool <name>    Allow only this Cua tool through the link. Repeatable.
                       Not given = every tool the driver's own policy allows.
";

fn parse_args() -> Result<Config, String> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("run") => {}
        Some("--help") | Some("-h") | Some("help") | None => {
            print!("{USAGE}");
            std::process::exit(0);
        }
        Some("--version") | Some("version") => {
            println!("connectd {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        Some(other) => return Err(format!("unknown command {other:?} (try: connectd run ...)")),
    }

    let mut url = None;
    let mut token = std::env::var("CONNECTD_TOKEN").ok().filter(|t| !t.is_empty());
    let mut allow_private = false;
    let mut roots = Vec::new();
    let mut device_id = None;
    let mut cua_bin = String::from("cua-driver");
    let mut no_computer = false;
    let mut mcp_tools: Option<Vec<String>> = None;

    while let Some(flag) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match flag.as_str() {
            "--url" => url = Some(value("--url")?),
            "--token" => token = Some(value("--token")?),
            "--allow-private" => allow_private = true,
            "--root" => roots.push(value("--root")?),
            "--device-id" => device_id = Some(value("--device-id")?),
            "--cua-bin" => cua_bin = value("--cua-bin")?,
            "--no-computer" => no_computer = true,
            "--mcp-tool" => mcp_tools.get_or_insert_with(Vec::new).push(value("--mcp-tool")?),
            other => return Err(format!("unknown option {other:?}")),
        }
    }

    let url = url.ok_or("--url is required")?;
    let token = token.ok_or("--token (or CONNECTD_TOKEN) is required")?;
    check_url(&url)?;

    // Roots are canonicalized once, here: the per-request check compares
    // canonical paths with canonical roots.
    let mut canon_roots = Vec::new();
    for r in roots {
        let p = std::fs::canonicalize(&r).map_err(|e| format!("--root {r}: {e}"))?;
        if !p.is_dir() {
            return Err(format!("--root {r}: not a directory"));
        }
        canon_roots.push(p);
    }

    let hostname = hostname();
    let device_id = device_id.unwrap_or_else(|| {
        // TODO(P1): a random id generated once and kept in the app's
        // config dir. The hostname is stable enough for the spike.
        let clean: String = hostname
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '-' })
            .take(100)
            .collect();
        format!("dev-{clean}")
    });
    if device_id.is_empty()
        || device_id.len() > 128
        || !device_id.chars().all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c))
    {
        return Err("--device-id may hold only letters, digits and . _ : - (max 128)".into());
    }

    Ok(Config {
        url,
        token,
        allow_private,
        roots: canon_roots,
        device_id,
        hostname,
        cua_bin,
        no_computer,
        mcp_tools,
    })
}

/// ws:// sends the token in the clear, so it is accepted only when the
/// synth is on this machine (local testing). Everything else needs wss://.
fn check_url(url: &str) -> Result<(), String> {
    if url.starts_with("wss://") {
        return Ok(());
    }
    let Some(rest) = url.strip_prefix("ws://") else {
        return Err("--url must start with wss:// (or ws:// for a loopback host)".into());
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else {
        authority.rsplit_once(':').map(|(h, _)| h).unwrap_or(authority)
    };
    let loopback = host == "localhost"
        || host.parse::<std::net::IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false);
    if loopback {
        Ok(())
    } else {
        Err(format!("ws:// is allowed only for loopback hosts, {host:?} is not one: use wss://"))
    }
}

fn hostname() -> String {
    let from_cmd = std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    from_cmd
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .unwrap_or_else(|| "unknown".into())
}

fn main() {
    let cfg = match parse_args() {
        Ok(c) => Arc::new(c),
        Err(e) => {
            eprintln!("connectd: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(run(cfg));
}

async fn run(cfg: Arc<Config>) {
    logln!(
        "starting: device {}, local network {}, {} shared folder(s), computer use {}",
        cfg.device_id,
        if cfg.allow_private { "ALLOWED" } else { "blocked" },
        cfg.roots.len(),
        if cfg.no_computer { "off" } else { "on when the Cua daemon runs" }
    );

    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        wait_for_signal().await;
        let _ = stop_tx.send(true);
    });

    // Reconnect with exponential backoff: 1 s doubling to 30 s, with
    // jitter so a fleet of apps does not redial a restarted synth in step.
    // A link that got as far as the hello ack resets the backoff.
    const BACKOFF_MIN: Duration = Duration::from_secs(1);
    const BACKOFF_MAX: Duration = Duration::from_secs(30);
    let mut backoff = BACKOFF_MIN;
    loop {
        match link::run(cfg.clone(), &mut stop_rx).await {
            LinkEnd::Shutdown => break,
            LinkEnd::Dropped(why) => {
                logln!("link dropped: {why}");
                backoff = BACKOFF_MIN;
            }
            LinkEnd::Failed(why) => logln!("link failed: {why}"),
        }
        let wait = jitter(backoff);
        logln!("reconnecting in {:.1} s", wait.as_secs_f64());
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = stop_rx.changed() => break,
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
    logln!("stopped");
}

/// `d` scaled by a factor in [0.75, 1.25).
fn jitter(d: Duration) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|t| t.subsec_nanos())
        .unwrap_or(0);
    d.mul_f64(0.75 + f64::from(nanos % 1000) / 2000.0)
}

async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        let mut int = signal(SignalKind::interrupt()).expect("SIGINT handler");
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::check_url;

    #[test]
    fn plain_ws_only_for_loopback() {
        assert!(check_url("wss://synth.example/api/connect/ws").is_ok());
        assert!(check_url("ws://127.0.0.1:18080/api/connect/ws").is_ok());
        assert!(check_url("ws://localhost:18080/api/connect/ws").is_ok());
        assert!(check_url("ws://[::1]:18080/api/connect/ws").is_ok());
        assert!(check_url("ws://synth.example/api/connect/ws").is_err());
        assert!(check_url("ws://10.0.0.5:8080/api/connect/ws").is_err());
        assert!(check_url("http://127.0.0.1/").is_err());
    }
}
