//! The island's microphone: record while a key is held, hand back a voice
//! note the synth takes like a Telegram voice message.
//!
//! The format is WAV, 16 kHz, mono, 16-bit. The synth's voice path is
//! Gemini speech-to-text (docs/media-ingestion.md in the synth's repo),
//! which takes audio/wav as it is and works at 16 kHz anyway; the
//! protocol lists audio/wav next to ogg and mp4. An Opus or AAC encoder
//! would shrink two minutes from 3.8 MB to a few hundred KB, but needs a C
//! library (libopus) or a system framework per OS in the build, and 3.8 MB
//! (5.1 MB of base64) is well inside the 8 MB request. So: plain PCM, one
//! code path on macOS and Windows, testable byte for byte.
//!
//! Capture is cpal (CoreAudio, WASAPI) on a thread of its own: the stream
//! is not `Send` on every system, and nothing else should wait for it. The
//! callback downmixes to mono and resamples to 16 kHz as it goes, so two
//! minutes hold 3.8 MB, not the device's 48 kHz floats. The level for the
//! face (0..1) comes out every 50 ms.
//!
//! macOS asks the owner once ("SNTH Connect would like to access the
//! microphone", `NSMicrophoneUsageDescription` in Info.plist, the
//! `com.apple.security.device.audio-input` entitlement under the hardened
//! runtime). A recording is only ever started after the permission is
//! known to be granted: an app without it gets silence, not an error.

use std::time::Duration;

/// The rate the synth gets.
pub const RATE: u32 = 16_000;

/// What macOS (or Windows) says about the microphone for this app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MicPermission {
    Granted,
    Denied,
    NotAsked,
    Unknown,
}

/// A finished recording.
pub struct Captured {
    pub wav: Vec<u8>,
    pub ms: u64,
    /// The samples in the WAV (16 kHz mono), for on-device speech-to-text.
    pub pcm: Vec<i16>,
}

/// One WAV file: RIFF header + 16-bit little-endian PCM, mono.
pub fn wav(samples: &[i16], rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // PCM header size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes()); // bytes per second
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Turns the device's mono stream (any rate) into 16 kHz, a block at a
/// time. Each output sample is the mean of the input samples that fall in
/// its slot (a box low-pass, enough for speech going to speech-to-text),
/// interpolated when the device is slower than 16 kHz.
pub struct Resampler {
    step: f64,
    /// Where the next output sample is, in input samples from the start of
    /// the next block.
    pos: f64,
    acc: f64,
    n: u32,
    last: f32,
}

impl Resampler {
    pub fn new(in_rate: u32) -> Resampler {
        Resampler { step: f64::from(in_rate.max(1)) / f64::from(RATE), pos: 0.0, acc: 0.0, n: 0, last: 0.0 }
    }

    pub fn push(&mut self, input: &[f32], out: &mut Vec<i16>) {
        if input.is_empty() {
            return;
        }
        let len = input.len() as f64;
        if self.step <= 1.0 {
            // Upsampling (a device slower than 16 kHz): linear
            // interpolation. `pos` may be -1..0: between the last sample
            // of the previous block and the first of this one.
            let last = self.last;
            let at = |i: isize| if i < 0 { last } else { input[i as usize] };
            while self.pos < len - 1.0 {
                let i0 = self.pos.floor();
                let frac = (self.pos - i0) as f32;
                let (a, b) = (at(i0 as isize), at(i0 as isize + 1));
                out.push(to_i16(a + (b - a) * frac));
                self.pos += self.step;
            }
            self.last = input[input.len() - 1];
            self.pos -= len;
            return;
        }
        // Downsampling: the mean of the input samples in each slot.
        for (i, s) in input.iter().enumerate() {
            self.acc += f64::from(*s);
            self.n += 1;
            if (i + 1) as f64 >= self.pos + self.step {
                out.push(to_i16((self.acc / f64::from(self.n)) as f32));
                self.acc = 0.0;
                self.n = 0;
                self.pos += self.step;
            }
        }
        self.pos -= len;
    }
}

fn to_i16(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

/// The level of a block for the face: RMS, lifted so speech fills most of
/// the range (speech sits around -30..-10 dBFS).
pub fn level(block: &[f32]) -> f32 {
    if block.is_empty() {
        return 0.0;
    }
    let rms = (block.iter().map(|s| s * s).sum::<f32>() / block.len() as f32).sqrt();
    let db = 20.0 * rms.max(1e-6).log10();
    ((db + 50.0) / 45.0).clamp(0.0, 1.0)
}

/// How often the level goes to the face.
const LEVEL_EVERY: Duration = Duration::from_millis(50);

/// Windows refused the microphone: it is off for apps (or for desktop
/// apps) in Settings > Privacy & security > Microphone. WASAPI says
/// E_ACCESSDENIED (0x80070005) when the stream is opened; cpal passes it
/// on as an OS error (a message in the system's language, then
/// "(os error -2147024891)"). The code is matched, not the words.
#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
pub fn denied_by_system(e: &str) -> bool {
    let low = e.to_ascii_lowercase();
    ["-2147024891", "0x80070005", "(os error 5)", "e_accessdenied", "access is denied", "permission denied"].iter().any(|m| low.contains(m))
}

/// The reason a recording did not start, with its code first: on Windows
/// a refusal of the system is "mic_denied" (the island says the microphone
/// is not allowed and offers the privacy settings), anything else
/// "mic_failed" unless it has a code already.
#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
pub fn open_error(e: String) -> String {
    if cfg!(windows) && denied_by_system(&e) {
        format!("mic_denied: {e}")
    } else if e.starts_with("mic_") || e.starts_with("no_microphone") {
        e
    } else {
        format!("mic_failed: {e}")
    }
}

#[cfg(any(target_os = "macos", windows))]
mod capture {
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, Sample, SampleFormat, SizedSample};

    use super::{level, open_error, wav, Captured, Resampler, LEVEL_EVERY, RATE};

    /// A recording that runs. `finish` ends it and returns the voice;
    /// dropping it throws the recording away.
    pub struct Recording {
        stop: Option<mpsc::Sender<()>>,
        done: mpsc::Receiver<Result<Vec<i16>, String>>,
        shared: Arc<Mutex<Shared>>,
    }

    /// A look at a running recording's samples (on-device speech-to-text
    /// reads them while the key is held).
    pub struct Tap {
        shared: Arc<Mutex<Shared>>,
    }

    impl Tap {
        /// The samples from index `from` on (16 kHz mono). Empty once the
        /// recording was finished.
        pub fn read_from(&self, from: usize) -> Vec<i16> {
            self.shared.lock().map(|s| s.samples.get(from..).map(<[i16]>::to_vec).unwrap_or_default()).unwrap_or_default()
        }

        /// Forget the first `n` samples: the next index 0 is the old `n`.
        /// Wake by name keeps only the last seconds while it listens, and
        /// cuts the recording to the request when the name is heard.
        pub fn drop_front(&self, n: usize) {
            if let Ok(mut s) = self.shared.lock() {
                let n = n.min(s.samples.len());
                s.samples.drain(..n);
            }
        }
    }

    struct Shared {
        samples: Vec<i16>,
        resampler: Resampler,
        block: Vec<f32>,
        last_level: Instant,
    }

    fn mono<T>(data: &[T], channels: usize, into: &mut Vec<f32>)
    where
        T: Sample,
        f32: FromSample<T>,
    {
        into.clear();
        for frame in data.chunks(channels.max(1)) {
            let sum: f32 = frame.iter().map(|s| f32::from_sample(*s)).sum();
            into.push(sum / frame.len() as f32);
        }
    }

    fn build<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        shared: Arc<Mutex<Shared>>,
        on_level: Arc<dyn Fn(f32) + Send + Sync>,
        failed: mpsc::Sender<String>,
    ) -> Result<cpal::Stream, String>
    where
        T: SizedSample + Send + 'static,
        f32: FromSample<T>,
    {
        let channels = usize::from(config.channels);
        let mut scratch = Vec::new();
        device
            .build_input_stream(
                config,
                move |data: &[T], _: &_| {
                    mono(data, channels, &mut scratch);
                    let Ok(mut sh) = shared.lock() else { return };
                    let sh = &mut *sh;
                    sh.resampler.push(&scratch, &mut sh.samples);
                    sh.block.extend_from_slice(&scratch);
                    if sh.last_level.elapsed() >= LEVEL_EVERY {
                        on_level(level(&sh.block));
                        sh.block.clear();
                        sh.last_level = Instant::now();
                    }
                },
                move |e| {
                    let _ = failed.send(e.to_string());
                },
                None,
            )
            .map_err(|e| e.to_string())
    }

    /// Start recording from the default input. Err with a short reason
    /// when there is no microphone or it cannot be opened.
    pub fn start(on_level: Arc<dyn Fn(f32) + Send + Sync>) -> Result<Recording, String> {
        start_on(None, on_level)
    }

    /// Start recording from the input with this CoreAudio UID (cpal's
    /// device id), or the default input for None. Wake by name uses it to
    /// stay off a Bluetooth headset's microphone.
    pub fn start_on(uid: Option<String>, on_level: Arc<dyn Fn(f32) + Send + Sync>) -> Result<Recording, String> {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (done_tx, done_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        // Made here (at 16 kHz until the device's rate is known), so the
        // recording can hand out a tap on it.
        let shared = Arc::new(Mutex::new(Shared {
            samples: Vec::with_capacity(RATE as usize * 10),
            resampler: Resampler::new(RATE),
            block: Vec::new(),
            last_level: Instant::now(),
        }));
        let tapped = shared.clone();
        std::thread::Builder::new()
            .name("island-mic".into())
            .spawn(move || {
                let host = cpal::default_host();
                let device = match &uid {
                    Some(uid) => host.input_devices().ok().and_then(|mut all| all.find(|d| d.id().is_ok_and(|id| id.id() == uid))),
                    None => host.default_input_device(),
                };
                let Some(device) = device else {
                    let _ = ready_tx.send(Err("no_microphone".into()));
                    return;
                };
                let supported = match device.default_input_config() {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = ready_tx.send(Err(open_error(e.to_string())));
                        return;
                    }
                };
                let format = supported.sample_format();
                let config: cpal::StreamConfig = supported.into();
                if let Ok(mut sh) = shared.lock() {
                    sh.resampler = Resampler::new(config.sample_rate);
                }
                let (fail_tx, fail_rx) = mpsc::channel::<String>();
                let built = match format {
                    SampleFormat::F32 => build::<f32>(&device, config, shared.clone(), on_level.clone(), fail_tx),
                    SampleFormat::I16 => build::<i16>(&device, config, shared.clone(), on_level.clone(), fail_tx),
                    SampleFormat::I32 => build::<i32>(&device, config, shared.clone(), on_level.clone(), fail_tx),
                    SampleFormat::U16 => build::<u16>(&device, config, shared.clone(), on_level.clone(), fail_tx),
                    SampleFormat::U8 => build::<u8>(&device, config, shared.clone(), on_level.clone(), fail_tx),
                    SampleFormat::I8 => build::<i8>(&device, config, shared.clone(), on_level.clone(), fail_tx),
                    other => Err(format!("mic_failed: sample format {other} is not handled")),
                };
                let stream = match built.and_then(|s| s.play().map(|_| s).map_err(|e| e.to_string())) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ready_tx.send(Err(open_error(e)));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                // Until stopped (or the recording is dropped).
                let _ = stop_rx.recv();
                drop(stream);
                on_level(0.0);
                let result = match fail_rx.try_recv() {
                    Ok(e) => Err(format!("mic_failed: {e}")),
                    Err(_) => Ok(std::mem::take(&mut shared.lock().map(|mut s| std::mem::take(&mut s.samples)).unwrap_or_default())),
                };
                let _ = done_tx.send(result);
            })
            .map_err(|e| format!("mic_failed: {e}"))?;
        match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Recording { stop: Some(stop_tx), done: done_rx, shared: tapped }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("mic_failed: the microphone did not start in time".into()),
        }
    }

    impl Recording {
        pub fn tap(&self) -> Tap {
            Tap { shared: self.shared.clone() }
        }

        /// Stop and take the voice (blocking for as long as the device
        /// needs to stop: call it off the async runtime's threads).
        pub fn finish(mut self) -> Result<Captured, String> {
            if let Some(stop) = self.stop.take() {
                let _ = stop.send(());
            }
            let samples = self
                .done
                .recv_timeout(std::time::Duration::from_secs(5))
                .map_err(|_| "mic_failed: the recording did not end".to_string())??;
            let ms = samples.len() as u64 * 1000 / u64::from(RATE);
            Ok(Captured { wav: wav(&samples, RATE), ms, pcm: samples })
        }
    }

    impl Drop for Recording {
        fn drop(&mut self) {
            if let Some(stop) = self.stop.take() {
                let _ = stop.send(());
            }
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod capture {
    use std::sync::Arc;

    use super::Captured;

    pub struct Recording;

    pub struct Tap;

    impl Tap {
        pub fn read_from(&self, _from: usize) -> Vec<i16> {
            Vec::new()
        }

        pub fn drop_front(&self, _n: usize) {}
    }

    pub fn start(_on_level: Arc<dyn Fn(f32) + Send + Sync>) -> Result<Recording, String> {
        Err("unsupported".into())
    }

    pub fn start_on(_uid: Option<String>, _on_level: Arc<dyn Fn(f32) + Send + Sync>) -> Result<Recording, String> {
        Err("unsupported".into())
    }

    impl Recording {
        pub fn tap(&self) -> Tap {
            Tap
        }

        pub fn finish(self) -> Result<Captured, String> {
            Err("unsupported".into())
        }
    }
}

pub use capture::{start, start_on, Recording, Tap};

// --- the permission ---

#[cfg(target_os = "macos")]
mod permission {
    //! `AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio`
    //! and `requestAccessForMediaType:completionHandler:`. Every send runs
    //! inside `exception::catch` (see safari.rs for why).

    use std::panic::AssertUnwindSafe;

    use block2::RcBlock;
    use objc2::exception;
    use objc2::rc::autoreleasepool;
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2::{msg_send, sel};

    use super::MicPermission;

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {
        static AVMediaTypeAudio: &'static AnyObject;
    }

    fn device_class() -> Option<&'static AnyObject> {
        let class = AnyClass::get(c"AVCaptureDevice")?;
        Some(unsafe { &*(class as *const AnyClass as *const AnyObject) })
    }

    pub fn status() -> MicPermission {
        let Some(class) = device_class() else {
            return MicPermission::Unknown;
        };
        let got = exception::catch(AssertUnwindSafe(|| {
            autoreleasepool(|_| {
                let responds: bool = unsafe { msg_send![class, respondsToSelector: sel!(authorizationStatusForMediaType:)] };
                if !responds {
                    return None;
                }
                let media = unsafe { AVMediaTypeAudio };
                let status: isize = unsafe { msg_send![class, authorizationStatusForMediaType: media] };
                Some(status)
            })
        }));
        match got {
            // AVAuthorizationStatus: 0 not determined, 1 restricted, 2 denied, 3 authorized.
            Ok(Some(0)) => MicPermission::NotAsked,
            Ok(Some(1 | 2)) => MicPermission::Denied,
            Ok(Some(3)) => MicPermission::Granted,
            _ => MicPermission::Unknown,
        }
    }

    /// Make macOS ask (once; later calls answer at once). `done` gets the
    /// answer, on a queue of the system's.
    pub fn request(done: Box<dyn Fn(bool) + Send + Sync>) {
        let Some(class) = device_class() else {
            done(false);
            return;
        };
        let block = RcBlock::new(move |granted: Bool| done(granted.as_bool()));
        let _ = exception::catch(AssertUnwindSafe(|| {
            autoreleasepool(|_| {
                let responds: bool = unsafe { msg_send![class, respondsToSelector: sel!(requestAccessForMediaType:completionHandler:)] };
                if responds {
                    let media = unsafe { AVMediaTypeAudio };
                    let _: () = unsafe { msg_send![class, requestAccessForMediaType: media, completionHandler: &*block] };
                }
            })
        }));
    }
}

#[cfg(not(target_os = "macos"))]
mod permission {
    use super::MicPermission;

    /// Windows asks nothing up front: a microphone switched off in the
    /// privacy settings fails when it is opened, and that is reported then
    /// ("mic_denied", `open_error`).
    pub fn status() -> MicPermission {
        if cfg!(windows) {
            MicPermission::Granted
        } else {
            MicPermission::Unknown
        }
    }

    pub fn request(done: Box<dyn Fn(bool) + Send + Sync>) {
        done(cfg!(windows));
    }
}

pub use permission::{request as request_permission, status as permission};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_microphone_blocked_by_windows_is_told_apart() {
        // cpal 0.18's text of E_ACCESSDENIED from IAudioClient::Initialize,
        // in English and in another language: the code decides.
        let en = "Failed to build audio client: Access is denied. (os error -2147024891)";
        let ru = "Failed to build audio client: \u{41e}\u{442}\u{43a}\u{430}\u{437}\u{430}\u{43d}\u{43e} \u{432} \u{434}\u{43e}\u{441}\u{442}\u{443}\u{43f}\u{435}. (os error -2147024891)";
        assert!(denied_by_system(en));
        assert!(denied_by_system(ru));
        assert!(denied_by_system("Permission denied. Grant the required access and retry."));
        assert!(!denied_by_system("The requested audio device is not available. It may have been disconnected."));
        assert!(!denied_by_system("Failed to initialize audio client: (os error -2004287480)"));
        if cfg!(windows) {
            assert!(open_error(en.into()).starts_with("mic_denied: "));
        } else {
            // A Mac knows its answer before opening (`permission`).
            assert!(open_error(en.into()).starts_with("mic_failed: "));
        }
        assert_eq!(open_error("mic_failed: sample format x".into()), "mic_failed: sample format x");
        assert_eq!(open_error("no device".into()), "mic_failed: no device");
    }

    #[test]
    fn a_wav_file_is_what_a_reader_expects() {
        let w = wav(&[0, 1, -1, i16::MAX], 16_000);
        assert_eq!(w.len(), 44 + 8);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(w[4..8].try_into().unwrap()), 36 + 8);
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes(w[20..22].try_into().unwrap()), 1, "PCM");
        assert_eq!(u16::from_le_bytes(w[22..24].try_into().unwrap()), 1, "mono");
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(w[28..32].try_into().unwrap()), 32_000);
        assert_eq!(u16::from_le_bytes(w[34..36].try_into().unwrap()), 16);
        assert_eq!(&w[36..40], b"data");
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 8);
        assert_eq!(i16::from_le_bytes(w[50..52].try_into().unwrap()), i16::MAX);
    }

    #[test]
    fn two_minutes_fit_the_request() {
        let bytes = 44 + 120 * RATE as usize * 2;
        assert!(bytes.div_ceil(3) * 4 < connect_core::island::MAX_REQUEST_BYTES - 2 * 1024 * 1024, "room left for a screenshot");
    }

    fn resample(rate: u32, seconds: f32, block: usize) -> Vec<i16> {
        let n = (rate as f32 * seconds) as usize;
        // A 440 Hz tone at half scale.
        let input: Vec<f32> = (0..n).map(|i| 0.5 * (i as f32 / rate as f32 * 440.0 * std::f32::consts::TAU).sin()).collect();
        let mut r = Resampler::new(rate);
        let mut out = Vec::new();
        for chunk in input.chunks(block) {
            r.push(chunk, &mut out);
        }
        out
    }

    #[test]
    fn any_device_rate_becomes_16_khz_whatever_the_block_size() {
        for rate in [48_000, 44_100, 16_000, 8_000] {
            for block in [64, 441, 512, 4096] {
                let out = resample(rate, 1.0, block);
                let want = RATE as i64;
                assert!((out.len() as i64 - want).abs() <= 2, "{rate} Hz in blocks of {block}: {} samples", out.len());
                let peak = out.iter().map(|s| s.unsigned_abs()).max().unwrap();
                assert!((14_000..=16_500).contains(&peak), "{rate} Hz: a half-scale tone stays about half scale ({peak})");
            }
        }
    }

    #[test]
    fn the_level_follows_loudness() {
        assert_eq!(level(&[]), 0.0);
        assert_eq!(level(&[0.0; 100]), 0.0);
        let quiet = level(&[0.003; 100]);
        let speech = level(&[0.05; 100]);
        let loud = level(&[0.9; 100]);
        assert!(quiet < speech && speech < loud, "{quiet} {speech} {loud}");
        assert_eq!(loud, 1.0);
    }
}
