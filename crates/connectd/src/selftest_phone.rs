//! `connectd selftest phone`: phone_control against the real thing, when it
//! is there (tz-phone-control.md P0 items 1 and 2).
//!
//!   connectd selftest phone [--phonecap <path>] [--fake-screen] [--touch]
//!                           [--udid <UDID>] [--list]
//!
//! The screen: the capture helper (`--phonecap`, else `SNTH_PHONECAP`, else
//! `snth-phonecap` next to connectd) through the whole gate: status, a gray
//! frame, a frame at 768 px and at the native size, a frame that waits for
//! the screen to be still. No iPhone on the cable = "SKIP screen".
//! `--fake-screen` runs the helper on its made-up screen (no camera, no
//! iPhone): the protocol between the core and the helper, on any Mac.
//!
//! The hands: the SNTH Phone Bridge board on its USB port. Its status, then
//! the port held open for a few seconds, closed (RTS before DTR) and opened
//! again: the phone must still be connected over Bluetooth after that (a
//! reset of the board would drop it). No board = "SKIP hands".
//!
//! `--touch`: one harmless action on the phone (home, checked on the
//! screen). Without it nothing is pressed on the phone.
//!
//! `--udid <UDID>`: the screen over Wi-Fi instead of the cable
//! (tz-phone-wireless.md 2.8), for exactly that phone: its status and
//! abilities, a frame at 768 px and at the native size, a frame that waits
//! for the screen to be still, and the phone's app list. Read-only: no app
//! is opened, nothing is pasted. Never a phone the UDID does not name.
//! `--list`: the physical iOS devices this Mac sees (only a list; to pick
//! one, pass its UDID).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use connect_core::grant::{Approval, ApprovalFuture, ApprovalRequest, Approver};
use connect_core::phone::{self, eyes, SystemFactory};
use connect_core::settings::PhoneSettings;
use connect_core::{cookies, Core, CoreConfig, CuaAttach, MemSecretStore, MultiReader};
use serde_json::{json, Value};

struct AllowAlways;
impl Approver for AllowAlways {
    fn ask(&self, _req: ApprovalRequest) -> ApprovalFuture {
        Box::pin(async { Approval::always() })
    }
}

fn step(text: &str) {
    println!("== {text}");
}

fn core_with(helper: PathBuf, udid: &str) -> Core {
    let core = Core::new(CoreConfig {
        settings_path: None,
        journal_path: None,
        approver: Arc::new(AllowAlways),
        cookies: Arc::new(MultiReader { chromium: vec![], firefox: vec![], keys: Arc::new(cookies::fixtures::NoKey) }),
        apps: Arc::new(connect_core::apps::stub::StubBackend::offering(&[])),
        secret: Arc::new(MemSecretStore::new()),
        hub: Arc::new(connect_core::UreqHubClient),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        cua_bin: "cua-driver".into(),
        cua_private_root: None,
        cua_attach: CuaAttach::User,
    });
    core.set_phone_factory(Arc::new(SystemFactory { helper: Some(helper) }));
    let udid = udid.to_string();
    core.mutate_settings(move |s| {
        s.phone_control.enabled = true;
        s.phone_control.device_udid = udid;
    });
    core
}

async fn call(core: &Core, method: &str, params: Value) -> Result<Value, String> {
    phone::handle(&core.shared(), method, &params).await
}

pub async fn phone(args: &[String]) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        println!("SKIP: phone control works on macOS only");
        return Ok(());
    }
    let mut helper: Option<PathBuf> = None;
    let mut fake = false;
    let mut touch = false;
    let mut udid = String::new();
    let mut list = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--phonecap" => helper = Some(PathBuf::from(it.next().ok_or("--phonecap needs a path")?)),
            "--fake-screen" => fake = true,
            "--touch" => touch = true,
            "--udid" => udid = it.next().ok_or("--udid needs the phone's UDID")?.trim().to_string(),
            "--list" => list = true,
            other => return Err(format!("unknown option {other}")),
        }
    }
    if !PhoneSettings::udid_ok(&udid) {
        return Err(format!("{udid:?} is not a UDID"));
    }
    if fake && !udid.is_empty() {
        return Err("--fake-screen is the cable helper's made-up screen; it does not go with --udid".into());
    }
    let helper = helper.unwrap_or_else(eyes::helper_path);
    if udid.is_empty() && !helper.is_file() {
        return Err(format!("no screen helper at {} (pass --phonecap)", helper.display()));
    }
    if fake {
        // Inherited by the helper the core starts.
        std::env::set_var("SNTH_PHONECAP_FAKE", "1");
    }
    let core = core_with(helper.clone(), &udid);
    if list {
        step("the phones this Mac sees (a list only)");
        match core.phone_devices().await {
            Ok(d) => {
                for p in d {
                    println!("{}  {} ({})  {}", p.udid, p.name, p.model, p.state);
                }
            }
            Err(why) => println!("SKIP list: {why}"),
        }
    }
    let screen = if udid.is_empty() {
        println!("helper: {}", helper.display());
        screen(&core, fake).await?
    } else {
        println!("the phone over Wi-Fi: {udid}");
        screen_net(&core).await?
    };
    let board = hands_check(&core, &helper).await?;

    if touch {
        if !(screen && board) {
            println!("SKIP touch: needs both the screen and the board");
        } else {
            step("one harmless action: home");
            let started = Instant::now();
            let r = call(&core, "phone_control.act", json!({"action": "home", "verify": true})).await?;
            println!("home: {r} in {:.2} s", started.elapsed().as_secs_f64());
            let r = call(&core, "phone_control.act", json!({"action": "release_all"})).await?;
            println!("release_all: {r}");
            let last = core.recent_journal().pop().ok_or("no journal line for the act")?;
            println!("journal: {} {} -> {}", last.action, last.target, last.outcome);
        }
    }
    Ok(())
}

/// The screen through the gate. Ok(false) = skipped (no iPhone).
async fn screen(core: &Core, fake: bool) -> Result<bool, String> {
    step("the screen: waiting up to 10 s for the capture");
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        let s = call(core, "phone_control.status", json!({})).await?;
        if s["screen"]["ok"] == true || Instant::now() > deadline {
            break s;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    println!("status: {status}");
    if status["screen"]["ok"] != true {
        let why = status["screen"]["error"].as_str().unwrap_or("").to_string();
        if fake {
            return Err(format!("the fake screen did not come up: {why}"));
        }
        println!("SKIP screen: {why}");
        return Ok(false);
    }
    let (nw, nh) = (status["screen"]["width"].as_u64().unwrap_or(0), status["screen"]["height"].as_u64().unwrap_or(0));

    let started = Instant::now();
    let f = call(core, "phone_control.frame", json!({"max_edge": 768, "quality": 0.8})).await?;
    let ms = started.elapsed().as_millis();
    check_frame(&f, 768, nw, nh)?;
    println!("frame 768: {}x{} of {}x{}, {} KB, {ms} ms", f["width"], f["height"], nw, nh, b64_len(&f) / 1024);

    let started = Instant::now();
    let f = call(core, "phone_control.frame", json!({})).await?;
    check_frame(&f, 0, nw, nh)?;
    println!("frame native: {}x{}, {} KB, {} ms", f["width"], f["height"], b64_len(&f) / 1024, started.elapsed().as_millis());

    let started = Instant::now();
    let f = call(core, "phone_control.frame", json!({"stable_ms": 600, "after_ms": now_ms()})).await?;
    let waited = started.elapsed();
    println!("frame stable_ms 600: stable {} after {:.2} s", f["stable"], waited.as_secs_f64());
    if f["stable"] == true && waited < Duration::from_millis(550) {
        return Err(format!("a still screen was reported after only {waited:?}"));
    }
    if waited > Duration::from_millis(4500) {
        return Err(format!("the stable frame took {waited:?}"));
    }
    Ok(true)
}

/// The screen over Wi-Fi through the gate. Ok(false) = skipped (Xcode
/// missing, or the phone not reachable).
async fn screen_net(core: &Core) -> Result<bool, String> {
    step("the screen over Wi-Fi: the status");
    let status = call(core, "phone_control.status", json!({})).await?;
    println!("status: {}", status["screen"]);
    println!("abilities: {}", status["abilities"]);
    if status["screen"]["source"] != "network" {
        return Err(format!("the screen's source is {} (expected network)", status["screen"]["source"]));
    }
    if status["screen"]["ok"] != true {
        println!("SKIP screen: {}", status["screen"]["error"].as_str().unwrap_or(""));
        return Ok(false);
    }
    if status["abilities"] != json!({"launch": true, "paste": true}) {
        return Err(format!("a reachable phone over Wi-Fi has abilities {}", status["abilities"]));
    }

    let started = Instant::now();
    let f = call(core, "phone_control.frame", json!({"max_edge": 768, "quality": 0.8})).await?;
    let ms = started.elapsed().as_millis();
    let (nw, nh) = (f["native_width"].as_u64().unwrap_or(0), f["native_height"].as_u64().unwrap_or(0));
    check_frame(&f, 768, nw, nh)?;
    println!("frame 768: {}x{} of {nw}x{nh}, {} KB, {ms} ms (one screenshot)", f["width"], f["height"], b64_len(&f) / 1024);

    let started = Instant::now();
    let f = call(core, "phone_control.frame", json!({})).await?;
    check_frame(&f, 0, nw, nh)?;
    println!("frame native: {}x{}, {} KB, {} ms", f["width"], f["height"], b64_len(&f) / 1024, started.elapsed().as_millis());

    let started = Instant::now();
    let f = call(core, "phone_control.frame", json!({"stable_ms": 600, "max_edge": 768})).await?;
    let waited = started.elapsed();
    println!("frame stable_ms 600: stable {} after {:.2} s", f["stable"], waited.as_secs_f64());
    if waited > Duration::from_millis(5000) {
        return Err(format!("the stable frame took {waited:?}"));
    }

    step("the phone's apps (read-only)");
    let started = Instant::now();
    let apps = core.phone_apps().await?;
    println!("{} apps in {} ms", apps.len(), started.elapsed().as_millis());
    for want in ["com.apple.calculator", "com.apple.Preferences"] {
        match apps.iter().find(|a| a.bundle_id == want) {
            Some(a) => println!("  {} = {}", a.name, a.bundle_id),
            None => println!("  note: no {want} on this phone"),
        }
    }
    if apps.is_empty() {
        return Err("the phone listed no apps".into());
    }
    Ok(true)
}

fn b64_len(f: &Value) -> usize {
    f["data_b64"].as_str().map(|s| s.len() * 3 / 4).unwrap_or(0)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn check_frame(f: &Value, max_edge: u64, nw: u64, nh: u64) -> Result<(), String> {
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD
        .decode(f["data_b64"].as_str().unwrap_or(""))
        .map_err(|e| format!("bad base64: {e}"))?;
    if data.len() < 4 || data[0] != 0xff || data[1] != 0xd8 {
        return Err("the frame is not a JPEG".into());
    }
    if f["mime"] != "image/jpeg" {
        return Err(format!("mime {}", f["mime"]));
    }
    let (w, h) = (f["width"].as_u64().unwrap_or(0), f["height"].as_u64().unwrap_or(0));
    if f["native_width"].as_u64() != Some(nw) || f["native_height"].as_u64() != Some(nh) {
        return Err(format!("native size {}x{}, status said {nw}x{nh}", f["native_width"], f["native_height"]));
    }
    let long = w.max(h);
    let want = if max_edge == 0 { nw.max(nh) } else { max_edge.min(nw.max(nh)) };
    if long != want {
        return Err(format!("a frame of {w}x{h}, expected the long edge {want}"));
    }
    Ok(())
}

/// The board. Ok(false) = skipped (no board).
async fn hands_check(core: &Core, helper: &std::path::Path) -> Result<bool, String> {
    step("the hands: the board's status");
    let v = core.phone_view().await;
    println!("board: {}", serde_json::to_string(&v.hands).unwrap_or_default());
    if !v.hands.ok {
        println!("SKIP hands: {}", v.hands.error);
        return Ok(false);
    }
    if !v.hands.ble_connected {
        println!("note: the board answers but the phone is not connected to it over Bluetooth");
    }
    let was_connected = v.hands.ble_connected;

    step("the port stays open: status again in 3 s");
    tokio::time::sleep(Duration::from_secs(3)).await;
    let again = core.phone_view().await.hands;
    if !again.ok || again.ble_connected != was_connected {
        return Err(format!("the board changed while its port was held open: {again:?}"));
    }

    step("close the port (RTS before DTR) and open it again");
    core.mutate_settings(|s| s.phone_control.enabled = false);
    tokio::time::sleep(Duration::from_millis(500)).await;
    core.set_phone_factory(Arc::new(SystemFactory { helper: Some(helper.to_path_buf()) }));
    // (The chosen phone, if any, stays in the settings: the same eyes come back.)
    core.mutate_settings(|s| s.phone_control.enabled = true);
    // A reset board reboots in about a second and the phone reconnects
    // only later: still connected two seconds after the reopen = no reset.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let after = core.phone_view().await.hands;
    println!("after reopen: {}", serde_json::to_string(&after).unwrap_or_default());
    if !after.ok {
        return Err(format!("the board is gone after the reopen: {}", after.error));
    }
    if was_connected && !after.ble_connected {
        return Err("the phone lost Bluetooth after the port was closed and opened: the board was reset".into());
    }
    Ok(true)
}
