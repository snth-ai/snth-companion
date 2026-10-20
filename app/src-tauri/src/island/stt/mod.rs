//! On-device speech-to-text for the hold keys (tz-local-stt.md).
//!
//! Off by default: the voice goes to the synth as audio, and the synth's
//! speech-to-text hears the tone of voice too. With it on (the Island
//! screen, or the synth's one-time offer on the island) the app recognizes
//! the speech itself while the key is held and sends the text
//! (owner.message `voice_transcript`, `stt` "device", no audio). That
//! saves the synth's speech-to-text call (about 5 s per message), keeps
//! the voice on this computer and works offline.
//!
//! - `model.rs`: the model files (download, check, removal).
//! - `engine.rs`: whisper.cpp with Metal. Apple silicon only; elsewhere
//!   the switch is shown as unavailable and the voice goes as audio.
//! - `stream.rs`: one hold, recognized piece by piece while it runs.
//! - `vad.rs`, `filter.rs`: where the speech is, what whisper invents.
//! - `release.rs`: what a released hold sends (a hold is never lost).
//!
//! One thread does all of the speech work (`island-stt`). The model is
//! loaded when a hold starts (or when the owner turns the switch on) and
//! unloaded after five minutes without a hold. A cold load takes 3 to 7
//! s, so the first hold after a start or a download usually ends before
//! the model is there. The release waits at most `RELEASE_WAIT` for the
//! text; whatever goes wrong (the model still loading, a load or decode
//! error, the thread gone), the audio still in memory goes to the synth
//! instead, as it did before. Nothing is sent only when the voice detector
//! went over the whole recording and heard no speech (`release.rs`). A
//! speech thread that ended on a panic is started again by the next hold,
//! and that hold goes as audio.

// Without the engine (Windows, Linux, Intel Macs) only the model files
// and the settings are used; the stream is there for its tests.
#![cfg_attr(not(all(target_os = "macos", target_arch = "aarch64")), allow(dead_code))]

pub mod filter;
pub mod model;
pub mod release;
pub mod stream;
pub mod vad;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod engine;

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use connect_core::island::{IslandSettings, SttMode};

pub use release::{decide, AtRelease, EngineState, Release};
pub use stream::{Decoder, Heard, Vad};

/// The engine exists in this build (Apple silicon).
pub fn available() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

/// How long the model stays loaded after the last hold.
pub const IDLE_UNLOAD: Duration = Duration::from_secs(5 * 60);
/// How often a running hold is looked at (a look decodes only when there
/// is something new: stream.rs).
const STEP_EVERY: Duration = Duration::from_millis(150);
/// How long the release waits for the text before the audio goes instead.
/// A loaded engine answers well inside it (the last piece is usually the
/// preview already, else under a second of decoding, plus a step that
/// was running at the release); a cold load (3 to 7 s) does not fit, and
/// that hold goes as audio rather than keep the owner waiting.
pub const RELEASE_WAIT: Duration = Duration::from_millis(2500);

/// Who recognizes this hold's speech.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Device,
    /// The synth, and why (for the log line).
    Synth(&'static str),
}

/// The text is sent only when every part of the device path is there;
/// anything missing sends the audio, as before.
pub fn route(mode: SttMode, available: bool, model_ready: bool, synth_takes_text: bool) -> Route {
    match mode {
        SttMode::Synth => Route::Synth("speech-to-text is off"),
        SttMode::Device if !available => Route::Synth("speech-to-text is not on this system"),
        SttMode::Device if !model_ready => Route::Synth("the model is not downloaded yet"),
        SttMode::Device if !synth_takes_text => Route::Synth("the synth does not take a transcript yet"),
        SttMode::Device => Route::Device,
    }
}

/// The synth's offer is due on the island now: it can be used here, the
/// owner has not answered, the speech still goes to the synth, the main
/// synth takes text, and the island is idle (no hold, no answer on its
/// way, nothing else on it).
pub fn offer_due(s: &IslandSettings, available: bool, synth_takes_text: bool, idle: bool) -> bool {
    available && s.enabled && s.stt == SttMode::Synth && s.stt_offer.open() && synth_takes_text && idle
}

/// New samples of a running recording, from this index on.
pub type Source = Box<dyn FnMut(usize) -> Vec<i16> + Send>;
/// The live text, as it grows.
pub type Live = Box<dyn Fn(String) + Send>;
/// Loads the engine (the model, the voice detector) on the speech thread.
pub type Loader = Arc<dyn Fn(&Path, &Path) -> Result<Box<dyn Decoder>, String> + Send + Sync>;
/// Called when the engine's state changes (the island shows "loading").
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// What the speech thread and the island share.
#[derive(Default)]
struct Shared {
    state: AtomicU8,
    notify: Mutex<Option<Notify>>,
}

impl Shared {
    fn get(&self) -> EngineState {
        EngineState::from_u8(self.state.load(Ordering::Acquire))
    }

    fn set(&self, s: EngineState) {
        if self.state.swap(s as u8, Ordering::AcqRel) != s as u8 {
            let notify = self.notify.lock().unwrap().clone();
            if let Some(f) = notify {
                f();
            }
        }
    }
}

struct Job {
    source: Source,
    live: Live,
    end: mpsc::Receiver<End>,
    /// Stop a step that is decoding (a slip, a cancel, a release that
    /// gave up waiting).
    finishing: Arc<AtomicBool>,
    /// The release gave up waiting: the answer is not wanted any more.
    abandoned: Arc<AtomicBool>,
}

struct End {
    /// The whole recording.
    pcm: Vec<i16>,
    reply: mpsc::SyncSender<Result<Heard, String>>,
}

enum Cmd {
    /// Load the model now (a hold is starting, the switch went on).
    Warm,
    Hold(Job),
}

/// What the release got from the speech thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Heard(Heard),
    /// Nothing within `RELEASE_WAIT`.
    TimedOut,
    /// The speech thread answered with an error, or is gone.
    Failed(String),
}

/// The speech side of a running hold. `finish` ends it; dropping it
/// throws it away (a slip, a cancel).
pub struct Session {
    end: Option<mpsc::Sender<End>>,
    finishing: Arc<AtomicBool>,
    abandoned: Arc<AtomicBool>,
}

impl Session {
    /// The key was let go: what the speech thread made of the whole
    /// recording. Blocks for up to `RELEASE_WAIT` (call it off the async
    /// runtime's threads).
    ///
    /// A step that is decoding right now runs to its end: its result is
    /// usually the last piece (whisper checks for an abort only after the
    /// encoder, which is most of the time anyway).
    pub fn finish(mut self, pcm: Vec<i16>) -> Answer {
        let Some(end) = self.end.take() else {
            return Answer::Failed("finished already".into());
        };
        let (reply, answer) = mpsc::sync_channel(1);
        if end.send(End { pcm, reply }).is_err() {
            return Answer::Failed("the speech thread is gone".into());
        }
        match answer.recv_timeout(RELEASE_WAIT) {
            Ok(Ok(heard)) => Answer::Heard(heard),
            Ok(Err(e)) => Answer::Failed(e),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Do not spend the thread on an answer nobody waits for.
                self.abandoned.store(true, Ordering::Release);
                self.finishing.store(true, Ordering::Release);
                Answer::TimedOut
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Answer::Failed("the speech thread is gone".into()),
        }
    }
}

impl Drop for Session {
    /// A slip or a cancel: a step decoding right now stops early.
    fn drop(&mut self) {
        if self.end.is_some() {
            self.finishing.store(true, Ordering::Release);
            self.abandoned.store(true, Ordering::Release);
        }
    }
}

pub struct Stt {
    pub models: Arc<model::Models>,
    tx: Mutex<Option<mpsc::Sender<Cmd>>>,
    shared: Arc<Shared>,
    /// None where the engine does not exist.
    loader: Option<Loader>,
    /// The thread was found gone and started again: the next hold goes as
    /// audio.
    revived: AtomicBool,
}

impl Stt {
    pub fn new(data_dir: &Path) -> Stt {
        Stt::with_loader(Arc::new(model::Models::new(data_dir.join("stt"))), engine_loader())
    }

    fn with_loader(models: Arc<model::Models>, loader: Option<Loader>) -> Stt {
        Stt { models, tx: Mutex::new(None), shared: Arc::new(Shared::default()), loader, revived: AtomicBool::new(false) }
    }

    /// Where the engine is now.
    pub fn engine(&self) -> EngineState {
        self.shared.get()
    }

    /// Call `f` whenever the engine's state changes (on the speech thread:
    /// keep it short).
    pub fn on_change(&self, f: Notify) {
        *self.shared.notify.lock().unwrap() = Some(f);
    }

    /// Hand the speech thread a command, starting the thread when there is
    /// none. A thread found gone (it ended on a panic) is started again
    /// and warmed for the next hold; this command is lost and the caller
    /// told.
    fn send(&self, cmd: Cmd) -> Result<(), &'static str> {
        let mut tx = self.tx.lock().unwrap();
        if let Some(t) = tx.as_ref() {
            if t.send(cmd).is_ok() {
                return Ok(());
            }
            connect_core::logln!("island: speech-to-text: the speech thread was gone; starting it again");
            self.revived.store(true, Ordering::Release);
            *tx = self.spawn();
            if let Some(t) = tx.as_ref() {
                let _ = t.send(Cmd::Warm);
            }
            return Err("the speech thread was gone (started again)");
        }
        *tx = self.spawn();
        match tx.as_ref() {
            Some(t) => t.send(cmd).map_err(|_| "the speech thread did not start"),
            None => Err("the speech thread did not start"),
        }
    }

    fn spawn(&self) -> Option<mpsc::Sender<Cmd>> {
        let loader = self.loader.clone()?;
        let (tx, rx) = mpsc::channel();
        let models = self.models.clone();
        let shared = self.shared.clone();
        shared.set(EngineState::Unloaded);
        let spawned = std::thread::Builder::new().name("island-stt".into()).spawn(move || run(rx, models, loader, shared));
        match spawned {
            Ok(_) => Some(tx),
            Err(e) => {
                connect_core::logln!("island: speech-to-text: the speech thread did not start: {e}");
                None
            }
        }
    }

    /// Load the model ahead of a hold (nothing when it is loaded, or
    /// cannot be).
    pub fn warm(&self) {
        if self.loader.is_some() && self.models.ready() {
            let _ = self.send(Cmd::Warm);
        }
    }

    /// Start recognizing a hold that just started recording. Err(why)
    /// when this hold goes as audio.
    pub fn hold(&self, source: Source, live: Live) -> Result<Session, &'static str> {
        if self.loader.is_none() {
            return Err("speech-to-text is not on this system");
        }
        if !self.models.ready() {
            return Err("the model is not downloaded yet");
        }
        // The thread had died since the last hold and the warm-up at the
        // press started it again: this hold goes as audio.
        if self.revived.swap(false, Ordering::AcqRel) {
            return Err("the speech thread was gone (started again)");
        }
        let (end_tx, end_rx) = mpsc::channel();
        let finishing = Arc::new(AtomicBool::new(false));
        let abandoned = Arc::new(AtomicBool::new(false));
        let job = Job { source, live, end: end_rx, finishing: finishing.clone(), abandoned: abandoned.clone() };
        if let Err(e) = self.send(Cmd::Hold(job)) {
            // Found gone right now: this hold is the one that goes as audio.
            self.revived.store(false, Ordering::Release);
            return Err(e);
        }
        Ok(Session { end: Some(end_tx), finishing, abandoned })
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn engine_loader() -> Option<Loader> {
    Some(Arc::new(|model: &Path, vad: &Path| engine::Engine::load(model, vad).map(|e| Box::new(e) as Box<dyn Decoder>)))
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn engine_loader() -> Option<Loader> {
    None
}

fn to_f32(pcm: &[i16]) -> Vec<f32> {
    pcm.iter().map(|s| f32::from(*s) / 32768.0).collect()
}

/// The speech thread. A panic ends it (the next hold starts another);
/// the engine state says "gone" and the log says why.
fn run(rx: mpsc::Receiver<Cmd>, models: Arc<model::Models>, loader: Loader, shared: Arc<Shared>) {
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| serve(rx, &models, &loader, &shared)));
    if let Err(p) = ran {
        let what = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
        connect_core::logln!("island: speech-to-text: the speech thread stopped on a panic: {what}");
    }
    shared.set(EngineState::Gone);
}

/// Load on demand, run holds, unload when idle.
fn serve(rx: mpsc::Receiver<Cmd>, models: &model::Models, loader: &Loader, shared: &Shared) {
    let mut engine: Option<Box<dyn Decoder>> = None;
    let mut last_use = Instant::now();
    loop {
        let cmd = if engine.is_some() {
            match rx.recv_timeout(IDLE_UNLOAD.saturating_sub(last_use.elapsed())) {
                Ok(c) => c,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    engine = None;
                    shared.set(EngineState::Unloaded);
                    connect_core::logln!("island: speech-to-text unloaded after {} min idle", IDLE_UNLOAD.as_secs() / 60);
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match rx.recv() {
                Ok(c) => c,
                Err(_) => return,
            }
        };
        // The files may have been deleted since.
        if !models.ready() {
            engine = None;
            shared.set(EngineState::Unloaded);
        } else if engine.is_none() {
            shared.set(EngineState::Loading);
            let (model, vad) = models.paths();
            match loader(&model, &vad) {
                Ok(e) => {
                    engine = Some(e);
                    shared.set(EngineState::Ready);
                }
                Err(e) => {
                    connect_core::logln!("island: speech-to-text did not load: {e}");
                    shared.set(EngineState::Failed);
                }
            }
        }
        if let Cmd::Hold(job) = cmd {
            match engine.as_mut() {
                Some(e) => run_hold(e.as_mut(), job),
                None => refuse(job, shared.get()),
            }
        }
        last_use = Instant::now();
    }
}

/// No engine: answer the release at once, so the audio goes.
fn refuse(job: Job, state: EngineState) {
    if let Ok(end) = job.end.recv() {
        let _ = end.reply.send(Err(state.reason().into()));
    }
}

/// One hold: a step about once a second until the key is let go.
fn run_hold(engine: &mut dyn Decoder, mut job: Job) {
    let mut s = stream::Streamer::new();
    let mut broken: Option<String> = None;
    loop {
        // The release gave up while this hold waited (a cold load).
        if job.abandoned.load(Ordering::Acquire) {
            return;
        }
        let started = Instant::now();
        if broken.is_none() {
            let new = (job.source)(s.len());
            s.push(&to_f32(&new));
            match s.step(engine, &job.finishing) {
                Ok(true) => (job.live)(s.live()),
                Ok(false) => {}
                // The release is here: finish below.
                Err(e) if e == stream::ABORTED => {}
                Err(e) => {
                    connect_core::logln!("island: speech-to-text step failed: {e}");
                    broken = Some(e);
                }
            }
        }
        match job.end.recv_timeout(STEP_EVERY.saturating_sub(started.elapsed())) {
            Ok(end) => {
                if job.abandoned.load(Ordering::Acquire) {
                    return;
                }
                let result = match broken {
                    Some(e) => Err(e),
                    None => {
                        let t = Instant::now();
                        if end.pcm.len() > s.len() {
                            s.push(&to_f32(&end.pcm[s.len()..]));
                        }
                        let tail = s.pending();
                        let r = s.finish(engine);
                        connect_core::logln!(
                            "island: speech-to-text final piece of {:.1} s in {} ms{} ({:.1} s in all, language {})",
                            tail as f32 / vad::RATE as f32,
                            t.elapsed().as_millis(),
                            if s.reused { ", the preview" } else { "" },
                            s.len() as f32 / vad::RATE as f32,
                            s.lang().unwrap_or("?")
                        );
                        r
                    }
                };
                let _ = end.reply.send(result);
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // A slip or a cancel: nothing to finish.
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use connect_core::island::SttOffer;

    #[test]
    fn the_text_goes_only_when_everything_is_there() {
        assert_eq!(route(SttMode::Device, true, true, true), Route::Device);
        assert_eq!(route(SttMode::Synth, true, true, true), Route::Synth("speech-to-text is off"));
        assert!(matches!(route(SttMode::Device, false, true, true), Route::Synth(_)));
        assert!(matches!(route(SttMode::Device, true, false, true), Route::Synth(_)));
        // The synth lacks the feature: the audio goes, even with the switch on.
        assert_eq!(route(SttMode::Device, true, true, false), Route::Synth("the synth does not take a transcript yet"));
    }

    #[test]
    fn the_offer_waits_for_an_idle_island_and_a_synth_that_takes_text() {
        let on = IslandSettings { enabled: true, ..IslandSettings::default() };
        assert!(offer_due(&on, true, true, true));
        assert!(!offer_due(&on, true, true, false), "not mid-hold or mid-answer");
        assert!(!offer_due(&on, true, false, true), "the synth must take text first");
        assert!(!offer_due(&on, false, true, true), "not where the engine is missing");
        let off = IslandSettings { enabled: false, ..on.clone() };
        assert!(!offer_due(&off, true, true, true), "the island must be on");
        let shown = IslandSettings { stt_offer: SttOffer::Shown, ..on.clone() };
        assert!(offer_due(&shown, true, true, true), "shown but not answered: again after a restart");
        for answered in [SttOffer::Accepted, SttOffer::Declined] {
            let s = IslandSettings { stt_offer: answered, ..on.clone() };
            assert!(!offer_due(&s, true, true, true), "{answered:?}");
        }
        let device = IslandSettings { stt: SttMode::Device, ..on.clone() };
        assert!(!offer_due(&device, true, true, true), "already on");
    }

    #[test]
    fn a_session_dropped_mid_hold_stops_its_step() {
        let (end, _rx) = mpsc::channel();
        let finishing = Arc::new(AtomicBool::new(false));
        let abandoned = Arc::new(AtomicBool::new(false));
        drop(Session { end: Some(end), finishing: finishing.clone(), abandoned: abandoned.clone() });
        assert!(finishing.load(Ordering::Acquire));
        assert!(abandoned.load(Ordering::Acquire));
    }

    #[test]
    fn samples_become_floats() {
        assert_eq!(to_f32(&[0, 16384, -32768]), vec![0.0, 0.5, -1.0]);
    }

    // --- the hold flow on the speech thread, with a stand-in engine ---

    /// A stand-in for whisper: every loud stretch is the word "слово",
    /// the voice detector is the loudness detector (or missing).
    struct FakeEngine {
        no_vad: bool,
        panic_on_decode: bool,
    }

    impl Decoder for FakeEngine {
        fn decode(&mut self, audio: &[f32], _: Option<&str>, _: Option<&AtomicBool>) -> Result<stream::Decoded, String> {
            if self.panic_on_decode {
                panic!("a bug in the decoder");
            }
            let words: Vec<&str> = vad::energy_spans(audio).iter().map(|_| "слово").collect();
            Ok(stream::Decoded { segments: vec![words.join(" ")], lang: Some(("ru".into(), 1.0)) })
        }

        fn vad(&mut self, audio: &[f32]) -> Option<Vec<vad::Span>> {
            (!self.no_vad).then(|| vad::energy_spans(audio))
        }
    }

    #[derive(Clone, Copy, Default)]
    struct Fake {
        /// A cold load takes this long.
        load: Duration,
        fail_load: bool,
        no_vad: bool,
        /// The decoder panics on the first decode of the first engine.
        panic_once: bool,
    }

    fn fake_loader(f: Fake) -> Loader {
        let loads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        Arc::new(move |_: &Path, _: &Path| {
            std::thread::sleep(f.load);
            if f.fail_load {
                return Err("load: the model file is broken".into());
            }
            let first = loads.fetch_add(1, Ordering::AcqRel) == 0;
            Ok(Box::new(FakeEngine { no_vad: f.no_vad, panic_on_decode: f.panic_once && first }) as Box<dyn Decoder>)
        })
    }

    /// An app that starts with the model not downloaded yet.
    fn fresh_app(f: Fake) -> Stt {
        let dir = std::env::temp_dir().join(format!("snth-stt-test-{}-{}", std::process::id(), rand_suffix()));
        let models = Arc::new(model::Models::new(dir));
        assert!(!models.ready());
        Stt::with_loader(models, Some(fake_loader(f)))
    }

    fn rand_suffix() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0)
    }

    fn pcm(secs: f32, speech: &[(f32, f32)]) -> Vec<i16> {
        vad::tests::clip(secs, speech).iter().map(|s| (s * 32767.0) as i16).collect()
    }

    /// One hold as the island runs it: the warm-up at the press, the
    /// session when the microphone is on, the audio arriving in real time
    /// through a tap that goes empty when the recording ends, the release
    /// after the whole clip, then `release::decide`. Returns what is sent
    /// and how long after the release it was known.
    fn hold(stt: &Stt, audio: Vec<i16>, picture: bool) -> (Release, Duration) {
        stt.warm();
        let audio = Arc::new(audio);
        let src = audio.clone();
        let start = Instant::now();
        let ended = Arc::new(AtomicBool::new(false));
        let tap_ended = ended.clone();
        let session = stt.hold(
            Box::new(move |from| {
                if tap_ended.load(Ordering::Acquire) {
                    return Vec::new();
                }
                let have = ((start.elapsed().as_secs_f32() * vad::RATE as f32) as usize).min(src.len());
                src.get(from..have).map(<[i16]>::to_vec).unwrap_or_default()
            }),
            Box::new(|_| {}),
        );
        std::thread::sleep(Duration::from_secs_f32(audio.len() as f32 / vad::RATE as f32));
        ended.store(true, Ordering::Release);
        let released = Instant::now();
        let release = match session {
            Err(why) => Release::Audio(why.into()),
            Ok(session) => {
                let answer = session.finish(audio.to_vec());
                let (heard, timed_out, error) = match &answer {
                    Answer::Heard(h) => (Some(h), false, None),
                    Answer::TimedOut => (None, true, None),
                    Answer::Failed(e) => (None, false, Some(e.as_str())),
                };
                decide(&AtRelease {
                    engine: stt.engine(),
                    vad: heard.map(|h| h.vad).unwrap_or(Vad::NotRun),
                    text: heard.map(|h| h.text.as_str()),
                    timed_out,
                    error,
                    picture,
                })
            }
        };
        (release, released.elapsed())
    }

    fn wait_for(stt: &Stt, state: EngineState) {
        let t = Instant::now();
        while stt.engine() != state {
            assert!(t.elapsed() < Duration::from_secs(20), "the engine never got {state:?}: {:?}", stt.engine());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Sasha's field report (0.1.19): the offer accepted, the model
    /// downloaded, then the first hold. The model is still loading at the
    /// release: that hold goes as audio within `RELEASE_WAIT`, never as
    /// nothing; the next hold goes as text.
    #[test]
    fn the_first_hold_after_the_download_is_never_lost() {
        // A cold load longer than two holds and their waits.
        let stt = fresh_app(Fake { load: Duration::from_millis(6500), ..Fake::default() });
        // Before the download: the audio goes.
        let (r, _) = hold(&stt, pcm(1.0, &[(0.2, 0.8)]), false);
        assert_eq!(r, Release::Audio("the model is not downloaded yet".into()));
        stt.models.pretend(model::Status::Ready);

        // The first hold after it: the model is still loading at the
        // release. The audio goes within RELEASE_WAIT.
        let (r, after) = hold(&stt, pcm(0.4, &[(0.05, 0.35)]), false);
        assert_eq!(r, Release::Audio("the model was still loading".into()));
        assert!(after < RELEASE_WAIT + Duration::from_millis(300), "{after:?}");

        // A silent hold while loading is not "silence": the voice detector
        // has not run, so the audio goes too.
        let (r, _) = hold(&stt, pcm(0.4, &[]), false);
        assert_eq!(r, Release::Audio("the model was still loading".into()));

        wait_for(&stt, EngineState::Ready);
        let (r, after) = hold(&stt, pcm(1.5, &[(0.2, 1.2)]), false);
        assert_eq!(r, Release::Text("слово".into()));
        assert!(after < Duration::from_secs(1), "{after:?}");
    }

    #[test]
    fn only_a_ready_engines_voice_detector_drops_a_silent_hold() {
        let stt = fresh_app(Fake::default());
        stt.models.pretend(model::Status::Ready);
        stt.warm();
        wait_for(&stt, EngineState::Ready);
        assert_eq!(hold(&stt, pcm(1.0, &[]), false).0, Release::Silent);
        // A silent screenshot + voice hold: the screenshot alone.
        assert_eq!(hold(&stt, pcm(1.0, &[]), true).0, Release::PictureOnly);
        // Speech goes, as text.
        assert_eq!(hold(&stt, pcm(1.5, &[(0.2, 1.2)]), true).0, Release::Text("слово".into()));

        // Without the voice detector, silence is never dropped.
        let stt = fresh_app(Fake { no_vad: true, ..Fake::default() });
        stt.models.pretend(model::Status::Ready);
        stt.warm();
        wait_for(&stt, EngineState::Ready);
        assert_eq!(hold(&stt, pcm(1.0, &[]), false).0, Release::Audio("the voice detector could not run".into()));
    }

    #[test]
    fn a_model_that_does_not_load_sends_the_audio() {
        let stt = fresh_app(Fake { fail_load: true, ..Fake::default() });
        stt.models.pretend(model::Status::Ready);
        let (r, after) = hold(&stt, pcm(0.8, &[(0.1, 0.7)]), false);
        assert_eq!(r, Release::Audio("the model did not load".into()));
        assert!(after < Duration::from_millis(500), "{after:?}");
        assert_eq!(stt.engine(), EngineState::Failed);
    }

    #[test]
    fn a_speech_thread_that_panicked_is_started_again_by_the_next_hold() {
        let stt = fresh_app(Fake { panic_once: true, ..Fake::default() });
        stt.models.pretend(model::Status::Ready);
        stt.warm();
        wait_for(&stt, EngineState::Ready);
        // The decoder panics: the thread ends, the hold goes as audio.
        let (r, _) = hold(&stt, pcm(1.5, &[(0.2, 1.2)]), false);
        assert_eq!(r, Release::Audio("the speech thread is gone".into()));
        wait_for(&stt, EngineState::Gone);
        // The next hold starts it again and goes as audio itself.
        let (r, _) = hold(&stt, pcm(1.0, &[(0.2, 0.8)]), false);
        assert_eq!(r, Release::Audio("the speech thread was gone (started again)".into()));
        // The one after that is text.
        wait_for(&stt, EngineState::Ready);
        assert_eq!(hold(&stt, pcm(1.5, &[(0.2, 1.2)]), false).0, Release::Text("слово".into()));
    }

    #[test]
    fn without_the_engine_every_hold_goes_as_audio() {
        let dir = std::env::temp_dir().join("snth-stt-test-none");
        let stt = Stt::with_loader(Arc::new(model::Models::new(dir)), None);
        stt.models.pretend(model::Status::Ready);
        assert_eq!(hold(&stt, pcm(1.0, &[]), false).0, Release::Audio("speech-to-text is not on this system".into()));
    }

    // --- the real model ---

    /// The real model on real speech, through the speech thread as a hold
    /// runs it: the audio arrives in real time, the key is let go when it
    /// ends, and the time from the release to the text is measured.
    ///
    /// Needs the two model files and the test clips in $STT_TEST_DIR
    /// (default /tmp/whisper-test): ru6.wav, ru10.wav, en8.wav (speech),
    /// silent5.wav (room noise), zero4.wav (digital silence), all 16 kHz
    /// mono 16-bit. Run in release, alone:
    ///
    /// `cargo test -p snth-connect --release -- --ignored stt_real --nocapture --test-threads 1`
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    #[ignore]
    fn stt_real_transcribes_russian_and_english_and_drops_silence() {
        let (dir, stt) = real_stt();
        let t = Instant::now();
        stt.warm();
        wait_for(&stt, EngineState::Ready);
        println!("load + warm-up: {} ms, peak RSS {} MB", t.elapsed().as_millis(), peak_rss_mb());

        // (clip, let go after this many seconds (the whole clip when 0),
        // words that must be heard). The clips end in 0.5 s of quiet; the
        // cut one is let go mid-word.
        let cases: [(&str, f32, &[&[&str]]); 6] = [
            ("ru6.wav", 0.0, &[&["мия", "мне"], &["экран"]]),
            ("ru10.wav", 0.0, &[&["отчет", "отчёт"], &["курьер"], &["телеграм"]]),
            ("ru10.wav", 9.4, &[&["отчет", "отчёт"], &["курьер"], &["сводк"]]),
            ("en8.wav", 0.0, &[&["calendar"], &["design"], &["afternoon"]]),
            ("silent5.wav", 0.0, &[]),
            ("zero4.wav", 0.0, &[]),
        ];
        for (name, cut, words) in cases {
            let mut pcm = read_clip(&dir, name);
            if cut > 0.0 {
                pcm.truncate((cut * vad::RATE as f32) as usize);
            }
            let secs = pcm.len() as f32 / vad::RATE as f32;
            let (out, after) = hold(&stt, pcm, false);
            println!("{name} ({secs:.1} s): {} ms after the release -> {out:?}", after.as_millis());
            if words.is_empty() {
                assert_eq!(out, Release::Silent, "{name}");
            } else {
                let Release::Text(text) = out else { panic!("{name}: {out:?}") };
                let low = filter::norm(&text);
                for any in words {
                    assert!(any.iter().any(|w| low.contains(&filter::norm(w))), "{name}: none of {any:?} in {text:?}");
                }
            }
        }
        println!("peak RSS {} MB", peak_rss_mb());
    }

    /// The first hold after the download with the real model, in a fresh
    /// process: the warm-up starts at the press, the key is let go after
    /// 1.5 s. Whether the load made it or not, the hold sends something
    /// (text, or the audio because the model was still loading) within
    /// `RELEASE_WAIT`; the next hold is text.
    ///
    /// `cargo test -p snth-connect --release -- --ignored stt_real_first --nocapture --test-threads 1`
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    #[ignore]
    fn stt_real_first_hold_after_the_download() {
        let (dir, stt) = real_stt();
        let mut first = read_clip(&dir, "ru6.wav");
        first.truncate(vad::RATE * 3 / 2);
        let (out, after) = hold(&stt, first, false);
        println!("first hold (1.5 s), engine {:?}: {} ms after the release -> {out:?}", stt.engine(), after.as_millis());
        assert!(matches!(out, Release::Text(_) | Release::Audio(_)), "{out:?}");
        if let Release::Audio(why) = &out {
            assert_eq!(why, "the model was still loading");
        }
        assert!(after < RELEASE_WAIT + Duration::from_millis(500), "{after:?}");
        wait_for(&stt, EngineState::Ready);
        let (out, after) = hold(&stt, read_clip(&dir, "ru6.wav"), false);
        println!("second hold: {} ms after the release -> {out:?}", after.as_millis());
        assert!(matches!(out, Release::Text(_)), "{out:?}");
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn real_stt() -> (std::path::PathBuf, Stt) {
        let dir = std::path::PathBuf::from(std::env::var("STT_TEST_DIR").unwrap_or_else(|_| "/tmp/whisper-test".into()));
        // The files are the pinned ones; mark them checked as the
        // download does.
        for f in model::FILES {
            let marked = std::fs::read_to_string(dir.join(format!("{}.ok", f.name))).is_ok_and(|s| s.trim() == f.sha256);
            if marked {
                continue;
            }
            let t = Instant::now();
            let hash = model::sha256_of(&dir.join(f.name), &AtomicBool::new(false)).expect("model file");
            assert_eq!(hash, f.sha256, "{}", f.name);
            println!("sha256 of {} checked in {} ms", f.name, t.elapsed().as_millis());
            std::fs::write(dir.join(format!("{}.ok", f.name)), f.sha256).unwrap();
        }
        let models = Arc::new(model::Models::new(dir.clone()));
        assert!(models.ready());
        (dir, Stt::with_loader(models, engine_loader()))
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn read_clip(dir: &Path, name: &str) -> Vec<i16> {
        let b = std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let at = b.windows(4).position(|w| w == b"data").unwrap() + 8;
        b[at..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect()
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn peak_rss_mb() -> u64 {
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
        // Bytes on macOS.
        ru.ru_maxrss as u64 / 1024 / 1024
    }
}
