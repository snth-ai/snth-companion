//! The island (TZ tz-island.md, section 5): the synth's face at the top
//! of the screen, merged with the notch (macOS), or floating just above
//! the taskbar (Windows, `geometry.rs`), and the owner's hold keys. The
//! owner may put it elsewhere (the settings' `position`: top, bottom or
//! the middle of a side, left, centre or right); every spot but the top
//! centre (the macOS default; on Windows when the owner chooses it, the
//! drawn notch flush with the top edge) is the floating pill
//! (`geometry::place`). A change of the setting places it again at once
//! (`sync`).
//!
//! One more window, `island` (island.html): transparent, always on top,
//! never activated (`panel.rs`), placed by `geometry.rs`. The webview draws
//! the black shape and the face and tells the shell which part of the
//! window takes clicks (`island_layout`); everywhere else the window lets
//! clicks through (`set_ignore_cursor_events`, switched by the poller as
//! the pointer moves in and out of the shape).
//!
//! The poller reads the pointer only while the island is shown: 30 times
//! a second while the pointer moves, 5 times a second while it rests, and
//! sends the face a gaze vector only when it changed. While the main synth
//! drives the computer the eyes follow the synth's pointer instead (when
//! the synth told the driver where to put it).
//!
//! Full screen: while the island's screen shows a full-screen app the
//! island hides (fades out and up) and lets every click through, and it
//! comes back with the menu bar when the pointer reaches the top edge
//! (`panel::cover` says when). On macOS that is the notch and the
//! floating top spots (they sit just under where the bar comes in); a
//! bottom or middle spot stays hidden while the bar shows (`reveal`). It stays while the owner is busy with it
//! (a hold, the line of text, a request, a send just made, a fresh
//! answer, a notice): the owner sees that the key worked. The keys work
//! the same while it is hidden. The screenshot's own invisibility
//! (`panel::set_invisible`, the window's alpha) is a separate switch, so
//! the two never undo each other: the island is seen only when neither
//! hides it.
//!
//! The keys (`hold.rs`) are registered only while the island is on and
//! the main synth is linked and the app is not paused: otherwise the
//! combinations reach the other apps as usual. A hold records the voice
//! (`audio.rs`), screenshot + voice also takes a picture at the press
//! (`shot.rs`); the release sends `owner.message` (connect-core
//! island.rs) and the synth's island.event moves the face through
//! accepted, thinking, working and done (with the summary).
//!
//! With on-device speech-to-text on (`stt/`, off by default) a hold is
//! also recognized while it runs: the island shows the words as they
//! come, and the release sends the text instead of the audio when the
//! synth takes it. The synth's one-time offer to switch it on is a card
//! on the island (`SttCard`), shown only while the island is idle.
//!
//! With wake by name on (`wake/`, off by default) the owner may say the
//! main synth's name instead of holding a key: the wake becomes a hold of
//! the voice key and is sent the same way.

pub mod audio;
pub mod geometry;
pub mod hold;
pub mod panel;
pub mod shot;
pub mod stt;
pub mod wake;

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use connect_core::island::{self as core_island, Audio, HotkeyKind, Hotkeys, IslandEvent, Mode, OwnerMessage, SttEngine, SttMode};
use connect_core::{CoreEvent, LinkState};
use serde::Serialize;
use serde_json::Value;
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::Notify;

use crate::approvals::ApprovalDto;
use crate::state::Ctx;
use geometry::{Placement, Rect, Spot};

pub const WINDOW: &str = "island";
/// What the island's webview listens to (only the island window gets
/// these; the main window never sees them).
const VIEW_EVENT: &str = "island:view";
const GAZE_EVENT: &str = "island:gaze";
const LEVEL_EVENT: &str = "island:level";

/// Pointer polling while it moves, and while it rests.
const POLL_MOVING: Duration = Duration::from_millis(33);
const POLL_RESTING: Duration = Duration::from_millis(200);
/// How long after the last movement the poller stays fast.
const MOVING_FOR: Duration = Duration::from_millis(800);
/// The distance (points) at which the eyes look as far as they go.
const GAZE_REACH: f64 = 600.0;
/// A message the synth never finished is forgotten after this.
const MESSAGE_FORGET: Duration = Duration::from_secs(20 * 60);
/// A notice (offline, the microphone, a failed send) stays this long.
const NOTICE_FOR: Duration = Duration::from_secs(6);
/// The answer's summary stays this long (as the webview shows it).
const SUMMARY_FOR: Duration = Duration::from_secs(6);
/// The tick of "accepted" is seen for this long after a send.
const ACCEPTED_FOR: Duration = Duration::from_millis(1500);
/// "The model is ready" stays on the island this long.
const STT_READY_FOR: Duration = Duration::from_secs(6);
/// How often the poller looks whether a full-screen app covers the
/// screen. Faster while it does and the pointer is near the top, where
/// the menu bar comes in, so the island comes in with it.
const COVER_EVERY: Duration = Duration::from_millis(190);
const COVER_FAST: Duration = Duration::from_millis(90);
/// How often the poller asks where a floating island goes (the taskbar
/// or the Dock moved or hides itself now, another resolution or scale).
/// Windows polls whatever the spot (its top centre too: a taskbar moved
/// to the top edge moves it). macOS hears of screen changes
/// (`panel::watch_screens`) but not for certain of the Dock's, so only a
/// floating spot polls there; the notch placement never does.
const REPLACE_EVERY: Duration = Duration::from_secs(3);

/// A debug build started with `SNTH_ISLAND_DEMO=1` shows the island
/// without a linked synth, with no main window and no helper install: to
/// look at the window and measure what it costs at rest. Never in a
/// release build.
pub fn demo() -> bool {
    cfg!(debug_assertions) && std::env::var("SNTH_ISLAND_DEMO").as_deref() == Ok("1")
}

/// The island only exists where it can be drawn properly.
pub fn supported_os() -> bool {
    cfg!(any(target_os = "macos", windows))
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The message the owner sent last, as the synth reports it.
#[derive(Debug, Clone, Serialize)]
pub struct MessageView {
    pub synth_id: String,
    pub message_id: String,
    /// "accepted", "working", "done", "error".
    pub stage: String,
    /// When this stage began (unix ms).
    pub at_ms: u64,
    pub summary: String,
    pub channel: String,
}

/// Something the island says by itself: why a key did nothing, why a
/// message did not go.
#[derive(Debug, Clone, Serialize)]
pub struct Notice {
    /// "offline", "outdated", "paused", "no_synth", "busy", "mic_denied",
    /// "mic_asking", "no_microphone", "mic_failed", "shot_failed",
    /// "shot_permission", "too_large", "too_long", "no_owner_session",
    /// "empty", "failed".
    pub code: String,
    pub detail: String,
    pub at_ms: u64,
}

/// The speech-to-text card on the island: the synth's offer, then (when
/// the owner accepted it there) the model's download.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SttCard {
    Offer {
        /// The download, in bytes (the text says "about 550 MB").
        size: u64,
    },
    Download {
        percent: u32,
    },
    Verifying,
    Ready,
    Failed {
        code: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct SynthChoice {
    pub id: String,
    pub name: String,
    pub online: bool,
}

/// Everything the island window renders.
#[derive(Debug, Clone, Serialize)]
pub struct View {
    pub os: &'static str,
    pub lang: &'static str,
    pub placement: Option<Placement>,
    /// The main synth (the keys talk to it).
    pub synth: Option<SynthChoice>,
    /// Its face; null = the default preset of its id.
    pub face: Option<Value>,
    /// It takes island requests (announced the feature).
    pub supported: bool,
    pub synths: Vec<SynthChoice>,
    pub paused: bool,
    /// The main synth drives the computer right now.
    pub driving: bool,
    pub task: Option<String>,
    /// A hold runs: "voice" or "screen_voice", since when (unix ms).
    pub hold: Option<HotkeyKind>,
    pub hold_since_ms: u64,
    pub text_open: bool,
    /// owner.message is on its way (until the synth accepted it).
    pub sending: bool,
    pub message: Option<MessageView>,
    pub notice: Option<Notice>,
    /// The oldest request waiting for the owner.
    pub approval: Option<ApprovalDto>,
    /// The pointer is over the island (it opens up).
    pub hover: bool,
    /// A full-screen app covers the screen and the menu bar is away: the
    /// island fades out (and lets every click through).
    pub hidden: bool,
    pub hotkeys: Hotkeys,
    pub mic: audio::MicPermission,
    /// On-device speech-to-text: the words of the hold so far, then the
    /// text that was sent. Null with the speech going to the synth.
    pub transcript: Option<String>,
    /// A hold recognized here runs while the model is still loading: no
    /// words yet (the island says so quietly).
    pub stt_loading: bool,
    pub stt_card: Option<SttCard>,
    /// Wake by name: off, listening (the dot in the right ear), paused
    /// and why, or recording a request.
    pub wake: wake::pause::Status,
}

#[derive(Default)]
struct Inner {
    created: bool,
    shown: bool,
    placement: Option<Placement>,
    /// The owner's spot the island was last placed at.
    spot: Option<Spot>,
    /// The part of the window that takes clicks, and the face's centre
    /// (window-local CSS pixels), as the webview last said.
    hit: Option<Rect>,
    face: Option<(f64, f64)>,
    hover: bool,
    ignoring: Option<bool>,
    text_open: bool,
    sending: bool,
    hold_since_ms: u64,
    message: Option<(MessageView, Instant)>,
    notice: Option<(Notice, Instant)>,
    last_target: Option<(f64, f64)>,
    last_move: Option<Instant>,
    /// The screen the island is on, as last placed.
    screen: Option<geometry::ScreenInfo>,
    /// What it showed at the last look, and when that was (None: look
    /// now).
    cover: geometry::Cover,
    cover_at: Option<Instant>,
    /// Out of the way of a full-screen app (before the owner's own
    /// business, which keeps it shown: `busy`), and what the webview was
    /// told last.
    tucked: bool,
    hidden: bool,
    /// The words of the current (or last) hold, recognized here.
    transcript: Option<String>,
    /// The owner accepted the offer on the island: the island shows the
    /// download until it is done (or the owner hides it).
    stt_follow: bool,
    stt_ready_at: Option<Instant>,
    /// A released hold is being turned into a message (the recording
    /// stopped, the text awaited): shown as sending.
    recognizing: bool,
    /// The number of the latest hold, and a release that came before its
    /// recording had started (Some(kind): send it, None: a slip).
    hold_seq: u64,
    early: Option<(u64, Option<HotkeyKind>)>,
}

/// Something the owner started or must see is on the island: it stays
/// even in full screen.
fn busy(inner: &Inner, holding: bool, approval: bool) -> bool {
    let message = inner.message.as_ref().is_some_and(|(m, at)| match m.stage.as_str() {
        "accepted" => at.elapsed() < ACCEPTED_FOR,
        "done" | "error" => at.elapsed() < SUMMARY_FOR,
        _ => false,
    });
    holding || approval || inner.text_open || inner.sending || inner.recognizing || message || inner.notice.as_ref().is_some_and(|(_, at)| at.elapsed() < NOTICE_FOR)
}

struct Hold {
    recording: audio::Recording,
    picture: Option<tauri::async_runtime::JoinHandle<Result<core_island::Image, String>>>,
    synth_id: String,
    stt: Speech,
    /// Started by the synth's name, not a key (`wake/`).
    wake: bool,
    /// The tone of voice, listened to while the speech is recognized here.
    tone: Option<stt::tone::ToneSession>,
}

/// Who recognizes a hold's speech.
enum Speech {
    /// This computer, while it runs (on-device speech-to-text).
    Device(stt::Session),
    /// The synth (the audio goes), and why.
    Synth(&'static str),
}

pub struct Island {
    inner: Mutex<Inner>,
    hold: Mutex<hold::Hold>,
    running: Mutex<Option<Hold>>,
    /// Wakes the pointer poller (the island was shown).
    wake: Notify,
    /// shortcut id -> which key; the accelerators registered now.
    keys: Mutex<HashMap<u32, HotkeyKind>>,
    registered: Mutex<Vec<(HotkeyKind, String)>>,
    /// Why a key could not be registered ("taken", "bad").
    problems: Mutex<HashMap<HotkeyKind, String>>,
    /// The microphone question was asked during this run.
    mic_asked: AtomicBool,
    /// On-device speech-to-text: the model files and the speech thread.
    pub stt: stt::Stt,
    /// Wake by name: its models and its listening thread.
    pub wake_word: wake::Wake,
    /// The tone of voice estimated here (stt/tone): its model, the owner's
    /// usual manner.
    pub tone: stt::tone::Tone,
}

impl Island {
    pub fn new(data_dir: &std::path::Path) -> Self {
        Island {
            inner: Mutex::new(Inner::default()),
            hold: Mutex::new(hold::Hold::default()),
            running: Mutex::new(None),
            wake: Notify::new(),
            keys: Mutex::new(HashMap::new()),
            registered: Mutex::new(Vec::new()),
            problems: Mutex::new(HashMap::new()),
            mic_asked: AtomicBool::new(false),
            stt: stt::Stt::new(data_dir),
            wake_word: wake::Wake::new(data_dir),
            tone: stt::tone::Tone::new(data_dir),
        }
    }
}

impl Island {
    /// Which island key this shortcut is, if it is one.
    pub fn kind_of(&self, id: u32) -> Option<HotkeyKind> {
        self.keys.lock().unwrap().get(&id).copied()
    }

    pub fn keys_active(&self) -> bool {
        !self.registered.lock().unwrap().is_empty()
    }

    pub fn problems(&self) -> HashMap<HotkeyKind, String> {
        self.problems.lock().unwrap().clone()
    }

    fn notice(&self, code: &str, detail: impl Into<String>) {
        let detail = detail.into();
        connect_core::logln!("island: {code}: {detail}");
        self.inner.lock().unwrap().notice = Some((Notice { code: code.into(), detail, at_ms: now_ms() }, Instant::now()));
    }
}

fn window(ctx: &Ctx) -> Option<WebviewWindow> {
    ctx.app.get_webview_window(WINDOW)
}

/// Build the view.
pub fn view(ctx: &Ctx) -> View {
    let island = &ctx.island;
    let settings = ctx.core.island();
    let main = ctx.core.island_main();
    let brief = ctx.core.link_brief();
    let paused = brief.paused;
    let synths: Vec<SynthChoice> = brief
        .links
        .iter()
        .map(|l| SynthChoice {
            id: l.synth_id.clone(),
            name: l.synth_name.clone().unwrap_or_else(|| l.synth_id.clone()),
            online: l.state == LinkState::Linked,
        })
        .collect();
    let synth = main.as_ref().and_then(|m| synths.iter().find(|s| s.id == m.synth_id).cloned());
    let driver = ctx.core.shared().computer_driver();
    let driving = match (&driver, &main) {
        (Some((id, _)), Some(m)) => *id == m.synth_id,
        _ => false,
    };
    let mut inner = island.inner.lock().unwrap();
    if inner.message.as_ref().is_some_and(|(_, at)| at.elapsed() > MESSAGE_FORGET) {
        inner.message = None;
    }
    if inner.notice.as_ref().is_some_and(|(_, at)| at.elapsed() > NOTICE_FOR) {
        inner.notice = None;
    }
    let hold = island.hold.lock().unwrap().holding();
    let approval = ctx.approvals.list().into_iter().next();
    let hidden = inner.tucked && !busy(&inner, hold.is_some(), approval.is_some());
    // The speech-to-text card only on an idle island: never mid-hold,
    // mid-answer, over a request or a notice.
    let answering = inner.message.as_ref().is_some_and(|(m, at)| match m.stage.as_str() {
        "accepted" | "working" => true,
        _ => at.elapsed() < SUMMARY_FOR,
    });
    let idle = hold.is_none()
        && !inner.text_open
        && !inner.sending
        && !inner.recognizing
        && approval.is_none()
        && !answering
        && !hidden
        && inner.notice.as_ref().is_none_or(|(_, at)| at.elapsed() >= NOTICE_FOR);
    let takes_text = main.as_ref().is_some_and(|m| m.state() == LinkState::Linked && core_island::supports_device_stt(m));
    let offer = stt::offer_due(&settings, stt::available(), takes_text, idle);
    let stt_card = if offer {
        Some(SttCard::Offer { size: island.stt.models_of(settings.stt_engine).size() })
    } else if inner.stt_follow && idle {
        follow_card(&mut inner, &island.stt.models_of(settings.stt_engine).status())
    } else {
        None
    };
    let view = View {
        os: std::env::consts::OS,
        lang: match ctx.lang() {
            crate::i18n::Lang::Ru => "ru",
            _ => "en",
        },
        placement: inner.placement,
        face: synth.as_ref().and_then(|s| ctx.core.face_of(&s.id)),
        supported: main.as_ref().is_some_and(|m| core_island::supports(m)),
        synth,
        synths,
        paused,
        driving,
        task: if driving { ctx.core.computer_task() } else { None },
        hold,
        hold_since_ms: if hold.is_some() { inner.hold_since_ms } else { 0 },
        text_open: inner.text_open,
        sending: inner.sending || inner.recognizing,
        message: inner.message.as_ref().map(|(m, _)| m.clone()),
        notice: inner.notice.as_ref().map(|(n, _)| n.clone()),
        approval,
        hover: inner.hover && !hidden,
        hidden,
        hotkeys: settings.hotkeys,
        mic: audio::permission(),
        transcript: inner.transcript.clone(),
        // `transcript` is Some only for a hold recognized here.
        stt_loading: hold.is_some() && inner.transcript.is_some() && island.stt.engine() != stt::EngineState::Ready,
        stt_card,
        wake: island.wake_word.status(),
    };
    drop(inner);
    // The owner has seen the offer: a restart shows it again until it is
    // answered, an update never makes it new again.
    if offer && settings.stt_offer == core_island::SttOffer::Pending {
        ctx.core.set_island(|i| i.stt_offer = i.stt_offer.shown());
    }
    view
}

/// The download's card after the owner accepted the offer on the island.
fn follow_card(inner: &mut Inner, status: &stt::model::Status) -> Option<SttCard> {
    use stt::model::Status;
    match status {
        Status::Downloading { done, total } => Some(SttCard::Download { percent: (done * 100 / (*total).max(1)).min(100) as u32 }),
        Status::Verifying => Some(SttCard::Verifying),
        Status::Ready => {
            let at = *inner.stt_ready_at.get_or_insert_with(Instant::now);
            if at.elapsed() < STT_READY_FOR {
                Some(SttCard::Ready)
            } else {
                inner.stt_follow = false;
                inner.stt_ready_at = None;
                None
            }
        }
        Status::Failed { code, .. } => Some(SttCard::Failed { code: code.clone() }),
        Status::Missing => {
            // Cancelled or deleted in Settings.
            inner.stt_follow = false;
            None
        }
    }
}

pub fn emit_view(ctx: &Ctx) {
    if let Some(w) = window(ctx) {
        let _ = w.emit_to(WINDOW, VIEW_EVENT, view(ctx));
    }
}

/// Bring the island in line with the settings and the links: show or hide
/// the window, register or release the keys. Called at start and after
/// anything that may change them. Never on the main thread.
pub fn sync(ctx: &Arc<Ctx>) {
    let settings = ctx.core.island();
    let want_window = supported_os() && ((settings.enabled && ctx.core.is_paired()) || demo());
    if want_window {
        ensure_window(ctx);
    } else {
        hide_window(ctx);
    }
    let main_up = ctx.core.island_main().is_some_and(|m| m.state() == LinkState::Linked);
    let want_keys = want_window && main_up && !ctx.core.link_brief().paused;
    sync_keys(ctx, if want_keys { Some(&settings.hotkeys) } else { None });
    if !want_keys {
        cancel_hold(ctx);
    }
    // The owner chose another spot: there now.
    let spot = spot_of(&settings.position);
    let moved = {
        let inner = ctx.island.inner.lock().unwrap();
        inner.shown && inner.spot.is_some_and(|s| s != spot)
    };
    if moved {
        let ctx2 = ctx.clone();
        let _ = ctx.app.run_on_main_thread(move || relayout_main_thread(&ctx2));
    }
    ctx.island.wake_word.kick();
    emit_view(ctx);
}

/// The settings' position as the geometry takes it.
fn spot_of(p: &core_island::IslandPosition) -> Spot {
    use core_island::{IslandHorizontal as H, IslandVertical as V};
    Spot {
        vertical: p.vertical.map(|v| match v {
            V::Top => geometry::Edge::Top,
            V::Middle => geometry::Edge::Middle,
            V::Bottom => geometry::Edge::Bottom,
        }),
        horizontal: p.horizontal.map(|h| match h {
            H::Left => geometry::Align::Left,
            H::Center => geometry::Align::Center,
            H::Right => geometry::Align::Right,
        }),
    }
}

fn ensure_window(ctx: &Arc<Ctx>) {
    let created = ctx.island.inner.lock().unwrap().created;
    if !created {
        let built = WebviewWindowBuilder::new(&ctx.app, WINDOW, WebviewUrl::App("island.html".into()))
            .title("SNTH Connect Island")
            .inner_size(geometry::WINDOW_W, geometry::WINDOW_H)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .resizable(false)
            .maximizable(false)
            .minimizable(false)
            .always_on_top(true)
            .visible_on_all_workspaces(true)
            .skip_taskbar(true)
            .focused(false)
            .content_protected(true)
            .accept_first_mouse(true)
            .visible(false)
            .on_navigation(crate::windows::is_app_url)
            .build();
        match built {
            Ok(w) => {
                let _ = w.set_ignore_cursor_events(true);
                let ctx2 = ctx.clone();
                let _ = ctx.app.run_on_main_thread(move || {
                    if let Some(w) = window(&ctx2) {
                        panel::setup(&w);
                    }
                    let ctx3 = ctx2.clone();
                    panel::watch_screens(Box::new(move || relayout_main_thread(&ctx3)));
                    let ctx4 = ctx2.clone();
                    panel::watch_spaces(Box::new(move || {
                        // The window list may lag the switch a little; the
                        // poller keeps looking after this first look.
                        ctx4.island.inner.lock().unwrap().cover_at = None;
                        ctx4.island.wake.notify_one();
                    }));
                });
                let mut inner = ctx.island.inner.lock().unwrap();
                inner.created = true;
                inner.ignoring = Some(true);
            }
            Err(e) => {
                connect_core::logln!("island: the window could not be made: {e}");
                return;
            }
        }
    }
    let shown = ctx.island.inner.lock().unwrap().shown;
    if !shown {
        ctx.island.inner.lock().unwrap().shown = true;
        let ctx2 = ctx.clone();
        let _ = ctx.app.run_on_main_thread(move || {
            relayout_main_thread(&ctx2);
            if let Some(w) = window(&ctx2) {
                panel::show(&w);
            }
        });
        ctx.island.wake.notify_one();
    }
}

fn hide_window(ctx: &Arc<Ctx>) {
    let was = std::mem::replace(&mut ctx.island.inner.lock().unwrap().shown, false);
    if was {
        if let Some(w) = window(ctx) {
            let _ = w.hide();
        }
    }
}

/// Place the island on the screen as it is now. Main thread.
fn relayout_main_thread(ctx: &Arc<Ctx>) {
    let Some(w) = window(ctx) else { return };
    let Some(screen) = panel::screen_info(&w) else { return };
    let spot = spot_of(&ctx.core.island().position);
    let placement = geometry::place(&screen, spot);
    let changed = {
        let mut inner = ctx.island.inner.lock().unwrap();
        inner.spot = Some(spot);
        if inner.screen.as_ref() != Some(&screen) {
            inner.screen = Some(screen);
            inner.cover_at = None;
        }
        inner.placement != Some(placement)
    };
    if changed {
        if placement.floating {
            connect_core::logln!(
                "island: placed at {:.0},{:.0} {:.0}x{:.0}, floating at the {:?} edge, {:?}, pill {:.0}x{:.0}",
                placement.window.x,
                placement.window.y,
                placement.window.w,
                placement.window.h,
                placement.edge,
                placement.align,
                placement.pill.w,
                placement.pill.h
            );
        } else {
            connect_core::logln!(
                "island: placed at {:.0},{:.0} {:.0}x{:.0}, {} notch {:.0}x{:.0}",
                placement.window.x,
                placement.window.y,
                placement.window.w,
                placement.window.h,
                if placement.real_notch { "real" } else { "drawn" },
                placement.notch.w,
                placement.notch.h
            );
        }
        panel::place(&w, &placement);
        ctx.island.inner.lock().unwrap().placement = Some(placement);
        let _ = w.emit_to(WINDOW, VIEW_EVENT, view(ctx));
    }
}

// --- the keys ---

fn sync_keys(ctx: &Arc<Ctx>, want: Option<&Hotkeys>) {
    let wanted: Vec<(HotkeyKind, String)> = match want {
        Some(keys) => HotkeyKind::ALL
            .into_iter()
            .map(|k| (k, keys.get(k).to_string()))
            .filter(|(_, a)| !a.trim().is_empty())
            .collect(),
        None => Vec::new(),
    };
    let mut registered = ctx.island.registered.lock().unwrap();
    if *registered == wanted {
        return;
    }
    let gs = ctx.app.global_shortcut();
    for (_, accelerator) in registered.drain(..) {
        if let Ok(s) = Shortcut::from_str(&accelerator) {
            let _ = gs.unregister(s);
        }
    }
    let mut keys = ctx.island.keys.lock().unwrap();
    keys.clear();
    let mut problems = ctx.island.problems.lock().unwrap();
    problems.clear();
    for (kind, accelerator) in wanted.iter() {
        match Shortcut::from_str(accelerator) {
            Ok(s) => match gs.register(s) {
                Ok(()) => {
                    keys.insert(s.id(), *kind);
                }
                Err(e) => {
                    connect_core::logln!("island: {accelerator} is not available: {e}");
                    problems.insert(*kind, "taken".into());
                }
            },
            Err(_) => {
                problems.insert(*kind, "bad".into());
            }
        }
    }
    // What is in `registered` is what was asked for: a key another app
    // holds is retried at the next change, not in a loop.
    *registered = wanted;
}

/// May this accelerator be used for this key? Err("bad_shortcut",
/// "conflict:<other key>", "conflict:stop", "shortcut_unavailable").
pub fn check_hotkey(ctx: &Arc<Ctx>, kind: HotkeyKind, accelerator: &str) -> Result<(), String> {
    if accelerator.trim().is_empty() {
        return Ok(());
    }
    let shortcut = Shortcut::from_str(accelerator).map_err(|_| "bad_shortcut".to_string())?;
    let settings = ctx.core.island();
    if let Some(other) = settings.hotkeys.taken_by(kind, accelerator) {
        return Err(format!("conflict:{}", serde_json::to_value(other).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()));
    }
    let stop = ctx.prefs.lock().unwrap().stop_shortcut.clone();
    if !stop.is_empty() && core_island::canonical(&stop) == core_island::canonical(accelerator) {
        return Err("conflict:stop".into());
    }
    // Is it free in the system? A key of ours that is registered now is.
    let ours = ctx.island.registered.lock().unwrap().iter().any(|(_, a)| core_island::canonical(a) == core_island::canonical(accelerator));
    if !ours {
        let gs = ctx.app.global_shortcut();
        gs.register(shortcut).map_err(|_| "shortcut_unavailable".to_string())?;
        let _ = gs.unregister(shortcut);
    }
    Ok(())
}

/// A key went down or up (main.rs routes the plugin's events here).
pub fn on_key(ctx: &Arc<Ctx>, kind: HotkeyKind, state: ShortcutState) {
    let now = Instant::now();
    match state {
        ShortcutState::Pressed => {
            // A repeat of the held key, or another key during a hold:
            // nothing to check, nothing to do.
            if ctx.island.hold.lock().unwrap().holding().is_some() {
                return;
            }
            let synth_id = match gate(ctx) {
                Ok(id) => id,
                Err((code, detail)) => {
                    ctx.island.notice(&code, detail);
                    emit_view(ctx);
                    return;
                }
            };
            let action = ctx.island.hold.lock().unwrap().down(kind, now);
            match action {
                hold::Action::OpenText => open_text(ctx),
                hold::Action::Start(kind) => start_hold(ctx, kind, synth_id),
                _ => {}
            }
        }
        ShortcutState::Released => {
            let action = ctx.island.hold.lock().unwrap().up(kind, now);
            act(ctx, action);
        }
    }
}

/// Can a message go to the main synth now? Ok(its id).
fn gate(ctx: &Arc<Ctx>) -> Result<String, (String, String)> {
    let main = ctx.core.island_main().ok_or(("no_synth".to_string(), String::new()))?;
    if ctx.core.link_brief().paused {
        return Err(("paused".into(), String::new()));
    }
    if main.state() != LinkState::Linked {
        return Err(("offline".into(), main.display_name()));
    }
    if !core_island::supports(&main) {
        return Err(("outdated".into(), main.display_name()));
    }
    {
        let inner = ctx.island.inner.lock().unwrap();
        if inner.sending || inner.recognizing {
            return Err(("busy".into(), String::new()));
        }
    }
    Ok(main.synth_id.clone())
}

fn act(ctx: &Arc<Ctx>, action: hold::Action) {
    match action {
        hold::Action::Discard => {
            // A slip: the recording (and the picture) go nowhere.
            let dropped = take_running(ctx, None);
            ctx.island.inner.lock().unwrap().transcript = None;
            if let Some(h) = dropped {
                drop_hold(h);
            }
            emit_view(ctx);
        }
        hold::Action::Send(kind, _) => {
            if let Some(h) = take_running(ctx, Some(kind)) {
                let ctx = ctx.clone();
                tauri::async_runtime::spawn(async move { finish(&ctx, kind, h).await });
            }
            emit_view(ctx);
        }
        _ => {}
    }
}

/// The running hold, at its end. None when its recording has not started
/// yet: the end is kept for it (`Inner::early`), and the recording, once
/// it starts, is sent (or dropped) at once instead of being lost.
fn take_running(ctx: &Arc<Ctx>, send: Option<HotkeyKind>) -> Option<Hold> {
    let mut running = ctx.island.running.lock().unwrap();
    let taken = running.take();
    if taken.is_none() {
        let mut inner = ctx.island.inner.lock().unwrap();
        inner.early = Some((inner.hold_seq, send));
    }
    taken
}

fn drop_hold(h: Hold) {
    if let Some(p) = h.picture {
        p.abort();
    }
    drop(h.recording);
}

fn cancel_hold(ctx: &Arc<Ctx>) {
    let action = ctx.island.hold.lock().unwrap().cancel();
    act(ctx, action);
}

fn start_hold(ctx: &Arc<Ctx>, kind: HotkeyKind, synth_id: String) {
    // The microphone: only with the owner's yes, asked once.
    match audio::permission() {
        audio::MicPermission::Denied => {
            ctx.island.hold.lock().unwrap().cancel();
            ctx.island.notice("mic_denied", "");
            emit_view(ctx);
            return;
        }
        audio::MicPermission::NotAsked => {
            ctx.island.hold.lock().unwrap().cancel();
            ctx.island.notice("mic_asking", "");
            emit_view(ctx);
            if !ctx.island.mic_asked.swap(true, Ordering::AcqRel) {
                let ctx = ctx.clone();
                audio::request_permission(Box::new(move |_granted| {
                    let ctx = ctx.clone();
                    tauri::async_runtime::spawn(async move { emit_view(&ctx) });
                }));
            }
            return;
        }
        _ => {}
    }
    let seq = {
        let mut inner = ctx.island.inner.lock().unwrap();
        inner.hold_since_ms = now_ms();
        inner.transcript = None;
        inner.hold_seq += 1;
        inner.early = None;
        inner.hold_seq
    };
    // On-device speech-to-text: is it for this hold? The model starts
    // loading now, while the microphone starts.
    let route = stt_route(ctx);
    if route == stt::Route::Device {
        ctx.island.stt.warm();
    }
    let toned = warm_tone(ctx, route);
    // The picture first, at the press, with the island out of it.
    let picture = (kind == HotkeyKind::ScreenVoice).then(|| {
        let ctx = ctx.clone();
        tauri::async_runtime::spawn(async move { take_picture(&ctx).await })
    });
    let level_ctx = ctx.clone();
    let on_level: Arc<dyn Fn(f32) + Send + Sync> = Arc::new(move |level: f32| {
        if let Some(w) = window(&level_ctx) {
            let _ = w.emit_to(WINDOW, LEVEL_EVENT, level);
        }
    });
    let ctx2 = ctx.clone();
    tauri::async_runtime::spawn(async move {
        let started = tokio::task::spawn_blocking(move || audio::start(on_level)).await;
        match started {
            Ok(Ok(recording)) => {
                let stt = match route {
                    stt::Route::Device => start_stt(&ctx2, &recording),
                    stt::Route::Synth(why) => Speech::Synth(why),
                };
                let tone = start_tone(&ctx2, toned, &recording, &stt);
                let h = Hold { recording, picture, synth_id, stt, wake: false, tone };
                // Under `running`'s lock, so a release either finds this
                // hold there or has left its end in `early` already.
                let early = {
                    let mut running = ctx2.island.running.lock().unwrap();
                    let mut inner = ctx2.island.inner.lock().unwrap();
                    match inner.early {
                        Some((s, end)) if s == seq => {
                            inner.early = None;
                            Some((end, h))
                        }
                        _ => {
                            if matches!(h.stt, Speech::Device(_)) {
                                inner.transcript = Some(String::new());
                            }
                            *running = Some(h);
                            None
                        }
                    }
                };
                match early {
                    // Let go while the microphone was starting: what was
                    // recorded goes now (a slip goes nowhere).
                    Some((Some(kind), h)) => {
                        connect_core::logln!("island: the key was let go while the microphone was starting; sending what was recorded");
                        let ctx = ctx2.clone();
                        tauri::async_runtime::spawn(async move { finish(&ctx, kind, h).await });
                        emit_view(&ctx2);
                        return;
                    }
                    Some((None, h)) => {
                        drop_hold(h);
                        emit_view(&ctx2);
                        return;
                    }
                    None => {}
                }
                emit_view(&ctx2);
                // The two-minute limit.
                let ctx3 = ctx2.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        if ctx3.island.hold.lock().unwrap().holding().is_none() {
                            break;
                        }
                        let action = ctx3.island.hold.lock().unwrap().tick(Instant::now());
                        if action != hold::Action::Nothing {
                            act(&ctx3, action);
                            break;
                        }
                    }
                });
            }
            Ok(Err(e)) => {
                if let Some(p) = picture {
                    p.abort();
                }
                ctx2.island.hold.lock().unwrap().cancel();
                let code = e.split(':').next().unwrap_or("mic_failed").to_string();
                // The path in the detail: the notice's log line says it.
                ctx2.island.notice(&code, format!("key hold, opening the microphone: {e}"));
                emit_view(&ctx2);
            }
            Err(e) => {
                ctx2.island.hold.lock().unwrap().cancel();
                ctx2.island.notice("mic_failed", format!("key hold, opening the microphone (the task failed): {e}"));
                emit_view(&ctx2);
            }
        }
    });
    emit_view(ctx);
}

/// Who recognizes the next hold's speech, by the switch, the model and
/// what the main synth takes.
fn stt_route(ctx: &Arc<Ctx>) -> stt::Route {
    let mode = ctx.core.island().stt;
    let takes_text = ctx.core.island_main().is_some_and(|m| core_island::supports_device_stt(&m));
    stt::route(mode, stt::available(), stt_files(ctx).ready(), takes_text)
}

/// The tone side of a hold that records now, when its speech is
/// recognized here and the tone is wanted (`toned`, decided at the press,
/// when the model was warmed).
fn start_tone(ctx: &Arc<Ctx>, toned: bool, recording: &audio::Recording, speech: &Speech) -> Option<stt::tone::ToneSession> {
    if !toned || !matches!(speech, Speech::Device(_)) {
        return None;
    }
    let tap = recording.tap();
    ctx.island.tone.hold(Box::new(move |from| tap.read_from(from)))
}

/// Warm the tone's model for a hold starting now (and fetch it when it is
/// missing). True when the tone is estimated for this hold.
fn warm_tone(ctx: &Arc<Ctx>, route: stt::Route) -> bool {
    let toned = route == stt::Route::Device && tone_wanted(ctx);
    if toned {
        ctx.island.tone.warm();
        tone_download(ctx, false);
    }
    toned
}

/// The tone of voice is estimated for the next hold: the owner's switch
/// is on and the main synth takes it (`island_device_tone`).
fn tone_wanted(ctx: &Arc<Ctx>) -> bool {
    ctx.core.island().stt_tone && ctx.core.island_main().is_some_and(|m| core_island::supports_device_tone(&m))
}

/// Start recognizing a hold that records now; its words go to the island
/// as they come.
fn start_stt(ctx: &Arc<Ctx>, recording: &audio::Recording) -> Speech {
    let tap = recording.tap();
    let live_ctx = ctx.clone();
    let session = ctx.island.stt.hold(
        Box::new(move |from| tap.read_from(from)),
        Box::new(move |text| {
            let ctx = live_ctx.clone();
            // Only while the key is still held: a late step must not
            // overwrite the final text.
            if ctx.island.hold.lock().unwrap().holding().is_none() {
                return;
            }
            ctx.island.inner.lock().unwrap().transcript = Some(text);
            tauri::async_runtime::spawn(async move { emit_view(&ctx) });
        }),
    );
    match session {
        Ok(s) => Speech::Device(s),
        Err(why) => Speech::Synth(why),
    }
}

async fn take_picture(ctx: &Arc<Ctx>) -> Result<core_island::Image, String> {
    let display = panel::display_under_cursor();
    let invisible = |on: bool| {
        let ctx = ctx.clone();
        let _ = ctx.app.clone().run_on_main_thread(move || {
            if let Some(w) = window(&ctx) {
                panel::set_invisible(&w, on);
            }
        });
    };
    invisible(true);
    // Let the window server draw the frame without the island.
    tokio::time::sleep(Duration::from_millis(60)).await;
    let shot = tokio::time::timeout(Duration::from_secs(12), shot::take(&ctx.core, &ctx.data_dir, display)).await;
    invisible(false);
    match shot {
        Ok(r) => r,
        Err(_) => Err("shot_failed: the picture took too long".into()),
    }
}

/// A released hold becomes a message: the text recognized here, the
/// audio, or (screenshot + voice) the screenshot alone; nothing only when
/// the voice detector heard no speech in the whole recording
/// (`stt::release`). One log line says which and why.
async fn finish(ctx: &Arc<Ctx>, kind: HotkeyKind, h: Hold) {
    let Hold { recording, picture, synth_id, stt, wake, tone } = h;
    let released = Instant::now();
    // From the release until the message is on its way the island says
    // "sending", whatever happens below (a panic included).
    struct Recognizing(Arc<Ctx>);
    impl Drop for Recognizing {
        fn drop(&mut self) {
            self.0.island.inner.lock().unwrap().recognizing = false;
        }
    }
    ctx.island.inner.lock().unwrap().recognizing = true;
    let recognizing = Recognizing(ctx.clone());
    emit_view(ctx);

    let captured = tokio::task::spawn_blocking(move || recording.finish()).await.map_err(|e| e.to_string()).and_then(|r| r);
    let mut captured = match captured {
        Ok(c) => c,
        Err(e) => {
            if let Some(p) = picture {
                p.abort();
            }
            drop(recognizing);
            ctx.island.inner.lock().unwrap().transcript = None;
            let path = if wake { "wake hold" } else { "key hold" };
            ctx.island.notice("mic_failed", format!("{path}, stopping the recording at the release: {e}"));
            emit_view(ctx);
            return;
        }
    };
    // The tone first, so it runs while the last words are decoded.
    let tone = match (&stt, tone) {
        (Speech::Device(_), Some(t)) => Some(t.finish(captured.pcm.clone())),
        _ => None,
    };
    // The words, recognized here (bounded by stt::RELEASE_WAIT).
    let answer = match stt {
        Speech::Device(session) => {
            let pcm = std::mem::take(&mut captured.pcm);
            let answer = tokio::task::spawn_blocking(move || session.finish(pcm)).await.unwrap_or_else(|e| stt::Answer::Failed(format!("the speech task failed: {e}")));
            Ok(answer)
        }
        Speech::Synth(why) => Err(why),
    };
    let engine = ctx.island.stt.engine();
    let mut images = Vec::new();
    if let Some(p) = picture {
        match p.await {
            Ok(Ok(image)) => images.push(image),
            // The voice still goes; the island says the picture did not.
            Ok(Err(e)) => {
                let code = if e.starts_with("shot_permission") { "shot_permission" } else { "shot_failed" };
                ctx.island.notice(code, e);
            }
            Err(_) => ctx.island.notice("shot_failed", "the picture was not taken"),
        }
    }
    let release = match &answer {
        Err(why) => stt::Release::Audio((*why).to_string()),
        Ok(a) => {
            let (heard, timed_out, error) = match a {
                stt::Answer::Heard(h) => (Some(h), false, None),
                stt::Answer::TimedOut => (None, true, None),
                stt::Answer::Failed(e) => (None, false, Some(e.as_str())),
            };
            stt::decide(&stt::AtRelease {
                engine,
                vad: heard.map(|h| h.vad).unwrap_or(stt::Vad::NotRun),
                text: heard.map(|h| h.text.as_str()),
                timed_out,
                error,
                picture: !images.is_empty(),
            })
        }
    };
    let waited = released.elapsed().as_millis();
    let secs = captured.ms as f32 / 1000.0;
    let audio = Audio { mime: "audio/wav".into(), data: captured.wav, ms: captured.ms };
    let mode = if kind == HotkeyKind::ScreenVoice && !images.is_empty() { Mode::ScreenVoice } else { Mode::Voice };
    let (msg, fallback) = match release {
        stt::Release::Text(text) => {
            // Wake by name: the request without the name in front; the
            // name alone sends nothing and shows nothing.
            let text = match wake.then(|| wake::request(ctx, &text)) {
                None => text,
                Some(Some(request)) => request,
                Some(None) => {
                    connect_core::logln!("island: wake: only the name was said; nothing sent");
                    ctx.island.wake_word.count_dropped();
                    drop(recognizing);
                    ctx.island.inner.lock().unwrap().transcript = None;
                    emit_view(ctx);
                    return;
                }
            };
            let voice_tone = match tone {
                Some(answer) => describe_tone(ctx, answer, &text).await,
                None => None,
            };
            let waited = released.elapsed().as_millis();
            connect_core::logln!(
                "island: speech-to-text: sent transcript {waited} ms after release ({} characters, {secs:.1} s of audio{})",
                text.chars().count(),
                if voice_tone.is_some() { ", with the tone" } else { "" }
            );
            let msg = OwnerMessage { client_id: core_island::new_client_id(), mode, text: String::new(), audio: None, images, voice_transcript: Some(text.clone()), voice_tone };
            ctx.island.inner.lock().unwrap().transcript = Some(text);
            (msg, Some(audio))
        }
        stt::Release::Audio(why) => {
            connect_core::logln!("island: speech-to-text: sent audio because {why} ({secs:.1} s of audio, {waited} ms after release)");
            ctx.island.inner.lock().unwrap().transcript = None;
            (OwnerMessage { client_id: core_island::new_client_id(), mode, text: String::new(), audio: Some(audio), images, voice_transcript: None, voice_tone: None }, None)
        }
        stt::Release::PictureOnly => {
            connect_core::logln!("island: speech-to-text: sent the screenshot alone: silence (vad) ({secs:.1} s of audio)");
            ctx.island.inner.lock().unwrap().transcript = None;
            (OwnerMessage { client_id: core_island::new_client_id(), mode: Mode::ScreenVoice, text: String::new(), audio: None, images, voice_transcript: None, voice_tone: None }, None)
        }
        stt::Release::Silent => {
            connect_core::logln!("island: speech-to-text: dropped: silence (vad) ({secs:.1} s of audio)");
            drop(recognizing);
            ctx.island.inner.lock().unwrap().transcript = None;
            // Seen, not just logged: the owner knows nothing went. A wake
            // heard nothing after the name: nothing to say.
            if !wake {
                ctx.island.notice("heard_nothing", "");
            }
            emit_view(ctx);
            return;
        }
    };
    // `send_with` says "sending" from here on.
    drop(recognizing);
    send_with(ctx, &synth_id, msg, fallback).await;
}

/// The tone line for a hold sent as text: the best answer of the tone
/// thread within `TONE_WAIT` of the text (none: the message goes without
/// it), described against the owner's usual manner. One log line, without
/// the words.
async fn describe_tone(ctx: &Arc<Ctx>, answer: std::sync::mpsc::Receiver<stt::tone::Acoustic>, text: &str) -> Option<String> {
    let ctx = ctx.clone();
    let text = text.to_string();
    let described = tokio::task::spawn_blocking(move || {
        let t = Instant::now();
        let got = stt::tone::wait_for(&answer, stt::tone::TONE_WAIT);
        let waited = t.elapsed().as_millis();
        match got {
            Some(a) => {
                let line = ctx.island.tone.describe(&a, &text);
                connect_core::logln!(
                    "island: tone: {} characters, emotion {} ({}), waited {waited} ms",
                    line.chars().count(),
                    a.emotion.map(|e| { let (l, p) = e.top(); format!("{l} {p:.2}") }).unwrap_or_else(|| "none".into()),
                    if a.complete { "complete" } else { "from the runs while held" }
                );
                Some(line).filter(|l| !l.is_empty())
            }
            None => {
                connect_core::logln!("island: tone: not ready within {waited} ms; sent without it");
                None
            }
        }
    })
    .await;
    described.ok().flatten()
}

/// Send one message and follow it.
pub async fn send(ctx: &Arc<Ctx>, synth_id: &str, msg: OwnerMessage) {
    send_with(ctx, synth_id, msg, None).await
}

/// Send one message and follow it. With a transcript, `fallback` is the
/// voice it came from: when the synth turns out not to take a transcript
/// (the link came back with an older synth) the voice goes instead, with
/// the same client_id.
async fn send_with(ctx: &Arc<Ctx>, synth_id: &str, mut msg: OwnerMessage, fallback: Option<Audio>) {
    {
        let mut inner = ctx.island.inner.lock().unwrap();
        if inner.sending {
            drop(inner);
            ctx.island.notice("busy", "");
            emit_view(ctx);
            return;
        }
        inner.sending = true;
    }
    emit_view(ctx);
    let mut result = ctx.core.owner_message(synth_id, &msg).await;
    if let (Err(e), Some(audio)) = (&result, fallback) {
        if e.code == core_island::NO_DEVICE_STT {
            connect_core::logln!("island: the synth does not take a transcript; sending the voice");
            msg.voice_transcript = None;
            msg.voice_tone = None;
            msg.audio = Some(audio);
            ctx.island.inner.lock().unwrap().transcript = None;
            emit_view(ctx);
            result = ctx.core.owner_message(synth_id, &msg).await;
        }
    }
    {
        let mut inner = ctx.island.inner.lock().unwrap();
        inner.sending = false;
        if let Ok(message_id) = &result {
            // The synth may have reported "accepted" (or more) already.
            let known = inner.message.as_ref().is_some_and(|(m, _)| m.message_id == *message_id);
            if !known {
                inner.message = Some((
                    MessageView {
                        synth_id: synth_id.to_string(),
                        message_id: message_id.clone(),
                        stage: "accepted".into(),
                        at_ms: now_ms(),
                        summary: String::new(),
                        channel: String::new(),
                    },
                    Instant::now(),
                ));
            }
        }
    }
    if let Err(e) = result {
        ctx.island.notice(&e.code, e.detail);
    }
    emit_view(ctx);
}

// --- the line of text ---

fn open_text(ctx: &Arc<Ctx>) {
    ctx.island.inner.lock().unwrap().text_open = true;
    if let Some(w) = window(ctx) {
        let _ = w.set_ignore_cursor_events(false);
    }
    ctx.island.inner.lock().unwrap().ignoring = Some(false);
    let ctx2 = ctx.clone();
    let _ = ctx.app.run_on_main_thread(move || {
        if let Some(w) = window(&ctx2) {
            panel::set_keyboard(&w, true);
        }
    });
    emit_view(ctx);
}

pub fn close_text(ctx: &Arc<Ctx>) {
    let was = std::mem::replace(&mut ctx.island.inner.lock().unwrap().text_open, false);
    if was {
        let ctx2 = ctx.clone();
        let _ = ctx.app.run_on_main_thread(move || {
            if let Some(w) = window(&ctx2) {
                panel::set_keyboard(&w, false);
            }
        });
        // The poller puts click-through back as the pointer says.
        ctx.island.inner.lock().unwrap().ignoring = None;
    }
    emit_view(ctx);
}

/// Enter in the line of text.
pub async fn submit_text(ctx: &Arc<Ctx>, text: String) -> Result<(), String> {
    close_text(ctx);
    let text = text.trim().to_string();
    if text.is_empty() {
        return Ok(());
    }
    let synth_id = gate(ctx).map_err(|(code, detail)| {
        ctx.island.notice(&code, detail);
        emit_view(ctx);
        code
    })?;
    ctx.island.inner.lock().unwrap().transcript = None;
    let msg = OwnerMessage { client_id: core_island::new_client_id(), mode: Mode::Text, text, audio: None, images: vec![], voice_transcript: None, voice_tone: None };
    send(ctx, &synth_id, msg).await;
    Ok(())
}

/// "Ask <name> to design their own face": a plain line of text to that
/// synth, in the owner's language. The synth uses its face tool itself.
pub async fn ask_design(ctx: &Arc<Ctx>, synth_id: &str) -> Result<(), String> {
    let text = match ctx.lang() {
        crate::i18n::Lang::Ru => "Придумай себе лицо для острова SNTH Connect на моём компьютере (инструмент connect_face): глаза, рот, цвета, аксессуар, как двигаешься. Такое, какое тебе по душе, и расскажи, что получилось и почему так.",
        _ => "Design your own face for the SNTH Connect island on my computer (your connect_face tool): eyes, mouth, colours, an accessory, how you move. Make it feel like you, and tell me what you chose and why.",
    };
    let msg = OwnerMessage { client_id: core_island::new_client_id(), mode: Mode::Text, text: text.into(), audio: None, images: vec![], voice_transcript: None, voice_tone: None };
    ctx.core.owner_message(synth_id, &msg).await.map(|_| ()).map_err(|e| e.code)
}

// --- on-device speech-to-text: the switch, the model, the offer ---

/// The chosen engine's files (the speech thread takes the choice too).
/// On Windows that is Parakeet's whatever the settings say (`effective`).
pub fn stt_files(ctx: &Ctx) -> Arc<stt::model::Models> {
    let e = ctx.core.island().stt_engine;
    ctx.island.stt.select(e);
    ctx.island.stt.models_of(e).clone()
}

/// The model's status changed: the island and the main window hear of it.
fn stt_changed(ctx: &Arc<Ctx>) -> Arc<dyn Fn() + Send + Sync> {
    let ctx = ctx.clone();
    Arc::new(move || {
        let ctx = ctx.clone();
        tauri::async_runtime::spawn(async move {
            emit_view(&ctx);
            crate::state::emit_state(&ctx);
            // The speech model is here: the tone's model follows it.
            let ready = stt_files(&ctx).ready();
            if ready {
                tone_download(&ctx, false);
            }
            // "Ready" leaves the island by itself.
            if ready && ctx.island.inner.lock().unwrap().stt_follow {
                tokio::time::sleep(STT_READY_FOR + Duration::from_millis(200)).await;
                emit_view(&ctx);
            }
        });
    })
}

/// Fetch an engine's model, the chosen one when None (the owner asked:
/// the switch, the engine choice, the offer, "download"). Err("no_space")
/// when the disk is short; the status says it too.
pub fn stt_download(ctx: &Arc<Ctx>, engine: Option<SttEngine>) -> Result<(), String> {
    if !stt::available() {
        return Err("unsupported".into());
    }
    let chosen = stt::effective(ctx.core.island().stt_engine);
    let e = stt::effective(engine.unwrap_or(chosen));
    let r = ctx.island.stt.models_of(e).download(stt_changed(ctx));
    if e == chosen {
        tone_download(ctx, true);
    }
    r
}

/// The engine choice in Settings. With speech recognized here, the
/// chosen engine's model is fetched if it is not here (the owner asked),
/// and the other engine is unloaded.
pub fn stt_engine_set(ctx: &Arc<Ctx>, engine: SttEngine) -> Result<(), String> {
    // Whisper on Windows: not in this build (Settings does not offer it).
    if !stt::available() || !stt::ENGINES.contains(&engine) {
        return Err("unsupported".into());
    }
    ctx.core.set_island(|i| i.stt_engine = engine);
    ctx.island.stt.switch(engine);
    connect_core::logln!("island: stt: engine {} chosen", stt::engine_name(engine));
    if ctx.core.island().stt == SttMode::Device {
        let _ = stt_download(ctx, Some(engine));
        ctx.island.stt.warm();
    }
    emit_view(ctx);
    Ok(())
}

/// Fetch the tone's emotion model when the switch is on, speech is
/// recognized here and its model is here already (the big one first).
/// `asked`: the owner asked now, so a recent failure is tried again.
fn tone_download(ctx: &Arc<Ctx>, asked: bool) {
    let s = ctx.core.island();
    if !s.stt_tone || s.stt != SttMode::Device || !ctx.island.stt.models_of(s.stt_engine).ready() {
        return;
    }
    let ctx2 = ctx.clone();
    ctx.island.tone.download(
        asked,
        Arc::new(move || {
            ctx2.island.tone.note_status();
            let ctx = ctx2.clone();
            tauri::async_runtime::spawn(async move { crate::state::emit_state(&ctx) });
        }),
    );
}

/// The tone switch in Settings. On: its model is fetched if it is not
/// here (once the speech model is).
pub fn stt_tone_set(ctx: &Arc<Ctx>, on: bool) {
    ctx.core.set_island(|i| i.stt_tone = on);
    if on {
        tone_download(ctx, true);
    } else {
        ctx.island.tone.models.cancel();
        ctx.island.tone.unload();
    }
}

/// The switch in Settings. On: the model is fetched if it is not here.
pub fn stt_set(ctx: &Arc<Ctx>, mode: SttMode) -> Result<(), String> {
    if mode == SttMode::Device && !stt::available() {
        return Err("unsupported".into());
    }
    ctx.core.set_island(|i| {
        i.stt = mode;
        i.stt_offer = i.stt_offer.chosen_in_settings(mode);
    });
    if mode == SttMode::Device {
        // A full disk shows in the model's status; the switch stays on and
        // the voice goes as audio until the model is here.
        let _ = stt_download(ctx, None);
        ctx.island.stt.warm();
    }
    emit_view(ctx);
    Ok(())
}

/// The owner answered the synth's offer on the island.
pub fn stt_offer_answer(ctx: &Arc<Ctx>, accept: bool) -> Result<(), String> {
    if accept && !stt::available() {
        return Err("unsupported".into());
    }
    let before = ctx.core.island().stt_offer;
    if !before.open() {
        return Ok(());
    }
    ctx.core.set_island(|i| {
        i.stt_offer = i.stt_offer.answered(accept);
        if accept {
            i.stt = SttMode::Device;
        }
    });
    connect_core::logln!("island: the speech-to-text offer was {}", if accept { "accepted" } else { "declined" });
    if accept {
        let ready = stt_files(ctx).ready();
        {
            let mut inner = ctx.island.inner.lock().unwrap();
            inner.stt_follow = !ready;
            inner.stt_ready_at = None;
        }
        let _ = stt_download(ctx, None);
    }
    emit_view(ctx);
    Ok(())
}

/// "Hide" on the download's card: it goes on, in Settings.
pub fn stt_card_hide(ctx: &Arc<Ctx>) {
    ctx.island.inner.lock().unwrap().stt_follow = false;
    emit_view(ctx);
}

/// "Delete the model" of an engine (the chosen one when None): its files
/// go. The chosen engine's: the speech goes to the synth. The tone's model
/// goes with the last speech model.
pub fn stt_delete(ctx: &Arc<Ctx>, engine: Option<SttEngine>) -> Result<(), String> {
    let chosen = stt::effective(ctx.core.island().stt_engine);
    let e = stt::effective(engine.unwrap_or(chosen));
    let mut result = ctx.island.stt.models_of(e).delete();
    // The other engine of this build (none on Windows).
    let other_ready = stt::ENGINES.iter().any(|o| *o != e && ctx.island.stt.models_of(*o).ready());
    if !other_ready {
        ctx.island.tone.unload();
        result = result.and(ctx.island.tone.models.delete());
    }
    if e == chosen {
        ctx.core.set_island(|i| i.stt = SttMode::Synth);
        ctx.island.inner.lock().unwrap().stt_follow = false;
    }
    emit_view(ctx);
    result
}

// --- the webview's side ---

/// The webview says which part of the window takes clicks and where the
/// face's centre is (window-local CSS pixels).
pub fn layout(ctx: &Arc<Ctx>, hit: Rect, face: (f64, f64)) {
    let mut inner = ctx.island.inner.lock().unwrap();
    inner.hit = Some(hit);
    inner.face = Some(face);
    inner.last_target = None;
}

// --- events ---

/// The core's events the island cares about.
pub fn spawn(ctx: Arc<Ctx>) {
    // The engine loaded (or failed) mid-hold: the island's "loading" goes.
    let weak = Arc::downgrade(&ctx);
    ctx.island.stt.on_change(Arc::new(move || {
        if let Some(ctx) = weak.upgrade() {
            tauri::async_runtime::spawn(async move { emit_view(&ctx) });
        }
    }));
    let mut events = ctx.core.subscribe();
    let poll_ctx = ctx.clone();
    tauri::async_runtime::spawn(async move { poll(poll_ctx).await });
    wake::spawn(ctx.clone());
    tauri::async_runtime::spawn(async move {
        sync(&ctx);
        loop {
            match events.recv().await {
                Ok(CoreEvent::Island(e)) => on_event(&ctx, e),
                Ok(CoreEvent::Face { .. }) | Ok(CoreEvent::ComputerTask) => emit_view(&ctx),
                Ok(CoreEvent::Link { .. } | CoreEvent::Paused { .. } | CoreEvent::Paired { .. } | CoreEvent::Unpaired { .. } | CoreEvent::Settings { .. })
                | Err(RecvError::Lagged(_)) => sync(&ctx),
                Ok(_) => {}
                Err(RecvError::Closed) => break,
            }
        }
    });
}

fn on_event(ctx: &Arc<Ctx>, e: IslandEvent) {
    {
        let mut inner = ctx.island.inner.lock().unwrap();
        let ours = match &inner.message {
            Some((m, _)) => m.message_id == e.message_id || (m.message_id.is_empty() && m.synth_id == e.synth_id),
            // An event before owner.message returned (it is fast): the
            // message is this one if it is from the synth just sent to.
            None => inner.sending,
        };
        if !ours {
            return;
        }
        let summary = if e.kind == "done" || e.kind == "error" { e.summary.clone() } else { String::new() };
        let order = |s: &str| ["accepted", "working", "done", "error"].iter().position(|k| *k == s).unwrap_or(0);
        // Never backwards (events may cross on the wire).
        if inner.message.as_ref().is_some_and(|(m, _)| order(&m.stage) > order(&e.kind)) {
            return;
        }
        inner.message = Some((
            MessageView { synth_id: e.synth_id, message_id: e.message_id, stage: e.kind, at_ms: now_ms(), summary, channel: e.channel },
            Instant::now(),
        ));
    }
    emit_view(ctx);
}

// --- the pointer ---

async fn poll(ctx: Arc<Ctx>) {
    let mut placed_at = Instant::now();
    loop {
        let shown = ctx.island.inner.lock().unwrap().shown;
        if !shown {
            ctx.island.wake.notified().await;
            continue;
        }
        // No word from the system when the taskbar (Windows) or the Dock
        // (macOS, not for certain) changes: a floating island looks again
        // now and then (the window moves only when the placement changed,
        // `relayout_main_thread`). The macOS notch never polls.
        if placed_at.elapsed() >= REPLACE_EVERY {
            placed_at = Instant::now();
            let floating = ctx.island.inner.lock().unwrap().placement.is_none_or(|p| p.floating);
            if floating || !cfg!(target_os = "macos") {
                let ctx2 = ctx.clone();
                let _ = ctx.app.run_on_main_thread(move || relayout_main_thread(&ctx2));
            }
        }
        let moving = ctx.island.inner.lock().unwrap().last_move.is_some_and(|t| t.elapsed() < MOVING_FOR);
        // A Space switch wakes it early (`watch_spaces`).
        tokio::select! {
            _ = tokio::time::sleep(if moving { POLL_MOVING } else { POLL_RESTING }) => {}
            _ = ctx.island.wake.notified() => {}
        }
        let Some(w) = window(&ctx) else { continue };
        let (placement, hit, face) = {
            let inner = ctx.island.inner.lock().unwrap();
            (inner.placement, inner.hit, inner.face)
        };
        let Some(placement) = placement else { continue };
        let tl = (placement.window.x, placement.window.y);
        let Some(cursor) = panel::cursor_local(&w, tl) else { continue };

        // Does the menu bar coming in over a full-screen app bring the
        // island back? The notch and the top spots (and Windows, where
        // there is no bar to come); a bottom or middle spot on macOS stays
        // hidden.
        let reveal = !(cfg!(target_os = "macos") && placement.floating && placement.edge != geometry::Edge::Top);
        // Full screen: look at the screen again when it is time.
        let look = {
            let inner = ctx.island.inner.lock().unwrap();
            let near_top = reveal && cursor.1 < placement.window.h;
            let every = if inner.cover.covered && near_top { COVER_FAST } else { COVER_EVERY };
            inner.cover_at.is_none_or(|t| t.elapsed() >= every).then(|| inner.screen.clone()).flatten()
        };
        if let Some(screen) = look {
            let cover = panel::cover(&screen).unwrap_or_default();
            let mut inner = ctx.island.inner.lock().unwrap();
            inner.cover = cover;
            inner.cover_at = Some(Instant::now());
        }
        let holding = ctx.island.hold.lock().unwrap().holding().is_some();
        let approval = !ctx.approvals.is_empty();

        // Clicks: only the shape takes them; the line of text keeps them;
        // a hidden island takes none.
        let inside = hit.is_some_and(|r| r.contains(cursor.0, cursor.1));
        let mut changed_view = false;
        let (hidden, appeared) = {
            let mut inner = ctx.island.inner.lock().unwrap();
            // Shown with the menu bar, it stays while the pointer is on it
            // (the bar itself goes once the pointer leaves the bar).
            let held = inside && !inner.hidden;
            let tucked = inner.cover.covered && !(reveal && inner.cover.bar) && !held;
            let hidden = tucked && !busy(&inner, holding, approval);
            inner.tucked = tucked;
            let appeared = inner.hidden && !hidden;
            if inner.hidden != hidden {
                inner.hidden = hidden;
                changed_view = true;
            }
            let inside = inside && !hidden;
            let want_ignore = hidden || !(inside || inner.text_open);
            if inner.ignoring != Some(want_ignore) {
                let _ = w.set_ignore_cursor_events(want_ignore);
                inner.ignoring = Some(want_ignore);
            }
            if inner.hover != inside {
                inner.hover = inside;
                changed_view = true;
            }
            (hidden, appeared)
        };
        if changed_view {
            emit_view(&ctx);
        }

        // The eyes: the synth's pointer while it drives, else the owner's.
        let Some(face) = face else { continue };
        let driving_point = ctx
            .core
            .shared()
            .computer_driver()
            .filter(|(id, _)| ctx.core.island_main().is_some_and(|m| m.synth_id == *id))
            .and_then(|_| ctx.core.shared().agent_cursor())
            .map(|p| panel::point_local(&w, tl, p));
        let target = driving_point.unwrap_or(cursor);
        let moved = {
            let mut inner = ctx.island.inner.lock().unwrap();
            let moved = inner.last_target.is_none_or(|(x, y)| (x - target.0).abs() > 0.5 || (y - target.1).abs() > 0.5);
            if moved {
                inner.last_target = Some(target);
                inner.last_move = Some(Instant::now());
            }
            moved
        };
        // Hidden: the pointer still sets the pace (the menu bar comes in
        // when it reaches the top), but there are no eyes to move; they
        // look at it again the moment the island is back.
        if !hidden && (moved || appeared) {
            let (gx, gy) = geometry::gaze(face, target, GAZE_REACH);
            let _ = w.emit_to(WINDOW, GAZE_EVENT, serde_json::json!({"x": gx, "y": gy, "agent": driving_point.is_some()}));
        }
    }
}
