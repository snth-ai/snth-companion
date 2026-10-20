//! Wake by name with the real models (ignored: they need the files).
//!
//! The models in $STT_TEST_DIR (default /tmp/whisper-test):
//! ggml-tiny-q5_1.bin and ggml-silero-v5.1.2.bin. The clips in
//! $WAKE_TEST_DIR (default /tmp/wake-test), 16 kHz mono 16-bit, made with
//! `say` and ffmpeg (0.6 s of silence before the speech, 1.6 s after):
//!
//! ```text
//! gen() { say -v "$2" -o "$1.aiff" "$3" && ffmpeg -loglevel error -y -i "$1.aiff" \
//!   -af "adelay=600|600,apad=pad_dur=1.6" -ar 16000 -ac 1 -sample_fmt s16 "$1.wav"; }
//! gen ru_wake Milena "Мия, открой почту"
//! gen en_wake Karen "Mia, open my mail"
//! gen en_wake_daniel Daniel "Mia, open my mail"
//! gen en_wake_flo "Flo (English (US))" "Mia, open my mail"
//! gen en_wake_samantha Samantha "Mia, open my mail"
//! gen ru_hey Milena "Эй, Мия, найди билеты в Бангкок"
//! gen ru_name_only Milena "Мия"
//! gen ru_mid Milena "я вчера видел Мию"
//! gen en_mid Samantha "I talked to Mia yesterday about the trip"
//! gen ru_other Milena "Сегодня хорошая погода, пойдём гулять в парк"
//! gen en_other Samantha "The quarterly report is due on Friday afternoon"
//! gen ru_mir Milena "Мир такой большой, правда"
//! gen ru_milo Milena "Мило, что ты зашла"
//! gen en_me Samantha "Me and my friends went out"
//! gen ru_maya Milena "Майя, привет, как дела"
//! ```
//!
//! Samantha says the name so that whisper tiny hears "Maya" in every
//! language and prompt tried, which is another name: that clip is printed,
//! not judged. So is "Эй, Мия, ..." (`ru_hey.wav`): tiny garbles the
//! Milena voice's "Эй, Мия" in both languages ("А, меня", "A- happened"),
//! a known miss of the name in second place.
//!
//! And `ru_name_pause.wav`: "Мия", 1.8 s of silence, "найди билеты в
//! Бангкок" (two `say` files joined by ffmpeg's concat).
//!
//! `cargo test -p snth-connect --release -- --ignored wake_real --nocapture --test-threads 1`

#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::ears::WhisperEars;
use super::listen::{Ears, Event, Listener};
use super::names::Names;
use super::TINY;
use crate::island::stt::model::VAD;
use crate::island::stt::vad::RATE;

fn dirs() -> (PathBuf, PathBuf) {
    let models = PathBuf::from(std::env::var("STT_TEST_DIR").unwrap_or_else(|_| "/tmp/whisper-test".into()));
    let clips = PathBuf::from(std::env::var("WAKE_TEST_DIR").unwrap_or_else(|_| "/tmp/wake-test".into()));
    (models, clips)
}

fn ears() -> WhisperEars {
    let (models, _) = dirs();
    let tiny = models.join(TINY.name);
    let hash = crate::island::stt::model::sha256_of(&tiny, &std::sync::atomic::AtomicBool::new(false)).expect("the tiny model file");
    assert_eq!(hash, TINY.sha256, "the pinned file");
    WhisperEars::load(&tiny, &models.join(VAD.name)).expect("load")
}

fn read(dir: &Path, name: &str) -> Vec<f32> {
    let b = std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let at = b.windows(4).position(|w| w == b"data").unwrap() + 8;
    b[at..].chunks_exact(2).map(|c| f32::from(i16::from_le_bytes([c[0], c[1]])) / 32768.0).collect()
}

/// Room noise at about -60 dBFS (the clips' own silence is digital zero,
/// which no room is).
fn room(secs: f32) -> Vec<f32> {
    let mut seed = 777u32;
    (0..(secs * RATE as f32) as usize)
        .map(|_| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (((seed >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0) * 0.001
        })
        .collect()
}

fn with_room(clip: &[f32]) -> Vec<f32> {
    let noise = room(clip.len() as f32 / RATE as f32 + 8.0);
    let mut out = noise.clone();
    let lead = RATE;
    for (i, s) in clip.iter().enumerate() {
        out[lead + i] += s;
    }
    out
}

fn run(l: &mut Listener, ears: &mut dyn Ears, names: &Names, audio: &[f32]) -> Vec<(f32, Event)> {
    let mut out = Vec::new();
    for block in audio.chunks(RATE / 10) {
        if let Some(e) = l.push(block, ears, names) {
            out.push((l.now() as f32 / RATE as f32, e));
        }
    }
    out
}

/// What whisper tiny hears at the start of a clip, for the log.
struct Spy<'a> {
    inner: &'a mut WhisperEars,
    heard: Vec<(String, u128)>,
    /// Wall time in the voice detector, and the audio it was given.
    vad: (Duration, usize, u32),
}

impl Ears for Spy<'_> {
    fn speech(&mut self, audio: &[f32]) -> Option<Vec<crate::island::stt::vad::Span>> {
        let t = Instant::now();
        let r = self.inner.speech(audio);
        self.vad.0 += t.elapsed();
        self.vad.1 += audio.len();
        self.vad.2 += 1;
        r
    }

    fn hear(&mut self, audio: &[f32], lang: &str, prompt: &str) -> Result<String, String> {
        let t = Instant::now();
        let r = self.inner.hear(audio, lang, prompt);
        self.heard.push((format!("{lang}: {}", r.clone().unwrap_or_default()), t.elapsed().as_millis()));
        r
    }
}

#[test]
#[ignore]
fn wake_real_clips_wake_only_on_the_name_at_the_start() {
    let (_, clips) = dirs();
    let mut e = ears();
    let names = Names::new("Mia", "");
    // (clip, wakes, the request must contain one of these words).
    let cases: &[(&str, bool, &[&str])] = &[
        ("ru_wake.wav", true, &["почт"]),
        ("en_wake.wav", true, &["mail"]),
        ("en_wake_daniel.wav", true, &["mail"]),
        ("en_wake_flo.wav", true, &["mail"]),
        ("ru_name_only.wav", true, &[]),
        ("ru_name_pause.wav", true, &[]),
        ("ru_mid.wav", false, &[]),
        ("en_mid.wav", false, &[]),
        ("ru_other.wav", false, &[]),
        ("en_other.wav", false, &[]),
        ("ru_mir.wav", false, &[]),
        ("ru_milo.wav", false, &[]),
        ("en_me.wav", false, &[]),
        ("ru_maya.wav", false, &[]),
    ];
    let mut failed = Vec::new();
    let cases: Vec<(&str, Option<bool>, &[&str])> = cases.iter().map(|(n, w, x)| (*n, Some(*w), *x)).chain([("en_wake_samantha.wav", None, &[][..]), ("ru_hey.wav", None, &[][..])]).collect();
    for (name, wakes, words) in &cases {
        let audio = with_room(&read(&clips, name));
        let mut l = Listener::new();
        let mut spy = Spy { inner: &mut e, heard: Vec::new(), vad: Default::default() };
        let ev = run(&mut l, &mut spy, &names, &audio);
        let woke = ev.iter().find_map(|(t, e)| match e {
            Event::Wake(w) => Some((*t, w.clone())),
            _ => None,
        });
        let ended = ev.iter().find_map(|(t, e)| match e {
            Event::Send(why) | Event::Drop(why) => Some((*t, *why)),
            _ => None,
        });
        println!("{name}: heard {:?} -> wake {:?}, end {:?}", spy.heard, woke.as_ref().map(|(t, w)| (t, &w.hit, w.name_only)), ended);
        let ok = match (&woke, wakes) {
            (_, None) => true,
            (Some(_), Some(false)) | (None, Some(true)) => false,
            (Some((_, w)), Some(true)) => words.is_empty() || words.iter().any(|x| w.hit.rest.to_lowercase().contains(x)),
            (None, Some(false)) => true,
        };
        if !ok {
            failed.push(*name);
        }
        if *name == "ru_name_only.wav" {
            assert!(woke.as_ref().is_some_and(|(_, w)| w.name_only), "the name alone waits");
            assert!(matches!(ended, Some((_, "nothing followed the name"))), "{ended:?}");
        }
        if *name == "ru_name_pause.wav" {
            assert!(matches!(ended, Some((_, "a second of silence"))), "the request after a pause is sent: {ended:?}");
        }
    }
    assert!(failed.is_empty(), "wrong on {failed:?}");
}

/// The CPU the listener costs, fed from a file as fast as it goes: CPU
/// time over audio time is the share of one core it takes in real time
/// (the microphone's own capture comes on top; `wake_real_mic` measures
/// the whole of it). Quiet room, then speech that never says the name.
#[test]
#[ignore]
fn wake_real_cpu_from_a_file() {
    let (_, clips) = dirs();
    let mut e = ears();
    let names = Names::new("Mia", "");
    let mut talk = Vec::new();
    for name in ["ru_other.wav", "en_other.wav", "ru_mid.wav", "en_mid.wav", "ru_mir.wav", "ru_milo.wav"] {
        talk.extend(read(&clips, name));
    }
    for (what, audio) in [("quiet room, 120 s", room(120.0)), ("speech without the name, 120 s", talk.iter().copied().cycle().take(RATE * 120).collect())] {
        let mut l = Listener::new();
        let cpu0 = cpu_time();
        let t = Instant::now();
        let mut spy = Spy { inner: &mut e, heard: Vec::new(), vad: Default::default() };
        let ev = run(&mut l, &mut spy, &names, &audio);
        let cpu = cpu_time() - cpu0;
        let hear_ms: u128 = spy.heard.iter().map(|(_, ms)| ms).sum();
        println!(
            "  voice detector: {} calls on {:.1} s of audio in {} ms; whisper: {} decodes in {hear_ms} ms",
            spy.vad.2,
            spy.vad.1 as f32 / RATE as f32,
            spy.vad.0.as_millis(),
            spy.heard.len()
        );
        let secs = audio.len() as f32 / RATE as f32;
        println!(
            "{what}: {:.0} ms CPU for {secs:.0} s of audio = {:.2}% of one core ({} whisper checks, wall {} ms, {} wakes)",
            cpu.as_secs_f32() * 1000.0,
            cpu.as_secs_f32() / secs * 100.0,
            l.checks,
            t.elapsed().as_millis(),
            ev.len()
        );
        assert!(ev.is_empty(), "{ev:?}");
    }
    println!("peak RSS {} MB", peak_rss_mb());
}

/// The whole listener on this Mac's microphone for `WAKE_MIC_SECS`
/// (default 30): the capture, the loudness, the checks. Play speech
/// nearby (`say`, a podcast) or keep the room quiet, and read the CPU and
/// the memory. Needs the microphone allowed for the terminal.
///
/// `WAKE_MIC_SECS=30 cargo test -p snth-connect --release -- --ignored wake_real_mic --nocapture`
#[test]
#[ignore]
fn wake_real_mic() {
    let secs: u64 = std::env::var("WAKE_MIC_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(30);
    let mut e = ears();
    let names = Names::new("Mia", "");
    let rec = crate::island::audio::start(std::sync::Arc::new(|_| {})).expect("the microphone");
    let tap = rec.tap();
    let mut l = Listener::new();
    let mut off = 0usize;
    let mut loud = 0f32;
    let cpu0 = cpu_time();
    let t = Instant::now();
    let mut wakes = 0;
    while t.elapsed() < Duration::from_secs(secs) {
        std::thread::sleep(Duration::from_millis(100));
        let new: Vec<f32> = tap.read_from(l.now() - off).iter().map(|s| f32::from(*s) / 32768.0).collect();
        loud = new.iter().fold(loud, |m, s| m.max(s.abs()));
        if let Some(ev) = l.push(&new, &mut e, &names) {
            println!("{:.1} s: {ev:?}", t.elapsed().as_secs_f32());
            if matches!(ev, Event::Wake(_)) {
                wakes += 1;
            }
        }
        if l.now() - off > super::TRIM_AT {
            let n = l.keep_from().saturating_sub(off);
            tap.drop_front(n);
            off += n;
        }
    }
    let cpu = cpu_time() - cpu0;
    println!(
        "{secs} s on the microphone: {:.2}% of one core, {} whisper checks, {wakes} wakes, loudest sample {loud:.3}, peak RSS {} MB (whisper tiny loaded)",
        cpu.as_secs_f32() / t.elapsed().as_secs_f32() * 100.0,
        l.checks,
        peak_rss_mb()
    );
    drop(rec);
}

fn cpu_time() -> Duration {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let tv = |t: libc::timeval| Duration::from_secs(t.tv_sec as u64) + Duration::from_micros(t.tv_usec as u64);
    tv(ru.ru_utime) + tv(ru.ru_stime)
}

fn peak_rss_mb() -> u64 {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    ru.ru_maxrss as u64 / 1024 / 1024
}
