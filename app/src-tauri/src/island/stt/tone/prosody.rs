//! The measurable side of a voice: how fast, how many pauses, how loud,
//! how high, from the hold's own audio (16 kHz mono) and its transcript.
//!
//! Cheap and deterministic: one pass of 20 ms frames for the level, YIN on
//! the same frames (at 8 kHz) for the pitch, the loudness detector's spans
//! (vad.rs) for where the speech and the pauses are, and the transcript's
//! vowels for the syllables. A 10 s hold takes a few milliseconds.

use super::super::vad::{self, Span, RATE};

/// 20 ms hops.
const HOP: usize = RATE / 50;
/// 40 ms pitch windows (two periods of an 50 Hz voice).
const WIN: usize = RATE / 25;
/// Pitch is searched between these (Hz): low male to high female speech.
const F0_MIN: f32 = 65.0;
const F0_MAX: f32 = 420.0;
/// YIN's threshold on the normalized difference (lower is stricter).
const YIN_THRESHOLD: f32 = 0.15;
/// A gap between speech spans this long or longer is a pause. The spans
/// already join gaps under 0.2 s and pad each side by 60 ms.
const MIN_PAUSE: usize = RATE / 4;
/// Less speech than this says nothing about the manner of speaking.
pub const MIN_SPEECH_SECS: f32 = 1.0;
/// Pitch needs at least this many pitched frames (0.4 s).
const MIN_PITCHED: usize = 20;

/// What one hold sounded like.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Features {
    /// The whole hold, in seconds.
    pub secs: f32,
    /// Speech in it, in seconds.
    pub speech_secs: f32,
    /// Pauses (gaps of at least `MIN_PAUSE`) between the first and the
    /// last speech.
    pub pauses: u32,
    /// The longest of them, in seconds.
    pub longest_pause: f32,
    /// The pauses' share of the time from the first to the last speech.
    pub pause_ratio: f32,
    /// Median level of the speech frames (dBFS).
    pub loud_db: f32,
    /// How much that level moves (standard deviation, dB).
    pub loud_sd: f32,
    /// Median pitch (Hz), when enough frames had one.
    pub f0: Option<f32>,
    /// Pitch range, 10th to 90th percentile (semitones).
    pub f0_range: Option<f32>,
    /// Syllables per second of speech, from the transcript.
    pub rate: Option<f32>,
}

impl Features {
    /// Enough speech to judge the manner of speaking at all.
    pub fn judgeable(&self) -> bool {
        self.speech_secs >= MIN_SPEECH_SECS
    }
}

fn db(frame: &[f32]) -> f32 {
    let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len().max(1) as f32).sqrt();
    20.0 * rms.max(1e-7).log10()
}

/// The audio's acoustic features (everything but the rate).
pub fn measure(audio: &[f32]) -> Features {
    let secs = audio.len() as f32 / RATE as f32;
    let spans = vad::energy_spans(audio);
    let speech = vad::speech_len(&spans);
    let mut f = Features { secs, speech_secs: speech as f32 / RATE as f32, ..Features::default() };
    if spans.is_empty() {
        return f;
    }
    pauses(&spans, &mut f);
    let inside = |at: usize| spans.iter().any(|(s, e)| at >= *s && at < *e);
    let mut levels = Vec::new();
    let mut pitches = Vec::new();
    let mut start = 0;
    while start + WIN <= audio.len() {
        let centre = start + WIN / 2;
        if inside(centre) {
            let frame = &audio[start + (WIN - HOP) / 2..start + (WIN + HOP) / 2];
            levels.push(db(frame));
            if let Some(hz) = yin(&audio[start..start + WIN]) {
                pitches.push(hz);
            }
        }
        start += HOP;
    }
    if !levels.is_empty() {
        f.loud_db = percentile(&mut levels, 0.5);
        let mean = levels.iter().sum::<f32>() / levels.len() as f32;
        f.loud_sd = (levels.iter().map(|l| (l - mean).powi(2)).sum::<f32>() / levels.len() as f32).sqrt();
    }
    if pitches.len() >= MIN_PITCHED {
        f.f0 = Some(percentile(&mut pitches, 0.5));
        let lo = percentile(&mut pitches, 0.1);
        let hi = percentile(&mut pitches, 0.9);
        f.f0_range = Some(12.0 * (hi / lo).log2());
    }
    f
}

fn pauses(spans: &[Span], f: &mut Features) {
    let first = spans[0].0;
    let last = spans[spans.len() - 1].1;
    let mut total = 0;
    for w in spans.windows(2) {
        let gap = w[1].0.saturating_sub(w[0].1);
        if gap >= MIN_PAUSE {
            f.pauses += 1;
            total += gap;
            f.longest_pause = f.longest_pause.max(gap as f32 / RATE as f32);
        }
    }
    if last > first {
        f.pause_ratio = total as f32 / (last - first) as f32;
    }
}

/// Sorts `v` and returns the value at fraction `q` of it.
fn percentile(v: &mut [f32], q: f32) -> f32 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f32 * q).round() as usize]
}

/// The pitch of one 40 ms window (16 kHz), by YIN (de Cheveigné and
/// Kawahara, 2002) on the window halved to 8 kHz. None: no clear period
/// (noise, a breath, a consonant).
pub fn yin(window: &[f32]) -> Option<f32> {
    const SR: f32 = (RATE / 2) as f32;
    // Halved by averaging pairs: a crude low-pass, enough for a voice's
    // fundamental.
    let x: Vec<f32> = window.chunks_exact(2).map(|p| (p[0] + p[1]) * 0.5).collect();
    let tau_min = (SR / F0_MAX).floor() as usize;
    let tau_max = (SR / F0_MIN).ceil() as usize;
    if x.len() <= tau_max + 16 {
        return None;
    }
    let n = x.len() - tau_max;
    let energy: f32 = x.iter().map(|s| s * s).sum();
    if energy < 1e-6 {
        return None;
    }
    // The difference function and its cumulative mean normalized form.
    let mut d = vec![0.0f32; tau_max + 1];
    for (tau, slot) in d.iter_mut().enumerate().skip(1) {
        let mut sum = 0.0;
        for j in 0..n {
            let diff = x[j] - x[j + tau];
            sum += diff * diff;
        }
        *slot = sum;
    }
    let mut cmnd = vec![1.0f32; tau_max + 1];
    let mut running = 0.0;
    for tau in 1..=tau_max {
        running += d[tau];
        cmnd[tau] = if running > 0.0 { d[tau] * tau as f32 / running } else { 1.0 };
    }
    let mut tau = tau_min;
    while tau < tau_max {
        if cmnd[tau] < YIN_THRESHOLD {
            while tau + 1 < tau_max && cmnd[tau + 1] < cmnd[tau] {
                tau += 1;
            }
            // Parabolic interpolation around the minimum.
            let (a, b, c) = (cmnd[tau - 1], cmnd[tau], cmnd[tau + 1]);
            let den = a + c - 2.0 * b;
            let shift = if den.abs() > 1e-9 { 0.5 * (a - c) / den } else { 0.0 };
            return Some(SR / (tau as f32 + shift.clamp(-1.0, 1.0)));
        }
        tau += 1;
    }
    None
}

/// Syllables in a transcript: its vowels (Russian and English). Close
/// enough for a rate: Russian has one vowel per syllable; English a few
/// more vowel letters than syllables.
pub fn syllables(text: &str) -> usize {
    let mut n = 0;
    let mut prev_vowel = false;
    for c in text.chars().flat_map(char::to_lowercase) {
        let ru = "аеёиоуыэюя".contains(c);
        let en = "aeiouy".contains(c);
        // A run of English vowels ("ea", "ou") is one syllable; Russian
        // vowels are always one each.
        if ru || (en && !prev_vowel) {
            n += 1;
        }
        prev_vowel = en;
    }
    n
}

/// Adds the speech rate from the transcript (syllables per second of
/// speech).
pub fn with_rate(mut f: Features, transcript: &str) -> Features {
    if f.judgeable() {
        let syl = syllables(transcript);
        if syl > 0 {
            f.rate = Some(syl as f32 / f.speech_secs);
        }
    }
    f
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn sine(hz: f32, secs: f32, amp: f32) -> Vec<f32> {
        (0..(secs * RATE as f32) as usize).map(|i| amp * (i as f32 / RATE as f32 * hz * std::f32::consts::TAU).sin()).collect()
    }

    fn noise(secs: f32, amp: f32, seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..(secs * RATE as f32) as usize)
            .map(|_| {
                s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
                amp * (((s >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0)
            })
            .collect()
    }

    /// A voice-like tone: a fundamental with two harmonics.
    pub fn voiced(hz: f32, secs: f32, amp: f32) -> Vec<f32> {
        let a = sine(hz, secs, amp * 0.6);
        let b = sine(hz * 2.0, secs, amp * 0.3);
        let c = sine(hz * 3.0, secs, amp * 0.15);
        a.iter().zip(&b).zip(&c).map(|((a, b), c)| a + b + c).collect()
    }

    #[test]
    fn pitch_of_a_known_sine() {
        for hz in [80.0, 120.0, 200.0, 310.0] {
            let w = sine(hz, 0.04, 0.3);
            let got = yin(&w).unwrap_or_else(|| panic!("no pitch at {hz}"));
            assert!((got - hz).abs() / hz < 0.02, "{hz} Hz heard as {got}");
            let got = yin(&voiced(hz, 0.04, 0.3)).unwrap();
            assert!((got - hz).abs() / hz < 0.02, "{hz} Hz with harmonics heard as {got}");
        }
    }

    #[test]
    fn noise_and_silence_have_no_pitch() {
        assert_eq!(yin(&vec![0.0; WIN]), None);
        let n = noise(0.04, 0.2, 7);
        assert_eq!(yin(&n), None);
    }

    #[test]
    fn a_steady_tone_has_its_pitch_level_and_no_range() {
        let f = measure(&voiced(150.0, 3.0, 0.2));
        assert!((f.f0.unwrap() - 150.0).abs() < 3.0, "{f:?}");
        assert!(f.f0_range.unwrap() < 0.5, "{f:?}");
        assert!(f.speech_secs > 2.8, "{f:?}");
        assert_eq!(f.pauses, 0);
        // Harmonics of 0.12, 0.06 and 0.03: an RMS of 0.097, -20.2 dBFS.
        assert!((f.loud_db + 20.2).abs() < 0.5, "{f:?}");
        assert!(f.loud_sd < 1.0, "{f:?}");
    }

    #[test]
    fn a_louder_voice_measures_louder() {
        let quiet = measure(&voiced(150.0, 2.0, 0.05));
        let loud = measure(&voiced(150.0, 2.0, 0.4));
        assert!((loud.loud_db - quiet.loud_db - 18.0).abs() < 1.0, "{quiet:?} {loud:?}");
    }

    #[test]
    fn a_gliding_voice_has_a_range() {
        // 120 Hz up to 240 Hz over two seconds: an octave.
        let n = 2 * RATE;
        let mut phase = 0.0f32;
        let glide: Vec<f32> = (0..n)
            .map(|i| {
                let hz = 120.0 * 2f32.powf(i as f32 / n as f32);
                phase += hz / RATE as f32 * std::f32::consts::TAU;
                0.2 * phase.sin()
            })
            .collect();
        let f = measure(&glide);
        let range = f.f0_range.unwrap();
        assert!((8.0..11.0).contains(&range), "an octave glide, 10-90%: about 9.6 st, got {range}");
    }

    #[test]
    fn pauses_between_speech_are_counted() {
        // Speech, 0.6 s quiet, speech, 1.2 s quiet, speech.
        let mut a = voiced(150.0, 1.0, 0.2);
        a.extend(noise(0.6, 0.001, 1));
        a.extend(voiced(150.0, 1.0, 0.2));
        a.extend(noise(1.2, 0.001, 2));
        a.extend(voiced(150.0, 1.0, 0.2));
        a.extend(noise(0.5, 0.001, 3));
        let f = measure(&a);
        assert_eq!(f.pauses, 2, "{f:?}");
        assert!((f.longest_pause - 1.08).abs() < 0.1, "{f:?}");
        // About 1.6 s of pauses in 4.8 s from the first to the last speech.
        assert!((0.25..0.4).contains(&f.pause_ratio), "{f:?}");
        // The trailing quiet is not a pause.
        let f = measure(&[voiced(150.0, 1.5, 0.2), noise(1.0, 0.001, 4)].concat());
        assert_eq!(f.pauses, 0);
    }

    #[test]
    fn noise_is_not_speech() {
        let f = measure(&noise(2.0, 0.0005, 9));
        assert!(!f.judgeable(), "{f:?}");
        assert_eq!(f.f0, None);
    }

    #[test]
    fn the_rate_comes_from_the_transcript() {
        assert_eq!(syllables("Привет, как дела?"), 5);
        assert_eq!(syllables("Найди мне рейс в пятницу"), 7);
        assert_eq!(syllables("open the calendar"), 6);
        assert_eq!(syllables("12 345"), 0);
        let f = Features { speech_secs: 2.0, ..Features::default() };
        assert_eq!(with_rate(f, "Привет, как дела?").rate, Some(2.5));
        // Too little speech: no rate.
        let f = Features { speech_secs: 0.5, ..Features::default() };
        assert_eq!(with_rate(f, "Привет").rate, None);
        assert_eq!(with_rate(Features { speech_secs: 2.0, ..f }, "123").rate, None);
    }
}
