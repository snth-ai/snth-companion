//! Wake by name (tz-wake-word.md): the owner says the main synth's name
//! ("Мия, найди билеты в Бангкок") and the island takes what follows as a
//! hold, with no key held.
//!
//! Off by default (`IslandSettings::wake`). On, one thread (`island-wake`)
//! keeps the microphone open (`audio.rs`, the hold's own capture) and
//! feeds the listener (`listen.rs`): the loudness finds stretches of
//! sound, Silero says which are speech, whisper tiny (`ears.rs`) decodes
//! the first two seconds of each, and `names.rs` says whether they start
//! with the name. Only the last few seconds of audio are kept, in memory.
//!
//! The name heard, the recording that runs becomes a hold (`start`): cut
//! to the start of the speech (with half a second before it), put where a
//! key's hold goes, with the same speech-to-text session, so the release
//! (`end`) sends it through the hold's own path (`island::finish`): the
//! text recognized here or the audio, with every fallback a hold has. A
//! text that is only the name sends nothing (`request`). The listener
//! ends the request after a second of silence, at a minute at the most;
//! Escape or a click on the island cancels it. Then the microphone opens
//! afresh and two seconds are skipped.
//!
//! It pauses by itself (`pause.rs`, `system.rs`): while the app is paused,
//! while a hold key is down, while the screen is locked, while another app
//! records (a call). It never opens a Bluetooth microphone (`device.rs`:
//! that turns AirPods, or any headset, into a phone line): with a
//! Bluetooth default it listens on the built-in microphone, and pauses
//! where there is none. Whisper tiny and the voice detector are their own
//! download (`FILES`, about 33 MB, in `wake/` of the app's data folder),
//! fetched when the owner turns the mode on, and in memory only while it
//! is on.
//!
//! Where (build.rs, cfg `wake_whisper`): Apple-silicon Macs and Windows
//! x64, both with whisper.cpp's whisper tiny on the CPU (`ears.rs`). On
//! Windows the voice detector is the ONNX Silero (`stt/silero.rs`, the
//! one the hold's Parakeet uses there), the system's answers come from
//! the microphone's consent store, the session and the MMDevice API
//! (`system.rs`), and the processor must have AVX2 (`available`), the
//! floor whisper.cpp is built for there. Whisper tiny rather than
//! Parakeet as the ears on Windows: about 80 MB in memory instead of
//! about 1.1 GB for as long as the mode is on, and the same model, prompt
//! and `names.rs` tuning as on a Mac.

pub mod device;
pub mod listen;
pub mod names;
pub mod pause;
pub mod system;

#[cfg(wake_whisper)]
pub mod ears;

use std::str::FromStr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use connect_core::island::{self as core_island, HotkeyKind};
use connect_core::LinkState;
use serde::Serialize;
use tauri::Emitter;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

use super::stt::model::{self, ModelFile, Models};
use super::{audio, hold, stt};
use crate::state::Ctx;
use listen::{Ears, Event, Listener};
use names::Names;
use pause::{Inputs, Status};

/// Whisper tiny, multilingual, q5_1 (ggerganov/whisper.cpp at 5359861,
/// the revision of the hold's model).
pub const TINY: ModelFile = ModelFile {
    name: "ggml-tiny-q5_1.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-tiny-q5_1.bin",
    sha256: "818710568da3ca15689e31a743197b520007872ff9576237bda97bd1b469c3d7",
    size: 32_152_673,
};

/// The voice detector: whisper.cpp's file on a Mac, the ONNX one where
/// whisper.cpp's detector is not in the build (Windows), as the hold's
/// Parakeet set has it (`model::PARAKEET_VAD`).
#[cfg(not(all(wake_whisper, not(whisper_stt))))]
pub const VAD: &ModelFile = &model::VAD;
#[cfg(all(wake_whisper, not(whisper_stt)))]
pub const VAD: &ModelFile = &model::SILERO_ONNX;

/// The voice detector (the hold's own file, a copy of its own here so
/// removing one model never breaks the other), then whisper tiny.
pub static FILES: [&ModelFile; 2] = [VAD, &TINY];

/// The mode exists in this build and on this processor: Apple silicon,
/// and Windows x64 with AVX2 (whisper.cpp is built for that floor there).
pub fn available() -> bool {
    cfg!(wake_whisper) && cpu_ok()
}

/// What whisper.cpp's CPU code is built for on Windows x64
/// (.cargo/ggml-x64.cmake): AVX2, with FMA and F16C (MSVC's AVX2 code
/// uses both) and BMI2. Without them the first decode would stop the app
/// on an illegal instruction. Asked once.
#[cfg(all(wake_whisper, target_arch = "x86_64"))]
pub fn cpu_ok() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| {
        let ok = std::is_x86_feature_detected!("avx2")
            && std::is_x86_feature_detected!("fma")
            && std::is_x86_feature_detected!("f16c")
            && std::is_x86_feature_detected!("bmi2");
        if !ok {
            connect_core::logln!("island: wake: this processor has no AVX2: wake by name is not offered");
        }
        ok
    })
}

#[cfg(not(all(wake_whisper, target_arch = "x86_64")))]
pub fn cpu_ok() -> bool {
    true
}

/// How often the microphone's new audio is read while listening.
const TICK: Duration = Duration::from_millis(100);
/// How often the thread looks around while not listening.
const IDLE_TICK: Duration = Duration::from_millis(500);
/// How often the system is asked about the microphone (in use, allowed)
/// and the lock.
const SYSTEM_EVERY: Duration = Duration::from_secs(1);
/// A capture that delivers nothing this long is opened again (the Mac
/// slept, the input device went away).
const STALL: Duration = Duration::from_secs(3);
/// While listening, the recording is cut back once it holds this much.
const TRIM_AT: usize = listen::KEEP + 6 * stt::vad::RATE;
/// Windows refused the microphone (privacy settings, "mic_denied"): the
/// mode waits for it this long before it tries again. Windows asks
/// nothing up front, so trying is the only way to know.
const DENIED_RETRY: Duration = Duration::from_secs(60);

/// The microphone did not open for the listener (any reason but a
/// refusal): how long to wait before try `fails + 1`. Quiet and growing:
/// the owner did nothing, so nothing is shown; a device that comes back
/// is found within half a minute.
fn reopen_backoff(fails: u32) -> Duration {
    match fails {
        0 => Duration::ZERO,
        1 => Duration::from_millis(500),
        2 => Duration::from_secs(1),
        3 => Duration::from_secs(2),
        4 => Duration::from_secs(5),
        5 => Duration::from_secs(10),
        _ => Duration::from_secs(30),
    }
}

/// Which failed tries are logged: the first three, then every tenth (the
/// log keeps the story without a line every half minute for hours).
fn log_open_failure(fails: u32) -> bool {
    fails <= 3 || fails % 10 == 0
}

/// What the Island screen shows of the mode.
#[derive(Debug, Clone, Serialize)]
pub struct WakeDto {
    /// The mode exists here (Apple silicon, Windows x64 with AVX2).
    pub available: bool,
    pub model: model::Status,
    /// The download, in bytes.
    pub size: u64,
    /// The names it answers to ("Mia", "Мия", "Миа", the extra phrase).
    pub names: Vec<String>,
    #[serde(flatten)]
    pub status: Status,
    /// Wakes since the app started, and those that sent nothing.
    pub wakes: u32,
    pub dropped: u32,
}

pub struct Wake {
    pub models: Arc<Models>,
    status: Mutex<Status>,
    /// The hold number of the wake that records now (0: none).
    active: AtomicU64,
    escape: Mutex<Option<Shortcut>>,
    kick: (Mutex<bool>, Condvar),
    wakes: AtomicU32,
    dropped: AtomicU32,
}

impl Wake {
    pub fn new(data_dir: &std::path::Path) -> Wake {
        Wake {
            models: Arc::new(Models::with_files(data_dir.join("wake"), &FILES)),
            status: Mutex::new(Status::Off),
            active: AtomicU64::new(0),
            escape: Mutex::new(None),
            kick: (Mutex::new(false), Condvar::new()),
            wakes: AtomicU32::new(0),
            dropped: AtomicU32::new(0),
        }
    }

    pub fn status(&self) -> Status {
        *self.status.lock().unwrap()
    }

    /// Something changed (the settings, the models): look now.
    pub fn kick(&self) {
        *self.kick.0.lock().unwrap() = true;
        self.kick.1.notify_all();
    }

    fn sleep(&self, d: Duration) {
        let mut kicked = self.kick.0.lock().unwrap();
        if !*kicked {
            kicked = self.kick.1.wait_timeout(kicked, d).unwrap().0;
        }
        *kicked = false;
    }

    /// The thing a sent wake that turned out empty counts as.
    pub fn count_dropped(&self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

/// The names of the main synth, with the owner's extra phrase.
pub fn names(ctx: &Ctx) -> Names {
    let name = ctx.core.island_main().map(|m| m.display_name()).unwrap_or_default();
    Names::new(&name, &ctx.core.island().wake_phrase)
}

/// What a wake's text sends: the request without the name in front. None:
/// only the name was said; nothing goes.
pub fn request(ctx: &Ctx, text: &str) -> Option<String> {
    names(ctx).request(text)
}

pub fn dto(ctx: &Ctx) -> WakeDto {
    let w = &ctx.island.wake_word;
    WakeDto {
        available: available(),
        model: w.models.status(),
        size: w.models.size(),
        names: names(ctx).shown().to_vec(),
        status: w.status(),
        wakes: w.wakes.load(Ordering::Relaxed),
        dropped: w.dropped.load(Ordering::Relaxed),
    }
}

// --- the owner's side: the switch, the phrase, the model ---

fn changed(ctx: &Arc<Ctx>) -> Arc<dyn Fn() + Send + Sync> {
    let ctx = ctx.clone();
    Arc::new(move || {
        ctx.island.wake_word.kick();
        let ctx = ctx.clone();
        tauri::async_runtime::spawn(async move {
            super::emit_view(&ctx);
            crate::state::emit_state(&ctx);
        });
    })
}

/// The switch. On: the model is fetched when it is not here, and the
/// microphone question asked when it was never asked.
pub fn set(ctx: &Arc<Ctx>, on: bool) -> Result<(), String> {
    if on && !available() {
        return Err("unsupported".into());
    }
    ctx.core.set_island(|i| i.wake = on);
    connect_core::logln!("island: wake by name switched {}", if on { "on" } else { "off" });
    if on {
        let _ = download(ctx);
        if audio::permission() == audio::MicPermission::NotAsked {
            let ctx = ctx.clone();
            audio::request_permission(Box::new(move |_| ctx.island.wake_word.kick()));
        }
    }
    ctx.island.wake_word.kick();
    super::emit_view(ctx);
    Ok(())
}

pub fn set_phrase(ctx: &Arc<Ctx>, phrase: &str) {
    ctx.core.set_island(|i| i.wake_phrase = phrase.to_string());
}

pub fn download(ctx: &Arc<Ctx>) -> Result<(), String> {
    if !available() {
        return Err("unsupported".into());
    }
    ctx.island.wake_word.models.download(changed(ctx))
}

/// The files go, and the mode switches off.
pub fn delete(ctx: &Arc<Ctx>) -> Result<(), String> {
    ctx.core.set_island(|i| i.wake = false);
    ctx.island.wake_word.kick();
    let result = ctx.island.wake_word.models.delete();
    super::emit_view(ctx);
    result
}

/// Escape, or a click on the island: the wake that records stops, nothing
/// is sent.
pub fn cancel(ctx: &Arc<Ctx>) {
    let seq = ctx.island.wake_word.active.load(Ordering::Acquire);
    if seq != 0 {
        connect_core::logln!("island: wake: cancelled by the owner");
        end(ctx, seq, false);
        ctx.island.wake_word.kick();
    }
}

/// The global shortcut handler: Escape while a wake records.
pub fn is_escape(ctx: &Ctx, id: u32) -> bool {
    ctx.island.wake_word.escape.lock().unwrap().is_some_and(|s| s.id() == id)
}

fn escape(ctx: &Arc<Ctx>, on: bool) {
    let gs = ctx.app.global_shortcut();
    let mut held = ctx.island.wake_word.escape.lock().unwrap();
    if let Some(old) = held.take() {
        let _ = gs.unregister(old);
    }
    if on {
        if let Ok(s) = Shortcut::from_str("Escape") {
            if gs.register(s).is_ok() {
                *held = Some(s);
            }
        }
    }
}

// --- the hold: the wake is one, sent the way a key's is ---

/// The name was heard: the running recording becomes a hold of the voice
/// key, as if the key had gone down at the start of the speech. Err gives
/// the recording back with the reason (the island is busy, the synth
/// offline).
fn start(ctx: &Arc<Ctx>, recording: audio::Recording) -> Result<u64, (audio::Recording, String)> {
    let synth_id = match super::gate(ctx) {
        Ok(id) => id,
        Err((code, _)) => return Err((recording, code)),
    };
    if ctx.island.inner.lock().unwrap().text_open {
        return Err((recording, "the line of text is open".into()));
    }
    let action = ctx.island.hold.lock().unwrap().down(HotkeyKind::Voice, Instant::now());
    if action != hold::Action::Start(HotkeyKind::Voice) {
        return Err((recording, "a hold runs".into()));
    }
    let seq = {
        let mut inner = ctx.island.inner.lock().unwrap();
        inner.hold_since_ms = super::now_ms();
        inner.transcript = None;
        inner.hold_seq += 1;
        inner.early = None;
        inner.hold_seq
    };
    let route = super::stt_route(ctx);
    if route == stt::Route::Device {
        ctx.island.stt.warm();
    }
    // The tone of voice, as for a hold of the key.
    let toned = super::warm_tone(ctx, route);
    let speech = match route {
        stt::Route::Device => super::start_stt(ctx, &recording),
        stt::Route::Synth(why) => super::Speech::Synth(why),
    };
    let tone = super::start_tone(ctx, toned, &recording, &speech);
    // The listening stream may have run for hours: what it reported
    // before the name is logged here, not counted against the request.
    let before = recording.take_errors();
    if before.count > 0 {
        connect_core::logln!("island: wake: the listening stream had {} before the name", before.describe());
    }
    {
        let mut running = ctx.island.running.lock().unwrap();
        let mut inner = ctx.island.inner.lock().unwrap();
        if matches!(speech, super::Speech::Device(_)) {
            inner.transcript = Some(String::new());
        }
        *running = Some(super::Hold { recording, picture: None, synth_id, stt: speech, wake: true, tone });
    }
    ctx.island.wake_word.active.store(seq, Ordering::Release);
    super::emit_view(ctx);
    Ok(seq)
}

/// This wake's hold still runs (not cancelled, not sent).
fn running(ctx: &Ctx, seq: u64) -> bool {
    ctx.island.hold.lock().unwrap().holding().is_some() && ctx.island.inner.lock().unwrap().hold_seq == seq
}

/// End this wake's hold: `send` releases it (the hold's send path),
/// otherwise it is dropped unsent.
fn end(ctx: &Arc<Ctx>, seq: u64, send: bool) {
    if ctx.island.wake_word.active.compare_exchange(seq, 0, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return;
    }
    escape(ctx, false);
    if !running(ctx, seq) {
        return;
    }
    let action = {
        let mut h = ctx.island.hold.lock().unwrap();
        if send {
            h.up(HotkeyKind::Voice, Instant::now())
        } else {
            h.cancel()
        }
    };
    super::act(ctx, action);
}

// --- the listening thread ---

/// The microphone, as the listener holds it.
struct Capture {
    /// None once it became a wake's hold.
    recording: Option<audio::Recording>,
    /// The device it records from (None: the default input).
    uid: Option<String>,
    tap: audio::Tap,
    /// The listener's index of the tap's first sample.
    off: usize,
    last_data: Instant,
}

pub fn spawn(ctx: Arc<Ctx>) {
    let spawned = std::thread::Builder::new().name("island-wake".into()).spawn(move || {
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&ctx)));
        if let Err(p) = ran {
            let what = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
            connect_core::logln!("island: wake: the listening thread stopped on a panic: {what}");
            *ctx.island.wake_word.status.lock().unwrap() = Status::Off;
        }
    });
    if let Err(e) = spawned {
        connect_core::logln!("island: wake: the listening thread did not start: {e}");
    }
}

#[cfg(wake_whisper)]
fn load_ears(models: &Models) -> Result<Box<dyn Ears>, String> {
    ears::WhisperEars::load(&models.path(&TINY), &models.path(VAD)).map(|e| Box::new(e) as Box<dyn Ears>)
}

#[cfg(not(wake_whisper))]
fn load_ears(_models: &Models) -> Result<Box<dyn Ears>, String> {
    Err("not on this system".into())
}

fn level_to_island(ctx: &Arc<Ctx>) -> Arc<dyn Fn(f32) + Send + Sync> {
    let ctx = ctx.clone();
    // The face moves with the voice only during a wake.
    Arc::new(move |level: f32| {
        if ctx.island.wake_word.active.load(Ordering::Relaxed) != 0 {
            if let Some(w) = super::window(&ctx) {
                let _ = w.emit_to(super::WINDOW, super::LEVEL_EVENT, level);
            }
        }
    })
}

fn to_f32(pcm: &[i16]) -> Vec<f32> {
    pcm.iter().map(|s| f32::from(*s) / 32768.0).collect()
}

fn run(ctx: &Arc<Ctx>) {
    let wake = &ctx.island.wake_word;
    let mut ears: Option<Box<dyn Ears>> = None;
    let mut ears_failed = false;
    let mut capture: Option<Capture> = None;
    let mut listener = Listener::new();
    let mut names = Names::default();
    let mut system = (false, false);
    let mut mic_granted = false;
    let mut mic = device::Mic::Default;
    let mut system_at: Option<Instant> = None;
    let mut refreshed;
    let mut busy_with: Vec<String> = Vec::new();
    let mut woke_at: Option<Instant> = None;
    let mut denied_at: Option<Instant> = None;
    // Why the microphone is opened next (for the log), and the failed
    // tries since it last opened (`reopen_backoff`).
    let mut open_why: &'static str = "first open";
    let mut open_fails: u32 = 0;
    let mut open_retry_at: Option<Instant> = None;
    loop {
        let settings = ctx.core.island();
        let on = settings.wake && available();
        // A wake that is no longer running: cancelled, or the hold path
        // took it (sent, or the island switched off).
        let seq = wake.active.load(Ordering::Acquire);
        if seq != 0 && !running(ctx, seq) {
            wake.active.store(0, Ordering::Release);
            escape(ctx, false);
        }
        if listener.woke() && wake.active.load(Ordering::Acquire) == 0 {
            listener.cooldown();
            capture = None;
            open_why = "reopen after a wake";
        }
        let woke = wake.active.load(Ordering::Acquire) != 0;
        refreshed = false;
        if on && system_at.is_none_or(|t| t.elapsed() >= SYSTEM_EVERY) {
            let (busy, who) = system::mic_busy();
            if busy && who != busy_with {
                connect_core::logln!("island: wake: paused, the microphone is in use by {}", who.join(", "));
            } else if !busy && !busy_with.is_empty() {
                connect_core::logln!("island: wake: the microphone is free again");
            }
            busy_with = if busy { who } else { Vec::new() };
            system = (busy, system::locked());
            mic_granted = audio::permission() == audio::MicPermission::Granted && denied_at.is_none_or(|t| t.elapsed() >= DENIED_RETRY);
            // The default input changed, a device came or went.
            let chosen = device::choose(&system::input_devices());
            if chosen != mic {
                match &chosen {
                    device::Mic::Use { name, instead_of: Some(bt), .. } => {
                        connect_core::logln!("island: wake: the default input is Bluetooth ({bt}); listening on {name}")
                    }
                    device::Mic::Use { name, uid, .. } => {
                        let transport = system::input_devices().into_iter().find(|d| &d.uid == uid).map(|d| device::transport::name(d.transport)).unwrap_or_default();
                        connect_core::logln!("island: wake: listening on {name} ({transport})")
                    }
                    device::Mic::BluetoothOnly { name } => {
                        connect_core::logln!("island: wake: paused, the only microphone is Bluetooth ({name})")
                    }
                    device::Mic::Default => {}
                }
                mic = chosen;
            }
            system_at = Some(Instant::now());
            refreshed = true;
        }
        let main = ctx.core.island_main();
        let inputs = Inputs {
            on,
            available: available(),
            island: settings.enabled && ctx.core.is_paired() && main.is_some(),
            synth_up: main.as_ref().is_some_and(|m| m.state() == LinkState::Linked && core_island::supports(m)),
            mic_granted,
            model_ready: wake.models.ready(),
            ears_loaded: ears.is_some(),
            app_paused: ctx.core.link_brief().paused,
            holding: !woke && ctx.island.hold.lock().unwrap().holding().is_some(),
            locked: system.1,
            mic_busy: system.0,
            bt_only: matches!(mic, device::Mic::BluetoothOnly { .. }),
            woke,
        };
        // Whisper tiny is in memory only while the mode is on.
        if Status::wants_ears(&inputs) {
            if ears.is_none() && !ears_failed {
                match load_ears(&wake.models) {
                    Ok(e) => ears = Some(e),
                    Err(e) => {
                        connect_core::logln!("island: wake: whisper tiny did not load: {e}");
                        ears_failed = true;
                    }
                }
            }
        } else if ears.is_some() || ears_failed {
            ears = None;
            ears_failed = false;
            connect_core::logln!("island: wake: whisper tiny unloaded");
        }
        let status = pause::status(&Inputs { ears_loaded: ears.is_some(), ..inputs });
        let before = std::mem::replace(&mut *wake.status.lock().unwrap(), status);
        if before != status {
            if !matches!((before, status), (Status::Listening, Status::Woke) | (Status::Woke, Status::Listening)) {
                connect_core::logln!("island: wake: {}", serde_json::to_string(&status).unwrap_or_default());
            }
            let ctx2 = ctx.clone();
            tauri::async_runtime::spawn(async move {
                super::emit_view(&ctx2);
                crate::state::emit_state(&ctx2);
            });
        }

        if !status.listens() {
            if capture.take().is_some() {
                listener.reset();
                open_why = "reopen after a pause";
            }
            open_fails = 0;
            open_retry_at = None;
            woke_at = None;
            wake.sleep(IDLE_TICK);
            continue;
        }
        // The synth's name and the phrase, looked at once a second.
        if names.is_empty() || refreshed {
            names = self::names(ctx);
        }

        // The microphone: open, or opened again after a wake or a stall.
        if capture.as_ref().is_some_and(|c| c.recording.is_some() && c.last_data.elapsed() > STALL) {
            connect_core::logln!("island: wake: the microphone went quiet; opening it again");
            capture = None;
            open_why = "reopen after a stall";
        }
        // Another microphone was chosen: open that one (never during a
        // wake, whose recording is the hold's now).
        if capture.as_ref().is_some_and(|c| c.recording.is_some() && c.uid.as_deref() != mic.uid()) {
            capture = None;
            listener.reset();
            open_why = "another microphone";
        }
        if capture.is_none() {
            // A failed open waits its turn, quietly.
            if open_retry_at.is_some_and(|t| Instant::now() < t) {
                wake.sleep(IDLE_TICK);
                continue;
            }
            let uid = mic.uid().map(String::from);
            match audio::start_on(uid.clone(), level_to_island(ctx)) {
                Ok(recording) => {
                    let tap = recording.tap();
                    capture = Some(Capture { recording: Some(recording), uid, tap, off: listener.now(), last_data: Instant::now() });
                    denied_at = None;
                    if open_fails > 0 {
                        connect_core::logln!("island: wake: the microphone opened ({open_why}) after {open_fails} failed tries");
                    }
                    open_fails = 0;
                    open_retry_at = None;
                }
                Err(e) if e.starts_with("mic_denied") => {
                    // Windows: the microphone is off for apps. Waiting
                    // for it (Settings says so), tried again in a minute.
                    if denied_at.is_none() {
                        connect_core::logln!("island: wake: Windows refused the microphone (privacy settings): {e}");
                    }
                    denied_at = Some(Instant::now());
                    mic_granted = false;
                    system_at = None;
                    wake.sleep(IDLE_TICK);
                    continue;
                }
                Err(e) => {
                    // Never a notice: the owner did nothing. Tried again
                    // later and later (`reopen_backoff`), logged with the
                    // path so a report can be read.
                    open_fails = open_fails.saturating_add(1);
                    let wait = reopen_backoff(open_fails);
                    if log_open_failure(open_fails) {
                        connect_core::logln!(
                            "island: wake: the microphone did not open ({open_why}, try {open_fails}, again in {:.1} s): {e}",
                            wait.as_secs_f32()
                        );
                    }
                    open_retry_at = Some(Instant::now() + wait);
                    wake.sleep(IDLE_TICK);
                    continue;
                }
            }
        }
        let Some(cap) = capture.as_mut() else { continue };
        let new = cap.tap.read_from(listener.now() - cap.off);
        if !new.is_empty() {
            cap.last_data = Instant::now();
        }
        let event = match ears.as_deref_mut() {
            Some(e) => listener.push(&to_f32(&new), e, &names),
            None => None,
        };
        // Keep only the last seconds while listening.
        if cap.recording.is_some() && listener.now() - cap.off > TRIM_AT {
            let n = listener.keep_from().saturating_sub(cap.off);
            cap.tap.drop_front(n);
            cap.off += n;
        }
        match event {
            Some(Event::Wake(w)) => {
                let Some(recording) = cap.recording.take() else { continue };
                // The recording from the start of the speech on.
                let n = w.from.saturating_sub(cap.off);
                cap.tap.drop_front(n);
                cap.off += n;
                match start(ctx, recording) {
                    Ok(_) => {
                        wake.wakes.fetch_add(1, Ordering::Relaxed);
                        woke_at = Some(Instant::now());
                        escape(ctx, true);
                        connect_core::logln!(
                            "island: wake: heard \"{}\" ({:.2}), recording{}",
                            w.hit.heard,
                            w.hit.score,
                            if w.name_only { ", waiting for the request" } else { "" }
                        );
                    }
                    Err((recording, why)) => {
                        cap.recording = Some(recording);
                        listener.cooldown();
                        connect_core::logln!("island: wake: heard \"{}\" ({:.2}) but did not wake: {why}", w.hit.heard, w.hit.score);
                    }
                }
            }
            Some(Event::Send(why)) => {
                let seq = wake.active.load(Ordering::Acquire);
                let secs = woke_at.take().map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
                connect_core::logln!("island: wake: sent after {why} ({secs:.1} s after the name)");
                end(ctx, seq, true);
                capture = None;
                open_why = "reopen after a wake";
            }
            Some(Event::Drop(why)) => {
                let seq = wake.active.load(Ordering::Acquire);
                woke_at = None;
                wake.count_dropped();
                connect_core::logln!("island: wake: sent nothing: {why}");
                end(ctx, seq, false);
                capture = None;
                open_why = "reopen after a wake";
            }
            None => {}
        }
        wake.sleep(TICK);
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod reopen_tests {
    use super::*;

    #[test]
    fn a_microphone_that_does_not_open_is_tried_again_later_and_later_and_quietly() {
        assert_eq!(reopen_backoff(0), Duration::ZERO);
        let waits: Vec<Duration> = (1..=8).map(reopen_backoff).collect();
        assert!(waits.windows(2).all(|w| w[0] <= w[1]), "never sooner: {waits:?}");
        assert!(waits[0] <= Duration::from_secs(1), "the first retry is quick (a device busy for a moment)");
        assert_eq!(reopen_backoff(1000), Duration::from_secs(30), "capped: a device that comes back is found");
        let logged: Vec<u32> = (1..=40).filter(|n| log_open_failure(*n)).collect();
        assert_eq!(logged, vec![1, 2, 3, 10, 20, 30, 40]);
    }
}
