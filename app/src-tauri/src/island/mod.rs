//! The island (TZ tz-island.md, section 5): the synth's face at the top
//! of the screen, merged with the notch, and the owner's hold keys.
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
//! The keys (`hold.rs`) are registered only while the island is on and
//! the main synth is linked and the app is not paused: otherwise the
//! combinations reach the other apps as usual. A hold records the voice
//! (`audio.rs`), screenshot + voice also takes a picture at the press
//! (`shot.rs`); the release sends `owner.message` (connect-core
//! island.rs) and the synth's island.event moves the face through
//! accepted, thinking, working and done (with the summary).

pub mod audio;
pub mod geometry;
pub mod hold;
pub mod panel;
pub mod shot;

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use connect_core::island::{self as core_island, Audio, HotkeyKind, Hotkeys, IslandEvent, Mode, OwnerMessage};
use connect_core::{CoreEvent, LinkState};
use serde::Serialize;
use serde_json::Value;
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::Notify;

use crate::approvals::ApprovalDto;
use crate::state::Ctx;
use geometry::{Placement, Rect};

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
    pub hotkeys: Hotkeys,
    pub mic: audio::MicPermission,
}

#[derive(Default)]
struct Inner {
    created: bool,
    shown: bool,
    placement: Option<Placement>,
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
}

struct Hold {
    recording: audio::Recording,
    picture: Option<tauri::async_runtime::JoinHandle<Result<core_island::Image, String>>>,
    synth_id: String,
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
}

impl Default for Island {
    fn default() -> Self {
        Island {
            inner: Mutex::new(Inner::default()),
            hold: Mutex::new(hold::Hold::default()),
            running: Mutex::new(None),
            wake: Notify::new(),
            keys: Mutex::new(HashMap::new()),
            registered: Mutex::new(Vec::new()),
            problems: Mutex::new(HashMap::new()),
            mic_asked: AtomicBool::new(false),
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
    View {
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
        sending: inner.sending,
        message: inner.message.as_ref().map(|(m, _)| m.clone()),
        notice: inner.notice.as_ref().map(|(n, _)| n.clone()),
        approval: ctx.approvals.list().into_iter().next(),
        hover: inner.hover,
        hotkeys: settings.hotkeys,
        mic: audio::permission(),
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
    emit_view(ctx);
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
    let placement = geometry::place(&screen);
    let changed = ctx.island.inner.lock().unwrap().placement != Some(placement);
    if changed {
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
    if ctx.island.inner.lock().unwrap().sending {
        return Err(("busy".into(), String::new()));
    }
    Ok(main.synth_id.clone())
}

fn act(ctx: &Arc<Ctx>, action: hold::Action) {
    match action {
        hold::Action::Discard => {
            // A slip: the recording (and the picture) go nowhere.
            let dropped = ctx.island.running.lock().unwrap().take();
            if let Some(h) = dropped {
                if let Some(p) = h.picture {
                    p.abort();
                }
                drop(h.recording);
            }
            emit_view(ctx);
        }
        hold::Action::Send(kind, _) => {
            let taken = ctx.island.running.lock().unwrap().take();
            if let Some(h) = taken {
                let ctx = ctx.clone();
                tauri::async_runtime::spawn(async move { finish(&ctx, kind, h).await });
            }
            emit_view(ctx);
        }
        _ => {}
    }
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
    ctx.island.inner.lock().unwrap().hold_since_ms = now_ms();
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
                // Released while the microphone was starting: the hold is
                // over already and this recording goes nowhere.
                if ctx2.island.hold.lock().unwrap().holding() != Some(kind) {
                    if let Some(p) = picture {
                        p.abort();
                    }
                    return;
                }
                *ctx2.island.running.lock().unwrap() = Some(Hold { recording, picture, synth_id });
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
                ctx2.island.notice(&code, e);
                emit_view(&ctx2);
            }
            Err(e) => {
                ctx2.island.hold.lock().unwrap().cancel();
                ctx2.island.notice("mic_failed", e.to_string());
                emit_view(&ctx2);
            }
        }
    });
    emit_view(ctx);
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

async fn finish(ctx: &Arc<Ctx>, kind: HotkeyKind, h: Hold) {
    let Hold { recording, picture, synth_id } = h;
    let captured = tokio::task::spawn_blocking(move || recording.finish()).await.map_err(|e| e.to_string()).and_then(|r| r);
    let captured = match captured {
        Ok(c) => c,
        Err(e) => {
            if let Some(p) = picture {
                p.abort();
            }
            ctx.island.notice("mic_failed", e);
            emit_view(ctx);
            return;
        }
    };
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
    let mode = if kind == HotkeyKind::ScreenVoice && !images.is_empty() { Mode::ScreenVoice } else { Mode::Voice };
    let msg = OwnerMessage {
        client_id: core_island::new_client_id(),
        mode,
        text: String::new(),
        audio: Some(Audio { mime: "audio/wav".into(), data: captured.wav, ms: captured.ms }),
        images,
    };
    send(ctx, &synth_id, msg).await;
}

/// Send one message and follow it.
pub async fn send(ctx: &Arc<Ctx>, synth_id: &str, msg: OwnerMessage) {
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
    let result = ctx.core.owner_message(synth_id, &msg).await;
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
    let msg = OwnerMessage { client_id: core_island::new_client_id(), mode: Mode::Text, text, audio: None, images: vec![] };
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
    let msg = OwnerMessage { client_id: core_island::new_client_id(), mode: Mode::Text, text: text.into(), audio: None, images: vec![] };
    ctx.core.owner_message(synth_id, &msg).await.map(|_| ()).map_err(|e| e.code)
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
    let mut events = ctx.core.subscribe();
    let poll_ctx = ctx.clone();
    tauri::async_runtime::spawn(async move { poll(poll_ctx).await });
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
    loop {
        let shown = ctx.island.inner.lock().unwrap().shown;
        if !shown {
            ctx.island.wake.notified().await;
            continue;
        }
        let moving = ctx.island.inner.lock().unwrap().last_move.is_some_and(|t| t.elapsed() < MOVING_FOR);
        tokio::time::sleep(if moving { POLL_MOVING } else { POLL_RESTING }).await;
        let Some(w) = window(&ctx) else { continue };
        let (placement, hit, face) = {
            let inner = ctx.island.inner.lock().unwrap();
            (inner.placement, inner.hit, inner.face)
        };
        let Some(placement) = placement else { continue };
        let tl = (placement.window.x, placement.window.y);
        let Some(cursor) = panel::cursor_local(&w, tl) else { continue };

        // Clicks: only the shape takes them; the line of text keeps them.
        let inside = hit.is_some_and(|r| r.contains(cursor.0, cursor.1));
        let mut changed_view = false;
        {
            let mut inner = ctx.island.inner.lock().unwrap();
            let want_ignore = !(inside || inner.text_open);
            if inner.ignoring != Some(want_ignore) {
                let _ = w.set_ignore_cursor_events(want_ignore);
                inner.ignoring = Some(want_ignore);
            }
            if inner.hover != inside {
                inner.hover = inside;
                changed_view = true;
            }
        }
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
        if moved {
            let (gx, gy) = geometry::gaze(face, target, GAZE_REACH);
            let _ = w.emit_to(WINDOW, GAZE_EVENT, serde_json::json!({"x": gx, "y": gy, "agent": driving_point.is_some()}));
        }
    }
}
