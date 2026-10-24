//! The tone of the owner's voice, estimated on this computer, for a hold
//! whose speech was recognized here (owner.message `voice_tone`, only to
//! a synth that announces `island_device_tone`). The synth's own
//! speech-to-text hears the tone in the audio; a transcript carries none,
//! so this puts a little of it back.
//!
//! Two layers, both local:
//!
//!  - `prosody.rs`: tempo, pauses, loudness and pitch of the hold,
//!    compared with the owner's usual manner (`baseline.rs`, a running
//!    average over past holds, in the data folder). Cheap, always there.
//!  - `emotion.rs`: emotion2vec+ base (ONNX Runtime on the CPU), run on
//!    4 s pieces while the key is held, so only the last piece is left at
//!    the release. Says only a sure "irritated" or "cheerful".
//!
//! `phrase.rs` makes one Russian line of both. The emotion model follows
//! the speech model's life: loaded when a hold starts, unloaded after
//! `IDLE_UNLOAD` without one, never loaded while speech goes to the synth.
//! Its file (118 MB) is fetched after the speech model, while the switch
//! «Определять тон голоса на компьютере» is on; until it is here (or where
//! it cannot run) the line has the prosody alone.
//!
//! The tone never holds a message up: the release waits for it at most
//! `TONE_WAIT` once the text is there, then sends without it.

pub mod baseline;
pub mod emotion;
pub mod phrase;
pub mod prosody;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::model;
use super::vad::{self, RATE};
use super::{Source, IDLE_UNLOAD};
use baseline::Baseline;
use emotion::{Classify, Emotion};
use prosody::Features;

/// How long the release waits for the tone once the text is there.
pub const TONE_WAIT: Duration = Duration::from_millis(300);
/// The emotion model runs on windows of this much audio (the hold's first
/// 6 s, the next 6 s, ...), each once, as soon as the key has been held
/// past its end. The eval clips are 3 to 10 s; 4 s pieces lost most of the
/// sure "angry", 6 s keeps the windows like the clips.
const WINDOW: usize = RATE * 6;
/// While the key is held the window in progress is also run as far as it
/// goes, again after every this much more audio: the release falls back on
/// that run when the full one is not done in time. On the eval set a run
/// on the first 70% of a clip is as good as one on all of it.
const PREFIX_EVERY: usize = RATE * 2;
/// A run on less speech than this is not made (a breath, a cough).
const MIN_RUN_SPEECH: usize = RATE;
/// How often a running hold is looked at.
const LOOK_EVERY: Duration = Duration::from_millis(400);
/// A failed download of the emotion model is tried again after this long
/// (a hold starting is what tries).
const RETRY_DOWNLOAD: Duration = Duration::from_secs(6 * 3600);
/// The baseline's file in the data folder.
pub const BASELINE_FILE: &str = "island-tone.json";

/// Loads the emotion model from its file.
pub type Loader = Arc<dyn Fn(&Path) -> Result<Box<dyn Classify>, String> + Send + Sync>;

enum Slot {
    Unloaded,
    Loading,
    Ready(Box<dyn Classify>),
    /// The last load failed: not tried again until the file changes (a
    /// delete and a new download) or the app restarts.
    Failed,
}

/// The emotion model, shared by the holds and the idle unloader.
struct Held {
    slot: Mutex<Slot>,
    last_use: Mutex<Instant>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic inside the model must not take the island down with it.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the tone thread heard in one hold.
#[derive(Debug, Clone, PartialEq)]
pub struct Acoustic {
    pub features: Features,
    pub emotion: Option<Emotion>,
    /// The model ran over all of the hold. False: the first answer, at
    /// once, with what was run while the key was held; the full one
    /// follows.
    pub complete: bool,
    /// From the release to this answer.
    pub took: Duration,
}

/// The best answer within `wait`: the complete one when it comes in time,
/// else the first one (the prosody is in both).
pub fn wait_for(answer: &mpsc::Receiver<Acoustic>, wait: Duration) -> Option<Acoustic> {
    let deadline = Instant::now() + wait;
    let mut best = None;
    loop {
        match answer.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(a) if a.complete => return Some(a),
            Ok(a) => best = Some(a),
            Err(_) => return best,
        }
    }
}

pub struct Tone {
    pub models: Arc<model::Models>,
    loader: Option<Loader>,
    held: Arc<Held>,
    unloader: AtomicBool,
    download_failed_at: Mutex<Option<Instant>>,
    baseline_path: PathBuf,
    baseline: Mutex<Baseline>,
}

impl Tone {
    pub fn new(data_dir: &Path) -> Tone {
        Tone::with_loader(Arc::new(model::Models::with_files(data_dir.join("stt"), &model::TONE_FILES)), loader(), data_dir.join(BASELINE_FILE))
    }

    fn with_loader(models: Arc<model::Models>, loader: Option<Loader>, baseline_path: PathBuf) -> Tone {
        let baseline = Baseline::load(&baseline_path);
        Tone {
            models,
            loader,
            held: Arc::new(Held { slot: Mutex::new(Slot::Unloaded), last_use: Mutex::new(Instant::now()) }),
            unloader: AtomicBool::new(false),
            download_failed_at: Mutex::new(None),
            baseline_path,
            baseline: Mutex::new(baseline),
        }
    }

    /// The emotion model can run in this build (Apple silicon, Windows x64).
    pub fn available(&self) -> bool {
        self.loader.is_some()
    }

    /// Fetch the emotion model when it is missing (the switch is on and the
    /// speech model is here). A failed download is tried again only after
    /// `RETRY_DOWNLOAD`, unless `asked` (the owner turned the switch on).
    pub fn download(&self, asked: bool, changed: Arc<dyn Fn() + Send + Sync>) {
        if !self.available() || self.models.ready() || self.models.downloading() {
            return;
        }
        let mut failed = lock(&self.download_failed_at);
        if !asked && failed.is_some_and(|t| t.elapsed() < RETRY_DOWNLOAD) {
            return;
        }
        *failed = None;
        drop(failed);
        if let Err(e) = self.models.download(changed) {
            connect_core::logln!("island: tone: the emotion model was not fetched: {e}");
            *lock(&self.download_failed_at) = Some(Instant::now());
        }
    }

    /// The download ended in a failure (the status says so): remember it,
    /// so holds do not try again at once.
    pub fn note_status(&self) {
        if matches!(self.models.status(), model::Status::Failed { .. }) {
            lock(&self.download_failed_at).get_or_insert_with(Instant::now);
        }
    }

    /// Load the emotion model ahead of a hold (nothing when it is loaded,
    /// loading, failed, or its file is not here).
    pub fn warm(&self) {
        let Some(loader) = self.loader.clone() else { return };
        *lock(&self.held.last_use) = Instant::now();
        if !self.models.ready() {
            return;
        }
        {
            let mut slot = lock(&self.held.slot);
            if !matches!(*slot, Slot::Unloaded) {
                return;
            }
            *slot = Slot::Loading;
        }
        let held = self.held.clone();
        let path = self.models.path(&model::EMOTION);
        let spawned = std::thread::Builder::new().name("island-tone-load".into()).spawn(move || {
            let t = Instant::now();
            let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| loader(&path))).unwrap_or_else(|_| Err("the load panicked".into()));
            let mut slot = lock(&held.slot);
            match loaded {
                Ok(m) => {
                    connect_core::logln!("island: tone: the emotion model loaded in {} ms", t.elapsed().as_millis());
                    *slot = Slot::Ready(m);
                }
                Err(e) => {
                    connect_core::logln!("island: tone: the emotion model did not load: {e}");
                    *slot = Slot::Failed;
                }
            }
            *lock(&held.last_use) = Instant::now();
        });
        if spawned.is_err() {
            *lock(&self.held.slot) = Slot::Unloaded;
            return;
        }
        self.start_unloader();
    }

    /// One thread for the life of the app unloads the model after
    /// `IDLE_UNLOAD` without a hold.
    fn start_unloader(&self) {
        if self.unloader.swap(true, Ordering::AcqRel) {
            return;
        }
        let held = Arc::downgrade(&self.held);
        let _ = std::thread::Builder::new().name("island-tone-idle".into()).spawn(move || loop {
            std::thread::sleep(Duration::from_secs(30));
            let Some(held) = held.upgrade() else { return };
            unload_if_idle(&held, IDLE_UNLOAD);
        });
    }

    /// The model's file is going (the owner deleted the models).
    pub fn unload(&self) {
        *lock(&self.held.slot) = Slot::Unloaded;
    }

    #[cfg(test)]
    pub fn loaded(&self) -> bool {
        matches!(*lock(&self.held.slot), Slot::Ready(_))
    }

    /// Start listening to a hold that records now.
    pub fn hold(&self, source: Source) -> Option<ToneSession> {
        let (end_tx, end_rx) = mpsc::channel();
        let held = self.held.clone();
        let runs = Arc::new(Mutex::new(Runs::default()));
        let published = runs.clone();
        let spawned = std::thread::Builder::new().name("island-tone".into()).spawn(move || run_hold(&held, source, end_rx, &published));
        match spawned {
            Ok(_) => Some(ToneSession { end: Some(end_tx), runs }),
            Err(e) => {
                connect_core::logln!("island: tone: the thread did not start: {e}");
                None
            }
        }
    }

    /// The line for the synth, from what the hold sounded like and what was
    /// said; the hold then joins the owner's usual manner. Writes a small
    /// file: call it off the async runtime's threads.
    pub fn describe(&self, a: &Acoustic, transcript: &str) -> String {
        let f = prosody::with_rate(a.features, transcript);
        let mut b = lock(&self.baseline);
        let line = phrase::describe(&f, &b, a.emotion.as_ref());
        let before = b.holds;
        b.add(&f);
        if b.holds != before {
            if let Err(e) = b.save(&self.baseline_path) {
                connect_core::logln!("island: tone: the baseline was not saved: {e}");
            }
        }
        line
    }
}

fn unload_if_idle(held: &Held, idle: Duration) {
    let last = *lock(&held.last_use);
    let mut slot = lock(&held.slot);
    if matches!(*slot, Slot::Ready(_)) && last.elapsed() >= idle {
        *slot = Slot::Unloaded;
        connect_core::logln!("island: tone: the emotion model unloaded after {} min idle", idle.as_secs() / 60);
    }
}

#[cfg(device_stt)]
fn loader() -> Option<Loader> {
    Some(Arc::new(|p: &Path| emotion::Model::load(p).map(|m| Box::new(m) as Box<dyn Classify>)))
}

#[cfg(not(device_stt))]
fn loader() -> Option<Loader> {
    None
}

/// The tone side of a running hold. `finish` ends it; dropping it throws
/// it away (a slip, a cancel, a hold that went as audio).
pub struct ToneSession {
    end: Option<mpsc::Sender<End>>,
    /// The runs made so far, as the tone thread last left them: the first
    /// answer at the release is made of these, so it never waits for a run
    /// in progress.
    runs: Arc<Mutex<Runs>>,
}

struct End {
    pcm: Vec<i16>,
    reply: mpsc::SyncSender<Acoustic>,
}

impl ToneSession {
    /// The key was let go: the answers come on the receiver (`wait_for`).
    /// Does not block.
    pub fn finish(mut self, pcm: Vec<i16>) -> mpsc::Receiver<Acoustic> {
        let (reply, answer) = mpsc::sync_channel(2);
        let t = Instant::now();
        let audio = to_f32(&pcm);
        if let Some(end) = self.end.take() {
            let _ = end.send(End { pcm, reply: reply.clone() });
        }
        // The first answer on a thread of its own: the tone thread may be
        // in the middle of a run (up to half a second).
        let runs = self.runs.clone();
        let _ = std::thread::Builder::new().name("island-tone-first".into()).spawn(move || {
            let features = prosody::measure(&audio);
            let emotion = lock(&runs).so_far(audio.len());
            let _ = reply.send(Acoustic { features, emotion, complete: false, took: t.elapsed() });
        });
        answer
    }
}

fn to_f32(pcm: &[i16]) -> Vec<f32> {
    pcm.iter().map(|s| f32::from(*s) / 32768.0).collect()
}

/// One run of the model: its probabilities and weight (seconds of speech).
type Run = ([f32; 9], f32);

enum Ran {
    /// None: nothing to run (too little speech, no model, a failure).
    Done(Option<Run>),
    /// The model is still loading.
    NotYet,
}

fn run_model(held: &Held, audio: &[f32]) -> Ran {
    let speech = vad::speech_len(&vad::energy_spans(audio));
    if speech < MIN_RUN_SPEECH {
        return Ran::Done(None);
    }
    let mut slot = lock(&held.slot);
    let model = match &mut *slot {
        Slot::Ready(m) => m,
        Slot::Loading => return Ran::NotYet,
        Slot::Unloaded | Slot::Failed => return Ran::Done(None),
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| model.classify(audio)));
    *lock(&held.last_use) = Instant::now();
    match result {
        Ok(Ok(p)) => Ran::Done(Some((p, speech as f32 / RATE as f32))),
        Ok(Err(e)) => {
            connect_core::logln!("island: tone: the emotion model failed: {e}");
            Ran::Done(None)
        }
        Err(_) => {
            connect_core::logln!("island: tone: the emotion model panicked; it is not used again until a restart");
            *slot = Slot::Failed;
            Ran::Done(None)
        }
    }
}

/// The runs of one hold.
#[derive(Debug, Clone, Default)]
struct Runs {
    /// Of the whole windows, in order.
    windows: Vec<Run>,
    /// Where the next window starts (the windows before it are done).
    next: usize,
    /// The window in progress, as far as it went: (its end, the run).
    prefix: Option<(usize, Option<Run>)>,
    /// The model was still loading at the release.
    gave_up: bool,
}

impl Runs {
    /// Run the whole windows not run yet. While the key is held a model
    /// still loading is waited for; `last`: the release, it is given up.
    fn windows(&mut self, held: &Held, audio: &[f32], last: bool) {
        while !self.gave_up && audio.len() >= self.next + WINDOW {
            match run_model(held, &audio[self.next..self.next + WINDOW]) {
                Ran::Done(r) => {
                    self.windows.extend(r);
                    self.next += WINDOW;
                    self.prefix = None;
                }
                Ran::NotYet if last => self.gave_up = true,
                Ran::NotYet => return,
            }
        }
    }

    /// Run the window in progress again when it grew enough.
    fn prefix(&mut self, held: &Held, audio: &[f32]) {
        let from = self.prefix.as_ref().map(|p| p.0).unwrap_or(self.next);
        if audio.len() < from + PREFIX_EVERY {
            return;
        }
        if let Ran::Done(r) = run_model(held, &audio[self.next..]) {
            self.prefix = Some((audio.len(), r));
        }
    }

    /// The emotion so far: the whole windows, and the window in progress
    /// as far as its prefix ran when that covers at least half of it.
    fn so_far(&self, len: usize) -> Option<Emotion> {
        let mut runs = self.windows.clone();
        if let Some((end, Some(r))) = &self.prefix {
            if (end - self.next) * 2 >= len - self.next {
                runs.push(*r);
            }
        }
        emotion::average(&runs)
    }

    /// The release: every window left, the last one as far as it goes.
    fn finish(&mut self, held: &Held, audio: &[f32]) -> Option<Emotion> {
        self.windows(held, audio, true);
        if !self.gave_up && audio.len() > self.next {
            // A prefix that ran over all of it already is the run.
            if let Some((end, r)) = self.prefix {
                if end == audio.len() {
                    self.windows.extend(r);
                    self.next = audio.len();
                    return emotion::average(&self.windows);
                }
            }
            match run_model(held, &audio[self.next..]) {
                Ran::Done(r) => {
                    self.prefix = Some((audio.len(), r));
                    self.windows.extend(r);
                    self.next = audio.len();
                }
                Ran::NotYet => self.gave_up = true,
            }
        }
        emotion::average(&self.windows)
    }
}

/// One hold on the tone thread: the windows (and the window in progress)
/// while the key is held; at the release the prosody and the emotion so
/// far at once, then the complete answer once the rest is run.
fn run_hold(held: &Held, mut source: Source, end: mpsc::Receiver<End>, published: &Mutex<Runs>) {
    let mut audio: Vec<f32> = Vec::new();
    let mut runs = Runs::default();
    loop {
        match end.recv_timeout(LOOK_EVERY) {
            Ok(End { pcm, reply }) => {
                let t = Instant::now();
                if pcm.len() > audio.len() {
                    audio.extend(to_f32(&pcm[audio.len()..]));
                }
                let features = prosody::measure(&audio);
                let emotion = runs.finish(held, &audio);
                let _ = reply.send(Acoustic { features, emotion, complete: true, took: t.elapsed() });
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let new = source(audio.len());
                audio.extend(to_f32(&new));
                runs.windows(held, &audio, false);
                runs.prefix(held, &audio);
                *lock(published) = runs.clone();
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::prosody::tests::voiced;
    use super::*;

    /// A stand-in model: "angry" in proportion to the piece's loudness,
    /// counting its runs.
    struct Fake {
        runs: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Classify for Fake {
        fn classify(&mut self, audio: &[f32]) -> Result<[f32; 9], String> {
            self.runs.fetch_add(1, Ordering::AcqRel);
            let peak = audio.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            let mut p = [0.0; 9];
            p[0] = (peak * 2.0).min(1.0);
            p[4] = 1.0 - p[0];
            Ok(p)
        }
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("snth-tone-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn tone(name: &str, load: Duration) -> (Tone, Arc<std::sync::atomic::AtomicUsize>) {
        let d = dir(name);
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let r = runs.clone();
        let loader: Loader = Arc::new(move |_: &Path| {
            std::thread::sleep(load);
            Ok(Box::new(Fake { runs: r.clone() }) as Box<dyn Classify>)
        });
        let models = Arc::new(model::Models::with_files(d.clone(), &model::TONE_FILES));
        models.pretend(model::Status::Ready);
        (Tone::with_loader(models, Some(loader), d.join(BASELINE_FILE)), runs)
    }

    fn pcm(a: &[f32]) -> Vec<i16> {
        a.iter().map(|s| (s * 32767.0) as i16).collect()
    }

    /// A hold in real time through a tap, then the release.
    fn hold(t: &Tone, audio: Vec<f32>) -> Acoustic {
        let pcm = Arc::new(pcm(&audio));
        let src = pcm.clone();
        let start = Instant::now();
        let session = t
            .hold(Box::new(move |from| {
                let have = ((start.elapsed().as_secs_f32() * RATE as f32) as usize).min(src.len());
                src.get(from..have).map(<[i16]>::to_vec).unwrap_or_default()
            }))
            .unwrap();
        std::thread::sleep(Duration::from_secs_f32(pcm.len() as f32 / RATE as f32));
        let released = Instant::now();
        let answer = session.finish(pcm.to_vec());
        let first = answer.recv_timeout(Duration::from_secs(5)).expect("an answer");
        assert!(released.elapsed() < Duration::from_millis(250), "the first answer at once: {:?}", released.elapsed());
        if first.complete {
            return first;
        }
        let full = wait_for(&answer, Duration::from_secs(5)).unwrap();
        assert!(full.complete);
        assert_eq!(full.features, first.features);
        full
    }

    fn wait_loaded(t: &Tone) {
        let s = Instant::now();
        while !t.loaded() {
            assert!(s.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn pieces_run_while_the_key_is_held_and_the_rest_at_the_release() {
        let (t, runs) = tone("pieces", Duration::ZERO);
        t.warm();
        wait_loaded(&t);
        // 9 s of loud voice: the first window and prefixes while held, the
        // rest at the release.
        let a = hold(&t, voiced(150.0, 9.0, 0.4));
        assert!(runs.load(Ordering::Acquire) >= 3);
        let e = a.emotion.unwrap();
        // The stand-in says angry at twice the peak (0.33 here).
        assert!((e.probs[0] - 0.67).abs() < 0.05, "{e:?}");
        assert!((e.secs - 9.0).abs() < 0.3, "{e:?}");
        assert!((a.features.f0.unwrap() - 150.0).abs() < 3.0);
    }

    #[test]
    fn without_the_model_the_prosody_still_comes() {
        // The file is not here: no load, no emotion.
        let (t, runs) = tone("nomodel", Duration::ZERO);
        t.models.pretend(model::Status::Missing);
        t.warm();
        let a = hold(&t, voiced(200.0, 2.0, 0.2));
        assert_eq!(a.emotion, None);
        assert_eq!(runs.load(Ordering::Acquire), 0);
        assert!(a.features.judgeable());
        // A model still loading at the release: no emotion, no wait.
        let (t, _) = tone("slow", Duration::from_secs(3));
        t.warm();
        let a = hold(&t, voiced(200.0, 1.5, 0.2));
        assert_eq!(a.emotion, None);
    }

    #[test]
    fn the_release_takes_the_complete_answer_or_the_first_in_time() {
        let (tx, rx) = mpsc::sync_channel(2);
        let a = |complete| Acoustic { features: Features::default(), emotion: None, complete, took: Duration::ZERO };
        tx.send(a(false)).unwrap();
        let t = Instant::now();
        assert_eq!(wait_for(&rx, Duration::from_millis(100)), Some(a(false)));
        assert!(t.elapsed() >= Duration::from_millis(100));
        tx.send(a(false)).unwrap();
        tx.send(a(true)).unwrap();
        assert_eq!(wait_for(&rx, Duration::from_secs(5)), Some(a(true)));
        drop(tx);
        assert_eq!(wait_for(&rx, Duration::from_secs(5)), None);
    }

    #[test]
    fn the_windows_cover_the_hold() {
        let (t, _) = tone("windows", Duration::ZERO);
        t.warm();
        wait_loaded(&t);
        let audio = voiced(150.0, 14.0, 0.3);
        let mut r = Runs::default();
        r.windows(&t.held, &audio[..RATE * 7], false);
        assert_eq!((r.windows.len(), r.next), (1, WINDOW));
        r.prefix(&t.held, &audio[..RATE * 7]);
        assert!(r.prefix.is_none(), "1 s into the window: not yet");
        r.prefix(&t.held, &audio[..RATE * 9]);
        assert_eq!(r.prefix.map(|p| p.0), Some(RATE * 9));
        // At 10 s the prefix covers 3 of 4 s: it counts in the first answer.
        assert_eq!(r.so_far(RATE * 10).unwrap().secs, 9.0);
        // At 14 s it covers 3 of 8 s: it does not.
        assert_eq!(r.so_far(RATE * 14).unwrap().secs, 6.0);
        let e = r.finish(&t.held, &audio).unwrap();
        assert_eq!(e.secs, 14.0);
    }

    #[test]
    fn silence_is_not_run_through_the_model() {
        let (t, runs) = tone("silence", Duration::ZERO);
        t.warm();
        wait_loaded(&t);
        let a = hold(&t, vec![0.0; RATE * 5]);
        assert_eq!(runs.load(Ordering::Acquire), 0);
        assert_eq!(a.emotion, None);
        assert!(!a.features.judgeable());
    }

    #[test]
    fn the_model_unloads_when_idle() {
        let (t, _) = tone("idle", Duration::ZERO);
        t.warm();
        wait_loaded(&t);
        unload_if_idle(&t.held, Duration::from_secs(60));
        assert!(t.loaded(), "used just now");
        unload_if_idle(&t.held, Duration::ZERO);
        assert!(!t.loaded());
        // The next hold loads it again.
        t.warm();
        wait_loaded(&t);
    }

    #[test]
    fn describing_a_hold_teaches_the_baseline_and_saves_it() {
        let (t, _) = tone("baseline", Duration::ZERO);
        // 3 s of speech with a usual range of pitch; 15 syllables: 5 a second.
        let features = Features { f0_range: Some(6.0), ..prosody::measure(&voiced(150.0, 3.0, 0.2)) };
        let a = Acoustic { features, emotion: None, complete: true, took: Duration::ZERO };
        let said = "Привет, я сегодня приду домой поздно вечером";
        assert_eq!(prosody::syllables(said), 15);
        for _ in 0..baseline::MIN_HOLDS {
            assert_eq!(t.describe(&a, said), "", "no baseline yet, nothing notable");
        }
        assert_eq!(t.describe(&a, said), phrase::NOTHING_NOTABLE);
        assert_eq!(Baseline::load(&t.baseline_path).holds, baseline::MIN_HOLDS + 1);
    }

    // --- the real model ---

    /// The real emotion model: the same probabilities as the Python export
    /// on the eval clips, its speed, the RAM it adds, and the hold flow
    /// with it (the time from the release to the tone).
    ///
    /// Needs in $TONE_TEST_DIR (default /tmp/tone-test) the model file and
    /// the clips with `manifest.txt` ("<wav> <label> <9 probabilities>" per
    /// line, from the eval). Run in release, alone:
    ///
    /// `cargo test -p snth-connect --release -- --ignored tone_real --nocapture --test-threads 1`
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    #[ignore]
    fn tone_real_matches_the_export_and_is_quick() {
        let dir = PathBuf::from(std::env::var("TONE_TEST_DIR").unwrap_or_else(|_| "/tmp/tone-test".into()));
        let file = dir.join(model::EMOTION.name);
        let marked = dir.join(format!("{}.ok", model::EMOTION.name));
        if std::fs::read_to_string(&marked).map(|s| s.trim() != model::EMOTION.sha256).unwrap_or(true) {
            let hash = model::sha256_of(&file, &AtomicBool::new(false)).expect("model file");
            assert_eq!(hash, model::EMOTION.sha256);
            std::fs::write(&marked, model::EMOTION.sha256).unwrap();
        }
        let rss0 = rss_mb();
        let t = Instant::now();
        let mut m = emotion::Model::load(&file).unwrap();
        println!("load: {} ms, resident {} -> {} MB", t.elapsed().as_millis(), rss0, rss_mb());
        let manifest = std::fs::read_to_string(dir.join("manifest.txt")).unwrap();
        let mut worst = 0.0f32;
        let mut per_sec = Vec::new();
        let clips: Vec<(String, String, Vec<f32>, [f32; 9])> = manifest
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                let mut it = l.split_whitespace();
                let name = it.next().unwrap().to_string();
                let label = it.next().unwrap().to_string();
                let mut want = [0.0; 9];
                for (w, v) in want.iter_mut().zip(it) {
                    *w = v.parse().unwrap();
                }
                let b = std::fs::read(dir.join(&name)).unwrap();
                let at = b.windows(4).position(|w| w == b"data").unwrap() + 8;
                let pcm: Vec<i16> = b[at..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
                (name, label, to_f32(&pcm), want)
            })
            .collect();
        for (name, label, audio, want) in &clips {
            let t = Instant::now();
            let got = m.classify(audio).unwrap();
            let secs = audio.len() as f32 / RATE as f32;
            per_sec.push(t.elapsed().as_secs_f32() * 1000.0 / secs);
            let took = t.elapsed().as_millis();
            let diff = got.iter().zip(want).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            worst = worst.max(diff);
            let e = Emotion { probs: got, secs };
            let python = Emotion { probs: *want, secs };
            println!("{name} ({label}, {secs:.1} s): top {:?}, max |dp| vs Python {diff:.4}, {took} ms, resident {} MB", e.top(), rss_mb());
            // The int8 kernels of ONNX Runtime 1.22 here and of the Python
            // wheel differ a little; the top label must not.
            assert_eq!(e.top().0, python.top().0, "{name}");
        }
        println!("{:.0} ms per second of audio", per_sec.iter().sum::<f32>() / per_sec.len() as f32);
        assert!(worst < 0.15, "the Rust run differs from the export: {worst}");
        drop(m);

        // The hold flow: pieces while held, the rest at the release.
        let tone = Tone::with_loader(
            Arc::new(model::Models::with_files(dir.clone(), &model::TONE_FILES)),
            loader(),
            std::env::temp_dir().join(format!("snth-tone-real-{}.json", std::process::id())),
        );
        assert!(tone.models.ready());
        tone.warm();
        wait_loaded(&tone);
        for (name, label, audio, _) in &clips {
            let a = hold(&tone, audio.clone());
            let line = phrase::describe(&a.features, &Baseline::default(), a.emotion.as_ref());
            println!("{name} ({label}): complete {} ms after the release; emotion {:?}; line {line:?}; resident {} MB", a.took.as_millis(), a.emotion.map(|e| e.top()), rss_mb());
            assert!(a.took < Duration::from_millis(1500), "{:?}", a.took);
        }
        println!("resident {} MB", rss_mb());
        // Unloaded, the memory goes back.
        tone.unload();
        std::thread::sleep(Duration::from_millis(500));
        println!("unloaded: resident {} MB", rss_mb());
    }

    /// This process's resident memory now (ps), in MB.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn rss_mb() -> u64 {
        let out = connect_core::proc::std_command("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().unwrap_or(0) / 1024
    }
}
