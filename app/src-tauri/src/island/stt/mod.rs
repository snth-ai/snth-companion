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
//!
//! One thread does all of the speech work (`island-stt`). The model is
//! loaded when a hold starts (or when the owner turns the switch on) and
//! unloaded after five minutes without a hold. The release waits at most
//! `FINISH_WAIT` for the text; whatever goes wrong, the audio still in
//! memory goes to the synth instead, as it did before (one log line). The
//! one exception: a hold in which nothing was said at all sends nothing,
//! like a tap shorter than a third of a second.

// Without the engine (Windows, Linux, Intel Macs) only the model files
// and the settings are used; the stream is there for its tests.
#![cfg_attr(not(all(target_os = "macos", target_arch = "aarch64")), allow(dead_code))]

pub mod filter;
pub mod model;
pub mod stream;
pub mod vad;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod engine;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use connect_core::island::{IslandSettings, SttMode};

pub use stream::Outcome;

/// The engine exists in this build (Apple silicon).
pub fn available() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

/// How long the model stays loaded after the last hold.
pub const IDLE_UNLOAD: Duration = Duration::from_secs(5 * 60);
/// How often a running hold is looked at (a look decodes only when there
/// is something new: stream.rs).
const STEP_EVERY: Duration = Duration::from_millis(150);
/// How long the release waits for the text before the audio goes instead
/// (the synth's own speech-to-text takes about this long).
pub const FINISH_WAIT: Duration = Duration::from_secs(6);

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
        SttMode::Synth => Route::Synth("off"),
        SttMode::Device if !available => Route::Synth("not on this system"),
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

struct Job {
    source: Source,
    live: Live,
    end: mpsc::Receiver<End>,
    finishing: Arc<AtomicBool>,
}

struct End {
    /// The whole recording.
    pcm: Vec<i16>,
    reply: mpsc::SyncSender<Result<Outcome, String>>,
}

enum Cmd {
    /// Load the model now (a hold is starting, the switch went on).
    Warm,
    Hold(Job),
}

/// The speech side of a running hold. `finish` ends it; dropping it
/// throws it away (a slip, a cancel).
pub struct Session {
    end: Option<mpsc::Sender<End>>,
    finishing: Arc<AtomicBool>,
}

impl Session {
    /// The key was let go: the text of the whole recording. Blocks for up
    /// to `FINISH_WAIT` (call it off the async runtime's threads).
    ///
    /// A step that is decoding right now runs to its end: its result is
    /// usually the last piece (whisper checks for an abort only after the
    /// encoder, which is most of the time anyway).
    pub fn finish(mut self, pcm: Vec<i16>) -> Result<Outcome, String> {
        let end = self.end.take().ok_or("finished already")?;
        let (reply, answer) = mpsc::sync_channel(1);
        end.send(End { pcm, reply }).map_err(|_| "the speech thread is gone".to_string())?;
        answer.recv_timeout(FINISH_WAIT).map_err(|_| format!("no text within {} s", FINISH_WAIT.as_secs()))?
    }
}

impl Drop for Session {
    /// A slip or a cancel: a step decoding right now stops early.
    fn drop(&mut self) {
        if self.end.is_some() {
            self.finishing.store(true, Ordering::Release);
        }
    }
}

pub struct Stt {
    pub models: Arc<model::Models>,
    tx: Mutex<Option<mpsc::Sender<Cmd>>>,
}

impl Stt {
    pub fn new(data_dir: &Path) -> Stt {
        Stt { models: Arc::new(model::Models::new(data_dir.join("stt"))), tx: Mutex::new(None) }
    }

    fn send(&self, cmd: Cmd) -> bool {
        let mut tx = self.tx.lock().unwrap();
        if tx.is_none() {
            *tx = spawn_thread(self.models.clone());
        }
        match tx.as_ref() {
            Some(t) => t.send(cmd).is_ok(),
            None => false,
        }
    }

    /// Load the model ahead of a hold (nothing when it is loaded, or
    /// cannot be).
    pub fn warm(&self) {
        if available() && self.models.ready() {
            self.send(Cmd::Warm);
        }
    }

    /// Start recognizing a hold that just started recording.
    pub fn hold(&self, source: Source, live: Live) -> Option<Session> {
        if !available() || !self.models.ready() {
            return None;
        }
        let (end_tx, end_rx) = mpsc::channel();
        let finishing = Arc::new(AtomicBool::new(false));
        let job = Job { source, live, end: end_rx, finishing: finishing.clone() };
        self.send(Cmd::Hold(job)).then_some(Session { end: Some(end_tx), finishing })
    }
}

fn to_f32(pcm: &[i16]) -> Vec<f32> {
    pcm.iter().map(|s| f32::from(*s) / 32768.0).collect()
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn spawn_thread(models: Arc<model::Models>) -> Option<mpsc::Sender<Cmd>> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new().name("island-stt".into()).spawn(move || run(rx, models));
    match spawned {
        Ok(_) => Some(tx),
        Err(e) => {
            connect_core::logln!("island: the speech thread did not start: {e}");
            None
        }
    }
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn spawn_thread(_models: Arc<model::Models>) -> Option<mpsc::Sender<Cmd>> {
    None
}

/// The speech thread: load on demand, run holds, unload when idle.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn run(rx: mpsc::Receiver<Cmd>, models: Arc<model::Models>) {
    use std::time::Instant;
    let mut engine: Option<engine::Engine> = None;
    let mut last_use = Instant::now();
    loop {
        let cmd = if engine.is_some() {
            match rx.recv_timeout(IDLE_UNLOAD.saturating_sub(last_use.elapsed())) {
                Ok(c) => c,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    engine = None;
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
        } else if engine.is_none() {
            let (model, vad) = models.paths();
            match engine::Engine::load(&model, &vad) {
                Ok(e) => engine = Some(e),
                Err(e) => connect_core::logln!("island: speech-to-text did not load: {e}"),
            }
        }
        if let Cmd::Hold(job) = cmd {
            match engine.as_mut() {
                Some(e) => run_hold(e, job),
                None => refuse(job),
            }
        }
        last_use = Instant::now();
    }
}

/// No engine: answer the release at once, so the audio goes.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn refuse(job: Job) {
    if let Ok(end) = job.end.recv() {
        let _ = end.reply.send(Err("the model did not load".into()));
    }
}

/// One hold: a step about once a second until the key is let go.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn run_hold(engine: &mut engine::Engine, mut job: Job) {
    use std::time::Instant;
    let mut s = stream::Streamer::new();
    let mut broken: Option<String> = None;
    loop {
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
        assert!(matches!(route(SttMode::Synth, true, true, true), Route::Synth("off")));
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
        drop(Session { end: Some(end), finishing: finishing.clone() });
        assert!(finishing.load(Ordering::Acquire));
    }

    #[test]
    fn samples_become_floats() {
        assert_eq!(to_f32(&[0, 16384, -32768]), vec![0.0, 0.5, -1.0]);
    }

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
        use std::time::Instant;
        let dir = std::path::PathBuf::from(std::env::var("STT_TEST_DIR").unwrap_or_else(|_| "/tmp/whisper-test".into()));
        // The files are the pinned ones; mark them checked as the
        // download does.
        for f in model::FILES {
            let t = Instant::now();
            let hash = model::sha256_of(&dir.join(f.name), &AtomicBool::new(false)).expect("model file");
            assert_eq!(hash, f.sha256, "{}", f.name);
            println!("sha256 of {} checked in {} ms", f.name, t.elapsed().as_millis());
            std::fs::write(dir.join(format!("{}.ok", f.name)), f.sha256).unwrap();
        }
        let models = Arc::new(model::Models::new(dir.clone()));
        assert!(models.ready());
        let stt = Stt { models, tx: Mutex::new(None) };

        let t = Instant::now();
        stt.warm();
        // The warm-up is done when a tiny hold answers.
        let s = stt.hold(Box::new(|_| Vec::new()), Box::new(|_| {})).unwrap();
        let _ = s.finish(vec![0; 1600]);
        println!("load + warm-up: {} ms, peak RSS {} MB", t.elapsed().as_millis(), peak_rss_mb());

        let read = |name: &str| -> Vec<i16> {
            let b = std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
            let at = b.windows(4).position(|w| w == b"data").unwrap() + 8;
            b[at..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect()
        };
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
            let mut pcm = read(name);
            if cut > 0.0 {
                pcm.truncate((cut * vad::RATE as f32) as usize);
            }
            let pcm = Arc::new(pcm);
            let secs = pcm.len() as f32 / vad::RATE as f32;
            let start = Instant::now();
            let src = pcm.clone();
            let lives = Arc::new(Mutex::new(Vec::<(u128, String)>::new()));
            let seen = lives.clone();
            let session = stt
                .hold(
                    Box::new(move |from| {
                        let have = ((start.elapsed().as_secs_f32() * vad::RATE as f32) as usize).min(src.len());
                        src.get(from..have).map(<[i16]>::to_vec).unwrap_or_default()
                    }),
                    Box::new(move |text| seen.lock().unwrap().push((start.elapsed().as_millis(), text))),
                )
                .unwrap();
            // The owner talks for as long as the clip runs, then lets go.
            std::thread::sleep(Duration::from_secs_f32(secs));
            let released = Instant::now();
            let out = session.finish(pcm.to_vec()).unwrap();
            let after = released.elapsed().as_millis();
            println!("{name} ({secs:.1} s): {after} ms after the release -> {out:?}");
            for (at, text) in lives.lock().unwrap().iter() {
                println!("    live at {:.1} s: {text}", *at as f32 / 1000.0);
            }
            if words.is_empty() {
                assert_eq!(out, Outcome::Silent, "{name}");
            } else {
                let Outcome::Text(text) = out else { panic!("{name}: {out:?}") };
                let low = filter::norm(&text);
                for any in words {
                    assert!(any.iter().any(|w| low.contains(&filter::norm(w))), "{name}: none of {any:?} in {text:?}");
                }
            }
        }
        println!("peak RSS {} MB", peak_rss_mb());
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn peak_rss_mb() -> u64 {
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
        // Bytes on macOS.
        ru.ru_maxrss as u64 / 1024 / 1024
    }
}
