//! The gate (pause, switch, per-synth grant, ask, lease), the act logic
//! (the re-press of a tap that did nothing) and the wire shapes, on a
//! fake screen and a fake board.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::diff::Gray;
use super::eyes::{Eyes, Fut, GrayFrame, Jpeg, ScreenStatus};
use super::hands::{Hands, HandsError, HandsStatus};
use super::*;
use crate::grant::{Approval, Duration_};
use crate::pairing::Pending;
use crate::settings::LinkSource;
use crate::slot::PEER;
use crate::testutil::{self, Owner};

/// The phone's screen: one brightness, changed by the fake board.
#[derive(Default)]
struct Screen {
    level: Mutex<u8>,
    gone: Mutex<bool>,
    looks: AtomicUsize,
}

struct FakeEyes(Arc<Screen>);

impl Eyes for FakeEyes {
    fn status(&self) -> Fut<'_, ScreenStatus> {
        let gone = *self.0.gone.lock().unwrap();
        Box::pin(async move {
            if gone {
                return ScreenStatus { error: "no iPhone is connected by cable".into(), ..Default::default() };
            }
            ScreenStatus { ok: true, device: "iPhone".into(), width: 828, height: 1792, last_frame_ms: 1, error: String::new() }
        })
    }
    fn jpeg(&self, _after: i64, max_edge: u32, _q: f64) -> Fut<'_, Result<Jpeg, String>> {
        let gone = *self.0.gone.lock().unwrap();
        Box::pin(async move {
            if gone {
                return Err("no iPhone is connected by cable".into());
            }
            let (w, h) = if max_edge > 0 { (max_edge * 828 / 1792, max_edge) } else { (828, 1792) };
            Ok(Jpeg { data: vec![0xff, 0xd8, 0xff, 0xd9], width: w, height: h, native_width: 828, native_height: 1792, frame_ms: 42 })
        })
    }
    fn gray(&self, _max: u32) -> Fut<'_, Result<GrayFrame, String>> {
        self.0.looks.fetch_add(1, Ordering::SeqCst);
        let gone = *self.0.gone.lock().unwrap();
        let level = *self.0.level.lock().unwrap();
        Box::pin(async move {
            if gone {
                return Err("no iPhone is connected by cable".into());
            }
            Ok(GrayFrame { gray: Gray::flat(207, 448, level), native_width: 828, native_height: 1792, frame_ms: 1 })
        })
    }
}

/// The board: records every command; the nth click changes the screen
/// when `clicks_that_work` says so.
struct FakeHands {
    screen: Arc<Screen>,
    sent: Mutex<Vec<Value>>,
    clicks_that_work: Vec<bool>,
    clicks: AtomicUsize,
    error: Mutex<Option<HandsError>>,
}

impl Hands for FakeHands {
    fn call(&self, cmd: Value, _t: Duration) -> Fut<'_, Result<Value, HandsError>> {
        Box::pin(async move {
            if let Some(e) = self.error.lock().unwrap().clone() {
                return Err(e);
            }
            let name = cmd["cmd"].as_str().unwrap_or("").to_string();
            self.sent.lock().unwrap().push(cmd);
            let changes = match name.as_str() {
                "click" | "up" => {
                    let n = self.clicks.fetch_add(1, Ordering::SeqCst);
                    self.clicks_that_work.get(n).copied().unwrap_or(true)
                }
                "key" | "type" | "scroll" => true,
                _ => false,
            };
            if changes {
                let mut l = self.screen.level.lock().unwrap();
                *l = l.wrapping_add(100);
            }
            Ok(json!({"ok": true}))
        })
    }
    fn status(&self) -> Fut<'_, HandsStatus> {
        Box::pin(async move {
            HandsStatus { ok: true, port: "/dev/cu.usbmodem3101".into(), ble_connected: true, firmware: "0.1.0".into(), error: String::new() }
        })
    }
}

struct FakeFactory {
    eyes: Arc<FakeEyes>,
    hands: Arc<FakeHands>,
    starts: AtomicUsize,
}

impl Factory for FakeFactory {
    fn start(&self, _t: &PhoneTuning) -> Rig {
        self.starts.fetch_add(1, Ordering::SeqCst);
        Rig { eyes: self.eyes.clone(), hands: self.hands.clone() }
    }
}

struct Env {
    core: crate::core::Core,
    owner: Arc<Owner>,
    screen: Arc<Screen>,
    hands: Arc<FakeHands>,
    factory: Arc<FakeFactory>,
}

fn pending(id: &str, name: &str) -> Pending {
    Pending {
        synth_id: id.into(),
        synth_url: format!("https://{id}.snth.ai"),
        token: format!("tok-{id}"),
        synth_name: name.into(),
        source: LinkSource::Code,
        hub_id: String::new(),
        linked_to_account: None,
        reason: None,
    }
}

/// A core linked to Mia and Maya, the phone on, the owner answering
/// `fallback` unless told otherwise.
fn env(fallback: Approval, clicks_that_work: Vec<bool>) -> Env {
    let owner = Owner::new(fallback);
    let core = testutil::core(owner.clone());
    core.shared().add_link(&pending("mia", "Mia")).unwrap();
    core.shared().add_link(&pending("maya", "Maya")).unwrap();
    let screen = Arc::new(Screen::default());
    let hands = Arc::new(FakeHands {
        screen: screen.clone(),
        sent: Mutex::new(Vec::new()),
        clicks_that_work,
        clicks: AtomicUsize::new(0),
        error: Mutex::new(None),
    });
    let factory = Arc::new(FakeFactory { eyes: Arc::new(FakeEyes(screen.clone())), hands: hands.clone(), starts: AtomicUsize::new(0) });
    core.set_phone_factory(factory.clone());
    core.mutate_settings(|s| s.phone_control.enabled = true);
    Env { core, owner, screen, hands, factory }
}

impl Env {
    async fn call(&self, synth: &str, method: &str, params: Value) -> Result<Value, String> {
        let shared = self.core.shared();
        let slot = shared.slot(synth).unwrap();
        let m = method.to_string();
        PEER.scope(slot, async move { crate::rpc::call_for_test(&shared, &m, params).await }).await
    }

    fn sent(&self) -> Vec<String> {
        self.hands
            .sent
            .lock()
            .unwrap()
            .iter()
            .map(|v| match v["cmd"].as_str().unwrap() {
                "corner" => format!("corner {}", v["which"].as_str().unwrap()),
                "move" => format!("move {} {}", v["dx"], v["dy"]),
                "click" => format!("click {}", v["count"]),
                "key" => format!("key {}", v["combo"].as_str().unwrap()),
                other => other.to_string(),
            })
            .collect()
    }

    fn last_journal(&self) -> crate::journal::Entry {
        self.core.recent_journal().pop().unwrap()
    }
}

#[tokio::test(start_paused = true)]
async fn the_switch_and_the_pause_come_first() {
    let e = env(Approval::always(), vec![]);
    e.core.mutate_settings(|s| s.phone_control.enabled = false);
    let err = e.call("mia", "phone_control.status", json!({})).await.unwrap_err();
    assert!(err.starts_with("denied: phone control is turned off"), "{err}");
    assert_eq!(e.owner.asks(), 0, "nobody is asked about a switched-off phone");

    e.core.mutate_settings(|s| s.phone_control.enabled = true);
    e.core.pause().await;
    let err = e.call("mia", "phone_control.act", json!({"action": "home"})).await.unwrap_err();
    assert_eq!(err, "denied: the phone is paused by the owner");
    assert_eq!(e.last_journal().outcome, err, "a refused act is in the journal");
    e.core.resume();
    assert!(e.call("mia", "phone_control.status", json!({})).await.is_ok());
    assert!(e.call("mia", "phone_control.nope", json!({})).await.unwrap_err_starts("denied: unknown method"));
}

trait StartsWith {
    fn unwrap_err_starts(self, prefix: &str) -> bool;
}
impl StartsWith for Result<Value, String> {
    fn unwrap_err_starts(self, prefix: &str) -> bool {
        matches!(self, Err(e) if e.starts_with(prefix))
    }
}

#[tokio::test(start_paused = true)]
async fn each_synth_needs_its_own_permission() {
    let e = env(Approval::deny(), vec![]);
    e.owner.next(Approval::always());
    let s = e.call("mia", "phone_control.status", json!({"reason": "set a timer"})).await.unwrap();
    assert_eq!(e.owner.asks(), 1);
    let asked = e.owner.last();
    assert_eq!((asked.capability.as_str(), asked.scope.as_str()), (CAP, SCOPE));
    assert_eq!(asked.synth.as_deref(), Some("Mia"));
    assert_eq!(asked.reason, "set a timer");
    assert_eq!(asked.remember, Remember::Any, "once, for the session, or always");
    assert_eq!(s["screen"]["ok"], true);
    // Mia again: remembered.
    e.call("mia", "phone_control.frame", json!({})).await.unwrap();
    assert_eq!(e.owner.asks(), 1);
    // Mia's grant is not Maya's: Maya is asked, and refused.
    let err = e.call("maya", "phone_control.status", json!({})).await.unwrap_err();
    assert_eq!(err, "denied: the owner did not allow this synth to control the phone");
    assert_eq!(e.owner.asks(), 2);
    assert_eq!(e.owner.last().synth.as_deref(), Some("Maya"));
    assert!(e.core.shared().grants().allows("mia", CAP, SCOPE));
    assert!(!e.core.shared().grants().allows("maya", CAP, SCOPE));
}

#[tokio::test(start_paused = true)]
async fn once_asks_again_and_session_does_not() {
    let e = env(Approval::deny(), vec![]);
    e.owner.next(Approval::once());
    e.call("mia", "phone_control.status", json!({})).await.unwrap();
    e.owner.next(Approval { allow: true, duration: Duration_::Session });
    e.call("mia", "phone_control.status", json!({})).await.unwrap();
    assert_eq!(e.owner.asks(), 2);
    e.call("mia", "phone_control.status", json!({})).await.unwrap();
    e.call("mia", "phone_control.act", json!({"action": "home"})).await.unwrap();
    assert_eq!(e.owner.asks(), 2);
}

#[tokio::test(start_paused = true)]
async fn one_synth_holds_the_phone_and_another_is_busy() {
    let e = env(Approval::always(), vec![]);
    e.call("mia", "phone_control.frame", json!({})).await.unwrap();
    let err = e.call("maya", "phone_control.act", json!({"action": "tap", "x": 100, "y": 100})).await.unwrap_err();
    assert_eq!(err, "busy: Mia is controlling the phone right now; try again in a minute");
    assert!(e.call("maya", "phone_control.frame", json!({})).await.unwrap_err_starts("busy: Mia"));
    assert!(e.hands.sent.lock().unwrap().is_empty(), "Maya moved nothing");
    let j = e.last_journal();
    assert_eq!((j.target.as_str(), j.synth.as_deref()), ("tap 100,100", Some("Maya")));
    // Status shows who holds it, to anyone allowed.
    let s = e.call("maya", "phone_control.status", json!({})).await.unwrap();
    assert_eq!(s["lease"]["holder"], "Mia");
    // Mia keeps it while she acts.
    e.call("mia", "phone_control.act", json!({"action": "home"})).await.unwrap();
    // Quiet for a minute: Maya may take it.
    e.core.shared().phone.lease_age(LEASE_QUIET);
    e.call("maya", "phone_control.act", json!({"action": "home"})).await.unwrap();
    assert!(e.call("mia", "phone_control.frame", json!({})).await.unwrap_err_starts("busy: Maya"));
    // Unlinked: the lease goes with the link.
    e.core.unpair_synth("maya").await;
    e.call("mia", "phone_control.frame", json!({})).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_tap_that_changes_the_screen_is_pressed_once() {
    let e = env(Approval::always(), vec![true]);
    let r = e.call("mia", "phone_control.act", json!({"action": "tap", "x": 400, "y": 800, "verify": true})).await.unwrap();
    assert_eq!(r, json!({"ok": true, "changed": true, "repressed": false, "verified": true, "pointer": {"x": 400, "y": 800}}));
    assert_eq!(e.sent(), ["corner tl", "move 210 0", "move 0 434", "click 1"]);
    let j = e.last_journal();
    assert_eq!((j.target.as_str(), j.outcome.as_str()), ("tap 400,800", "ok"));
    assert!(j.duration_ms.is_some());
    // The next tap starts from there.
    e.call("mia", "phone_control.act", json!({"action": "double_tap", "x": 579, "y": 800})).await.unwrap();
    assert_eq!(&e.sent()[4..], ["move 100 0", "click 2"]);
}

#[tokio::test(start_paused = true)]
async fn a_tap_that_did_nothing_is_pressed_again_from_the_corner() {
    let e = env(Approval::always(), vec![false, true]);
    let r = e.call("mia", "phone_control.act", json!({"action": "tap", "x": 400, "y": 800})).await.unwrap();
    assert_eq!(r["changed"], true);
    assert_eq!(r["repressed"], true);
    assert_eq!(e.sent(), ["corner tl", "move 210 0", "move 0 434", "click 1", "corner tl", "move 210 0", "move 0 434", "click 1"]);
    assert_eq!(e.last_journal().outcome, "ok: re-pressed");
}

#[tokio::test(start_paused = true)]
async fn a_tap_that_never_lands_forgets_the_pointer() {
    let e = env(Approval::always(), vec![false, false]);
    let r = e.call("mia", "phone_control.act", json!({"action": "tap", "x": 400, "y": 800})).await.unwrap();
    assert_eq!(r["changed"], false);
    assert_eq!(r["repressed"], true);
    assert_eq!(r["pointer"], Value::Null);
    assert_eq!(e.last_journal().outcome, "ok: no change after a re-press");
    // So the next tap homes again.
    e.call("mia", "phone_control.act", json!({"action": "tap", "x": 410, "y": 800})).await.unwrap();
    assert_eq!(e.sent()[8], "corner tl");
}

#[tokio::test(start_paused = true)]
async fn a_change_under_the_pointer_alone_is_no_change() {
    // The screen changes only at the click: the pointer's own reaction.
    struct Blink(Arc<Screen>);
    impl Eyes for Blink {
        fn status(&self) -> Fut<'_, ScreenStatus> {
            Box::pin(async move { FakeEyes(self.0.clone()).status().await })
        }
        fn jpeg(&self, a: i64, m: u32, q: f64) -> Fut<'_, Result<Jpeg, String>> {
            Box::pin(async move { FakeEyes(self.0.clone()).jpeg(a, m, q).await })
        }
        fn gray(&self, _m: u32) -> Fut<'_, Result<GrayFrame, String>> {
            let lit = *self.0.level.lock().unwrap() != 0;
            Box::pin(async move {
                let mut g = Gray::flat(207, 448, 0);
                if lit {
                    // A 6x6 blob at small (100, 200) = native (400, 800).
                    for row in 197..203 {
                        for col in 97..103 {
                            g.px[row * 207 + col] = 255;
                        }
                    }
                }
                Ok(GrayFrame { gray: g, native_width: 828, native_height: 1792, frame_ms: 1 })
            })
        }
    }
    let e = env(Approval::always(), vec![true, true]);
    let factory = Arc::new(FakeFactory { eyes: Arc::new(FakeEyes(e.screen.clone())), hands: e.hands.clone(), starts: AtomicUsize::new(0) });
    struct BlinkFactory(Arc<FakeFactory>, Arc<Screen>);
    impl Factory for BlinkFactory {
        fn start(&self, _t: &PhoneTuning) -> Rig {
            Rig { eyes: Arc::new(Blink(self.1.clone())), hands: self.0.hands.clone() }
        }
    }
    e.core.set_phone_factory(Arc::new(BlinkFactory(factory, e.screen.clone())));
    let r = e.call("mia", "phone_control.act", json!({"action": "tap", "x": 400, "y": 800})).await.unwrap();
    assert_eq!(r["repressed"], true, "the blob under the pointer is not the app reacting");
}

#[tokio::test(start_paused = true)]
async fn keys_text_and_the_rest() {
    let e = env(Approval::always(), vec![]);
    e.call("mia", "phone_control.act", json!({"action": "key", "keys": ["cmd+space", "enter"]})).await.unwrap();
    e.call("mia", "phone_control.act", json!({"action": "home"})).await.unwrap();
    let r = e.call("mia", "phone_control.act", json!({"action": "type", "text": "1234*56=", "verify": true})).await.unwrap();
    assert_eq!(r["changed"], true);
    assert_eq!(r["verified"], true);
    e.call("mia", "phone_control.act", json!({"action": "scroll", "amount": 3})).await.unwrap();
    e.call("mia", "phone_control.act", json!({"action": "swipe", "x": 414, "y": 1200, "x2": 414, "y2": 500})).await.unwrap();
    e.call("mia", "phone_control.act", json!({"action": "release_all"})).await.unwrap();
    assert_eq!(
        e.sent(),
        ["key cmd+space", "key enter", "key cmd+h", "type", "scroll", "corner br", "move -218 0", "move 0 -317", "down", "move 0 -391", "up", "release_all"]
    );
    let sent = e.hands.sent.lock().unwrap().clone();
    assert_eq!(sent[3]["text"], "1234*56=");
    assert_eq!(sent[4]["dy"], -3, "positive amount = content further down = wheel down");
    // The journal has the length of the text, never the text.
    let journal = e.core.recent_journal();
    let typed = journal.iter().find(|j| j.target.starts_with("type")).unwrap();
    assert_eq!(typed.target, "type (8 characters)");
    assert!(!serde_json::to_string(&journal).unwrap().contains("1234"));
    assert!(journal.iter().any(|j| j.target == "key cmd+space, enter"));
}

#[tokio::test(start_paused = true)]
async fn bad_and_unsupported_requests_move_nothing() {
    let e = env(Approval::always(), vec![]);
    for (params, want) in [
        (json!({"action": "type", "text": "привет"}), "unsupported: non_ascii text, paste is not configured"),
        (json!({"action": "type", "text": "ok 👍"}), "unsupported: non_ascii text, paste is not configured"),
        (json!({"action": "tap"}), "bad request: tap needs x and y"),
        (json!({"action": "swipe", "x": 1, "y": 1}), "bad request: swipe needs"),
        (json!({"action": "fly"}), "bad request: unknown action"),
        (json!({"action": "key", "keys": []}), "bad request: key needs keys"),
        (json!({"action": "key", "keys": ["cmd+h; rm"]}), "bad request: "),
        (json!({"action": "scroll"}), "bad request: scroll needs"),
        (json!({"action": "tap", "x": 900, "y": 10}), "bad request: (900, 10) is outside the 828x1792 screen"),
    ] {
        let err = e.call("mia", "phone_control.act", params.clone()).await.unwrap_err();
        assert!(err.starts_with(want), "{params}: {err}");
    }
    assert!(e.hands.sent.lock().unwrap().is_empty());
    assert_eq!(e.owner.asks(), 1, "only the request that passed parsing reached the ask");
}

#[tokio::test(start_paused = true)]
async fn no_board_no_bluetooth_and_no_cable_are_their_own_errors() {
    let e = env(Approval::always(), vec![]);
    *e.hands.error.lock().unwrap() = Some(HandsError::NotConnected);
    let err = e.call("mia", "phone_control.act", json!({"action": "home"})).await.unwrap_err();
    assert!(err.starts_with("no_hands: the phone is not connected to the board over Bluetooth"), "{err}");
    *e.hands.error.lock().unwrap() = Some(HandsError::NoBoard("the SNTH Phone Bridge board is not plugged in".into()));
    let err = e.call("mia", "phone_control.act", json!({"action": "home"})).await.unwrap_err();
    assert_eq!(err, "no_hands: the SNTH Phone Bridge board is not plugged in");
    assert_eq!(e.last_journal().outcome, err);
    *e.hands.error.lock().unwrap() = None;

    *e.screen.gone.lock().unwrap() = true;
    let err = e.call("mia", "phone_control.frame", json!({"stable_ms": 600})).await.unwrap_err();
    assert_eq!(err, "no_screen: no iPhone is connected by cable");
    let err = e.call("mia", "phone_control.act", json!({"action": "tap", "x": 10, "y": 10})).await.unwrap_err();
    assert!(err.starts_with("no_screen: "), "a tap is checked on the screen: {err}");
    // Acting blind is allowed when the synth does not ask to check.
    let r = e.call("mia", "phone_control.act", json!({"action": "home", "verify": false})).await.unwrap();
    assert_eq!(r["verified"], false);
    let s = e.call("mia", "phone_control.status", json!({})).await.unwrap();
    assert_eq!(s["screen"]["ok"], false);
    assert_eq!(s["screen"]["error"], "no iPhone is connected by cable");
    assert_eq!(s["hands"]["ble_connected"], true);
}

#[tokio::test(start_paused = true)]
async fn the_frame_and_the_status_have_the_wire_shape() {
    let e = env(Approval::always(), vec![]);
    let f = e.call("mia", "phone_control.frame", json!({"stable_ms": 600, "max_edge": 768, "quality": 0.8})).await.unwrap();
    assert_eq!(f["mime"], "image/jpeg");
    assert_eq!(f["data_b64"], "/9j/2Q==");
    assert_eq!((f["width"].as_u64(), f["height"].as_u64()), (Some(354), Some(768)));
    assert_eq!((f["native_width"].as_u64(), f["native_height"].as_u64()), (Some(828), Some(1792)));
    assert_eq!(f["frame_ms"], 42);
    assert_eq!(f["stable"], true);
    // A screen at rest for 600 ms takes several looks.
    assert!(e.screen.looks.load(Ordering::SeqCst) >= 7, "{}", e.screen.looks.load(Ordering::SeqCst));

    let s = e.call("mia", "phone_control.status", json!({})).await.unwrap();
    assert_eq!(
        s,
        json!({
            "screen": {"ok": true, "device": "iPhone", "width": 828, "height": 1792, "last_frame_ms": 1},
            "hands": {"ok": true, "port": "/dev/cu.usbmodem3101", "ble_connected": true, "firmware": "0.1.0"},
            "lease": {"holder": "Mia"},
        })
    );
}

/// A screen that never stops moving.
struct Busy(AtomicUsize);
impl Eyes for Busy {
    fn status(&self) -> Fut<'_, ScreenStatus> {
        Box::pin(async { ScreenStatus::default() })
    }
    fn jpeg(&self, _: i64, _: u32, _: f64) -> Fut<'_, Result<Jpeg, String>> {
        Box::pin(async { Err("x".to_string()) })
    }
    fn gray(&self, _: u32) -> Fut<'_, Result<GrayFrame, String>> {
        let n = self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move { Ok(GrayFrame { gray: Gray::flat(10, 10, (n * 50) as u8), native_width: 10, native_height: 10, frame_ms: 0 }) })
    }
}

#[tokio::test(start_paused = true)]
async fn waiting_for_a_still_screen_gives_up_at_the_limit() {
    let busy = Busy(AtomicUsize::new(0));
    let started = Instant::now();
    let (stable, _) = wait_stable(&busy, Duration::from_millis(600), STABLE_MAX).await.unwrap();
    assert!(!stable);
    assert!(started.elapsed() >= STABLE_MAX && started.elapsed() < STABLE_MAX + Duration::from_millis(300));
}

#[tokio::test(start_paused = true)]
async fn switching_off_stops_the_rig_and_on_starts_it() {
    let e = env(Approval::always(), vec![]);
    assert_eq!(e.factory.starts.load(Ordering::SeqCst), 1, "switched on in a runtime: started");
    e.call("mia", "phone_control.status", json!({})).await.unwrap();
    assert_eq!(e.factory.starts.load(Ordering::SeqCst), 1, "the same rig serves");
    e.core.mutate_settings(|s| s.phone_control.enabled = false);
    assert!(e.core.shared().phone.rig.lock().unwrap().is_none());
    e.core.mutate_settings(|s| s.phone_control.enabled = true);
    assert_eq!(e.factory.starts.load(Ordering::SeqCst), 2);
    // A new calibration restarts it (the board port may have changed).
    e.core.mutate_settings(|s| s.phone_control.tuning.port = "/dev/cu.other".into());
    assert_eq!(e.factory.starts.load(Ordering::SeqCst), 3);
    // A preset turns the phone off and keeps the calibration.
    e.core.apply_preset(crate::settings::Preset::Full);
    assert!(!e.core.settings().phone_control.enabled);
    assert_eq!(e.core.settings().phone_control.tuning.port, "/dev/cu.other");
}

#[tokio::test(start_paused = true)]
async fn the_hello_carries_the_switch() {
    let e = env(Approval::always(), vec![]);
    let caps = crate::wire::capabilities_value(&e.core.settings(), "", "ok", e.core.shared().apps.backend().as_ref());
    assert_eq!(caps["phone_control"], json!({"enabled": true, "max_request_bytes": crate::rpc::MAX_REQUEST_LINE}));
    e.core.mutate_settings(|s| s.phone_control.enabled = false);
    let caps = crate::wire::capabilities_value(&e.core.settings(), "", "ok", e.core.shared().apps.backend().as_ref());
    assert_eq!(caps["phone_control"]["enabled"], false);
}

#[tokio::test(start_paused = true)]
async fn the_owners_view_does_not_ask() {
    let e = env(Approval::deny(), vec![]);
    let v = e.core.phone_view().await;
    assert!(v.enabled && v.screen.ok && v.hands.ok);
    let j = e.core.phone_preview(480).await.unwrap();
    assert_eq!(j.height, 480);
    assert_eq!(e.owner.asks(), 0);
    e.core.mutate_settings(|s| s.phone_control.enabled = false);
    let v = e.core.phone_view().await;
    assert!(!v.screen.ok && v.screen.error == "phone control is off");
}
