//! The synth's spoken answer (tz-voice-reply.md): `island.event
//! kind=speech` brings the audio of the answer to a voice message sent
//! from here, and the island plays it on the default output device.
//!
//! Decoding (`decode`) is pure Rust: symphonia for mp3 (what the synth
//! sends) and WAV, the `ogg` pages with `opus-decoder` for Ogg Opus. The
//! result is mono f32 at the file's rate; `play` resamples it once to the
//! device's rate (linear, enough for a voice) and copies it to every
//! channel of the device.
//!
//! Playback is cpal (CoreAudio, WASAPI) on a thread of its own, as the
//! microphone is (the stream is not `Send` on every system). One answer
//! at a time: a new one stops the one that plays. The thread looks at
//! the stream every 50 ms: the level for the face (the loudness of what
//! is being heard, precomputed per 50 ms), the end, a stop. `ears_off`
//! stays true while it plays and for `TAIL` after it, so wake by name
//! never hears the synth say its own name.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// After the answer ends (or is stopped) wake by name stays deaf this
/// long: the room's echo, the device's own latency.
pub const TAIL: Duration = Duration::from_millis(300);
/// The longest answer played (the synth keeps to about 90 s); the rest
/// of a longer file is cut.
pub const MAX_SECONDS: u32 = 240;
/// How often the playing thread looks at the stream (the face's level).
const TICK: Duration = Duration::from_millis(50);
/// No progress in this long: the device is stuck, the answer ends.
const STALL: Duration = Duration::from_secs(5);

/// Decoded audio: mono, `rate` samples a second, -1..1.
#[derive(Debug, Clone, PartialEq)]
pub struct Pcm {
    pub rate: u32,
    pub samples: Vec<f32>,
}

impl Pcm {
    pub fn ms(&self) -> u64 {
        self.samples.len() as u64 * 1000 / u64::from(self.rate.max(1))
    }
}

/// Decode one answer. `mime` is one of connect-core's `SPEECH_MIMES`
/// (audio/mpeg, audio/wav, audio/ogg); Err with a short reason.
pub fn decode(mime: &str, data: &[u8]) -> Result<Pcm, String> {
    let pcm = match mime {
        "audio/mpeg" => decode_symphonia(data, "mp3", mime)?,
        "audio/wav" => decode_symphonia(data, "wav", mime)?,
        "audio/ogg" => decode_ogg_opus(data)?,
        other => return Err(format!("audio type {other} is not played here")),
    };
    if pcm.samples.is_empty() || pcm.rate == 0 {
        return Err("no audio in the file".into());
    }
    Ok(pcm)
}

/// Downmix interleaved frames into `out`.
fn push_mono(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    let channels = channels.max(1);
    for frame in interleaved.chunks(channels) {
        out.push(frame.iter().sum::<f32>() / frame.len() as f32);
    }
}

fn decode_symphonia(data: &[u8], ext: &str, mime: &str) -> Result<Pcm, String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
    use symphonia::core::errors::Error;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(data.to_vec())), Default::default());
    let mut hint = Hint::new();
    hint.with_extension(ext);
    hint.mime_type(mime);
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("not a {ext} file: {e}"))?;
    let mut format = probed.format;
    let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).ok_or("no audio track")?;
    let track_id = track.id;
    let mut rate = track.codec_params.sample_rate.unwrap_or(0);
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("no decoder for the {ext} track: {e}"))?;
    let mut samples = Vec::new();
    let mut bad = 0u32;
    let mut buf: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(Error::ResetRequired) => break,
            Err(e) => {
                // A broken tail: keep what was decoded.
                if samples.is_empty() {
                    return Err(format!("reading the {ext} file: {e}"));
                }
                break;
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                rate = spec.rate;
                let need = decoded.capacity() as u64;
                let b = match &mut buf {
                    Some(b) if b.capacity() as u64 >= need * spec.channels.count() as u64 => b,
                    _ => buf.insert(SampleBuffer::<f32>::new(need, spec)),
                };
                b.copy_interleaved_ref(decoded);
                push_mono(b.samples(), spec.channels.count(), &mut samples);
                if rate > 0 && samples.len() as u64 >= u64::from(rate) * u64::from(MAX_SECONDS) {
                    samples.truncate((rate * MAX_SECONDS) as usize);
                    break;
                }
            }
            // A damaged frame: skipped, as players do.
            Err(Error::DecodeError(_)) => {
                bad += 1;
                if bad > 50 && samples.is_empty() {
                    return Err(format!("the {ext} file does not decode"));
                }
            }
            Err(Error::IoError(_)) => break,
            Err(e) => return Err(format!("decoding the {ext} file: {e}")),
        }
    }
    Ok(Pcm { rate, samples })
}

/// Ogg Opus (RFC 7845): the head packet (channels, pre-skip), the tags,
/// then the audio packets, decoded at 48 kHz mono.
fn decode_ogg_opus(data: &[u8]) -> Result<Pcm, String> {
    const RATE: u32 = 48_000;
    let mut reader = ogg::PacketReader::new(std::io::Cursor::new(data));
    let head = reader.read_packet().map_err(|e| format!("not an Ogg file: {e}"))?.ok_or("an empty Ogg file")?;
    if !head.data.starts_with(b"OpusHead") || head.data.len() < 19 {
        return Err("an Ogg file that is not Opus".into());
    }
    let pre_skip = usize::from(u16::from_le_bytes([head.data[10], head.data[11]]));
    if head.data[18] != 0 {
        // Mapping family 1+ (surround): not a voice.
        return Err("an Opus file with more than two channels".into());
    }
    let mut decoder = opus_decoder::OpusDecoder::new(RATE, 1).map_err(|e| format!("opus: {e}"))?;
    let mut frame = vec![0f32; decoder.max_frame_size_per_channel()];
    let mut samples: Vec<f32> = Vec::new();
    let mut packets = 0usize;
    let mut bad = 0u32;
    loop {
        let packet = match reader.read_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(e) => {
                if samples.is_empty() {
                    return Err(format!("reading the Ogg file: {e}"));
                }
                break;
            }
        };
        packets += 1;
        // The tags packet.
        if packets == 1 && packet.data.starts_with(b"OpusTags") {
            continue;
        }
        match decoder.decode_float(&packet.data, &mut frame, false) {
            Ok(n) => samples.extend_from_slice(&frame[..n.min(frame.len())]),
            Err(_) => {
                bad += 1;
                if bad > 50 && samples.is_empty() {
                    return Err("the Opus stream does not decode".into());
                }
            }
        }
        if samples.len() as u64 >= u64::from(RATE) * u64::from(MAX_SECONDS) {
            samples.truncate((RATE * MAX_SECONDS) as usize);
            break;
        }
    }
    samples.drain(..pre_skip.min(samples.len()));
    Ok(Pcm { rate: RATE, samples })
}

/// The voice at another rate (linear interpolation).
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() || from == 0 || to == 0 {
        return input.to_vec();
    }
    let step = f64::from(from) / f64::from(to);
    let n = ((input.len() as f64) / step).floor() as usize;
    let last = input.len() - 1;
    (0..n)
        .map(|i| {
            let pos = i as f64 * step;
            let i0 = (pos.floor() as usize).min(last);
            let i1 = (i0 + 1).min(last);
            let frac = (pos - i0 as f64) as f32;
            input[i0] + (input[i1] - input[i0]) * frac
        })
        .collect()
}

/// The face's level for every `TICK` of the voice (as the microphone's).
pub fn envelope(samples: &[f32], rate: u32) -> Vec<f32> {
    let block = (rate as usize * TICK.as_millis() as usize / 1000).max(1);
    samples.chunks(block).map(super::audio::level).collect()
}

/// How an answer ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum End {
    /// Played to the end.
    Finished,
    /// Stopped: by the owner (a key, a wake, a click), by a newer answer,
    /// by the app's pause.
    Stopped(String),
    /// The device failed (or never started).
    Failed(String),
}

struct Playing {
    id: u64,
    stop: mpsc::Sender<String>,
}

#[derive(Default)]
struct State {
    seq: u64,
    playing: Option<Playing>,
    /// When the last answer ended (the tail counts from here).
    ended: Option<Instant>,
}

/// The one speaker of the island.
#[derive(Default)]
pub struct Speaker {
    state: Mutex<State>,
}

/// One answer, ready to go out.
pub struct Job {
    pub pcm: Pcm,
    /// The face's level, every `TICK` (0 at the end).
    pub on_level: Arc<dyn Fn(f32) + Send + Sync>,
    /// Called once, from the playing thread, at the end.
    pub on_end: Box<dyn FnOnce(End, Duration) + Send>,
}

impl Speaker {
    /// An answer plays right now.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn speaking(&self) -> bool {
        self.state.lock().unwrap().playing.is_some()
    }

    /// Wake by name must not listen: an answer plays, or ended less than
    /// `TAIL` ago.
    pub fn ears_off(&self) -> bool {
        let s = self.state.lock().unwrap();
        s.playing.is_some() || s.ended.is_some_and(|t| t.elapsed() < TAIL)
    }

    /// Stop the answer that plays (if any). True when one did.
    pub fn stop(&self, why: &str) -> bool {
        let mut s = self.state.lock().unwrap();
        match s.playing.take() {
            Some(p) => {
                let _ = p.stop.send(why.to_string());
                s.ended = Some(Instant::now());
                true
            }
            None => false,
        }
    }

    /// Mark the start of an answer (wake by name goes deaf now) and get
    /// its number; `play` with that number starts it. A newer answer stops
    /// this one.
    pub fn begin(&self) -> (u64, mpsc::Receiver<String>) {
        let (tx, rx) = mpsc::channel();
        let mut s = self.state.lock().unwrap();
        if let Some(old) = s.playing.take() {
            let _ = old.stop.send("replaced by a newer answer".into());
        }
        s.seq += 1;
        let id = s.seq;
        s.playing = Some(Playing { id, stop: tx });
        (id, rx)
    }

    /// The answer `id` is over (the playing thread, or a start that failed).
    pub fn finished(&self, id: u64) {
        let mut s = self.state.lock().unwrap();
        if s.playing.as_ref().is_some_and(|p| p.id == id) {
            s.playing = None;
            s.ended = Some(Instant::now());
        }
    }

    /// Is `id` still the answer that plays (not stopped, not replaced)?
    pub fn current(&self, id: u64) -> bool {
        self.state.lock().unwrap().playing.as_ref().is_some_and(|p| p.id == id)
    }

    /// Play `job` as answer `id` (from `begin`) on the default output, on
    /// a thread of its own. Returns at once; `on_end` says how it ended.
    pub fn play(self: &Arc<Self>, id: u64, stop: mpsc::Receiver<String>, job: Job) {
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("island-speaker".into()).spawn(move || {
            let started = Instant::now();
            let Job { pcm, on_level, on_end } = job;
            let end = if me.current(id) { output::run(pcm, &stop, on_level.as_ref()) } else { End::Stopped("stopped before it started".into()) };
            on_level(0.0);
            me.finished(id);
            on_end(end, started.elapsed());
        });
        if let Err(e) = spawned {
            self.finished(id);
            connect_core::logln!("island: speech: the speaker thread did not start: {e}");
        }
    }
}

#[cfg(any(target_os = "macos", windows))]
mod output {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::Instant;

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, SampleFormat, SizedSample};

    use super::{envelope, resample, End, Pcm, STALL, TICK};

    fn build<T>(device: &cpal::Device, config: cpal::StreamConfig, voice: Arc<Vec<f32>>, pos: Arc<AtomicUsize>, error: Arc<Mutex<Option<String>>>) -> Result<cpal::Stream, String>
    where
        T: SizedSample + FromSample<f32> + Send + 'static,
    {
        let channels = usize::from(config.channels).max(1);
        device
            .build_output_stream(
                config,
                move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                    let mut at = pos.load(Ordering::Relaxed);
                    for frame in data.chunks_mut(channels) {
                        let v = voice.get(at).copied().unwrap_or(0.0);
                        if at < voice.len() {
                            at += 1;
                        }
                        for s in frame.iter_mut() {
                            *s = T::from_sample(v);
                        }
                    }
                    pos.store(at, Ordering::Relaxed);
                },
                move |e: cpal::Error| {
                    let mut slot = error.lock().unwrap();
                    if slot.is_none() {
                        *slot = Some(format!("{e} ({:?})", e.kind()));
                    }
                },
                None,
            )
            .map_err(|e| e.to_string())
    }

    /// Play to the end, a stop or a failure (blocking: the playing thread).
    pub fn run(pcm: Pcm, stop: &mpsc::Receiver<String>, on_level: &(dyn Fn(f32) + Send + Sync)) -> End {
        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else {
            return End::Failed("no output device".into());
        };
        let supported = match device.default_output_config() {
            Ok(c) => c,
            Err(e) => return End::Failed(format!("the output device: {e}")),
        };
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let voice = Arc::new(resample(&pcm.samples, pcm.rate, config.sample_rate));
        let levels = envelope(&voice, config.sample_rate);
        let block = (config.sample_rate as usize * TICK.as_millis() as usize / 1000).max(1);
        let pos = Arc::new(AtomicUsize::new(0));
        let error = Arc::new(Mutex::new(None::<String>));
        let built = match format {
            SampleFormat::F32 => build::<f32>(&device, config, voice.clone(), pos.clone(), error.clone()),
            SampleFormat::I16 => build::<i16>(&device, config, voice.clone(), pos.clone(), error.clone()),
            SampleFormat::I32 => build::<i32>(&device, config, voice.clone(), pos.clone(), error.clone()),
            SampleFormat::U16 => build::<u16>(&device, config, voice.clone(), pos.clone(), error.clone()),
            SampleFormat::U8 => build::<u8>(&device, config, voice.clone(), pos.clone(), error.clone()),
            SampleFormat::I8 => build::<i8>(&device, config, voice.clone(), pos.clone(), error.clone()),
            SampleFormat::F64 => build::<f64>(&device, config, voice.clone(), pos.clone(), error.clone()),
            other => Err(format!("sample format {other} is not handled")),
        };
        let stream = match built.and_then(|s| s.play().map(|_| s).map_err(|e| e.to_string())) {
            Ok(s) => s,
            Err(e) => return End::Failed(format!("the output did not start: {e}")),
        };
        let mut handed = false;
        let mut last = (0usize, Instant::now());
        let end = loop {
            match stop.recv_timeout(TICK) {
                Ok(why) => break End::Stopped(why),
                Err(mpsc::RecvTimeoutError::Disconnected) => break End::Stopped("the speaker was dropped".into()),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            let at = pos.load(Ordering::Relaxed);
            if let Some(e) = error.lock().unwrap().take() {
                // A glitch keeps playing; a device that went away does not
                // move any more (the stall below ends it).
                connect_core::logln!("island: speech: the output reported an error: {e}");
            }
            on_level(levels.get(at / block).copied().unwrap_or(0.0));
            if at >= voice.len() {
                // Handed to the device: one more tick for its buffer.
                if handed {
                    break End::Finished;
                }
                handed = true;
            }
            if at != last.0 {
                last = (at, Instant::now());
            } else if last.1.elapsed() > STALL {
                break End::Failed(format!("the output stopped moving at {} of {} samples", at, voice.len()));
            }
        };
        drop(stream);
        end
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod output {
    use std::sync::mpsc;

    use super::{End, Pcm};

    pub fn run(_pcm: Pcm, _stop: &mpsc::Receiver<String>, _on_level: &(dyn Fn(f32) + Send + Sync)) -> End {
        End::Failed("not on this system".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny mp3 (0.3 s of a 440 Hz tone, 16 kHz mono, made with
    /// `ffmpeg -f lavfi -i sine=frequency=440:duration=0.3 -ar 16000 -ac 1
    /// -b:a 24k tone.mp3`).
    const MP3: &[u8] = include_bytes!("testdata/tone.mp3");
    /// The same tone as Ogg Opus (`-c:a libopus -b:a 24k tone.ogg`).
    const OGG: &[u8] = include_bytes!("testdata/tone.ogg");

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|v| v * v).sum::<f32>() / s.len().max(1) as f32).sqrt()
    }

    #[test]
    fn an_mp3_answer_decodes_to_the_voice_it_holds() {
        let pcm = decode("audio/mpeg", MP3).unwrap();
        assert_eq!(pcm.rate, 16_000);
        let ms = pcm.ms();
        assert!((250..=450).contains(&ms), "{ms} ms");
        // A tone, not silence.
        assert!(rms(&pcm.samples) > 0.05, "rms {}", rms(&pcm.samples));
        assert!(pcm.samples.iter().all(|v| v.is_finite() && v.abs() <= 1.01));
    }

    #[test]
    fn an_ogg_opus_answer_decodes_at_48_khz() {
        let pcm = decode("audio/ogg", OGG).unwrap();
        assert_eq!(pcm.rate, 48_000);
        let ms = pcm.ms();
        assert!((250..=450).contains(&ms), "{ms} ms");
        assert!(rms(&pcm.samples) > 0.05, "rms {}", rms(&pcm.samples));
    }

    #[test]
    fn a_wav_answer_decodes() {
        let samples: Vec<i16> = (0..2400).map(|i| ((i as f32 * 0.1).sin() * 12_000.0) as i16).collect();
        let wav = super::super::audio::wav(&samples, 24_000);
        let pcm = decode("audio/wav", &wav).unwrap();
        assert_eq!(pcm.rate, 24_000);
        assert_eq!(pcm.samples.len(), 2400);
        assert!((pcm.samples[10] - samples[10] as f32 / 32768.0).abs() < 1e-3);
    }

    #[test]
    fn what_does_not_decode_says_why() {
        assert!(decode("audio/mpeg", b"").is_err());
        assert!(decode("audio/ogg", b"not an ogg file at all").is_err());
        assert!(decode("audio/ogg", MP3).is_err());
        assert!(decode("audio/aac", MP3).unwrap_err().contains("not played"));
        let garbage: Vec<u8> = (0..4000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        assert!(decode("audio/wav", &garbage).is_err());
    }

    #[test]
    fn the_voice_is_resampled_to_the_device_rate() {
        let one_s: Vec<f32> = (0..16_000).map(|i| (i as f32 / 16_000.0 * 440.0 * std::f32::consts::TAU).sin() * 0.5).collect();
        let up = resample(&one_s, 16_000, 48_000);
        assert_eq!(up.len(), 48_000);
        let down = resample(&one_s, 16_000, 8_000);
        assert_eq!(down.len(), 8_000);
        assert!((rms(&up) - rms(&one_s)).abs() < 0.02);
        assert_eq!(resample(&one_s, 16_000, 16_000), one_s);
        assert!(resample(&[], 16_000, 48_000).is_empty());
    }

    #[test]
    fn the_face_follows_the_voice() {
        let mut v = vec![0f32; 4800];
        v.extend((0..4800).map(|i| (i as f32 * 0.2).sin() * 0.5));
        let env = envelope(&v, 48_000);
        assert_eq!(env.len(), 4, "a level every 50 ms");
        assert!(env[0] < 0.05 && env[3] > 0.5, "{env:?}");
    }

    /// The playing thread's bookkeeping, without a device: a new answer
    /// stops the old one, a stop ends it, and the ears stay off for the
    /// tail after the end.
    #[test]
    fn one_answer_at_a_time_and_the_ears_stay_off_for_the_tail() {
        let sp = Arc::new(Speaker::default());
        assert!(!sp.speaking() && !sp.ears_off());
        let (a, rx_a) = sp.begin();
        assert!(sp.speaking() && sp.ears_off() && sp.current(a));
        let (b, rx_b) = sp.begin();
        assert_eq!(rx_a.try_recv().unwrap(), "replaced by a newer answer");
        assert!(!sp.current(a) && sp.current(b));
        // The old thread ending does not end the new answer.
        sp.finished(a);
        assert!(sp.speaking());
        // Barge-in.
        assert!(sp.stop("barge-in: a key"));
        assert_eq!(rx_b.try_recv().unwrap(), "barge-in: a key");
        assert!(!sp.speaking());
        assert!(sp.ears_off(), "the tail");
        assert!(!sp.stop("again"), "nothing plays");
        std::thread::sleep(TAIL + Duration::from_millis(50));
        assert!(!sp.ears_off(), "the ears come back after the tail");
    }

    /// The real output device, when there is one: the fixture played at a
    /// tenth of its loudness to the end. Run by hand (and on the Windows
    /// CI machine, which may have no output: then it says so and passes):
    /// `cargo test -p snth-connect speak_real_device -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn speak_real_device() {
        let mut pcm = decode("audio/mpeg", MP3).unwrap();
        pcm.samples.iter_mut().for_each(|v| *v *= 0.1);
        let ms = pcm.ms();
        let sp = Arc::new(Speaker::default());
        let (id, rx) = sp.begin();
        let levels = Arc::new(Mutex::new(Vec::new()));
        let seen = levels.clone();
        let (tx, ended) = mpsc::channel();
        sp.play(
            id,
            rx,
            Job {
                pcm,
                on_level: Arc::new(move |l| seen.lock().unwrap().push(l)),
                on_end: Box::new(move |end, took| {
                    let _ = tx.send((end, took));
                }),
            },
        );
        assert!(sp.ears_off());
        let (end, took) = ended.recv_timeout(Duration::from_secs(10)).unwrap();
        println!("speak_real_device: {end:?} after {} ms ({ms} ms of audio), {} levels", took.as_millis(), levels.lock().unwrap().len());
        match end {
            End::Finished => {
                assert!(took.as_millis() as u64 >= ms, "played faster than the audio is long");
                assert!(levels.lock().unwrap().iter().any(|l| *l > 0.0), "the face heard nothing");
            }
            End::Failed(e) if e.contains("no output device") => println!("speak_real_device: no output device here"),
            other => panic!("{other:?}"),
        }
        assert!(!sp.speaking());
    }

    /// A stop that comes before the thread got to the device ends the
    /// answer without opening it.
    #[test]
    fn a_stopped_answer_never_reaches_the_device() {
        let sp = Arc::new(Speaker::default());
        let (id, rx) = sp.begin();
        sp.stop("barge-in: wake");
        let (tx, ended) = mpsc::channel();
        sp.play(
            id,
            rx,
            Job {
                pcm: Pcm { rate: 16_000, samples: vec![0.1; 16_000] },
                on_level: Arc::new(|_| {}),
                on_end: Box::new(move |end, _| {
                    let _ = tx.send(end);
                }),
            },
        );
        let end = ended.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(end, End::Stopped(_)), "{end:?}");
        assert!(!sp.speaking());
    }
}
