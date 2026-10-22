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
//!
//! On Windows (and anywhere `wake_whisper` is on) the same listener runs
//! with that build's ears: whisper tiny on whisper.cpp's CPU code and the
//! ONNX Silero (`super::VAD` is `silero_vad.onnx` there).
//! `wake_real_refs` is the check the Windows CI runs: clips of its own
//! speech synthesizer with a `refs.tsv`, the models downloaded as the app
//! downloads them, and Parakeet on the same clips for comparison.

#![cfg(wake_whisper)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::ears::WhisperEars;
use super::listen::{Ears, Event, Listener};
use super::names::Names;
use super::{TINY, VAD};
use crate::island::stt::model;
use crate::island::stt::vad::RATE;

fn dirs() -> (PathBuf, PathBuf) {
    let models = PathBuf::from(std::env::var("STT_TEST_DIR").unwrap_or_else(|_| "/tmp/whisper-test".into()));
    let clips = PathBuf::from(std::env::var("WAKE_TEST_DIR").unwrap_or_else(|_| "/tmp/wake-test".into()));
    (models, clips)
}

fn ears_in(models: &Path) -> WhisperEars {
    let tiny = models.join(TINY.name);
    let hash = model::sha256_of(&tiny, &std::sync::atomic::AtomicBool::new(false)).expect("the tiny model file");
    assert_eq!(hash, TINY.sha256, "the pinned file");
    WhisperEars::load(&tiny, &models.join(VAD.name)).expect("load")
}

fn ears() -> WhisperEars {
    ears_in(&dirs().0)
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
    inner: &'a mut dyn Ears,
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

/// CPU time of this process (user + system).
#[cfg(unix)]
fn cpu_time() -> Duration {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let tv = |t: libc::timeval| Duration::from_secs(t.tv_sec as u64) + Duration::from_micros(t.tv_usec as u64);
    tv(ru.ru_utime) + tv(ru.ru_stime)
}

#[cfg(windows)]
fn cpu_time() -> Duration {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut c, mut e, mut k, mut u) = (z, z, z, z);
    unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) };
    let t = |f: FILETIME| Duration::from_nanos(((u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)) * 100);
    t(k) + t(u)
}

/// The peak resident memory so far, in MB.
#[cfg(unix)]
fn peak_rss_mb() -> u64 {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    ru.ru_maxrss as u64 / 1024 / 1024
}

#[cfg(windows)]
fn peak_rss_mb() -> u64 {
    mem().1
}

/// (working set now, peak working set, private bytes now), in MB.
#[cfg(windows)]
fn mem() -> (u64, u64, u64) {
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut c: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    let mb = |b: usize| b as u64 / 1024 / 1024;
    (mb(c.WorkingSetSize), mb(c.PeakWorkingSetSize), mb(c.PagefileUsage))
}

/// The memory now, as the system's own tool would show it.
fn mem_now() -> String {
    #[cfg(windows)]
    {
        let (ws, peak, private) = mem();
        format!("working set {ws} MB (peak {peak} MB), private {private} MB")
    }
    #[cfg(target_os = "macos")]
    {
        let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
        let ok = unsafe { libc::proc_pid_rusage(std::process::id() as i32, libc::RUSAGE_INFO_V2, &mut info as *mut _ as *mut libc::rusage_info_t) } == 0;
        format!("footprint {} MB", if ok { info.ri_phys_footprint / 1024 / 1024 } else { 0 })
    }
}

// --- the check on refs (the Windows CI) ---

/// Parakeet as the ears, for the comparison: its voice detector, and one
/// decode per stretch (it has no language or prompt to set, so the second
/// language gets the first one's text).
struct ParakeetEars {
    engine: crate::island::stt::parakeet::Engine,
    last: Option<(usize, String)>,
    decodes: Vec<u128>,
}

impl Ears for ParakeetEars {
    fn speech(&mut self, audio: &[f32]) -> Option<Vec<crate::island::stt::vad::Span>> {
        use crate::island::stt::stream::Decoder;
        self.engine.vad(audio)
    }

    fn hear(&mut self, audio: &[f32], _lang: &str, _prompt: &str) -> Result<String, String> {
        use crate::island::stt::stream::Decoder;
        if let Some((n, text)) = &self.last {
            if *n == audio.len() {
                return Ok(text.clone());
            }
        }
        let t = Instant::now();
        let text = self.engine.decode(audio, None, None)?.segments.join(" ");
        self.decodes.push(t.elapsed().as_millis());
        self.last = Some((audio.len(), text.clone()));
        Ok(text)
    }
}

/// Download a pinned set into `dir` as the app does and wait for it.
fn download_into(dir: &Path, files: &'static [&'static model::ModelFile]) -> Arc<model::Models> {
    let m = Arc::new(model::Models::with_files(dir.to_path_buf(), files));
    let t = Instant::now();
    m.download(Arc::new(|| {})).expect("the download starts");
    while !m.ready() {
        if let model::Status::Failed { code, detail } = m.status() {
            panic!("download failed: {code}: {detail}");
        }
        assert!(t.elapsed() < Duration::from_secs(1200), "the download took too long");
        std::thread::sleep(Duration::from_millis(200));
    }
    println!("{} ready in {} s", files.iter().map(|f| f.name).collect::<Vec<_>>().join(", "), t.elapsed().as_secs());
    m
}

/// One line of refs.tsv: `<wav>\t<expect>\t<word>\t<name>`. `expect`:
/// `wake` (the request must hold `word`, when given), `name_only` (the
/// name alone: it waits, then drops), `no` (must not wake), `any` (printed,
/// not judged). `name`: the synth's name (default Mia).
struct Case {
    wav: String,
    expect: String,
    word: String,
    name: String,
}

fn cases(dir: &Path) -> Vec<Case> {
    let refs = std::fs::read_to_string(dir.join("refs.tsv")).expect("refs.tsv");
    refs.trim_start_matches('\u{feff}')
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split('\t').map(str::trim).collect();
            let at = |i: usize| f.get(i).copied().unwrap_or("").to_string();
            Case { wav: at(0), expect: at(1), word: at(2).to_lowercase(), name: if at(3).is_empty() { "Mia".into() } else { at(3) } }
        })
        .collect()
}

/// The listener on each clip; the failures (by the refs), and the lines
/// for the log.
fn judge(ears: &mut dyn Ears, clips: &Path, cases: &[Case], label: &str) -> Vec<String> {
    let mut failed = Vec::new();
    for c in cases {
        let names = Names::new(&c.name, "");
        let audio = with_room(&read(clips, &c.wav));
        let mut l = Listener::new();
        let mut spy = Spy { inner: &mut *ears, heard: Vec::new(), vad: Default::default() };
        let ev = run(&mut l, &mut spy, &names, &audio);
        let woke = ev.iter().find_map(|(t, e)| match e {
            Event::Wake(w) => Some((*t, w.clone())),
            _ => None,
        });
        let ended = ev.iter().find_map(|(t, e)| match e {
            Event::Send(why) | Event::Drop(why) => Some((*t, *why)),
            _ => None,
        });
        println!(
            "[{label}] {} ({}, {}): heard {:?} -> wake {:?}, end {:?}",
            c.wav,
            c.name,
            c.expect,
            spy.heard,
            woke.as_ref().map(|(t, w)| (t, &w.hit, w.name_only)),
            ended
        );
        let ok = match c.expect.as_str() {
            "wake" => woke.as_ref().is_some_and(|(_, w)| c.word.is_empty() || w.hit.rest.to_lowercase().contains(&c.word)) && matches!(ended, Some((_, "a second of silence"))),
            "name_only" => woke.as_ref().is_some_and(|(_, w)| w.name_only) && matches!(ended, Some((_, "nothing followed the name"))),
            "no" => woke.is_none(),
            _ => true,
        };
        if !ok {
            failed.push(c.wav.clone());
        }
    }
    failed
}

/// Wake by name on real models, the way the Windows CI runs it: the clips
/// in $WAKE_E2E_CLIPS (16 kHz mono 16-bit WAVs and refs.tsv, see `Case`),
/// the wake files downloaded into $WAKE_E2E_DOWNLOAD_TO as the app
/// downloads them (else taken from $STT_TEST_DIR), then the CPU and memory
/// the ears cost. With $WAKE_E2E_PARAKEET (a folder with Parakeet's
/// pinned files, as the hold's check downloads them) the same clips go
/// through Parakeet as the ears too, printed, not judged: the measurement
/// behind whisper tiny as the ears on Windows.
///
/// `cargo test -p snth-connect -- --ignored wake_real_refs --nocapture --test-threads 1`
#[test]
#[ignore]
fn wake_real_refs() {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let clips = PathBuf::from(env("WAKE_E2E_CLIPS").unwrap_or_else(|| "/tmp/wake-e2e".into()));
    let cases = cases(&clips);
    assert!(cases.iter().any(|c| c.expect == "wake") && cases.iter().any(|c| c.expect == "no"), "the refs need clips that wake and clips that do not");
    let models = match env("WAKE_E2E_DOWNLOAD_TO") {
        Some(dir) => {
            let m = download_into(Path::new(&dir), &super::FILES);
            m.path(&TINY).parent().unwrap().to_path_buf()
        }
        None => dirs().0,
    };
    println!("wake files: {}, voice detector {}", super::FILES.iter().map(|f| f.name).collect::<Vec<_>>().join(" + "), VAD.name);
    println!("before the ears: {}", mem_now());
    let t = Instant::now();
    let mut e = ears_in(&models);
    println!("whisper tiny + Silero loaded in {} ms; {}", t.elapsed().as_millis(), mem_now());
    let failed = judge(&mut e, &clips, &cases, "whisper tiny");

    // The decode cost on the wake clips, and the CPU of the listener on
    // speech without the name.
    let mut spy = Spy { inner: &mut e, heard: Vec::new(), vad: Default::default() };
    let names = Names::new("Mia", "");
    let mut talk = Vec::new();
    for c in cases.iter().filter(|c| c.expect == "no") {
        talk.extend(read(&clips, &c.wav));
        talk.extend(room(1.0));
    }
    let audio: Vec<f32> = talk.iter().copied().cycle().take(RATE * 60).collect();
    let mut l = Listener::new();
    let cpu0 = cpu_time();
    let ev = run(&mut l, &mut spy, &names, &audio);
    let cpu = cpu_time() - cpu0;
    let ms: Vec<u128> = spy.heard.iter().map(|(_, ms)| *ms).collect();
    println!(
        "60 s of speech without the name: {:.0} ms CPU = {:.2}% of one core; {} whisper decodes, {:?} ms each; voice detector {} calls in {} ms; {} wakes",
        cpu.as_secs_f32() * 1000.0,
        cpu.as_secs_f32() / 60.0 * 100.0,
        ms.len(),
        ms,
        spy.vad.2,
        spy.vad.0.as_millis(),
        ev.len()
    );
    println!("with whisper tiny loaded and used: {}", mem_now());
    drop(e);

    if let Some(dir) = env("WAKE_E2E_PARAKEET") {
        let dir = PathBuf::from(dir);
        let t = Instant::now();
        let before = mem_now();
        let engine = crate::island::stt::parakeet::Engine::load(
            &dir.join(model::PARAKEET_ENCODER.name),
            &dir.join(model::PARAKEET_JOINT.name),
            &dir.join(model::PARAKEET_VOCAB.name),
            &dir.join(env("WAKE_E2E_PARAKEET_VAD").unwrap_or_else(|| model::PARAKEET_VAD.name.into())),
        )
        .expect("parakeet loads");
        println!("[parakeet] loaded in {} ms; before {before}, after {}", t.elapsed().as_millis(), mem_now());
        let mut p = ParakeetEars { engine, last: None, decodes: Vec::new() };
        let missed = judge(&mut p, &clips, &cases, "parakeet");
        println!("[parakeet] decodes {:?} ms; would fail on {missed:?}; {}", p.decodes, mem_now());
    }
    assert!(failed.is_empty(), "whisper tiny is wrong on {failed:?}");
}
