//! `phone_control`: the synth drives an iPhone plugged into this Mac
//! (tz-phone-control.md section 2; connect.md "Phone").
//!
//! Eyes: the phone's screen over the cable, from the capture helper
//! (eyes.rs). Hands: the SNTH Phone Bridge board, a Bluetooth keyboard
//! and mouse for the phone, on a USB serial port (hands.rs). The pointer,
//! its calibration and the re-press of a tap that did nothing live here,
//! next to the frames (0.01-0.04 s each) and the board.
//!
//! RPC (`phone_control.status`, `.frame`, `.act`): every call goes through
//! `handle`, the one gate: pause, the switch (off by default and in every
//! preset; macOS only), this synth's permission (asked once / for the
//! session / always, per synth: a grant for Mia is not one for Maya), then
//! the lease: one synth at a time drives the phone, another gets `busy`
//! with the holder's name until the holder has been quiet for
//! `LEASE_QUIET`. Every `act` is one journal line (the action, the point
//! or the keys, the length of typed text and never the text, whether the
//! screen changed, the time it took).
//!
//! Error classes beyond the usual `denied:` / `bad request:`:
//! `no_screen:` (no capture), `no_hands:` (no board, or the phone is not
//! on its Bluetooth), `busy:` (another synth holds the phone),
//! `unsupported:` (text the keyboard cannot type).

pub mod diff;
pub mod eyes;
pub mod hands;
pub mod pointer;

use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use base64::Engine;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::time::Instant;

use crate::core::{Shared, LEASE_QUIET};
use crate::grant::{ApprovalRequest, Remember};
use crate::journal::Entry;
use crate::settings::{PhoneSettings, PhoneTuning};

use diff::{GRAY_EDGE, MIN_CHANGED, STILL_LEVEL};
use eyes::{Eyes, GrayFrame, ScreenStatus};
use hands::{Hands, HandsError, HandsStatus};
use pointer::{Calib, Pointer, BOARD_BASE};

/// The capability's name (settings, grants, the journal, the wire).
pub const CAP: &str = "phone_control";
/// The one scope a grant has: control of the phone.
pub const SCOPE: &str = "control";

/// The longest `stable_ms` wait, and how often the screen is looked at
/// while waiting.
const STABLE_MAX: Duration = Duration::from_millis(2500);
const STABLE_POLL: Duration = Duration::from_millis(100);
/// After a tap, the screen counts as settled after this much quiet
/// (two looks in a row alike, as in the prototype).
const AFTER_TAP_QUIET: Duration = Duration::from_millis(200);
const AFTER_TAP_MAX: Duration = Duration::from_millis(1200);
const AFTER_REPRESS_MAX: Duration = Duration::from_millis(1000);
/// Before the second press.
const REPRESS_PAUSE: Duration = Duration::from_millis(300);
/// A long press without `hold_ms`.
const LONG_PRESS: Duration = Duration::from_millis(900);
/// Between the keys of one `key` action.
const KEY_GAP: Duration = Duration::from_millis(150);
/// The most text one `type` takes (the board reads lines of 4 KiB).
const MAX_TEXT: usize = 1000;
const MAX_KEYS: usize = 10;
/// The screen size used when no frame tells it (the iPhone XR).
const DEFAULT_SIZE: (u32, u32) = (828, 1792);
/// A swipe: the pointer moves this many units per report.
const SWIPE_STEP: i64 = 12;

/// The phone works in the app on a Mac only. (The gate's tests run with
/// fakes on every system.)
pub fn supported() -> bool {
    cfg!(any(target_os = "macos", test))
}

/// `capabilities.phone_control` in the hello: the switch, and the longest
/// request line. Only the switch: whether the cable and the board are
/// there changes by the minute and is `phone_control.status`.
pub fn wire_value(s: &PhoneSettings) -> Value {
    json!({"enabled": s.enabled && supported(), "max_request_bytes": crate::rpc::MAX_REQUEST_LINE})
}

/// The eyes and hands while the phone is switched on.
pub struct Rig {
    pub eyes: Arc<dyn Eyes>,
    pub hands: Arc<dyn Hands>,
}

/// Makes the eyes and hands (the helper and the board on a Mac; fakes in
/// tests).
pub trait Factory: Send + Sync {
    fn start(&self, tuning: &PhoneTuning) -> Rig;
}

/// The real thing: the capture helper and the board's serial port.
pub struct SystemFactory {
    /// The helper program; None = next to this program (`eyes::helper_path`).
    pub helper: Option<std::path::PathBuf>,
}

impl Factory for SystemFactory {
    fn start(&self, tuning: &PhoneTuning) -> Rig {
        if !supported() {
            let why = "phone control works in SNTH Connect on a Mac only".to_string();
            return Rig { eyes: Arc::new(eyes::NoEyes(why.clone())), hands: Arc::new(hands::NoHands(why)) };
        }
        let path = self.helper.clone().unwrap_or_else(eyes::helper_path);
        let eyes: Arc<dyn Eyes> = Arc::new(eyes::HelperEyes::start(Arc::new(eyes::ProgramSpawn(path))));
        #[cfg(unix)]
        let hands: Arc<dyn Hands> = Arc::new(hands::Board::start(
            Arc::new(hands::posix::SystemOpener),
            Some(tuning.port.clone()).filter(|p| !p.is_empty()),
        ));
        #[cfg(not(unix))]
        let hands: Arc<dyn Hands> = {
            let _ = tuning;
            Arc::new(hands::NoHands("no serial port support on this system".into()))
        };
        Rig { eyes, hands }
    }
}

/// Who holds the phone.
#[derive(Debug, Clone)]
struct Lease {
    synth_id: String,
    name: String,
    last: Instant,
}

/// The phone's state in the core: the running eyes and hands, the lease,
/// and the pointer (its lock also makes acts run one at a time).
pub struct State {
    factory: RwLock<Arc<dyn Factory>>,
    rig: Mutex<Option<(PhoneTuning, Arc<Rig>)>>,
    lease: Mutex<Option<Lease>>,
    pointer: tokio::sync::Mutex<Pointer>,
}

impl Default for State {
    fn default() -> Self {
        State {
            factory: RwLock::new(Arc::new(SystemFactory { helper: None })),
            rig: Mutex::new(None),
            lease: Mutex::new(None),
            pointer: tokio::sync::Mutex::new(Pointer::default()),
        }
    }
}

impl State {
    /// Use other eyes and hands (tests, the self-test). Stops what runs.
    pub fn set_factory(&self, f: Arc<dyn Factory>) {
        *self.factory.write().unwrap() = f;
        self.stop();
    }

    /// The running rig, started now if needed. Needs a tokio runtime.
    fn rig(&self, s: &PhoneSettings) -> Arc<Rig> {
        let mut rig = self.rig.lock().unwrap();
        if let Some((tuning, r)) = rig.as_ref() {
            if *tuning == s.tuning {
                return r.clone();
            }
        }
        let started = Arc::new(self.factory.read().unwrap().start(&s.tuning));
        *rig = Some((s.tuning.clone(), started.clone()));
        started
    }

    /// Switched off: the helper is killed, the board's port closed (RTS,
    /// then DTR), the pointer forgotten.
    pub fn stop(&self) {
        let old = self.rig.lock().unwrap().take();
        drop(old);
        if let Ok(mut p) = self.pointer.try_lock() {
            p.forget();
        }
    }

    /// Start or stop with the switch. Starting needs a runtime; without
    /// one it happens at the first use.
    pub(crate) fn sync(&self, s: &PhoneSettings) {
        if s.enabled && supported() {
            if tokio::runtime::Handle::try_current().is_ok() {
                let _ = self.rig(s);
            }
        } else if self.rig.lock().unwrap().is_some() {
            self.stop();
        }
    }

    /// The synth that holds the phone: (id, name), while it acted less
    /// than `LEASE_QUIET` ago.
    pub fn holder(&self) -> Option<(String, String)> {
        self.lease
            .lock()
            .unwrap()
            .as_ref()
            .filter(|l| l.last.elapsed() < LEASE_QUIET)
            .map(|l| (l.synth_id.clone(), l.name.clone()))
    }

    /// Take (or keep) the phone for this synth. Err(holder's name).
    fn lease_take(&self, synth_id: &str, name: &str) -> Result<(), String> {
        let mut lease = self.lease.lock().unwrap();
        if let Some(l) = lease.as_ref() {
            if l.synth_id != synth_id && l.last.elapsed() < LEASE_QUIET {
                return Err(l.name.clone());
            }
        }
        *lease = Some(Lease { synth_id: synth_id.to_string(), name: name.to_string(), last: Instant::now() });
        Ok(())
    }

    /// Let go (the synth was unlinked).
    pub(crate) fn release(&self, synth_id: &str) {
        let mut lease = self.lease.lock().unwrap();
        if lease.as_ref().is_some_and(|l| l.synth_id == synth_id) {
            *lease = None;
        }
    }

    #[cfg(test)]
    pub(crate) fn lease_age(&self, by: Duration) {
        if let Some(l) = self.lease.lock().unwrap().as_mut() {
            l.last -= by;
        }
    }
}

/// What the owner's Phone section shows.
#[derive(Debug, Clone, Serialize)]
pub struct PhoneView {
    pub supported: bool,
    pub enabled: bool,
    pub screen: ScreenStatus,
    pub hands: HandsStatus,
    /// The synth that controls the phone right now, if any.
    pub holder: Option<String>,
}

/// The owner's look at the phone (no gate: the owner's own screen).
pub(crate) async fn view(shared: &Arc<Shared>) -> PhoneView {
    let s = shared.settings().phone_control;
    let holder = shared.phone.holder().map(|(_, n)| n);
    if !(s.enabled && supported()) {
        let off = if supported() { "phone control is off" } else { "phone control works on a Mac only" };
        return PhoneView {
            supported: supported(),
            enabled: s.enabled,
            screen: ScreenStatus { error: off.into(), ..Default::default() },
            hands: HandsStatus { error: off.into(), ..Default::default() },
            holder,
        };
    }
    let rig = shared.phone.rig(&s);
    let (screen, hands) = tokio::join!(rig.eyes.status(), rig.hands.status());
    PhoneView { supported: true, enabled: true, screen, hands, holder }
}

/// The newest frame for the owner's preview, as JPEG.
pub(crate) async fn preview(shared: &Arc<Shared>, max_edge: u32) -> Result<eyes::Jpeg, String> {
    let s = shared.settings().phone_control;
    if !(s.enabled && supported()) {
        return Err("phone control is off".into());
    }
    shared.phone.rig(&s).eyes.jpeg(0, max_edge, 0.8).await
}

// --- the gate ---

fn paused() -> String {
    "denied: the phone is paused by the owner".into()
}
fn off() -> String {
    "denied: phone control is turned off on this computer; only the owner can turn it on (SNTH Connect, Phone)".into()
}
fn no_screen(why: String) -> String {
    format!("no_screen: {why}")
}
fn hands_error(e: HandsError) -> String {
    match e {
        HandsError::NoBoard(why) => format!("no_hands: {why}"),
        HandsError::NotConnected => {
            "no_hands: the phone is not connected to the board over Bluetooth (pair \"SNTH Phone Bridge\" in the phone's Bluetooth settings)".into()
        }
        HandsError::Failed(why) => format!("failed: the board: {why}"),
    }
}

/// Every `phone_control.*` call comes through here.
pub async fn handle(shared: &Arc<Shared>, method: &str, params: &Value) -> Result<Value, String> {
    let action = match method.strip_prefix("phone_control.") {
        Some(a @ ("status" | "frame" | "act")) => a,
        _ => return Err(format!("denied: unknown method {method:?}")),
    };
    // A bad request is refused before the owner is asked anything.
    let act = if action == "act" { Some(ActRequest::parse(params)?) } else { None };
    admit(shared, params, action, act.as_ref()).await?;
    let s = shared.settings().phone_control;
    let rig = shared.phone.rig(&s);
    match action {
        "status" => Ok(status(shared, &rig).await),
        "frame" => {
            take_lease(shared, None)?;
            frame(&rig, params).await
        }
        _ => {
            let req = act.expect("parsed above");
            take_lease(shared, Some(&req))?;
            run_act(shared, &rig, &s.tuning, req).await
        }
    }
}

/// Pause, the switch, this synth's permission.
async fn admit(shared: &Arc<Shared>, params: &Value, action: &str, act: Option<&ActRequest>) -> Result<(), String> {
    let refuse = |why: String| {
        if let Some(req) = act {
            shared.journal().record(Entry::new(CAP, "phone_control.act", req.target(), why.clone()));
        }
        Err(why)
    };
    if shared.paused() {
        return refuse(paused());
    }
    if !supported() {
        return refuse("unavailable: phone control works in SNTH Connect on a Mac only".into());
    }
    if !shared.settings().phone_control.enabled {
        return refuse(off());
    }
    if shared.grant_allows(CAP, SCOPE) {
        return Ok(());
    }
    let reason = params.get("reason").and_then(Value::as_str).unwrap_or("");
    let ask = ApprovalRequest::new(CAP, SCOPE, reason, "control your phone (see its screen, tap and type on it)")
        .with_details(vec![("first".to_string(), action.to_string())])
        .with_remember(Remember::Any);
    let answer = shared.ask_scoped(CAP, SCOPE, ask).await;
    if !answer.allow {
        shared.journal().record(Entry::new(CAP, format!("phone_control.{action}").as_str(), "", "denied: owner"));
        return Err("denied: the owner did not allow this synth to control the phone".into());
    }
    // Paused or switched off while the owner decided.
    if shared.paused() {
        return refuse(paused());
    }
    if !shared.settings().phone_control.enabled {
        return refuse(off());
    }
    Ok(())
}

/// One synth at a time.
fn take_lease(shared: &Arc<Shared>, act: Option<&ActRequest>) -> Result<(), String> {
    let slot = shared.current_slot();
    let id = slot.as_ref().map(|s| s.synth_id.clone()).unwrap_or_default();
    let name = slot.map(|s| s.display_name()).unwrap_or_else(|| "a synth".into());
    shared.phone.lease_take(&id, &name).map_err(|holder| {
        let why = format!("busy: {holder} is controlling the phone right now; try again in a minute");
        if let Some(req) = act {
            shared.journal().record(Entry::new(CAP, "phone_control.act", req.target(), why.clone()));
        }
        why
    })
}

async fn status(shared: &Arc<Shared>, rig: &Rig) -> Value {
    let (screen, hands) = tokio::join!(rig.eyes.status(), rig.hands.status());
    let holder = shared.phone.holder().map(|(_, n)| n).unwrap_or_default();
    json!({"screen": screen, "hands": hands, "lease": {"holder": holder}})
}

/// Wait until the screen stops changing: `quiet` without a change (two
/// looks whose mean difference is under `STILL_LEVEL`), at most `max`.
/// (stable, the last look).
pub(crate) async fn wait_stable(eyes: &dyn Eyes, quiet: Duration, max: Duration) -> Result<(bool, GrayFrame), String> {
    let start = Instant::now();
    let mut since = start;
    let mut prev: Option<GrayFrame> = None;
    loop {
        let g = eyes.gray(GRAY_EDGE).await?;
        let now = Instant::now();
        let still = prev.as_ref().is_some_and(|p| diff::mean_diff(&p.gray, &g.gray) < STILL_LEVEL);
        if !still {
            since = now;
        }
        if prev.is_some() && now.duration_since(since) >= quiet {
            return Ok((true, g));
        }
        if now.duration_since(start) >= max {
            return Ok((false, g));
        }
        prev = Some(g);
        tokio::time::sleep(STABLE_POLL).await;
    }
}

async fn frame(rig: &Rig, params: &Value) -> Result<Value, String> {
    let after_ms = params.get("after_ms").and_then(Value::as_i64).unwrap_or(0).max(0);
    let stable_ms = params.get("stable_ms").and_then(Value::as_u64).unwrap_or(0).min(STABLE_MAX.as_millis() as u64);
    let max_edge = params.get("max_edge").and_then(Value::as_u64).unwrap_or(0).min(8192) as u32;
    let quality = params.get("quality").and_then(Value::as_f64).unwrap_or(0.85).clamp(0.1, 1.0);
    let stable = if stable_ms > 0 {
        wait_stable(rig.eyes.as_ref(), Duration::from_millis(stable_ms), STABLE_MAX).await.map_err(no_screen)?.0
    } else {
        true
    };
    let j = rig.eyes.jpeg(after_ms, max_edge, quality).await.map_err(no_screen)?;
    Ok(json!({
        "data_b64": base64::engine::general_purpose::STANDARD.encode(&j.data),
        "mime": "image/jpeg",
        "width": j.width,
        "height": j.height,
        "native_width": j.native_width,
        "native_height": j.native_height,
        "frame_ms": j.frame_ms,
        "stable": stable,
    }))
}

// --- act ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Tap,
    DoubleTap,
    LongPress,
    Swipe,
    Scroll,
    Key,
    Type,
    Home,
    ReleaseAll,
}

impl Action {
    fn name(self) -> &'static str {
        match self {
            Action::Tap => "tap",
            Action::DoubleTap => "double_tap",
            Action::LongPress => "long_press",
            Action::Swipe => "swipe",
            Action::Scroll => "scroll",
            Action::Key => "key",
            Action::Type => "type",
            Action::Home => "home",
            Action::ReleaseAll => "release_all",
        }
    }

    fn presses(self) -> bool {
        matches!(self, Action::Tap | Action::DoubleTap | Action::LongPress)
    }
}

/// One `phone_control.act`, checked.
#[derive(Debug, Clone)]
pub struct ActRequest {
    pub action: Action,
    pub at: Option<(i64, i64)>,
    pub to: Option<(i64, i64)>,
    pub hold_ms: u64,
    pub keys: Vec<String>,
    pub text: String,
    pub amount: i64,
    pub verify: bool,
}

/// Printable ASCII, newline and tab: what the board's US keyboard types.
fn typeable(text: &str) -> bool {
    text.chars().all(|c| (' '..='~').contains(&c) || c == '\n' || c == '\t')
}

/// A key combination as the firmware takes it: words and `+`.
fn combo_ok(k: &str) -> bool {
    !k.is_empty() && k.len() <= 64 && k.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '_' | '-' | ' '))
}

impl ActRequest {
    pub fn parse(p: &Value) -> Result<ActRequest, String> {
        let action = match p.get("action").and_then(Value::as_str).unwrap_or("") {
            "tap" => Action::Tap,
            "double_tap" => Action::DoubleTap,
            "long_press" => Action::LongPress,
            "swipe" => Action::Swipe,
            "scroll" => Action::Scroll,
            "key" => Action::Key,
            "type" => Action::Type,
            "home" => Action::Home,
            "release_all" => Action::ReleaseAll,
            "" => return Err("bad request: params.action is required".into()),
            other => {
                return Err(format!(
                    "bad request: unknown action {other:?} (tap, double_tap, long_press, swipe, scroll, key, type, home, release_all)"
                ))
            }
        };
        let num = |k: &str| p.get(k).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f.round() as i64)));
        let point = |kx: &str, ky: &str| match (num(kx), num(ky)) {
            (Some(x), Some(y)) => Some((x, y)),
            _ => None,
        };
        let at = point("x", "y");
        let to = point("x2", "y2");
        let hold_ms = p.get("hold_ms").and_then(Value::as_u64).unwrap_or(0).min(10_000);
        let amount = num("amount").unwrap_or(0).clamp(-200, 200);
        let keys: Vec<String> = match p.get("keys") {
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(|s| s.trim().to_lowercase()).collect(),
            Some(Value::String(s)) => vec![s.trim().to_lowercase()],
            _ => Vec::new(),
        };
        let text = p.get("text").and_then(Value::as_str).unwrap_or("").to_string();
        let verify = p.get("verify").and_then(Value::as_bool).unwrap_or(action.presses());
        match action {
            Action::Tap | Action::DoubleTap | Action::LongPress if at.is_none() => {
                return Err(format!("bad request: {} needs x and y (frame pixels of the native size)", action.name()))
            }
            Action::Swipe if at.is_none() || to.is_none() => return Err("bad request: swipe needs x, y, x2 and y2".into()),
            Action::Scroll if amount == 0 => {
                return Err("bad request: scroll needs a non-zero amount (positive = see content further down)".into())
            }
            Action::Key => {
                if keys.is_empty() {
                    return Err("bad request: key needs keys, e.g. [\"cmd+h\"]".into());
                }
                if keys.len() > MAX_KEYS {
                    return Err(format!("bad request: at most {MAX_KEYS} keys in one action"));
                }
                if let Some(bad) = keys.iter().find(|k| !combo_ok(k)) {
                    return Err(format!("bad request: {bad:?} is not a key combination (cmd+space, enter, esc, tab...)"));
                }
            }
            Action::Type => {
                if text.is_empty() {
                    return Err("bad request: type needs text".into());
                }
                if !typeable(&text) {
                    return Err("unsupported: non_ascii text, paste is not configured".into());
                }
                if text.chars().count() > MAX_TEXT {
                    return Err(format!("bad request: at most {MAX_TEXT} characters in one type"));
                }
            }
            _ => {}
        }
        Ok(ActRequest { action, at, to, hold_ms, keys, text, amount, verify })
    }

    /// What the journal says the action was: never the typed text.
    pub fn target(&self) -> String {
        let pt = |p: Option<(i64, i64)>| p.map(|(x, y)| format!("{x},{y}")).unwrap_or_default();
        match self.action {
            Action::Tap | Action::DoubleTap | Action::LongPress => format!("{} {}", self.action.name(), pt(self.at)),
            Action::Swipe => format!("swipe {} -> {}", pt(self.at), pt(self.to)),
            Action::Scroll => format!("scroll {}", self.amount),
            Action::Key => format!("key {}", self.keys.join(", ")),
            Action::Type => format!("type ({} characters)", self.text.chars().count()),
            Action::Home => "home".into(),
            Action::ReleaseAll => "release_all".into(),
        }
    }
}

/// How an act went.
#[derive(Debug, Clone, Default)]
struct Outcome {
    changed: bool,
    verified: bool,
    repressed: bool,
}

/// The board, stopped by a pause or the switch between commands.
struct Driver<'a> {
    shared: &'a Arc<Shared>,
    hands: &'a dyn Hands,
}

impl Driver<'_> {
    async fn send(&self, cmd: Value, timeout: Duration) -> Result<Value, String> {
        if self.shared.paused() {
            return Err(paused());
        }
        if !self.shared.settings().phone_control.enabled {
            return Err(off());
        }
        self.hands.call(cmd, timeout).await.map_err(hands_error)
    }

    async fn all(&self, cmds: Vec<(Value, Duration)>) -> Result<(), String> {
        for (cmd, t) in cmds {
            self.send(cmd, t).await?;
        }
        Ok(())
    }

    async fn key(&self, combo: &str) -> Result<(), String> {
        self.send(json!({"cmd": "key", "combo": combo}), BOARD_BASE).await.map(|_| ())
    }
}

async fn run_act(shared: &Arc<Shared>, rig: &Rig, tuning: &PhoneTuning, req: ActRequest) -> Result<Value, String> {
    let started = Instant::now();
    let target = req.target();
    // One act at a time; the pointer is the phone's, not a synth's.
    let mut ptr = shared.phone.pointer.lock().await;
    let result = act_inner(shared, rig, tuning, &req, &mut ptr).await;
    let mut entry = Entry::new(
        CAP,
        "phone_control.act",
        target,
        match &result {
            Ok(o) if !o.verified => "ok".to_string(),
            Ok(o) if o.changed && o.repressed => "ok: re-pressed".to_string(),
            Ok(o) if o.changed => "ok".to_string(),
            Ok(o) if o.repressed => "ok: no change after a re-press".to_string(),
            Ok(_) => "ok: no change".to_string(),
            Err(why) => why.clone(),
        },
    );
    entry.duration_ms = Some(started.elapsed().as_millis() as u64);
    shared.journal().record(entry);
    let o = result?;
    let pointer = ptr.position().map(|(x, y)| json!({"x": x, "y": y})).unwrap_or(Value::Null);
    Ok(json!({"ok": true, "changed": o.changed, "repressed": o.repressed, "verified": o.verified, "pointer": pointer}))
}

async fn act_inner(shared: &Arc<Shared>, rig: &Rig, tuning: &PhoneTuning, req: &ActRequest, ptr: &mut Pointer) -> Result<Outcome, String> {
    let c = Calib::from_tuning(tuning);
    let board = Driver { shared, hands: rig.hands.as_ref() };
    let eyes = rig.eyes.as_ref();
    let look = || async { eyes.gray(GRAY_EDGE).await.map_err(no_screen) };

    // The screen's size: from a frame when there is one. Checking the
    // result needs frames; acting blind does not.
    let size = match eyes.gray(GRAY_EDGE).await {
        Ok(g) if g.native_width > 0 && g.native_height > 0 => (g.native_width, g.native_height),
        Ok(_) => DEFAULT_SIZE,
        Err(why) if req.verify => return Err(no_screen(why)),
        Err(_) => DEFAULT_SIZE,
    };
    let (w, h) = size;
    let mut out = Outcome { changed: true, verified: false, repressed: false };

    match req.action {
        Action::Tap | Action::DoubleTap | Action::LongPress => {
            let (x, y) = req.at.expect("checked in parse");
            let route = ptr.route(x, y, w, h, false, Instant::now(), &c)?;
            board.all(route.cmds).await?;
            tokio::time::sleep(c.settle).await;
            let before = if req.verify { Some(look().await?) } else { None };
            press(&board, req).await?;
            if let Some(before) = before {
                out.verified = true;
                let at = Some((x as f64, y as f64));
                let (_, after) = wait_stable(eyes, AFTER_TAP_QUIET, AFTER_TAP_MAX).await.map_err(no_screen)?;
                if diff::changed(&before.gray, &after.gray, at, size) < MIN_CHANGED {
                    // A click that lands while iOS still moves the pointer is
                    // sometimes dropped: home the pointer, come back, press once
                    // more.
                    out.repressed = true;
                    tokio::time::sleep(REPRESS_PAUSE).await;
                    let route = ptr.route(x, y, w, h, true, Instant::now(), &c)?;
                    board.all(route.cmds).await?;
                    tokio::time::sleep(c.settle).await;
                    press(&board, req).await?;
                    let (_, again) = wait_stable(eyes, AFTER_TAP_QUIET, AFTER_REPRESS_MAX).await.map_err(no_screen)?;
                    out.changed = diff::changed(&before.gray, &again.gray, at, size) >= MIN_CHANGED;
                    if !out.changed {
                        ptr.forget();
                    }
                }
            }
        }
        Action::Swipe => {
            let (x, y) = req.at.expect("checked in parse");
            let (x2, y2) = req.to.expect("checked in parse");
            if x2 < 0 || y2 < 0 || x2 >= w as i64 || y2 >= h as i64 {
                return Err(format!("bad request: ({x2}, {y2}) is outside the {w}x{h} screen"));
            }
            let route = ptr.route(x, y, w, h, false, Instant::now(), &c)?;
            board.all(route.cmds).await?;
            tokio::time::sleep(c.settle).await;
            let before = if req.verify { Some(look().await?) } else { None };
            board.send(json!({"cmd": "down", "button": "left"}), BOARD_BASE).await?;
            tokio::time::sleep(Duration::from_millis(100 + req.hold_ms)).await;
            let moved = c.move_cmd(c.units((x2 - x) as f64), c.units((y2 - y) as f64), SWIPE_STEP);
            let moved = board.send(moved.0, moved.1).await;
            // Never leave the button down.
            let up = board.send(json!({"cmd": "up", "button": "left"}), BOARD_BASE).await;
            moved?;
            up?;
            // Where a drag ends is less certain than a move: home next time.
            ptr.forget();
            if let Some(before) = before {
                out.verified = true;
                let (_, after) = wait_stable(eyes, AFTER_TAP_QUIET, AFTER_TAP_MAX).await.map_err(no_screen)?;
                out.changed = diff::changed(&before.gray, &after.gray, None, size) >= MIN_CHANGED;
            }
        }
        Action::Scroll | Action::Key | Action::Type | Action::Home | Action::ReleaseAll => {
            if req.action == Action::Scroll {
                if let Some((x, y)) = req.at {
                    let route = ptr.route(x, y, w, h, false, Instant::now(), &c)?;
                    board.all(route.cmds).await?;
                    tokio::time::sleep(c.settle).await;
                }
            }
            let before = if req.verify { Some(look().await?) } else { None };
            match req.action {
                // Positive = see content further down: the wheel turns down.
                Action::Scroll => {
                    let n = req.amount.unsigned_abs() as f64;
                    let t = BOARD_BASE + Duration::from_secs_f64(n * 0.04);
                    board.send(json!({"cmd": "scroll", "dy": -req.amount, "dx": 0}), t).await?;
                }
                Action::Key => {
                    for (i, k) in req.keys.iter().enumerate() {
                        if i > 0 {
                            tokio::time::sleep(KEY_GAP).await;
                        }
                        board.key(k).await?;
                    }
                }
                Action::Type => {
                    let n = req.text.chars().count() as f64;
                    let t = BOARD_BASE + Duration::from_secs_f64(n * 0.045);
                    board.send(json!({"cmd": "type", "text": req.text, "delay_ms": 15}), t).await?;
                }
                Action::Home => board.key("cmd+h").await?,
                _ => {
                    board.send(json!({"cmd": "release_all"}), BOARD_BASE).await?;
                }
            }
            if let Some(before) = before {
                out.verified = true;
                let (_, after) = wait_stable(eyes, AFTER_TAP_QUIET, AFTER_TAP_MAX).await.map_err(no_screen)?;
                out.changed = diff::changed(&before.gray, &after.gray, None, size) >= MIN_CHANGED;
            }
        }
    }
    Ok(out)
}

/// The press of a tap, a double tap or a long press.
async fn press(board: &Driver<'_>, req: &ActRequest) -> Result<(), String> {
    let hold = match req.action {
        Action::LongPress => Some(if req.hold_ms > 0 { Duration::from_millis(req.hold_ms) } else { LONG_PRESS }),
        Action::Tap if req.hold_ms > 0 => Some(Duration::from_millis(req.hold_ms)),
        _ => None,
    };
    match hold {
        Some(hold) => {
            board.send(json!({"cmd": "down", "button": "left"}), BOARD_BASE).await?;
            tokio::time::sleep(hold).await;
            board.send(json!({"cmd": "up", "button": "left"}), BOARD_BASE).await?;
        }
        None => {
            let count = if req.action == Action::DoubleTap { 2 } else { 1 };
            board.send(json!({"cmd": "click", "button": "left", "count": count}), BOARD_BASE).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
