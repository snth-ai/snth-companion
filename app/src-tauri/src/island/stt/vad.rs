//! Where the speech is in a hold, and where the stream may cut it.
//!
//! The engine asks whisper.cpp's Silero VAD first (engine.rs). The energy
//! detector here is the fallback when that model cannot be used, and the
//! second opinion for "this hold was silent": a hold is dropped unsent
//! only when both find no speech, so a quiet voice Silero misses still
//! goes (as text, or as audio when nothing could be read from it).
//!
//! Spans are sample ranges at 16 kHz, start inclusive, end exclusive.

pub const RATE: usize = 16_000;

/// A stretch of speech: [start, end) in samples.
pub type Span = (usize, usize);

/// 30 ms frames.
const FRAME: usize = RATE * 30 / 1000;
/// Frames of speech closer than this are one span.
const JOIN_GAP: usize = RATE * 200 / 1000;
/// A span shorter than this is a click, not speech.
const MIN_SPAN: usize = RATE * 120 / 1000;
/// Padding around each span, so a soft start or end is not cut off.
const PAD: usize = RATE * 60 / 1000;

/// whisper's encoder size for a piece (engine.rs, and wake by name's
/// ears): the smallest (in 20 ms frames, a multiple of 64, at least 192 =
/// 3.8 s) that holds this many samples with 0.16 s of room; 0 (whisper's
/// own 30 s windows) past 29 s.
pub fn audio_ctx(samples: usize) -> i32 {
    if samples >= RATE * 29 {
        return 0;
    }
    let need = (samples * 50 / RATE) as i32 + 8;
    ((need + 63) / 64 * 64).clamp(192, 1500)
}

/// A frame's level in dBFS.
fn frame_db(frame: &[f32]) -> f32 {
    let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len().max(1) as f32).sqrt();
    20.0 * rms.max(1e-7).log10()
}

/// Speech by loudness against the clip's own noise floor: a frame is
/// speech when it stands 10 dB over the quietest tenth of the frames,
/// and over -50 dBFS whatever the floor. The bar never goes above -35
/// dBFS, so a window of continuous speech (no quiet frames, a high floor)
/// still counts as speech.
pub fn energy_spans(samples: &[f32]) -> Vec<Span> {
    let dbs: Vec<f32> = samples.chunks(FRAME).map(frame_db).collect();
    if dbs.is_empty() {
        return Vec::new();
    }
    let mut sorted = dbs.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let floor = sorted[sorted.len() / 10];
    let bar = (floor + 10.0).clamp(-50.0, -35.0);
    let mut spans: Vec<Span> = Vec::new();
    for (i, db) in dbs.iter().enumerate() {
        if *db <= bar {
            continue;
        }
        let (start, end) = (i * FRAME, ((i + 1) * FRAME).min(samples.len()));
        match spans.last_mut() {
            Some(last) if start.saturating_sub(last.1) < JOIN_GAP => last.1 = end,
            _ => spans.push((start, end)),
        }
    }
    spans
        .into_iter()
        .filter(|(s, e)| e - s >= MIN_SPAN)
        .map(|(s, e)| (s.saturating_sub(PAD), (e + PAD).min(samples.len())))
        .collect()
}

/// How much speech the spans hold, in samples.
pub fn speech_len(spans: &[Span]) -> usize {
    spans.iter().map(|(s, e)| e.saturating_sub(*s)).sum()
}

/// Less speech than this in a whole hold: nothing was said.
pub const MIN_SPEECH: usize = RATE / 4;

/// The spans that fall in [from, to), clipped to it and shifted to start
/// at `from`.
pub fn within(spans: &[Span], from: usize, to: usize) -> Vec<Span> {
    spans
        .iter()
        .filter_map(|(s, e)| {
            let (s, e) = ((*s).max(from), (*e).min(to));
            (s < e).then(|| (s - from, e - from))
        })
        .collect()
}

/// The stream's cutting rules (in samples).
#[derive(Debug, Clone, Copy)]
pub struct Cuts {
    /// A pause at least this long between two spans may be cut in.
    pub min_gap: usize,
    /// Silence at least this long at the end may be cut in too.
    pub trailing_gap: usize,
    /// No piece shorter than this.
    pub min_chunk: usize,
    /// Without a pause, cut anyway once this much waits, so the piece
    /// left for the release stays short.
    pub force_after: usize,
}

impl Default for Cuts {
    fn default() -> Self {
        Cuts { min_gap: RATE / 4, trailing_gap: RATE * 6 / 10, min_chunk: RATE * 3 / 2, force_after: RATE * 8 }
    }
}

/// Where to cut the audio that is not committed yet (`len` samples, its
/// speech `spans`): the middle of the latest pause that leaves a piece of
/// at least `min_chunk` before it. Without a pause, once `force_after`
/// waits: the middle of the widest gap there is, else the quietest frame
/// (`quietest` of the range it may fall in). No speech at all: everything
/// but the last moment (the piece is silence and is skipped).
pub fn cut(spans: &[Span], len: usize, c: &Cuts, quietest: impl Fn(usize, usize) -> Option<usize>) -> Option<usize> {
    if spans.is_empty() {
        return (len >= c.min_chunk).then(|| len - RATE * 3 / 10);
    }
    let mut gaps: Vec<(usize, usize)> = spans.windows(2).map(|w| (w[0].1, w[1].0)).filter(|(a, b)| b > a).collect();
    let last_end = spans.last().map(|s| s.1).unwrap_or(0);
    let trailing = (last_end < len && len - last_end >= c.trailing_gap).then_some((last_end, len));
    let mid = |(a, b): (usize, usize)| a + (b - a) / 2;
    let ok = |at: usize| at >= c.min_chunk;
    let paused = gaps.iter().copied().filter(|(a, b)| b - a >= c.min_gap).chain(trailing).map(mid).filter(|m| ok(*m)).max();
    if paused.is_some() || len < c.force_after {
        return paused;
    }
    // Forced: never in the last second, where a word may be cut.
    let latest = len.saturating_sub(RATE);
    gaps.retain(|g| ok(mid(*g)) && mid(*g) <= latest);
    if let Some(widest) = gaps.iter().max_by_key(|(a, b)| b - a) {
        return Some(mid(*widest));
    }
    if latest > c.min_chunk {
        return quietest(c.min_chunk, latest).or(Some(latest));
    }
    None
}

/// The middle of the quietest 30 ms frame in [from, to).
pub fn quietest(samples: &[f32], from: usize, to: usize) -> Option<usize> {
    let to = to.min(samples.len());
    if from + FRAME > to {
        return None;
    }
    (from..to - FRAME + 1)
        .step_by(FRAME / 2)
        .map(|at| (at, frame_db(&samples[at..at + FRAME])))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(at, _)| at + FRAME / 2)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A test clip: speech-like noise bursts at -20 dBFS where `on`, room
    /// noise at about -60 dBFS elsewhere.
    pub fn clip(secs: f32, on: &[(f32, f32)]) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        let mut seed = 12345u32;
        let mut rnd = move || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            ((seed >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0
        };
        (0..n)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let loud = on.iter().any(|(a, b)| t >= *a && t < *b);
                let tone = (t * 220.0 * std::f32::consts::TAU).sin();
                if loud {
                    0.1 * tone + 0.05 * rnd()
                } else {
                    0.001 * rnd()
                }
            })
            .collect()
    }

    fn secs(spans: &[Span]) -> Vec<(f32, f32)> {
        spans.iter().map(|(s, e)| ((*s as f32 / RATE as f32 * 10.0).round() / 10.0, (*e as f32 / RATE as f32 * 10.0).round() / 10.0)).collect()
    }

    #[test]
    fn silence_and_room_noise_hold_no_speech() {
        assert!(energy_spans(&[]).is_empty());
        assert!(energy_spans(&vec![0.0; RATE * 3]).is_empty());
        assert!(energy_spans(&clip(4.0, &[])).is_empty());
        // A click is not speech.
        let mut c = clip(3.0, &[]);
        for s in c.iter_mut().skip(RATE).take(RATE / 50) {
            *s = 0.5;
        }
        assert!(energy_spans(&c).is_empty());
    }

    #[test]
    fn speech_is_found_with_its_pauses() {
        let spans = energy_spans(&clip(5.0, &[(0.5, 1.8), (2.4, 4.0)]));
        assert_eq!(secs(&spans), vec![(0.4, 1.9), (2.3, 4.1)]);
        assert!(speech_len(&spans) > MIN_SPEECH);
        // A short breath inside a phrase does not split it.
        let spans = energy_spans(&clip(3.0, &[(0.5, 1.0), (1.1, 2.0)]));
        assert_eq!(spans.len(), 1);
    }

    #[test]
    fn continuous_speech_is_speech_even_without_quiet_frames() {
        let spans = energy_spans(&clip(4.0, &[(0.0, 4.0)]));
        assert_eq!(spans.len(), 1);
        assert!(speech_len(&spans) > RATE * 39 / 10);
    }

    fn at(s: f32) -> usize {
        (s * RATE as f32).round() as usize
    }

    #[test]
    fn the_stream_cuts_in_the_latest_pause() {
        let c = Cuts::default();
        let none = |_: usize, _: usize| None;
        // Speech 0.2-1.0, pause, 1.6-2.6, pause, 3.2-4.0 (still talking).
        let spans = [(at(0.2), at(1.0)), (at(1.6), at(2.6)), (at(3.2), at(4.0))];
        assert_eq!(cut(&spans, at(4.0), &c, none), Some(at(2.9)), "the middle of the latest pause");
        // The first pause alone leaves too short a piece.
        assert_eq!(cut(&spans[..2], at(2.6), &c, none), None);
        // Silence at the end long enough: cut in it.
        assert_eq!(cut(&spans[..2], at(3.4), &c, none), Some(at(3.0)));
        // Nothing said: all but the last moment goes (as silence).
        assert_eq!(cut(&[], at(2.0), &c, none), Some(at(1.7)));
        assert_eq!(cut(&[], at(1.0), &c, none), None);
    }

    #[test]
    fn without_pauses_the_stream_cuts_anyway_and_never_in_the_last_second() {
        let c = Cuts::default();
        // One long span: the quietest frame between min_chunk and len - 1 s.
        let spans = [(0, at(9.0))];
        let got = cut(&spans, at(9.0), &c, |from, to| {
            assert_eq!((from, to), (at(1.5), at(8.0)));
            Some(at(3.3))
        });
        assert_eq!(got, Some(at(3.3)));
        // Short gaps (below min_gap, a comma) are better than any frame.
        let spans = [(0, at(3.0)), (at(3.1), at(6.0)), (at(6.05), at(9.0))];
        assert_eq!(cut(&spans, at(9.0), &c, |_, _| None), Some(at(3.05)));
        // Before force_after: wait for a pause.
        assert_eq!(cut(&[(0, at(7.0))], at(7.0), &c, |_, _| Some(1)), None);
    }

    #[test]
    fn the_quietest_frame_is_found() {
        let c = clip(3.0, &[(0.0, 1.2), (1.4, 3.0)]);
        let q = quietest(&c, at(0.5), at(2.5)).unwrap();
        assert!(q >= at(1.2) && q <= at(1.4), "{q}");
        assert_eq!(quietest(&c, 10, 20), None);
    }

    #[test]
    fn spans_within_a_range_are_clipped_and_shifted() {
        let spans = [(100, 200), (300, 400), (500, 600)];
        assert_eq!(within(&spans, 350, 550), vec![(0, 50), (150, 200)]);
        assert!(within(&spans, 200, 300).is_empty());
    }
}
