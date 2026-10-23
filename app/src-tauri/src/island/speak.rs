//! The synth's spoken answer (tz-voice-reply.md): `island.event
//! kind=speech` brings the audio of the answer to a voice message sent
//! from here, and the island plays it on the default output device.
//!
//! Decoding is pure Rust: symphonia for mp3 and WAV (whole, fast: 90 s of
//! mp3 in about 60 ms), the `ogg` pages with `opus-decoder` for Ogg Opus
//! (common: Gemini voices, multi-chunk ElevenLabs). That decoder runs about
//! 15 times faster than real time on Apple silicon (90 s in 4 to 8 s), so
//! Opus is streamed: `prepare` decodes the first half second (the file is
//! checked, the synth gets its answer at once) and the rest is decoded on
//! a thread while it plays. The voice is mono f32, resampled as it comes
//! to the device's rate (linear, enough for a voice) and copied to every
//! channel of the device.
//!
//! Playback is cpal (CoreAudio, WASAPI) on a thread of its own, as the
//! microphone is (the stream is not `Send` on every system). One answer
//! at a time: a new one stops the one that plays. The thread looks at
//! the stream every 50 ms: the level for the face (the loudness of what
//! is being heard), the end, a stop. `ears_off` stays true while it plays
//! and for `TAIL` after it, so wake by name never hears the synth say its
//! own name.

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
/// No progress in this long while there is audio to play: the device is
/// stuck, the answer ends.
const STALL: Duration = Duration::from_secs(5);
/// What `prepare` decodes of a streamed (Opus) answer before it answers.
const HEAD_MS: u64 = 500;

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

/// The rest of a streamed answer: the next piece of the voice (mono, at
/// the answer's rate), None at the end.
pub trait Chunks: Send {
    fn next_chunk(&mut self) -> Option<Vec<f32>>;
}

/// An answer ready to play: its beginning decoded, the rest (Opus) to
/// decode while it plays.
pub struct Voice {
    pub head: Pcm,
    pub rest: Option<Box<dyn Chunks>>,
    /// How long it is, as far as the file says (0: unknown).
    pub ms: u64,
}

/// Check and start decoding one answer. `mime` is one of connect-core's
/// `SPEECH_MIMES` (audio/mpeg, audio/wav, audio/ogg); Err with a short
/// reason when it does not decode.
pub fn prepare(mime: &str, data: &[u8]) -> Result<Voice, String> {
    let voice = match mime {
        "audio/mpeg" | "audio/wav" => {
            let pcm = decode_symphonia(data, if mime == "audio/mpeg" { "mp3" } else { "wav" }, mime)?;
            let ms = pcm.ms();
            Voice { head: pcm, rest: None, ms }
        }
        "audio/ogg" => {
            let mut stream = OpusStream::new(data.to_vec())?;
            let ms = stream.ms_hint();
            let mut head = Vec::new();
            let want = (OpusStream::RATE as u64 * HEAD_MS / 1000) as usize;
            let mut ended = false;
            while head.len() < want {
                match stream.next_chunk() {
                    Some(c) => head.extend(c),
                    None => {
                        ended = true;
                        break;
                    }
                }
            }
            if let Some(e) = stream.failed.take() {
                if head.is_empty() {
                    return Err(e);
                }
            }
            let rest: Option<Box<dyn Chunks>> = if ended { None } else { Some(Box::new(stream)) };
            Voice { head: Pcm { rate: OpusStream::RATE, samples: head }, rest, ms }
        }
        other => return Err(format!("audio type {other} is not played here")),
    };
    if voice.head.samples.is_empty() || voice.head.rate == 0 {
        return Err("no audio in the file".into());
    }
    Ok(voice)
}

/// Decode one answer whole (the tests; playback streams).
#[cfg_attr(not(test), allow(dead_code))]
pub fn decode(mime: &str, data: &[u8]) -> Result<Pcm, String> {
    let mut voice = prepare(mime, data)?;
    if let Some(rest) = voice.rest.as_mut() {
        while let Some(c) = rest.next_chunk() {
            voice.head.samples.extend(c);
        }
    }
    Ok(voice.head)
}

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
/// Ogg Opus (RFC 7845), decoded a page's worth at a time: the head packet
/// (channels, pre-skip), the tags, then the audio packets, at 48 kHz
/// mono.
struct OpusStream {
    reader: ogg::PacketReader<std::io::Cursor<Vec<u8>>>,
    decoder: opus_decoder::OpusDecoder,
    frame: Vec<f32>,
    /// Samples still to drop from the start (the encoder's pre-skip).
    skip: usize,
    pre_skip: u64,
    /// The last page's granule position (48 kHz samples), 0 unknown.
    granule: u64,
    produced: u64,
    packets: usize,
    bad: u32,
    /// Why it stopped early (logged by the caller when nothing came).
    failed: Option<String>,
    done: bool,
}

impl OpusStream {
    const RATE: u32 = 48_000;

    fn new(data: Vec<u8>) -> Result<OpusStream, String> {
        let granule = last_granule(&data);
        let mut reader = ogg::PacketReader::new(std::io::Cursor::new(data));
        let head = reader.read_packet().map_err(|e| format!("not an Ogg file: {e}"))?.ok_or("an empty Ogg file")?;
        if !head.data.starts_with(b"OpusHead") || head.data.len() < 19 {
            return Err("an Ogg file that is not Opus".into());
        }
        let pre_skip = u16::from_le_bytes([head.data[10], head.data[11]]);
        if head.data[18] != 0 {
            // Mapping family 1+ (surround): not a voice.
            return Err("an Opus file with more than two channels".into());
        }
        let decoder = opus_decoder::OpusDecoder::new(Self::RATE, 1).map_err(|e| format!("opus: {e}"))?;
        let frame = vec![0f32; decoder.max_frame_size_per_channel()];
        Ok(OpusStream {
            reader,
            decoder,
            frame,
            skip: usize::from(pre_skip),
            pre_skip: u64::from(pre_skip),
            granule,
            produced: 0,
            packets: 0,
            bad: 0,
            failed: None,
            done: false,
        })
    }

    fn ms_hint(&self) -> u64 {
        self.granule.saturating_sub(self.pre_skip) * 1000 / u64::from(Self::RATE)
    }
}

/// The granule position of the last Ogg page in the file (0: none).
fn last_granule(data: &[u8]) -> u64 {
    let Some(at) = data.windows(4).rposition(|w| w == b"OggS") else { return 0 };
    data.get(at + 6..at + 14).and_then(|b| b.try_into().ok()).map(u64::from_le_bytes).filter(|g| *g != u64::MAX).unwrap_or(0)
}

impl Chunks for OpusStream {
    /// About a tenth of a second (a few packets) per call.
    fn next_chunk(&mut self) -> Option<Vec<f32>> {
        let mut out = Vec::new();
        while !self.done && out.len() < (Self::RATE / 10) as usize {
            let packet = match self.reader.read_packet() {
                Ok(Some(p)) => p,
                Ok(None) => {
                    self.done = true;
                    break;
                }
                Err(e) => {
                    self.failed = Some(format!("reading the Ogg file: {e}"));
                    self.done = true;
                    break;
                }
            };
            self.packets += 1;
            // The tags packet.
            if self.packets == 1 && packet.data.starts_with(b"OpusTags") {
                continue;
            }
            match self.decoder.decode_float(&packet.data, &mut self.frame, false) {
                Ok(n) => {
                    let got = &self.frame[..n.min(self.frame.len())];
                    let drop = self.skip.min(got.len());
                    self.skip -= drop;
                    out.extend_from_slice(&got[drop..]);
                }
                Err(e) => {
                    self.bad += 1;
                    if self.bad > 50 && self.produced == 0 && out.is_empty() {
                        self.failed = Some(format!("the Opus stream does not decode: {e}"));
                        self.done = true;
                    }
                }
            }
            if self.produced + out.len() as u64 >= u64::from(Self::RATE) * u64::from(MAX_SECONDS) {
                out.truncate((u64::from(Self::RATE) * u64::from(MAX_SECONDS)).saturating_sub(self.produced) as usize);
                self.done = true;
            }
        }
        self.produced += out.len() as u64;
        if out.is_empty() && self.done {
            None
        } else {
            Some(out)
        }
    }
}

/// The voice at another rate, a piece at a time (linear interpolation,
/// across the pieces' edges).
pub struct Linear {
    step: f64,
    /// Where the next output sample is, in input samples from the start
    /// of the next piece (-1..0: between the last piece's last sample and
    /// this one's first).
    pos: f64,
    prev: Option<f32>,
}

impl Linear {
    pub fn new(from: u32, to: u32) -> Linear {
        Linear { step: f64::from(from.max(1)) / f64::from(to.max(1)), pos: 0.0, prev: None }
    }

    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }
        let len = input.len() as f64;
        let prev = self.prev.unwrap_or(input[0]);
        let at = |i: isize| if i < 0 { prev } else { input[i as usize] };
        while self.pos <= len - 1.0 {
            let i0 = self.pos.floor();
            let frac = (self.pos - i0) as f32;
            let i0 = i0 as isize;
            let a = at(i0);
            let b = if (i0 + 1) as f64 <= len - 1.0 { at(i0 + 1) } else { a };
            out.push(a + (b - a) * frac);
            self.pos += self.step;
        }
        self.prev = Some(input[input.len() - 1]);
        self.pos -= len;
    }
}

/// A whole voice at another rate.
#[cfg_attr(not(test), allow(dead_code))]
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || from == 0 || to == 0 {
        return input.to_vec();
    }
    let mut out = Vec::with_capacity((input.len() as f64 * f64::from(to) / f64::from(from)) as usize + 1);
    Linear::new(from, to).push(input, &mut out);
    out
}

/// The face's level for every `TICK` of the voice (as the microphone's).
#[cfg_attr(not(test), allow(dead_code))]
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
    pub voice: Voice,
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
            let Job { voice, on_level, on_end } = job;
            let end = if me.current(id) { output::run(voice, &stop, on_level.as_ref()) } else { End::Stopped("stopped before it started".into()) };
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::Instant;

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, SampleFormat, SizedSample};

    use super::{End, Linear, Voice, MAX_SECONDS, STALL, TICK};

    /// The voice at the device's rate as it is decoded, and how far the
    /// device has played it.
    struct Shared {
        buf: Mutex<Vec<f32>>,
        /// Everything is decoded (the buffer will not grow).
        complete: AtomicBool,
        /// The answer was stopped: the decoding thread ends.
        cancel: AtomicBool,
        pos: AtomicUsize,
        error: Mutex<Option<String>>,
    }

    fn build<T>(device: &cpal::Device, config: cpal::StreamConfig, sh: Arc<Shared>) -> Result<cpal::Stream, String>
    where
        T: SizedSample + FromSample<f32> + Send + 'static,
    {
        let channels = usize::from(config.channels).max(1);
        let errors = sh.clone();
        device
            .build_output_stream(
                config,
                move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                    let buf = sh.buf.lock().unwrap();
                    let mut at = sh.pos.load(Ordering::Relaxed);
                    for frame in data.chunks_mut(channels) {
                        // Not decoded yet: silence, the place kept.
                        let v = match buf.get(at) {
                            Some(v) => {
                                at += 1;
                                *v
                            }
                            None => 0.0,
                        };
                        for s in frame.iter_mut() {
                            *s = T::from_sample(v);
                        }
                    }
                    sh.pos.store(at, Ordering::Relaxed);
                },
                move |e: cpal::Error| {
                    let mut slot = errors.error.lock().unwrap();
                    if slot.is_none() {
                        *slot = Some(format!("{e} ({:?})", e.kind()));
                    }
                },
                None,
            )
            .map_err(|e| e.to_string())
    }

    /// Play to the end, a stop or a failure (blocking: the playing thread).
    pub fn run(voice: Voice, stop: &mpsc::Receiver<String>, on_level: &(dyn Fn(f32) + Send + Sync)) -> End {
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
        let rate = config.sample_rate;
        let Voice { head, rest, ms } = voice;
        let mut linear = Linear::new(head.rate, rate);
        // Room for the whole voice, so the decoding thread never moves the
        // buffer under the device's callback.
        let expect = ms.max(head.ms()).min(u64::from(MAX_SECONDS) * 1000) as usize * rate as usize / 1000;
        let mut first = Vec::with_capacity(expect + rate as usize);
        linear.push(&head.samples, &mut first);
        let sh = Arc::new(Shared {
            buf: Mutex::new(first),
            complete: AtomicBool::new(rest.is_none()),
            cancel: AtomicBool::new(false),
            pos: AtomicUsize::new(0),
            error: Mutex::new(None),
        });
        if let Some(mut rest) = rest {
            let dec = sh.clone();
            let spawned = std::thread::Builder::new().name("island-speech-decode".into()).spawn(move || {
                let mut piece = Vec::new();
                while !dec.cancel.load(Ordering::Relaxed) {
                    let Some(chunk) = rest.next_chunk() else { break };
                    piece.clear();
                    linear.push(&chunk, &mut piece);
                    dec.buf.lock().unwrap().extend_from_slice(&piece);
                }
                dec.complete.store(true, Ordering::Relaxed);
            });
            if spawned.is_err() {
                sh.complete.store(true, Ordering::Relaxed);
            }
        }
        let built = match format {
            SampleFormat::F32 => build::<f32>(&device, config, sh.clone()),
            SampleFormat::I16 => build::<i16>(&device, config, sh.clone()),
            SampleFormat::I32 => build::<i32>(&device, config, sh.clone()),
            SampleFormat::U16 => build::<u16>(&device, config, sh.clone()),
            SampleFormat::U8 => build::<u8>(&device, config, sh.clone()),
            SampleFormat::I8 => build::<i8>(&device, config, sh.clone()),
            SampleFormat::F64 => build::<f64>(&device, config, sh.clone()),
            other => Err(format!("sample format {other} is not handled")),
        };
        let stream = match built.and_then(|s| s.play().map(|_| s).map_err(|e| e.to_string())) {
            Ok(s) => s,
            Err(e) => {
                sh.cancel.store(true, Ordering::Relaxed);
                return End::Failed(format!("the output did not start: {e}"));
            }
        };
        let block = (rate as usize * TICK.as_millis() as usize / 1000).max(1);
        let mut handed = false;
        let mut last = (0usize, Instant::now());
        let end = loop {
            match stop.recv_timeout(TICK) {
                Ok(why) => break End::Stopped(why),
                Err(mpsc::RecvTimeoutError::Disconnected) => break End::Stopped("the speaker was dropped".into()),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            let at = sh.pos.load(Ordering::Relaxed);
            if let Some(e) = sh.error.lock().unwrap().take() {
                // A glitch keeps playing; a device that went away does not
                // move any more (the stall below ends it).
                connect_core::logln!("island: speech: the output reported an error: {e}");
            }
            let (len, level) = {
                let buf = sh.buf.lock().unwrap();
                (buf.len(), super::super::audio::level(&buf[at.saturating_sub(block).min(buf.len())..at.min(buf.len())]))
            };
            on_level(level);
            let complete = sh.complete.load(Ordering::Relaxed);
            if complete && at >= len {
                // Handed to the device: one more tick for its buffer.
                if handed {
                    break End::Finished;
                }
                handed = true;
            }
            // Waiting for the decoder is not a stall.
            if at != last.0 || at >= len {
                last = (at, Instant::now());
            } else if last.1.elapsed() > STALL {
                break End::Failed(format!("the output stopped moving at {at} of {len} samples"));
            }
        };
        sh.cancel.store(true, Ordering::Relaxed);
        drop(stream);
        end
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod output {
    use std::sync::mpsc;

    use super::{End, Voice};

    pub fn run(_voice: Voice, _stop: &mpsc::Receiver<String>, _on_level: &(dyn Fn(f32) + Send + Sync)) -> End {
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

    /// A longer Opus answer (1.5 s, `-b:a 12k tone15.ogg`): the first half
    /// second is decoded before the answer goes back, the rest streams,
    /// and the pieces add up to the whole.
    const OGG_LONG: &[u8] = include_bytes!("testdata/tone15.ogg");

    #[test]
    fn a_long_opus_answer_starts_with_its_head_and_streams_the_rest() {
        let mut voice = prepare("audio/ogg", OGG_LONG).unwrap();
        assert!((1400..=1600).contains(&voice.ms), "the file says {} ms", voice.ms);
        let head = voice.head.ms();
        assert!((500..900).contains(&head), "head {head} ms");
        let mut rest = voice.rest.take().expect("the rest streams");
        let mut all = voice.head.samples.clone();
        while let Some(c) = rest.next_chunk() {
            all.extend(c);
        }
        let whole = decode("audio/ogg", OGG_LONG).unwrap();
        assert_eq!(all.len(), whole.samples.len());
        assert!((1400..=1600).contains(&whole.ms()), "{} ms", whole.ms());
        // A short one is decoded whole up front.
        assert!(prepare("audio/ogg", OGG).unwrap().rest.is_none());
        assert!(prepare("audio/mpeg", MP3).unwrap().rest.is_none());
    }

    #[test]
    fn resampling_in_pieces_is_resampling_whole() {
        let v: Vec<f32> = (0..24_000).map(|i| (i as f32 * 0.05).sin()).collect();
        for (from, to) in [(48_000, 44_100), (24_000, 48_000), (16_000, 48_000), (48_000, 48_000)] {
            let whole = resample(&v, from, to);
            let mut lin = Linear::new(from, to);
            let mut pieces = Vec::new();
            for c in v.chunks(997) {
                lin.push(c, &mut pieces);
            }
            assert!(pieces.len().abs_diff(whole.len()) <= 1, "{from}->{to}: {} vs {}", pieces.len(), whole.len());
            let n = pieces.len().min(whole.len());
            let worst = (0..n).map(|i| (pieces[i] - whole[i]).abs()).fold(0f32, f32::max);
            assert!(worst < 1e-4, "{from}->{to}: {worst}");
        }
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
        // Within a sample or two of the rate (the last fraction of a
        // step is not interpolated past the end).
        assert!(up.len().abs_diff(48_000) <= 3, "{}", up.len());
        let down = resample(&one_s, 16_000, 8_000);
        assert!(down.len().abs_diff(8_000) <= 1, "{}", down.len());
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

    /// The real output device, when there is one: the Opus fixture (the
    /// streamed path) played at a tenth of its loudness to the end. Run by hand (and on the Windows
    /// CI machine, which may have no output: then it says so and passes):
    /// `cargo test -p snth-connect speak_real_device -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn speak_real_device() {
        // SPEAK_FILE (an Ogg Opus file, played as it is) tries the
        // streamed path on a longer answer.
        let file = std::env::var("SPEAK_FILE").ok().map(|p| std::fs::read(p).unwrap());
        let data = file.as_deref().unwrap_or(OGG);
        let mut voice = prepare("audio/ogg", data).unwrap();
        if file.is_none() {
            voice.head.samples.iter_mut().for_each(|v| *v *= 0.1);
        }
        let ms = decode("audio/ogg", data).unwrap().ms();
        let sp = Arc::new(Speaker::default());
        let (id, rx) = sp.begin();
        let levels = Arc::new(Mutex::new(Vec::new()));
        let seen = levels.clone();
        let (tx, ended) = mpsc::channel();
        sp.play(
            id,
            rx,
            Job {
                voice,
                on_level: Arc::new(move |l| seen.lock().unwrap().push(l)),
                on_end: Box::new(move |end, took| {
                    let _ = tx.send((end, took));
                }),
            },
        );
        assert!(sp.ears_off());
        let (end, took) = ended.recv_timeout(Duration::from_secs(10) + Duration::from_millis(ms)).unwrap();
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

    /// How long a long answer takes to decode (the synth waits for the
    /// app's answer, which comes after the decode): `SPEAK_FILE=<path>
    /// SPEAK_MIME=audio/ogg cargo test -p snth-connect --release
    /// speak_decode_speed -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn speak_decode_speed() {
        let (Ok(path), Ok(mime)) = (std::env::var("SPEAK_FILE"), std::env::var("SPEAK_MIME")) else {
            println!("speak_decode_speed: set SPEAK_FILE and SPEAK_MIME");
            return;
        };
        let data = std::fs::read(&path).unwrap();
        let t = Instant::now();
        let voice = prepare(&mime, &data).unwrap();
        let ready = t.elapsed().as_millis();
        let pcm = decode(&mime, &data).unwrap();
        println!(
            "speak_decode_speed: {} bytes of {mime} ({} ms said by the file) -> {} ms of audio, ready to play in {ready} ms, whole in {} ms",
            data.len(),
            voice.ms,
            pcm.ms(),
            t.elapsed().as_millis() - ready
        );
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
                voice: Voice { head: Pcm { rate: 16_000, samples: vec![0.1; 16_000] }, rest: None, ms: 1000 },
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

