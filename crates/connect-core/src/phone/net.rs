//! The phone over Wi-Fi: Xcode's `xcrun devicectl` (tz-phone-wireless.md,
//! section 2), for exactly the one phone the owner chose on the Phone page
//! (`PhoneSettings::device_udid`).
//!
//! IRON RULE: a UDID is never picked here. This Mac may see several
//! iPhones in `devicectl list devices` (the owner's own phone among them);
//! the only UDID ever passed to a `devicectl device ...` command is the
//! one from the settings. No fallback to "the first" or "the only" phone,
//! and none from these eyes to the cable capture.
//!
//! What it does, measured on the test iPhone XR over Wi-Fi:
//! - eyes (`NetPhone` implements `Eyes`): `device capture screenshot` into
//!   a private file (0600 in a 0700 folder, deleted after the read), about
//!   0.45-0.7 s each; the PNG is decoded here, scaled and sent as JPEG, and
//!   the small gray frames for the change check come from the same
//!   screenshot. One `devicectl` at a time for the phone. A frame right
//!   after a look reuses that look's screenshot (younger than 150 ms, and
//!   nothing done on the phone since).
//! - `device process launch <bundle id>` (0.3-0.4 s), the bundle id from
//!   `device info apps --include-all-apps` (cached 10 minutes);
//! - `device pasteboard copy` with the text on stdin (0.3-0.6 s): the
//!   board then presses cmd+v. The text is never logged.
//!
//! Every command runs with `--json-output -` (the JSON on stdout is the
//! stable interface, the rest goes to stderr) and its own `--timeout`.
//! Without Xcode (`xcode-select -p` has no `usr/bin/devicectl`) nothing is
//! run at all: no "install the developer tools" dialog.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::time::Instant;

use super::diff::{gray_size, Gray, GRAY_EDGE};
use super::eyes::{Eyes, Fut, GrayFrame, Jpeg, Pace, ScreenStatus};

/// What the owner sees without Xcode.
pub const NO_XCODE: &str = "Xcode is not installed on this Mac (the phone over Wi-Fi needs it)";

/// A screenshot reused for a frame asked right after it.
const REUSE: Duration = Duration::from_millis(150);
/// The oldest look a wait may start from (nothing done on the phone since).
const SEED_MAX: Duration = Duration::from_secs(5);
/// How long the device list is trusted.
const LIST_TTL: Duration = Duration::from_secs(5);
/// How long the phone's app list is trusted.
const APPS_TTL: Duration = Duration::from_secs(600);
/// A failed screenshot makes the status "not ok" for this long.
const FAIL_TTL: Duration = Duration::from_secs(10);
/// The size reported before the first screenshot (the iPhone XR).
const DEFAULT_SIZE: (u32, u32) = (828, 1792);

/// Per command (devicectl's own `--timeout`; the process is killed a
/// little after that).
pub const SHOT_TIMEOUT: Duration = Duration::from_secs(5);
pub const LIST_TIMEOUT: Duration = Duration::from_secs(8);
pub const APPS_TIMEOUT: Duration = Duration::from_secs(8);
pub const LAUNCH_TIMEOUT: Duration = Duration::from_secs(8);
pub const PASTE_TIMEOUT: Duration = Duration::from_secs(5);

/// A physical iOS device this Mac knows (the Phone page's picker).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PhoneDevice {
    pub udid: String,
    pub name: String,
    /// "iPhone XR".
    pub model: String,
    /// `connected` (devicectl's tunnel to it is up), `available` (paired,
    /// reached on demand), `unpaired`, or devicectl's own word.
    pub state: String,
    /// `connected` or `available`.
    pub reachable: bool,
}

/// An app on the phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneApp {
    pub name: String,
    pub bundle_id: String,
}

/// What `devicectl` does here (`Devicectl` runs it; tests fake it). Errors
/// are short words.
pub trait Device: Send + Sync {
    /// The physical iOS devices this Mac knows.
    fn list(&self) -> Fut<'_, Result<Vec<PhoneDevice>, String>>;
    /// One screenshot of this phone, PNG.
    fn screenshot<'a>(&'a self, udid: &'a str) -> Fut<'a, Result<Vec<u8>, String>>;
    /// The apps on this phone.
    fn apps<'a>(&'a self, udid: &'a str) -> Fut<'a, Result<Vec<PhoneApp>, String>>;
    /// Open this app on this phone (to the front).
    fn launch<'a>(&'a self, udid: &'a str, bundle_id: &'a str) -> Fut<'a, Result<(), String>>;
    /// Put this text on the phone's pasteboard.
    fn paste<'a>(&'a self, udid: &'a str, text: &'a str) -> Fut<'a, Result<(), String>>;
}

// --- devicectl's JSON (jsonVersion 5, Xcode 27.2) ---

fn s<'a>(v: &'a Value, path: &[&str]) -> &'a str {
    let mut cur = v;
    for k in path {
        cur = &cur[*k];
    }
    cur.as_str().unwrap_or("")
}

/// The first non-empty of the paths (the new `properties` dictionary, then
/// the deprecated fields).
fn first<'a>(v: &'a Value, paths: &[&[&str]]) -> &'a str {
    paths.iter().map(|p| s(v, p)).find(|x| !x.is_empty()).unwrap_or("")
}

/// `result.devices` of `devicectl list devices --json-output -`: the
/// physical iOS devices (phones and iPads), simulators and watches left
/// out.
pub fn parse_devices(doc: &Value) -> Vec<PhoneDevice> {
    let Some(list) = doc["result"]["devices"].as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for d in list {
        let reality = first(d, &[&["properties", "hardware", "reality"], &["hardwareProperties", "reality"]]);
        let platform = first(d, &[&["properties", "hardware", "platform"], &["hardwareProperties", "platform"]]);
        if reality != "physical" || platform != "iOS" {
            continue;
        }
        let udid = first(d, &[&["properties", "hardware", "udid"], &["hardwareProperties", "udid"]]);
        if udid.is_empty() || !crate::settings::PhoneSettings::udid_ok(udid) {
            continue;
        }
        let name = first(d, &[&["properties", "state", "name"], &["deviceProperties", "name"]]);
        let model = first(d, &[&["properties", "hardware", "marketingName"], &["hardwareProperties", "marketingName"]]);
        let conn = first(d, &[&["properties", "connection", "state"]]);
        let tunnel = s(d, &["connectionProperties", "tunnelState"]);
        let pairing = first(d, &[&["properties", "connection", "pairingState"], &["connectionProperties", "pairingState"]]);
        let boot = first(d, &[&["properties", "state", "bootState"], &["deviceProperties", "bootState"]]);
        let state = if conn == "connected" || tunnel == "connected" {
            "connected".to_string()
        } else if pairing != "paired" {
            "unpaired".to_string()
        } else if boot == "booted" {
            "available".to_string()
        } else if !conn.is_empty() {
            conn.to_string()
        } else {
            "unavailable".to_string()
        };
        let reachable = state == "connected" || state == "available";
        out.push(PhoneDevice { udid: udid.into(), name: name.into(), model: model.into(), state, reachable });
    }
    out
}

/// `result.apps` of `devicectl device info apps --json-output -`.
pub fn parse_apps(doc: &Value) -> Vec<PhoneApp> {
    let Some(list) = doc["result"]["apps"].as_array() else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|a| {
            let bundle_id = a["bundleIdentifier"].as_str().unwrap_or("").trim();
            if bundle_id.is_empty() || bundle_id.starts_with('-') {
                return None;
            }
            Some(PhoneApp { name: a["name"].as_str().unwrap_or("").trim().to_string(), bundle_id: bundle_id.to_string() })
        })
        .collect()
}

/// devicectl's reason, short: the error's description from the JSON (and
/// the underlying one when it adds something), else its stderr.
pub fn ctl_error(doc: Option<&Value>, stderr: &str) -> String {
    let mut why = String::new();
    if let Some(e) = doc.map(|d| &d["error"]).filter(|e| e.is_object()) {
        why = s(e, &["userInfo", "NSLocalizedDescription", "string"]).trim().to_string();
        let under = s(e, &["userInfo", "NSUnderlyingError", "error", "userInfo", "NSLocalizedDescription", "string"]).trim();
        if !under.is_empty() && !why.contains(under) {
            why = if why.is_empty() { under.to_string() } else { format!("{why} ({under})") };
        }
    }
    if why.is_empty() {
        let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        why = lines
            .iter()
            .find_map(|l| l.strip_prefix("ERROR:"))
            .or_else(|| lines.last().copied())
            .unwrap_or("devicectl failed")
            .trim()
            .to_string();
    }
    let why = why.replace('\u{2014}', "-");
    if why.chars().count() > 240 {
        format!("{}...", why.chars().take(237).collect::<String>())
    } else {
        why
    }
}

/// How an app's name compares: case, spaces and apostrophes aside.
fn norm(x: &str) -> String {
    x.replace(['\u{a0}', '\u{2009}', '\u{202f}'], " ")
        .replace(['\u{2019}', '\u{2018}'], "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Which app the synth means: the exact bundle id, else the display name
/// (any case), else the one app whose name starts with it. Err(the
/// candidates when several fit; empty when none does).
pub fn resolve_app(apps: &[PhoneApp], query: &str) -> Result<String, Vec<String>> {
    let q = query.trim();
    if let Some(a) = apps.iter().find(|a| a.bundle_id == q).or_else(|| apps.iter().find(|a| a.bundle_id.eq_ignore_ascii_case(q))) {
        return Ok(a.bundle_id.clone());
    }
    let nq = norm(q);
    if nq.is_empty() {
        return Err(Vec::new());
    }
    let named: Vec<&PhoneApp> = apps.iter().filter(|a| norm(&a.name) == nq).collect();
    match named.len() {
        1 => return Ok(named[0].bundle_id.clone()),
        n if n > 1 => return Err(named.iter().map(|a| format!("{} ({})", a.name, a.bundle_id)).collect()),
        _ => {}
    }
    let starts: Vec<&PhoneApp> = apps.iter().filter(|a| norm(&a.name).starts_with(&nq)).collect();
    match starts.len() {
        1 => Ok(starts[0].bundle_id.clone()),
        _ => Err(starts.iter().take(6).map(|a| a.name.clone()).collect()),
    }
}

// --- the real devicectl ---

/// `xcrun devicectl`, one process per command.
pub struct Devicectl {
    /// A private folder (0700) for the screenshots, made at the first one.
    dir: Mutex<Option<PathBuf>>,
    n: AtomicU64,
    /// Is Xcode there: yes is remembered, no for 30 s.
    xcode: Mutex<Option<(Instant, bool)>>,
}

/// The one `Devicectl` of this process.
pub fn system() -> Arc<dyn Device> {
    static ONE: OnceLock<Arc<Devicectl>> = OnceLock::new();
    ONE.get_or_init(|| Arc::new(Devicectl { dir: Mutex::new(None), n: AtomicU64::new(0), xcode: Mutex::new(None) })).clone()
}

impl Drop for Devicectl {
    fn drop(&mut self) {
        if let Some(d) = self.dir.lock().unwrap().take() {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

impl Devicectl {
    /// Xcode's devicectl is there. Checked without running xcrun: on a Mac
    /// without developer tools xcrun itself offers to install them.
    async fn xcode_ok(&self) -> Result<(), String> {
        if !cfg!(target_os = "macos") {
            return Err("the phone over Wi-Fi works on a Mac only".into());
        }
        if let Some((at, ok)) = *self.xcode.lock().unwrap() {
            if ok {
                return Ok(());
            }
            if at.elapsed() < Duration::from_secs(30) {
                return Err(NO_XCODE.into());
            }
        }
        let out = crate::proc::command("/usr/bin/xcode-select")
            .arg("-p")
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .await;
        let ok = match out {
            Ok(o) if o.status.success() => {
                let dir = String::from_utf8_lossy(&o.stdout).trim().to_string();
                !dir.is_empty() && std::path::Path::new(&dir).join("usr/bin/devicectl").exists()
            }
            _ => false,
        };
        *self.xcode.lock().unwrap() = Some((Instant::now(), ok));
        if ok {
            Ok(())
        } else {
            Err(NO_XCODE.into())
        }
    }

    /// `xcrun devicectl <cmd...> --timeout N --json-output - <positional...>`:
    /// the JSON's `result`, or devicectl's reason.
    async fn run(&self, cmd: &[&str], positional: &[&str], stdin: Option<&[u8]>, timeout: Duration) -> Result<Value, String> {
        use tokio::io::AsyncWriteExt;
        self.xcode_ok().await?;
        let secs = timeout.as_secs().max(1).to_string();
        let mut c = crate::proc::command("/usr/bin/xcrun");
        c.arg("devicectl")
            .args(cmd)
            .args(["--timeout", &secs, "--json-output", "-"])
            .args(positional)
            .stdin(if stdin.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() })
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        // Its own process group: Xcode's devicectl is a script that may
        // start `xcodebuild -runFirstLaunch` (after an Xcode update), which
        // can hang and does not honour --timeout. A timeout kills the group.
        #[cfg(unix)]
        c.process_group(0);
        let mut child = c.spawn().map_err(|e| format!("cannot run xcrun devicectl: {e}"))?;
        let pid = child.id();
        if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
            // The text goes in and the pipe closes: devicectl reads to the end.
            pipe.write_all(data).await.map_err(|e| format!("cannot hand the text to devicectl: {e}"))?;
            drop(pipe);
        }
        let out = match tokio::time::timeout(timeout + Duration::from_secs(2), child.wait_with_output()).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => return Err(format!("devicectl failed: {e}")),
            Err(_) => {
                #[cfg(unix)]
                if let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()).filter(|p| *p > 1) {
                    unsafe { libc::kill(-pid, libc::SIGKILL) };
                }
                let _ = pid;
                return Err(format!(
                    "devicectl did not answer in {} s (after an Xcode install or update, open Xcode once so it finishes its setup)",
                    timeout.as_secs()
                ));
            }
        };
        let stderr = String::from_utf8_lossy(&out.stderr);
        let doc: Option<Value> = serde_json::from_slice(&out.stdout).ok();
        match &doc {
            Some(d) if d["info"]["outcome"] == "success" => Ok(d["result"].clone()),
            _ => Err(ctl_error(doc.as_ref(), &stderr)),
        }
    }

    /// A new private file for one screenshot (0600, in the 0700 folder).
    fn shot_path(&self) -> Result<PathBuf, String> {
        let mut dir = self.dir.lock().unwrap();
        if dir.is_none() {
            let d = std::env::temp_dir().join(format!("snth-phone-{}-{:08x}", std::process::id(), rand::random::<u32>()));
            let mut b = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                b.mode(0o700);
            }
            b.create(&d).map_err(|e| format!("cannot make a private folder for the screenshots: {e}"))?;
            *dir = Some(d);
        }
        let n = self.n.fetch_add(1, Ordering::SeqCst);
        let path = dir.as_ref().expect("just made").join(format!("shot-{n}.png"));
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        o.open(&path).map_err(|e| format!("cannot make a private file for the screenshot: {e}"))?;
        Ok(path)
    }
}

impl Device for Devicectl {
    fn list(&self) -> Fut<'_, Result<Vec<PhoneDevice>, String>> {
        Box::pin(async move {
            let result = self.run(&["list", "devices"], &[], None, LIST_TIMEOUT).await?;
            Ok(parse_devices(&serde_json::json!({ "result": result })))
        })
    }

    fn screenshot<'a>(&'a self, udid: &'a str) -> Fut<'a, Result<Vec<u8>, String>> {
        Box::pin(async move {
            let path = self.shot_path()?;
            let p = path.to_string_lossy().to_string();
            let r = self
                .run(&["device", "capture", "screenshot", "--device", udid, "--destination", &p], &[], None, SHOT_TIMEOUT)
                .await;
            let data = r.and_then(|_| std::fs::read(&path).map_err(|e| format!("cannot read the screenshot: {e}")));
            let _ = std::fs::remove_file(&path);
            let data = data?;
            if data.is_empty() {
                return Err("devicectl wrote an empty screenshot".into());
            }
            Ok(data)
        })
    }

    fn apps<'a>(&'a self, udid: &'a str) -> Fut<'a, Result<Vec<PhoneApp>, String>> {
        Box::pin(async move {
            let result = self.run(&["device", "info", "apps", "--device", udid, "--include-all-apps"], &[], None, APPS_TIMEOUT).await?;
            Ok(parse_apps(&serde_json::json!({ "result": result })))
        })
    }

    fn launch<'a>(&'a self, udid: &'a str, bundle_id: &'a str) -> Fut<'a, Result<(), String>> {
        Box::pin(async move {
            self.run(&["device", "process", "launch", "--device", udid], &[bundle_id], None, LAUNCH_TIMEOUT).await.map(|_| ())
        })
    }

    fn paste<'a>(&'a self, udid: &'a str, text: &'a str) -> Fut<'a, Result<(), String>> {
        Box::pin(async move {
            self.run(&["device", "pasteboard", "copy", "--device", udid], &[], Some(text.as_bytes()), PASTE_TIMEOUT)
                .await
                .map(|_| ())
        })
    }
}

// --- the phone over Wi-Fi ---

/// One screenshot, decoded.
struct Shot {
    started: Instant,
    done: Instant,
    unix_ms: i64,
    rgb: Arc<image::RgbImage>,
    gray: Gray,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// PNG -> RGB and the gray frame (long edge `GRAY_EDGE`).
fn decode(png: &[u8]) -> Result<(image::RgbImage, Gray), String> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).map_err(|e| format!("the screenshot is not a PNG: {e}"))?;
    let rgb = img.to_rgb8();
    let gray = gray_of(&rgb, GRAY_EDGE)?;
    Ok((rgb, gray))
}

fn gray_of(rgb: &image::RgbImage, edge: u32) -> Result<Gray, String> {
    let luma = image::DynamicImage::ImageRgb8(rgb.clone()).to_luma8();
    let (w, h) = if edge == 0 { (rgb.width(), rgb.height()) } else { fit(rgb.width(), rgb.height(), edge) };
    let small = if (w, h) == (rgb.width(), rgb.height()) {
        luma
    } else {
        image::imageops::resize(&luma, w, h, image::imageops::FilterType::Triangle)
    };
    Gray::new(w, h, small.into_raw())
}

/// The size with the long edge at most `edge` (the gray frames' rule).
fn fit(w: u32, h: u32, edge: u32) -> (u32, u32) {
    if edge == GRAY_EDGE {
        return gray_size(w, h);
    }
    let long = w.max(h).max(1);
    if edge == 0 || long <= edge {
        return (w.max(1), h.max(1));
    }
    let s = edge as f64 / long as f64;
    (((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1))
}

/// RGB -> JPEG, the long edge at most `max_edge` (0 = native).
fn encode(rgb: &image::RgbImage, max_edge: u32, quality: f64) -> Result<(Vec<u8>, u32, u32), String> {
    let (w, h) = fit(rgb.width(), rgb.height(), max_edge);
    let scaled;
    let img = if (w, h) == (rgb.width(), rgb.height()) {
        rgb
    } else {
        scaled = image::imageops::resize(rgb, w, h, image::imageops::FilterType::Triangle);
        &scaled
    };
    let q = (quality.clamp(0.1, 1.0) * 100.0).round() as u8;
    let mut out = Vec::with_capacity(256 * 1024);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q)
        .encode_image(img)
        .map_err(|e| format!("cannot make a JPEG of the screenshot: {e}"))?;
    Ok((out, w, h))
}

type DeviceList = Result<Vec<PhoneDevice>, String>;

/// The owner's chosen phone over Wi-Fi: its eyes (`Eyes`), and the two
/// things devicectl does that the board cannot (open an app, paste).
pub struct NetPhone {
    dev: Arc<dyn Device>,
    udid: String,
    /// One devicectl at a time for this phone.
    gate: tokio::sync::Mutex<()>,
    last: Mutex<Option<Arc<Shot>>>,
    touched_at: Mutex<Option<Instant>>,
    failed: Mutex<Option<(Instant, String)>>,
    list: tokio::sync::Mutex<Option<(Instant, DeviceList)>>,
    apps: tokio::sync::Mutex<Option<(Instant, Vec<PhoneApp>)>>,
}

impl NetPhone {
    pub fn new(dev: Arc<dyn Device>, udid: String) -> NetPhone {
        NetPhone {
            dev,
            udid,
            gate: tokio::sync::Mutex::new(()),
            last: Mutex::new(None),
            touched_at: Mutex::new(None),
            failed: Mutex::new(None),
            list: tokio::sync::Mutex::new(None),
            apps: tokio::sync::Mutex::new(None),
        }
    }

    pub fn udid(&self) -> &str {
        &self.udid
    }

    /// A new screenshot (one devicectl at a time).
    async fn capture(&self) -> Result<Arc<Shot>, String> {
        let _one = self.gate.lock().await;
        let started = Instant::now();
        let unix_ms = now_ms();
        let png = match self.dev.screenshot(&self.udid).await {
            Ok(p) => p,
            Err(why) => {
                *self.failed.lock().unwrap() = Some((Instant::now(), why.clone()));
                return Err(why);
            }
        };
        let (rgb, gray) = tokio::task::spawn_blocking(move || decode(&png)).await.map_err(|e| format!("decoding the screenshot failed: {e}"))??;
        let shot = Arc::new(Shot { started, done: Instant::now(), unix_ms, rgb: Arc::new(rgb), gray });
        *self.last.lock().unwrap() = Some(shot.clone());
        *self.failed.lock().unwrap() = None;
        Ok(shot)
    }

    /// The last screenshot, if nothing was done on the phone after it was
    /// started.
    fn untouched(&self) -> Option<Arc<Shot>> {
        let last = self.last.lock().unwrap().clone()?;
        match *self.touched_at.lock().unwrap() {
            Some(t) if last.started <= t => None,
            _ => Some(last),
        }
    }

    /// The devices this Mac sees (cached `LIST_TTL`).
    async fn devices(&self) -> Result<Vec<PhoneDevice>, String> {
        let mut cache = self.list.lock().await;
        if let Some((at, r)) = cache.as_ref() {
            if at.elapsed() < LIST_TTL {
                return r.clone();
            }
        }
        let r = self.dev.list().await;
        *cache = Some((Instant::now(), r.clone()));
        r
    }

    /// The phone's apps (cached `APPS_TTL`; `fresh` = asked anew). (apps,
    /// whether they were just asked).
    pub async fn apps(&self, fresh: bool) -> Result<(Vec<PhoneApp>, bool), String> {
        let mut cache = self.apps.lock().await;
        if !fresh {
            if let Some((at, a)) = cache.as_ref() {
                if at.elapsed() < APPS_TTL {
                    return Ok((a.clone(), false));
                }
            }
        }
        let a = {
            let _one = self.gate.lock().await;
            self.dev.apps(&self.udid).await?
        };
        *cache = Some((Instant::now(), a.clone()));
        Ok((a, true))
    }

    /// The bundle id the synth means by `app`. Err: `bad request: no app
    /// named ...` (not on the phone), `no_screen: ...` (the phone does not
    /// answer).
    pub async fn resolve(&self, app: &str) -> Result<String, String> {
        let (apps, fresh) = self.apps(false).await.map_err(|e| format!("no_screen: {e}"))?;
        let mut answer = resolve_app(&apps, app);
        if answer.is_err() && !fresh {
            // Installed since the list was read?
            let (apps, _) = self.apps(true).await.map_err(|e| format!("no_screen: {e}"))?;
            answer = resolve_app(&apps, app);
        }
        answer.map_err(|several| {
            let shown: String = app.trim().chars().take(80).collect();
            if several.is_empty() {
                format!("bad request: no app named {shown:?} on the phone")
            } else {
                format!("bad request: no app named {shown:?} on the phone (it could be {}; name one)", several.join(", "))
            }
        })
    }

    /// Open the app (by bundle id) on the phone.
    pub async fn launch(&self, bundle_id: &str) -> Result<(), String> {
        let r = {
            let _one = self.gate.lock().await;
            self.dev.launch(&self.udid, bundle_id).await
        };
        self.touched();
        r
    }

    /// Put the text on the phone's pasteboard. The text is never logged.
    pub async fn paste(&self, text: &str) -> Result<(), String> {
        let _one = self.gate.lock().await;
        self.dev.paste(&self.udid, text).await.map_err(|e| {
            // devicectl's words, never the text itself.
            if text.chars().count() >= 4 && e.contains(text) {
                e.replace(text, "<the text>")
            } else {
                e
            }
        })
    }
}

impl Eyes for NetPhone {
    fn status(&self) -> Fut<'_, ScreenStatus> {
        Box::pin(async move {
            let mut st = ScreenStatus { source: "network".into(), ..Default::default() };
            let devices = match self.devices().await {
                Ok(d) => d,
                Err(why) => {
                    st.error = why;
                    return st;
                }
            };
            let Some(d) = devices.iter().find(|d| d.udid == self.udid) else {
                st.error = format!(
                    "the chosen phone ({}) is not visible to this Mac: pair it once by cable with Developer Mode on, then keep it awake on the same Wi-Fi",
                    self.udid
                );
                return st;
            };
            st.device = d.name.clone();
            let (w, h) = self.known_size().unwrap_or(DEFAULT_SIZE);
            (st.width, st.height) = (w, h);
            st.last_frame_ms = self.last.lock().unwrap().as_ref().map(|s| s.unix_ms).unwrap_or(0);
            if !d.reachable {
                st.error = format!("{} is {}: keep it awake (Auto-Lock Never) and on the same Wi-Fi as this Mac", d.name, d.state);
                return st;
            }
            if let Some((at, why)) = self.failed.lock().unwrap().clone() {
                if at.elapsed() < FAIL_TTL {
                    st.error = why;
                    return st;
                }
            }
            st.ok = true;
            st
        })
    }

    fn jpeg(&self, after_ms: i64, max_edge: u32, quality: f64) -> Fut<'_, Result<Jpeg, String>> {
        Box::pin(async move {
            // A frame newer than `after_ms`: wait for that moment (at most
            // 1.5 s, as the cable helper does).
            let ahead = after_ms - now_ms();
            if ahead > 0 {
                tokio::time::sleep(Duration::from_millis(ahead.min(1500) as u64)).await;
            }
            let shot = match self.untouched().filter(|s| s.done.elapsed() < REUSE && s.unix_ms >= after_ms) {
                Some(s) => s,
                None => self.capture().await?,
            };
            let rgb = shot.rgb.clone();
            let (data, width, height) =
                tokio::task::spawn_blocking(move || encode(&rgb, max_edge, quality)).await.map_err(|e| format!("encoding failed: {e}"))??;
            Ok(Jpeg {
                data,
                width,
                height,
                native_width: shot.rgb.width(),
                native_height: shot.rgb.height(),
                frame_ms: shot.unix_ms,
            })
        })
    }

    fn gray(&self, max_edge: u32) -> Fut<'_, Result<GrayFrame, String>> {
        Box::pin(async move {
            // Always a new screenshot: a look is the screen now.
            let shot = self.capture().await?;
            let gray = if max_edge == GRAY_EDGE {
                shot.gray.clone()
            } else {
                let rgb = shot.rgb.clone();
                tokio::task::spawn_blocking(move || gray_of(&rgb, max_edge)).await.map_err(|e| format!("scaling failed: {e}"))??
            };
            Ok(GrayFrame { gray, native_width: shot.rgb.width(), native_height: shot.rgb.height(), frame_ms: shot.unix_ms })
        })
    }

    fn pace(&self) -> Pace {
        Pace::Shots
    }

    fn seed(&self) -> Option<GrayFrame> {
        let s = self.untouched().filter(|s| s.done.elapsed() < SEED_MAX)?;
        Some(GrayFrame { gray: s.gray.clone(), native_width: s.rgb.width(), native_height: s.rgb.height(), frame_ms: s.unix_ms })
    }

    fn touched(&self) {
        *self.touched_at.lock().unwrap() = Some(Instant::now());
    }

    fn known_size(&self) -> Option<(u32, u32)> {
        self.last.lock().unwrap().as_ref().map(|s| (s.rgb.width(), s.rgb.height()))
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! A phone over Wi-Fi for the tests: a screen of one brightness that
    //! takes `shot` per screenshot, an app list, a pasteboard. Records
    //! every UDID a command was sent to.
    use super::*;

    pub struct FakeDevice {
        pub devices: Mutex<Vec<PhoneDevice>>,
        pub level: Mutex<u8>,
        pub shot: Duration,
        pub shots: AtomicU64,
        pub apps_asked: AtomicU64,
        pub apps: Mutex<Vec<PhoneApp>>,
        pub launched: Mutex<Vec<String>>,
        pub pasted: Mutex<Vec<String>>,
        pub udids: Mutex<Vec<String>>,
        pub fail: Mutex<Option<String>>,
        /// Each screenshot, the screen's brightness after it (an animation).
        pub then: Mutex<Vec<u8>>,
    }

    impl FakeDevice {
        pub fn new(shot: Duration) -> FakeDevice {
            FakeDevice {
                devices: Mutex::new(vec![
                    PhoneDevice {
                        udid: "00008140-000C685E0EE0801C".into(),
                        name: "Owner's iPhone 16 Pro".into(),
                        model: "iPhone 16 Pro".into(),
                        state: "available".into(),
                        reachable: true,
                    },
                    PhoneDevice {
                        udid: TEST_UDID.into(),
                        name: "Test iPhone".into(),
                        model: "iPhone XR".into(),
                        state: "connected".into(),
                        reachable: true,
                    },
                ]),
                level: Mutex::new(0),
                shot,
                shots: AtomicU64::new(0),
                apps_asked: AtomicU64::new(0),
                apps: Mutex::new(vec![
                    PhoneApp { name: "Calculator".into(), bundle_id: "com.apple.calculator".into() },
                    PhoneApp { name: "Settings".into(), bundle_id: "com.apple.Preferences".into() },
                    PhoneApp { name: "Telegram".into(), bundle_id: "ph.telegra.Telegraph".into() },
                    PhoneApp { name: "TikTok".into(), bundle_id: "com.zhiliaoapp.musically".into() },
                    PhoneApp { name: "TikTok Studio".into(), bundle_id: "com.ss.iphone.tt.creator".into() },
                ]),
                launched: Mutex::new(Vec::new()),
                pasted: Mutex::new(Vec::new()),
                udids: Mutex::new(Vec::new()),
                fail: Mutex::new(None),
                then: Mutex::new(Vec::new()),
            }
        }

        fn used(&self, udid: &str) -> Result<(), String> {
            self.udids.lock().unwrap().push(udid.to_string());
            match self.fail.lock().unwrap().clone() {
                Some(why) => Err(why),
                None => Ok(()),
            }
        }
    }

    pub const TEST_UDID: &str = "00008020-0003650E0192002E";

    /// A small PNG of one brightness (the real phone's is 828x1792; this
    /// is 207x448 so the tests stay quick, and says so as its size).
    pub fn png(level: u8) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(207, 448, image::Rgb([level, level, level]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    impl Device for FakeDevice {
        fn list(&self) -> Fut<'_, Result<Vec<PhoneDevice>, String>> {
            Box::pin(async move { Ok(self.devices.lock().unwrap().clone()) })
        }
        fn screenshot<'a>(&'a self, udid: &'a str) -> Fut<'a, Result<Vec<u8>, String>> {
            Box::pin(async move {
                self.used(udid)?;
                tokio::time::sleep(self.shot).await;
                self.shots.fetch_add(1, Ordering::SeqCst);
                let level = *self.level.lock().unwrap();
                let mut then = self.then.lock().unwrap();
                if !then.is_empty() {
                    *self.level.lock().unwrap() = then.remove(0);
                }
                Ok(png(level))
            })
        }
        fn apps<'a>(&'a self, udid: &'a str) -> Fut<'a, Result<Vec<PhoneApp>, String>> {
            Box::pin(async move {
                self.used(udid)?;
                self.apps_asked.fetch_add(1, Ordering::SeqCst);
                Ok(self.apps.lock().unwrap().clone())
            })
        }
        fn launch<'a>(&'a self, udid: &'a str, bundle_id: &'a str) -> Fut<'a, Result<(), String>> {
            Box::pin(async move {
                self.used(udid)?;
                tokio::time::sleep(Duration::from_millis(400)).await;
                self.launched.lock().unwrap().push(bundle_id.to_string());
                let mut l = self.level.lock().unwrap();
                *l = l.wrapping_add(100);
                Ok(())
            })
        }
        fn paste<'a>(&'a self, udid: &'a str, text: &'a str) -> Fut<'a, Result<(), String>> {
            Box::pin(async move {
                self.used(udid)?;
                tokio::time::sleep(Duration::from_millis(300)).await;
                self.pasted.lock().unwrap().push(text.to_string());
                Ok(())
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;
    use serde_json::json;

    /// `devicectl list devices --json-output -` as Xcode 27.2 writes it,
    /// cut down: a simulator, the owner's own phone (available), the test
    /// phone (connected, over Wi-Fi), a watch.
    fn list_doc() -> Value {
        json!({
            "info": {"commandType": "devicectl.list.devices", "jsonVersion": 5, "outcome": "success", "version": "651.32"},
            "result": {"devices": [
                {"identifier": "5E933679-6013-4A46-B7AA-6E65B7E4D860", "visibilityClass": "simulators",
                 "deviceProperties": {"bootState": "shutdown", "name": "AFTR Duo Screenshots"},
                 "hardwareProperties": {"marketingName": "iPhone Duo", "platform": "iOS", "reality": "simulated", "udid": "5E933679-6013-4A46-B7AA-6E65B7E4D860"},
                 "properties": {"connection": {"pairingState": "paired", "state": "disconnected", "transportType": "sameMachine"},
                                "hardware": {"marketingName": "iPhone Duo", "platform": "iOS", "reality": "simulated", "udid": "5E933679-6013-4A46-B7AA-6E65B7E4D860"},
                                "state": {"bootState": "shutdown", "name": "AFTR Duo Screenshots"}}},
                {"identifier": "04CEBBC5-D386-53B7-8350-6DF362FF9603", "visibilityClass": "default",
                 "connectionProperties": {"pairingState": "paired", "transportType": "localNetwork", "tunnelState": "disconnected"},
                 "deviceProperties": {"bootState": "booted", "name": "Aleksandr iPhone 16 Pro"},
                 "hardwareProperties": {"marketingName": "iPhone 16 Pro", "platform": "iOS", "reality": "physical", "udid": "00008140-000C685E0EE0801C"},
                 "properties": {"connection": {"pairingState": "paired", "state": "disconnected", "transportType": "localNetwork"},
                                "hardware": {"marketingName": "iPhone 16 Pro", "platform": "iOS", "reality": "physical", "udid": "00008140-000C685E0EE0801C"},
                                "state": {"bootState": "booted", "name": "Aleksandr iPhone 16 Pro"}}},
                {"identifier": "6D811468-3D4D-5BC6-B726-7E32E679B379", "visibilityClass": "default",
                 "connectionProperties": {"pairingState": "paired", "transportType": "localNetwork", "tunnelState": "connected"},
                 "deviceProperties": {"bootState": "booted", "name": "Aleksandr\u{2019}s iPhone"},
                 "hardwareProperties": {"marketingName": "iPhone XR", "platform": "iOS", "reality": "physical", "udid": "00008020-0003650E0192002E"},
                 "properties": {"connection": {"pairingState": "paired", "state": "connected", "transportType": "localNetwork"},
                                "hardware": {"marketingName": "iPhone XR", "platform": "iOS", "reality": "physical", "udid": "00008020-0003650E0192002E"},
                                "state": {"bootState": "booted", "name": "Aleksandr\u{2019}s iPhone"}}},
                {"identifier": "771B1DAF-C5E6-59D7-8BFE-B878C99174DC", "visibilityClass": "default",
                 "hardwareProperties": {"marketingName": "Apple Watch Series 5", "platform": "watchOS", "reality": "physical", "udid": "00008006-000E75500CEB002E"},
                 "properties": {"connection": {"pairingState": "paired", "state": "disconnected"},
                                "hardware": {"platform": "watchOS", "reality": "physical", "udid": "00008006-000E75500CEB002E"},
                                "state": {"bootState": "booted", "name": "Apple Watch"}}}
            ]}
        })
    }

    #[test]
    fn the_list_has_the_physical_ios_devices_and_their_state() {
        let d = parse_devices(&list_doc());
        assert_eq!(d.len(), 2, "no simulator, no watch: {d:?}");
        assert_eq!(
            d[0],
            PhoneDevice {
                udid: "00008140-000C685E0EE0801C".into(),
                name: "Aleksandr iPhone 16 Pro".into(),
                model: "iPhone 16 Pro".into(),
                state: "available".into(),
                reachable: true
            }
        );
        assert_eq!((d[1].udid.as_str(), d[1].name.as_str(), d[1].state.as_str()), ("00008020-0003650E0192002E", "Aleksandr\u{2019}s iPhone", "connected"));
        // Only the deprecated fields (an older Xcode): the same answer.
        let mut old = list_doc();
        for dev in old["result"]["devices"].as_array_mut().unwrap() {
            dev.as_object_mut().unwrap().remove("properties");
        }
        let o = parse_devices(&old);
        assert_eq!(o.iter().map(|x| (x.udid.as_str(), x.state.as_str())).collect::<Vec<_>>(), [
            ("00008140-000C685E0EE0801C", "available"),
            ("00008020-0003650E0192002E", "connected")
        ]);
        assert!(parse_devices(&json!({"result": {}})).is_empty());
    }

    #[test]
    fn the_app_list_and_which_app_is_meant() {
        let doc = json!({"result": {"apps": [
            {"name": "Calculator", "bundleIdentifier": "com.apple.calculator", "hidden": false},
            {"name": "Settings", "bundleIdentifier": "com.apple.Preferences"},
            {"name": "App Store", "bundleIdentifier": "com.apple.AppStore"},
            {"name": "TikTok", "bundleIdentifier": "com.zhiliaoapp.musically"},
            {"name": "TikTok Studio", "bundleIdentifier": "com.ss.iphone.tt.creator"},
            {"name": "Clock", "bundleIdentifier": "com.apple.mobiletimer"},
            {"name": "Broken", "bundleIdentifier": ""}
        ]}});
        let apps = parse_apps(&doc);
        assert_eq!(apps.len(), 6);
        assert_eq!(resolve_app(&apps, "com.apple.Preferences"), Ok("com.apple.Preferences".into()));
        assert_eq!(resolve_app(&apps, "com.apple.preferences"), Ok("com.apple.Preferences".into()));
        assert_eq!(resolve_app(&apps, "settings"), Ok("com.apple.Preferences".into()));
        assert_eq!(resolve_app(&apps, " app\u{a0}store "), Ok("com.apple.AppStore".into()));
        assert_eq!(resolve_app(&apps, "Calc"), Ok("com.apple.calculator".into()), "a unique prefix");
        assert_eq!(resolve_app(&apps, "tiktok"), Ok("com.zhiliaoapp.musically".into()), "the exact name wins over the prefix");
        assert_eq!(resolve_app(&apps, "tik"), Err(vec!["TikTok".to_string(), "TikTok Studio".to_string()]));
        assert_eq!(resolve_app(&apps, "Maps"), Err(vec![]));
        assert_eq!(resolve_app(&apps, ""), Err(vec![]));
    }

    #[test]
    fn devicectl_errors_are_its_own_words_short() {
        let doc = json!({
            "error": {"code": 1000, "domain": "com.apple.dt.CoreDeviceError",
                      "userInfo": {"NSLocalizedDescription": {"string": "The specified device was not found. (Name: 00000000-0000000000000000)"}}},
            "info": {"outcome": "failed"}
        });
        assert_eq!(ctl_error(Some(&doc), "ERROR: whatever"), "The specified device was not found. (Name: 00000000-0000000000000000)");
        let nested = json!({"error": {"userInfo": {
            "NSLocalizedDescription": {"string": "Failed to launch."},
            "NSUnderlyingError": {"error": {"userInfo": {"NSLocalizedDescription": {"string": "The device is locked."}}}}
        }}});
        assert_eq!(ctl_error(Some(&nested), ""), "Failed to launch. (The device is locked.)");
        assert_eq!(ctl_error(None, "progress\nERROR: The device is not connected (code 4)\n  more"), "The device is not connected (code 4)");
        assert_eq!(ctl_error(None, ""), "devicectl failed");
        assert!(ctl_error(None, &"x".repeat(500)).chars().count() <= 240);
    }

    #[test]
    fn frames_scale_to_the_long_edge() {
        assert_eq!(fit(828, 1792, 768), (355, 768));
        assert_eq!(fit(828, 1792, 0), (828, 1792));
        assert_eq!(fit(828, 1792, 4000), (828, 1792));
        assert_eq!(fit(828, 1792, GRAY_EDGE), (207, 448));
        let rgb = image::RgbImage::from_pixel(828, 1792, image::Rgb([10, 200, 30]));
        let (jpeg, w, h) = encode(&rgb, 768, 0.8).unwrap();
        assert_eq!((w, h), (355, 768));
        assert_eq!(&jpeg[..2], [0xff, 0xd8]);
        let back = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((back.width(), back.height()), (355, 768));
        let g = gray_of(&rgb, GRAY_EDGE).unwrap();
        assert_eq!((g.w, g.h), (207, 448));
    }

    #[tokio::test(start_paused = true)]
    async fn only_the_chosen_phone_is_ever_used() {
        let dev = Arc::new(FakeDevice::new(Duration::from_millis(500)));
        let phone = NetPhone::new(dev.clone(), TEST_UDID.into());
        let s = phone.status().await;
        assert!(s.ok, "{s:?}");
        assert_eq!((s.device.as_str(), s.source.as_str(), s.width, s.height), ("Test iPhone", "network", 828, 1792));
        let g = phone.gray(GRAY_EDGE).await.unwrap();
        assert_eq!((g.gray.w, g.gray.h, g.native_width, g.native_height), (207, 448, 207, 448));
        let j = phone.jpeg(0, 0, 0.8).await.unwrap();
        assert_eq!(dev.shots.load(Ordering::SeqCst), 1, "a frame right after a look reuses its screenshot");
        assert_eq!(&j.data[..2], [0xff, 0xd8]);
        phone.resolve("Telegram").await.unwrap();
        phone.launch("ph.telegra.Telegraph").await.unwrap();
        phone.paste("привет").await.unwrap();
        assert!(dev.udids.lock().unwrap().iter().all(|u| u == TEST_UDID), "{:?}", dev.udids.lock().unwrap());

        // The chosen phone gone from the list: not ok, and the other phone
        // this Mac sees is NOT taken instead.
        let other = Arc::new(FakeDevice::new(Duration::from_millis(500)));
        other.devices.lock().unwrap().retain(|d| d.udid != TEST_UDID);
        let phone = NetPhone::new(other.clone(), TEST_UDID.into());
        let s = phone.status().await;
        assert!(!s.ok);
        assert!(s.error.contains("is not visible to this Mac"), "{s:?}");
        assert!(s.device.is_empty(), "the owner's own phone is not shown as the phone: {s:?}");
        assert!(other.udids.lock().unwrap().is_empty(), "nothing was sent to any phone");
    }

    #[tokio::test(start_paused = true)]
    async fn a_frame_after_a_touch_is_a_new_screenshot_and_failures_show() {
        let dev = Arc::new(FakeDevice::new(Duration::from_millis(500)));
        let phone = NetPhone::new(dev.clone(), TEST_UDID.into());
        phone.gray(GRAY_EDGE).await.unwrap();
        assert!(phone.seed().is_some(), "nothing was done since");
        phone.touched();
        assert!(phone.seed().is_none(), "a look before the touch is stale");
        phone.jpeg(0, 768, 0.8).await.unwrap();
        assert_eq!(dev.shots.load(Ordering::SeqCst), 2);
        // Too old to reuse.
        tokio::time::sleep(Duration::from_millis(200)).await;
        phone.jpeg(0, 768, 0.8).await.unwrap();
        assert_eq!(dev.shots.load(Ordering::SeqCst), 3);

        *dev.fail.lock().unwrap() = Some("The device is locked.".into());
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(phone.jpeg(0, 768, 0.8).await.err().as_deref(), Some("The device is locked."));
        let s = phone.status().await;
        assert!(!s.ok && s.error == "The device is locked.", "{s:?}");
        *dev.fail.lock().unwrap() = None;
        phone.gray(GRAY_EDGE).await.unwrap();
        assert!(phone.status().await.ok);
    }

    #[tokio::test(start_paused = true)]
    async fn an_app_installed_since_the_list_is_found_and_a_miss_says_so() {
        let dev = Arc::new(FakeDevice::new(Duration::from_millis(500)));
        let phone = NetPhone::new(dev.clone(), TEST_UDID.into());
        assert_eq!(phone.resolve("calculator").await.unwrap(), "com.apple.calculator");
        assert_eq!(phone.resolve("Settings").await.unwrap(), "com.apple.Preferences");
        assert_eq!(dev.apps_asked.load(Ordering::SeqCst), 1, "the list is cached");
        dev.apps.lock().unwrap().push(PhoneApp { name: "Notes".into(), bundle_id: "com.apple.mobilenotes".into() });
        assert_eq!(phone.resolve("notes").await.unwrap(), "com.apple.mobilenotes");
        assert_eq!(dev.apps_asked.load(Ordering::SeqCst), 2, "a miss reads the list again");
        assert_eq!(phone.resolve("Maps").await.unwrap_err(), "bad request: no app named \"Maps\" on the phone");
        let e = phone.resolve("tik").await.unwrap_err();
        assert!(e.starts_with("bad request: no app named \"tik\" on the phone (it could be TikTok, TikTok Studio"), "{e}");
    }
}
