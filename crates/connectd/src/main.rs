//! connectd: a headless CLI over `connect-core`, for tests and debugging.
//!
//! It is NOT the product (that is the Tauri shell). It gives the core a
//! terminal approver (or a scripted `--approve` / `--deny-all` rule set),
//! a file secret store and a file journal, and drives one link.
//!
//! Subcommands:
//!   connectd pair   --hub <url> --code <6-digit> [--secret-file <p>]
//!   connectd run    --url <ws[s]://.../api/connect/ws> --token <t> [options]
//!   connectd status   [--settings <p>]
//!   connectd settings [--settings <p>]
//!   connectd preset <careful|balanced|full> [--settings <p>]
//!   connectd pause | resume | unpair           (REPL commands under `run`)
//!   connectd make-cookie-fixture --db <p> --key <pw> --domain <d> --name <n> --value <v>
//!   connectd selftest cua [--root <dir>] [--keep]   (see selftest.rs)
//!   connectd selftest apps [--touch]                (see selftest.rs)
//!   connectd selftest host [--helper <p>] [--system] (see selftest_host.rs)
//!   connectd browser-manifest --dir <NativeMessagingHosts dir> --helper <p>
//!
//! `run` reads command lines from stdin (pause / resume / unpair /
//! preset <name> / addroot <path>[:rw] / apps ... / browsers / link-browser <id> /
//! unlink-browser <id> / vault-list / vault-forget <site> / revoke <cap> <scope> /
//! status / quit) so a test script
//! can drive it; approvals come from the chosen approver, never stdin,
//! when a rule set is given.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use connect_core::grant::{Approval, ApprovalFuture, ApprovalRequest, Approver};
use connect_core::{
    cookies, Core, CoreConfig, CuaAttach, FileSecretStore, MultiReader, Preset, SecretStore,
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::watch;

mod selftest;
mod selftest_host;

const TOKEN_KEY: &str = "pairing_token";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().cloned().unwrap_or_default();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime");
    let code = rt.block_on(async move {
        match cmd.as_str() {
            "run" => cmd_run(&args[1..]).await,
            "pair" => cmd_pair(&args[1..]).await,
            "status" => cmd_status(&args[1..]).await,
            "settings" => cmd_settings(&args[1..]).await,
            "preset" => cmd_preset(&args[1..]).await,
            "make-cookie-fixture" => cmd_make_fixture(&args[1..]),
            "browser-manifest" => cmd_browser_manifest(&args[1..]),
            "selftest" => selftest::run(&args[1..]).await,
            "-h" | "--help" | "help" | "" => {
                print_usage();
                0
            }
            other => {
                eprintln!("connectd: unknown command {other:?}");
                print_usage();
                2
            }
        }
    });
    std::process::exit(code);
}

fn print_usage() {
    eprint!(
        "connectd: headless CLI over connect-core\n\
         \n\
         USAGE:\n\
         \u{20}  connectd run --url <ws[s]://host/api/connect/ws> --token <t> [options]\n\
         \u{20}  connectd pair --hub <url> --code <6 digits> [--secret-file <p>]\n\
         \u{20}  connectd status|settings [--settings <p>]\n\
         \u{20}  connectd preset <careful|balanced|full> [--settings <p>]\n\
         \u{20}  connectd make-cookie-fixture --db <p> --key <pw> --domain <d> --name <n> --value <v>\n\
         \u{20}  connectd selftest cua [--root <dir>] [--keep]\n\
         \u{20}  connectd selftest apps [--touch]\n\
         \u{20}  connectd selftest host [--helper <path>] [--system]\n\
         \u{20}  connectd browser-manifest --dir <NativeMessagingHosts dir> --helper <path>\n\
         \n\
         run options:\n\
         \u{20}  --settings <p>        settings.json path (default: in-memory)\n\
         \u{20}  --secret-file <p>     token store path (default: temp)\n\
         \u{20}  --journal <p>         journal.jsonl path\n\
         \u{20}  --preset <name>       apply a preset before linking\n\
         \u{20}  --root <path>[:rw]    share a folder (append :rw for write)\n\
         \u{20}  --cua-attach <user|private>   which Cua daemon (default: user)\n\
         \u{20}  --apps <none|mac|windows|system>  the owner's apps: none (default), an\n\
         \u{20}                        in-memory stand-in for a Mac or a Windows computer,\n\
         \u{20}                        or the real apps of this computer\n\
         \u{20}  --approve <spec>      auto-allow rule, repeatable (cap | cap:scope | *)\n\
         \u{20}  --deny-all           auto-deny every ask\n\
         \u{20}  --cookie-chromium <name>=<db>  a Chromium cookie DB fixture\n\
         \u{20}  --cookie-firefox <db>          a Firefox cookie DB fixture\n\
         \u{20}  --cookie-key <pw>              Safe Storage password for the fixtures\n\
         \u{20}  --browser-endpoint <path>      serve the SNTH Vault extension's helper on this\n\
         \u{20}                        endpoint (a socket path; a pipe name on Windows)\n\
         \u{20}  --cookies-extension-first      ask a linked browser before reading browser files\n"
    );
}

// --- approvers ---

/// Auto-decide from rules (test mode). A spec matches by capability or
/// `capability:scope`; `*` matches everything. A match allows (always);
/// no match denies.
struct RuleApprover {
    allow: Vec<String>,
    deny_all: bool,
}

impl Approver for RuleApprover {
    fn ask(&self, req: ApprovalRequest) -> ApprovalFuture {
        let decided = if self.deny_all {
            Approval::deny()
        } else {
            let cap = req.capability.clone();
            let full = format!("{}:{}", req.capability, req.scope);
            let hit = self
                .allow
                .iter()
                .any(|r| r == "*" || r == &cap || r == &full);
            // Log the ask so a script can see it happened (untrusted
            // reason flagged).
            eprintln!(
                "[approver] ask cap={} scope={} synth-says={:?} -> {}",
                req.capability,
                req.scope,
                req.reason,
                if hit { "allow" } else { "deny" }
            );
            if hit {
                Approval::always()
            } else {
                Approval::deny()
            }
        };
        Box::pin(async move { decided })
    }
}

/// Interactive: print the ask and read y/n from stdin.
struct TerminalApprover;

impl Approver for TerminalApprover {
    fn ask(&self, req: ApprovalRequest) -> ApprovalFuture {
        Box::pin(async move {
            println!(
                "\nApproval needed: {} for {}\n  (the synth says: {:?})",
                req.capability, req.scope, req.reason
            );
            // The action itself, in full (a message's recipient and text...).
            for (key, value) in &req.details {
                println!("  {key}: {value}");
            }
            println!("  {} [y / a=always / N]: ", req.summary);
            let line = tokio::task::spawn_blocking(|| {
                let mut s = String::new();
                let _ = std::io::stdin().read_line(&mut s);
                s
            })
            .await
            .unwrap_or_default();
            match line.trim().to_ascii_lowercase().as_str() {
                "y" | "yes" | "once" => Approval::once(),
                "a" | "always" => Approval::always(),
                _ => Approval::deny(),
            }
        })
    }
}

// --- arg helpers ---

struct Args {
    map: Vec<(String, String)>,
    flags: Vec<String>,
    positional: Vec<String>,
}

fn parse(args: &[String], value_keys: &[&str], flag_keys: &[&str]) -> Args {
    let mut map = Vec::new();
    let mut flags = Vec::new();
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if flag_keys.contains(&a.as_str()) {
            flags.push(a.clone());
            i += 1;
        } else if value_keys.contains(&a.as_str()) {
            if i + 1 < args.len() {
                map.push((a.clone(), args[i + 1].clone()));
                i += 2;
            } else {
                i += 1;
            }
        } else {
            positional.push(a.clone());
            i += 1;
        }
    }
    Args { map, flags, positional }
}

impl Args {
    fn get(&self, k: &str) -> Option<&str> {
        self.map.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.as_str())
    }
    fn all(&self, k: &str) -> Vec<String> {
        self.map.iter().filter(|(kk, _)| kk == k).map(|(_, v)| v.clone()).collect()
    }
    fn has(&self, k: &str) -> bool {
        self.flags.iter().any(|f| f == k)
    }
}

fn preset_from(name: &str) -> Option<Preset> {
    match name {
        "careful" => Some(Preset::Careful),
        "balanced" => Some(Preset::Balanced),
        "full" => Some(Preset::Full),
        _ => None,
    }
}

fn build_core(a: &Args, approver: Arc<dyn Approver>) -> Core {
    let settings_path = a.get("--settings").map(PathBuf::from);
    let journal_path = a.get("--journal").map(PathBuf::from);
    let secret_path = a
        .get("--secret-file")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("connectd-secrets-{}.json", std::process::id())));
    let secret: Arc<dyn SecretStore> = Arc::new(FileSecretStore::new(secret_path));

    // Token comes from --token (test) and is stored before Core reads it.
    if let Some(tok) = a.get("--token") {
        let _ = secret.set(TOKEN_KEY, tok);
    }

    // Cookie fixtures.
    let mut chromium = Vec::new();
    for spec in a.all("--cookie-chromium") {
        if let Some((name, db)) = spec.split_once('=') {
            chromium.push((name.to_string(), PathBuf::from(db)));
        }
    }
    let firefox: Vec<PathBuf> = a.all("--cookie-firefox").into_iter().map(PathBuf::from).collect();
    let keys: Arc<dyn connect_core::KeySource> = match a.get("--cookie-key") {
        Some(pw) => Arc::new(cookies::fixtures::FixedKey(pw.as_bytes().to_vec())),
        None => Arc::new(cookies::fixtures::NoKey),
    };
    let cookie_source = Arc::new(MultiReader { chromium, firefox, keys });

    let attach = match a.get("--cua-attach") {
        Some("private") => CuaAttach::Private,
        _ => CuaAttach::User,
    };

    // The owner's apps. A debugging tool must not reach the real ones by
    // accident, so the default offers none; `--apps system` is explicit.
    let apps: Arc<dyn connect_core::apps::Backend> = match a.get("--apps") {
        Some("system") => connect_core::apps::system_backend(None),
        Some(kind) => match connect_core::apps::stub::StubBackend::named(kind) {
            Some(stub) => Arc::new(stub),
            None => {
                eprintln!("connectd: unknown --apps value {kind:?} (none, mac, windows, system)");
                std::process::exit(2);
            }
        },
        None => Arc::new(connect_core::apps::stub::StubBackend::offering(&[])),
    };

    let core = Core::new(CoreConfig {
        settings_path,
        journal_path,
        approver,
        cookies: cookie_source,
        apps,
        secret,
        hub: Arc::new(connect_core::UreqHubClient),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        cua_bin: a.get("--cua-bin").unwrap_or("cua-driver").to_string(),
        cua_private_root: None,
        cua_attach: attach,
    });

    if let Some(p) = a.get("--preset").and_then(preset_from) {
        core.apply_preset(p);
    }
    for r in a.all("--root") {
        let (path, write) = match r.strip_suffix(":rw") {
            Some(p) => (p.to_string(), true),
            None => (r.trim_end_matches(":ro").to_string(), false),
        };
        core.add_root(PathBuf::from(path), write);
    }
    core
}

// --- commands ---

async fn cmd_run(args: &[String]) -> i32 {
    let a = parse(
        args,
        &[
            "--url", "--token", "--settings", "--secret-file", "--journal", "--preset",
            "--root", "--cua-attach", "--cua-bin", "--approve", "--cookie-chromium",
            "--cookie-firefox", "--cookie-key", "--apps", "--browser-endpoint",
        ],
        &["--deny-all", "--allow-local", "--cookies-extension-first"],
    );
    let url = match a.get("--url") {
        Some(u) => u.to_string(),
        None => {
            eprintln!("connectd run: --url is required");
            return 2;
        }
    };
    let rules = a.all("--approve");
    let approver: Arc<dyn Approver> = if a.has("--deny-all") || !rules.is_empty() {
        Arc::new(RuleApprover { allow: rules, deny_all: a.has("--deny-all") })
    } else {
        Arc::new(TerminalApprover)
    };
    let scripted = a.has("--deny-all") || !a.all("--approve").is_empty();

    let core = build_core(&a, approver);
    if a.has("--allow-local") {
        core.mutate_settings(|s| s.net.allow_local_network = true);
    }
    if a.has("--cookies-extension-first") {
        core.set_cookies_extension_first(true);
    }
    let (stop_tx, stop_rx) = watch::channel(false);

    // The browser link: the extension's helper connects here. Only when
    // asked for, and only on the endpoint given: a debugging tool never
    // takes the endpoint of an installed app.
    if let Some(path) = a.get("--browser-endpoint") {
        let endpoint = connect_host::endpoint::Endpoint::at(path);
        let (browser_core, browser_stop) = (core.clone(), stop_rx.clone());
        tokio::spawn(async move {
            if let Err(e) = browser_core.run_browser_link(&endpoint, browser_stop).await {
                eprintln!("connectd: browser link on {endpoint}: {e}");
            }
        });
    }

    let link_core = core.clone();
    let link = tokio::spawn(async move {
        link_core.run_link(Some(url), stop_rx).await;
    });

    // Signal handler: SIGINT and (Unix) SIGTERM.
    let stop_for_sig = stop_tx.clone();
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut term = signal(SignalKind::terminate()).expect("SIGTERM");
            let mut int = signal(SignalKind::interrupt()).expect("SIGINT");
            tokio::select! {
                _ = term.recv() => {}
                _ = int.recv() => {}
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        let _ = stop_for_sig.send(true);
    });

    println!("READY");

    // In scripted mode, read command lines from stdin so a test can drive
    // pause / resume / unpair / preset / addroot / status.
    if scripted {
        let stdin = BufReader::new(tokio::io::stdin());
        let mut lines = stdin.lines();
        loop {
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(l)) => {
                        if run_command(&core, &l).await {
                            let _ = stop_tx.send(true);
                            break;
                        }
                    }
                    // stdin closed: keep the link up until a signal.
                    Ok(None) => {
                        let mut rx = stop_tx.subscribe();
                        let _ = rx.changed().await;
                        break;
                    }
                    Err(_) => break,
                },
                _ = async {
                    let mut rx = stop_tx.subscribe();
                    let _ = rx.changed().await;
                } => break,
            }
        }
    } else {
        let mut rx = stop_tx.subscribe();
        let _ = rx.changed().await;
    }
    let _ = stop_tx.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(5), link).await;
    0
}

/// Returns true if the loop should stop.
async fn run_command(core: &Core, line: &str) -> bool {
    let line = line.trim();
    let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
    match cmd {
        "" => {}
        "pause" => {
            core.pause().await;
            println!("OK paused");
        }
        "resume" => {
            core.resume();
            println!("OK resumed");
        }
        "unpair" => {
            core.unpair().await;
            println!("OK unpaired");
        }
        "preset" => {
            if let Some(p) = preset_from(rest.trim()) {
                let rev = core.apply_preset(p);
                println!("OK preset {} rev={rev}", rest.trim());
            } else {
                println!("ERR unknown preset {:?}", rest.trim());
            }
        }
        "addroot" => {
            let (path, write) = match rest.trim().strip_suffix(":rw") {
                Some(p) => (p.to_string(), true),
                None => (rest.trim().to_string(), false),
            };
            let rev = core.add_root(PathBuf::from(path), write);
            println!("OK addroot rev={rev}");
        }
        // apps on <ask|ask_new|auto> | apps off | apps app <id> <on|off>
        "apps" => {
            use connect_core::settings::AppsMode;
            let words: Vec<&str> = rest.split_whitespace().collect();
            let rev = match words.as_slice() {
                ["off"] => Some(core.mutate_settings(|s| s.apps.enabled = false)),
                ["on", mode] => match *mode {
                    "ask" => Some(AppsMode::Ask),
                    "ask_new" => Some(AppsMode::AskNew),
                    "auto" => Some(AppsMode::Auto),
                    _ => None,
                }
                .map(|m| {
                    core.mutate_settings(|s| {
                        s.apps.enabled = true;
                        s.apps.mode = m;
                    })
                }),
                ["app", id, state @ ("on" | "off")] => {
                    let (id, on) = (id.to_string(), *state == "on");
                    Some(core.mutate_settings(move |s| {
                        s.apps.apps.insert(id, on);
                    }))
                }
                _ => None,
            };
            match rev {
                Some(rev) => println!("OK apps rev={rev}"),
                None => println!("ERR usage: apps on <ask|ask_new|auto> | apps off | apps app <id> <on|off>"),
            }
        }
        "browsers" => {
            println!("BROWSERS {}", serde_json::to_string(&core.browsers()).unwrap_or_default());
        }
        "link-browser" => match core.browser_link(rest.trim()).await {
            Ok(()) => println!("OK link-browser"),
            Err(why) => println!("ERR link-browser {}", why.code()),
        },
        "unlink-browser" => match core.browser_unlink(rest.trim()).await {
            Ok(()) => println!("OK unlink-browser"),
            Err(why) => println!("ERR unlink-browser {why}"),
        },
        // Names and dates only: there is no command that prints a login.
        "vault-list" => match core.vault_list().await {
            Ok(list) => println!("VAULT {}", serde_json::to_string(&list).unwrap_or_default()),
            Err(why) => println!("ERR vault-list {why}"),
        },
        "vault-forget" => match core.vault_forget(rest.trim()).await {
            Ok(()) => println!("OK vault-forget"),
            Err(why) => println!("ERR vault-forget {why}"),
        },
        "revoke" => {
            let (cap, scope) = rest.trim().split_once(' ').unwrap_or((rest.trim(), ""));
            core.revoke(cap, scope.trim());
            println!("OK revoke");
        }
        "status" => {
            let snap = core.snapshot().await;
            println!("SNAPSHOT {}", serde_json::to_string(&snap).unwrap_or_default());
        }
        "quit" | "exit" => return true,
        other => println!("ERR unknown command {other:?}"),
    }
    false
}

async fn cmd_pair(args: &[String]) -> i32 {
    let a = parse(args, &["--hub", "--code", "--secret-file", "--settings"], &["--yes"]);
    let (Some(hub), Some(code)) = (a.get("--hub"), a.get("--code")) else {
        eprintln!("connectd pair: --hub and --code are required");
        return 2;
    };
    let core = build_core(&a, Arc::new(TerminalApprover));
    match core.pair_begin(hub.to_string(), code.to_string()).await {
        Ok(p) => {
            println!("Claimed. Synth id: {}  url: {}", p.synth_id, p.synth_url);
            let confirm = if a.has("--yes") {
                true
            } else {
                println!("Confirm pairing with this synth? [y/N]: ");
                let l = tokio::task::spawn_blocking(|| {
                    let mut s = String::new();
                    let _ = std::io::stdin().read_line(&mut s);
                    s
                })
                .await
                .unwrap_or_default();
                matches!(l.trim().to_ascii_lowercase().as_str(), "y" | "yes")
            };
            if confirm {
                match core.pair_confirm() {
                    Ok(_) => {
                        println!("Paired. Token stored.");
                        0
                    }
                    Err(e) => {
                        eprintln!("confirm failed: {e}");
                        1
                    }
                }
            } else {
                core.pair_cancel();
                println!("Cancelled. Nothing stored.");
                0
            }
        }
        Err(e) => {
            eprintln!("pair failed: {e}");
            1
        }
    }
}

async fn cmd_status(args: &[String]) -> i32 {
    let a = parse(args, &["--settings", "--secret-file", "--cua-bin", "--cua-attach"], &[]);
    let core = build_core(&a, Arc::new(TerminalApprover));
    let snap = core.snapshot().await;
    println!("{}", serde_json::to_string_pretty(&snap).unwrap_or_default());
    0
}

async fn cmd_settings(args: &[String]) -> i32 {
    let a = parse(args, &["--settings", "--secret-file"], &[]);
    let core = build_core(&a, Arc::new(TerminalApprover));
    println!("{}", serde_json::to_string_pretty(&core.settings()).unwrap_or_default());
    0
}

async fn cmd_preset(args: &[String]) -> i32 {
    let a = parse(args, &["--settings", "--secret-file"], &[]);
    let Some(name) = a.positional.first() else {
        eprintln!("connectd preset: a preset name is required");
        return 2;
    };
    let Some(p) = preset_from(name) else {
        eprintln!("connectd preset: unknown preset {name:?}");
        return 2;
    };
    let core = build_core(&a, Arc::new(TerminalApprover));
    let rev = core.apply_preset(p);
    println!("applied {name}, settings_rev={rev}");
    0
}

/// Write a host manifest into one folder: the `NativeMessagingHosts`
/// folder of a throwaway browser profile in a test. Never a real
/// browser's folder: the app registers itself there, not this tool.
fn cmd_browser_manifest(args: &[String]) -> i32 {
    let a = parse(args, &["--dir", "--helper"], &[]);
    let (Some(dir), Some(helper)) = (a.get("--dir"), a.get("--helper")) else {
        eprintln!("browser-manifest: --dir and --helper are required");
        return 2;
    };
    let helper = match std::fs::canonicalize(helper) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("browser-manifest: {helper}: {e}");
            return 1;
        }
    };
    match connect_host::install::write_manifest(
        std::path::Path::new(dir),
        connect_host::HOST_NAME,
        &helper,
        connect_host::EXTENSION_IDS,
    ) {
        Ok(file) => {
            println!("wrote {}", file.display());
            0
        }
        Err(e) => {
            eprintln!("browser-manifest: {e}");
            1
        }
    }
}

fn cmd_make_fixture(args: &[String]) -> i32 {
    let a = parse(
        args,
        &["--db", "--key", "--domain", "--name", "--value", "--kind"],
        &[],
    );
    let (Some(db), Some(key), Some(domain), Some(name), Some(value)) = (
        a.get("--db"),
        a.get("--key"),
        a.get("--domain"),
        a.get("--name"),
        a.get("--value"),
    ) else {
        eprintln!("make-cookie-fixture: --db --key --domain --name --value are required");
        return 2;
    };
    let kind = a.get("--kind").unwrap_or("chromium");
    let path = PathBuf::from(db);
    let rows = [(domain, name, value, "/", 0i64, 1i64, 1i64, 1i64)];
    match kind {
        "firefox" => cookies::fixtures::write_firefox_db(&path, &rows),
        _ => {
            let k = cookies::derive_key(key.as_bytes());
            cookies::fixtures::write_chromium_db(&path, &k, &rows);
        }
    }
    println!("wrote {kind} cookie fixture at {}", path.display());
    0
}
