//! NVIDIA Parakeet TDT 0.6B v3 (CC-BY-4.0) as the second decoder, next to
//! whisper: faster (about 25x on the CPU), better on short Russian
//! commands, worse at keeping English words in Latin letters.
//!
//! The model is istupakov's ONNX export (`istupakov/parakeet-tdt-0.6b-v3-onnx`,
//! int8): an encoder (FastConformer, 652 MB) and a small decoder + joint
//! network (18 MB), run by ONNX Runtime on the CPU (the CoreML provider
//! splits the encoder into hundreds of pieces and takes 4 GB). The steps
//! follow the onnx-asr Python package, which made the reference texts the
//! test checks against:
//!
//!  - `features`: NeMo's log-mel spectrum (128 bands, 25 ms windows every
//!    10 ms, per-band normalized over the clip), as onnx-asr's NumPy code.
//!  - the encoder: 8x fewer frames (80 ms each), 1024 values a frame.
//!  - `greedy`: TDT decoding. Each step the joint network gives a token
//!    and a duration (0..4 frames to skip); a blank or a duration of 0
//!    after 10 tokens moves one frame on.
//!  - `Vocab::text`: SentencePiece pieces, "▁" is a space.
//!
//! Language is not asked: the model writes what it hears in any of its 25
//! languages.

use std::path::Path;

/// Mel bands the encoder takes.
pub const BANDS: usize = 128;
const N_FFT: usize = 512;
const WIN: usize = 400;
const HOP: usize = 160;
const PREEMPH: f32 = 0.97;
const LOG_GUARD: f32 = 1.0 / (1u32 << 24) as f32;
/// Tokens one encoder frame may emit before the decoder must move on.
const MAX_TOKENS_PER_STEP: usize = 10;

/// The slaney-normalized mel filters (librosa's `filters.mel`, as NeMo
/// builds them): `[bin][band]`, 257 x 128.
pub fn mel_filters() -> Vec<f32> {
    let sr = 16_000.0f64;
    let bins = N_FFT / 2 + 1;
    let f_sp = 200.0 / 3.0;
    let logstep = 6.4f64.ln() / 27.0;
    let hz_to_mel = |f: f64| if f < 1000.0 { f / f_sp } else { 15.0 + (f / 1000.0).ln() / logstep };
    let mel_to_hz = |m: f64| if m < 15.0 { f_sp * m } else { 1000.0 * (logstep * (m - 15.0)).exp() };
    let (lo, hi) = (hz_to_mel(0.0), hz_to_mel(sr / 2.0));
    let points: Vec<f64> = (0..BANDS + 2).map(|i| mel_to_hz(lo + (hi - lo) * i as f64 / (BANDS + 1) as f64)).collect();
    let freqs: Vec<f64> = (0..bins).map(|k| sr / 2.0 * k as f64 / (bins - 1) as f64).collect();
    let mut w = vec![0.0f32; bins * BANDS];
    for m in 0..BANDS {
        let enorm = 2.0 / (points[m + 2] - points[m]);
        for (k, f) in freqs.iter().enumerate() {
            let lower = (f - points[m]) / (points[m + 1] - points[m]);
            let upper = (points[m + 2] - f) / (points[m + 2] - points[m + 1]);
            w[k * BANDS + m] = (lower.min(upper).max(0.0) * enorm) as f32;
        }
    }
    w
}

/// A 512-point FFT (radix 2), for the spectrum of one window.
pub struct Fft {
    cos: Vec<f64>,
    sin: Vec<f64>,
    rev: Vec<usize>,
}

impl Fft {
    pub fn new() -> Fft {
        let n = N_FFT;
        let bits = n.trailing_zeros();
        let rev = (0..n).map(|i| i.reverse_bits() >> (usize::BITS - bits)).collect();
        let cos = (0..n / 2).map(|k| (2.0 * std::f64::consts::PI * k as f64 / n as f64).cos()).collect();
        let sin = (0..n / 2).map(|k| -(2.0 * std::f64::consts::PI * k as f64 / n as f64).sin()).collect();
        Fft { cos, sin, rev }
    }

    /// |X[k]| for k in 0..=256 of a real input of 512 samples.
    fn magnitudes(&self, input: &[f64], re: &mut [f64], im: &mut [f64], out: &mut [f32]) {
        let n = N_FFT;
        for i in 0..n {
            re[self.rev[i]] = input[i];
            im[self.rev[i]] = 0.0;
        }
        let mut len = 2;
        while len <= n {
            let step = n / len;
            for start in (0..n).step_by(len) {
                for j in 0..len / 2 {
                    let (c, s) = (self.cos[j * step], self.sin[j * step]);
                    let (a, b) = (start + j, start + j + len / 2);
                    let tr = re[b] * c - im[b] * s;
                    let ti = re[b] * s + im[b] * c;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len *= 2;
        }
        for (k, o) in out.iter_mut().enumerate() {
            *o = re[k].hypot(im[k]) as f32;
        }
    }
}

/// NeMo's features of 16 kHz mono audio: `[band][frame]` (128 x frames),
/// the frame count, and how many of them are real (the encoder's
/// `length`; the frames after it are zero). None for under 20 ms of
/// audio (no spread to normalize by).
pub fn features(audio: &[f32], filters: &[f32], fft: &Fft) -> Option<(Vec<f32>, usize, usize)> {
    let n = audio.len();
    let len = n / HOP;
    if len < 2 {
        return None;
    }
    let frames = n / HOP + 1;
    let bins = N_FFT / 2 + 1;
    // Pre-emphasis, then 256 zeros on each side (a centered STFT).
    let mut padded = vec![0.0f32; n + N_FFT];
    for i in 0..n {
        let prev = if i > 0 { audio[i - 1] } else { 0.0 };
        padded[N_FFT / 2 + i] = audio[i] - PREEMPH * prev;
    }
    // A symmetric Hann window of 400 in the middle of the 512.
    let off = (N_FFT - WIN) / 2;
    let mut window = vec![0.0f64; N_FFT];
    for i in 0..WIN {
        window[off + i] = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (WIN - 1) as f64).cos();
    }
    let mut logmel = vec![0.0f32; frames * BANDS];
    let (mut buf, mut re, mut im) = (vec![0.0f64; N_FFT], vec![0.0f64; N_FFT], vec![0.0f64; N_FFT]);
    let mut mag = vec![0.0f32; bins];
    for f in 0..frames {
        let at = f * HOP;
        for i in 0..N_FFT {
            buf[i] = padded[at + i] as f64 * window[i];
        }
        fft.magnitudes(&buf, &mut re, &mut im, &mut mag);
        let row = &mut logmel[f * BANDS..(f + 1) * BANDS];
        for (k, m) in mag.iter().enumerate() {
            let power = m * m;
            if power == 0.0 {
                continue;
            }
            let w = &filters[k * BANDS..(k + 1) * BANDS];
            for (r, x) in row.iter_mut().zip(w) {
                *r += power * x;
            }
        }
        for r in row.iter_mut() {
            *r = (*r + LOG_GUARD).ln();
        }
    }
    // Per band: mean and spread over the real frames.
    let mut out = vec![0.0f32; BANDS * frames];
    for b in 0..BANDS {
        let mean = (0..len).map(|f| logmel[f * BANDS + b] as f64).sum::<f64>() / len as f64;
        let mean = mean as f32;
        let var = (0..len).map(|f| ((logmel[f * BANDS + b] - mean) as f64).powi(2)).sum::<f64>() / (len - 1) as f64;
        let scale = (var as f32).sqrt() + 1e-5;
        for f in 0..len {
            out[b * frames + f] = (logmel[f * BANDS + b] - mean) / scale;
        }
    }
    Some((out, frames, len))
}

/// The model's tokens (vocab.txt: "<piece> <id>" a line).
pub struct Vocab {
    pieces: Vec<String>,
    pub blank: usize,
}

impl Vocab {
    pub fn parse(text: &str) -> Result<Vocab, String> {
        let mut pieces: Vec<String> = Vec::new();
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            let (piece, id) = line.rsplit_once(' ').ok_or("vocab: a line without an id")?;
            let id: usize = id.parse().map_err(|_| "vocab: a bad id")?;
            if pieces.len() <= id {
                pieces.resize(id + 1, String::new());
            }
            pieces[id] = piece.replace('\u{2581}', " ");
        }
        let blank = pieces.iter().position(|p| p == "<blk>").ok_or("vocab: no <blk>")?;
        Ok(Vocab { pieces, blank })
    }

    pub fn load(path: &Path) -> Result<Vocab, String> {
        Vocab::parse(&std::fs::read_to_string(path).map_err(|e| format!("vocab: {e}"))?)
    }

    /// Tokens and blank (the joint network's first outputs).
    pub fn len(&self) -> usize {
        self.pieces.len()
    }

    /// The text of the tokens. As onnx-asr: a space at the very start goes,
    /// and a space stays only before a word character.
    pub fn text(&self, tokens: &[usize]) -> String {
        let joined: String = tokens.iter().filter_map(|t| self.pieces.get(*t)).map(String::as_str).collect();
        let chars: Vec<char> = joined.chars().collect();
        let word = |c: char| c.is_alphanumeric() || c == '_';
        let mut out = String::with_capacity(joined.len());
        for (i, c) in chars.iter().enumerate() {
            if !c.is_whitespace() {
                out.push(*c);
            } else if i > 0 && chars.get(i + 1).is_some_and(|n| word(*n)) {
                out.push(' ');
            }
        }
        out
    }
}

/// One step of the decoder + joint network: the scores for the encoder
/// frame `t` after the last token (blank before any). Updates its state
/// only when `keep` is called.
pub trait Joint {
    /// Scores: `vocab` token scores, then the durations'.
    fn step(&mut self, t: usize, last: usize) -> Result<Vec<f32>, String>;
    /// The state of the last step becomes the decoder's state (a token
    /// was emitted).
    fn keep(&mut self);
}

fn argmax(x: &[f32]) -> usize {
    let mut best = 0;
    for (i, v) in x.iter().enumerate() {
        if *v > x[best] {
            best = i;
        }
    }
    best
}

/// TDT greedy decoding over `frames` encoder frames (onnx-asr's
/// `_AsrWithTransducerDecoding._decoding` with the TDT duration head).
/// `stop` is asked between steps.
pub fn greedy(joint: &mut dyn Joint, frames: usize, vocab: usize, blank: usize, stop: &dyn Fn() -> bool) -> Result<Vec<usize>, String> {
    let mut tokens = Vec::new();
    let mut t = 0;
    let mut emitted = 0;
    while t < frames {
        if stop() {
            return Err(super::stream::ABORTED.into());
        }
        let out = joint.step(t, tokens.last().copied().unwrap_or(blank))?;
        if out.len() <= vocab {
            return Err(format!("the joint network gave {} values", out.len()));
        }
        let token = argmax(&out[..vocab]);
        let step = argmax(&out[vocab..]);
        if token != blank {
            joint.keep();
            tokens.push(token);
            emitted += 1;
        }
        if step > 0 {
            t += step;
            emitted = 0;
        } else if token == blank || emitted == MAX_TOKENS_PER_STEP {
            t += 1;
            emitted = 0;
        }
    }
    Ok(tokens)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use onnx::Engine;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod onnx {
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    use ort::session::builder::GraphOptimizationLevel;
    use ort::session::{RunOptions, Session};
    use ort::value::TensorRef;
    use whisper_rs::WhisperVadContext;

    use super::super::engine;
    use super::super::stream::{Decoded, Decoder, ABORTED};
    use super::super::vad::{self, Span, RATE};
    use super::{features, greedy, mel_filters, Fft, Joint, Vocab, BANDS};

    /// CPU threads for the encoder (the M1 has four fast cores).
    const THREADS: usize = 4;
    /// The decoder's state: 2 layers x 640, twice.
    const STATE: usize = 2 * 640;
    /// The encoder's values per frame.
    const DIM: usize = 1024;

    pub struct Engine {
        encoder: Session,
        joint: Session,
        vocab: Vocab,
        filters: Vec<f32>,
        fft: Fft,
        vad: Option<WhisperVadContext>,
        /// The encoder gives its working memory back after each run (a 30
        /// s piece needs a few hundred MB).
        shrink: RunOptions,
    }

    fn session(path: &Path, threads: usize) -> Result<Session, String> {
        Session::builder()
            .and_then(|b| b.with_optimization_level(GraphOptimizationLevel::Level3))
            .and_then(|b| b.with_intra_threads(threads))
            .and_then(|b| b.commit_from_file(path))
            .map_err(|e| format!("{}: {e}", path.file_name().and_then(|n| n.to_str()).unwrap_or("model")))
    }

    impl Engine {
        /// Load the two networks, the tokens and the voice detector, then
        /// decode a moment of silence so the first piece is warm.
        pub fn load(encoder: &Path, joint: &Path, vocab: &Path, vad_model: &Path) -> Result<Engine, String> {
            let started = Instant::now();
            let vocab = Vocab::load(vocab)?;
            let encoder = session(encoder, THREADS)?;
            let joint = session(joint, 1)?;
            let mut shrink = RunOptions::new().map_err(|e| e.to_string())?;
            shrink.add_config_entry("memory.enable_memory_arena_shrinkage", "cpu:0").map_err(|e| e.to_string())?;
            let vad = engine::load_vad(vad_model);
            let mut e = Engine { encoder, joint, vocab, filters: mel_filters(), fft: Fft::new(), vad, shrink };
            let loaded = started.elapsed();
            e.transcribe(&vec![0.0; RATE / 2], None)?;
            connect_core::logln!(
                "island: stt: parakeet loaded in {} ms, warm in {} ms",
                loaded.as_millis(),
                started.elapsed().as_millis()
            );
            Ok(e)
        }

        /// The text of a piece of 16 kHz mono audio.
        pub fn transcribe(&mut self, audio: &[f32], abort: Option<&AtomicBool>) -> Result<String, String> {
            let stop = || abort.is_some_and(|a| a.load(Ordering::Relaxed));
            let Some((feats, frames, len)) = features(audio, &self.filters, &self.fft) else {
                return Ok(String::new());
            };
            if stop() {
                return Err(ABORTED.into());
            }
            let enc = {
                let x = TensorRef::from_array_view(([1usize, BANDS, frames], feats.as_slice())).map_err(|e| e.to_string())?;
                let l = [len as i64];
                let l = TensorRef::from_array_view(([1usize], &l[..])).map_err(|e| e.to_string())?;
                let out = self
                    .encoder
                    .run_with_options(ort::inputs!["audio_signal" => x, "length" => l], &self.shrink)
                    .map_err(|e| format!("encoder: {e}"))?;
                let (shape, data) = out["outputs"].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
                let (_, lens) = out["encoded_lengths"].try_extract_tensor::<i64>().map_err(|e| e.to_string())?;
                let t = *shape.get(2).ok_or("encoder: no frames")? as usize;
                if shape.get(1).copied() != Some(DIM as i64) {
                    return Err(format!("encoder: shape {shape:?}"));
                }
                let n = (lens.first().copied().unwrap_or(0).max(0) as usize).min(t);
                // [dim][frame] -> [frame][dim]
                let mut frames_out = vec![0.0f32; n * DIM];
                for d in 0..DIM {
                    let row = &data[d * t..d * t + n];
                    for (f, v) in row.iter().enumerate() {
                        frames_out[f * DIM + d] = *v;
                    }
                }
                (frames_out, n)
            };
            let (enc, n) = enc;
            let mut j = Steps { joint: &mut self.joint, enc: &enc, s: [vec![0.0; STATE], vec![0.0; STATE]], next: None };
            let tokens = greedy(&mut j, n, self.vocab.len(), self.vocab.blank, &stop)?;
            Ok(self.vocab.text(&tokens))
        }
    }

    /// The decoder + joint network over one piece's encoder frames.
    struct Steps<'a> {
        joint: &'a mut Session,
        enc: &'a [f32],
        s: [Vec<f32>; 2],
        /// The state after the last step, kept when a token is emitted.
        next: Option<[Vec<f32>; 2]>,
    }

    impl Joint for Steps<'_> {
        fn step(&mut self, t: usize, last: usize) -> Result<Vec<f32>, String> {
            let frame = &self.enc[t * DIM..(t + 1) * DIM];
            let err = |e: ort::Error| format!("decoder: {e}");
            let target = [last as i32];
            let one = [1i32];
            let inputs = ort::inputs![
                "encoder_outputs" => TensorRef::from_array_view(([1usize, DIM, 1], frame)).map_err(err)?,
                "targets" => TensorRef::from_array_view(([1usize, 1], &target[..])).map_err(err)?,
                "target_length" => TensorRef::from_array_view(([1usize], &one[..])).map_err(err)?,
                "input_states_1" => TensorRef::from_array_view(([2usize, 1, 640], self.s[0].as_slice())).map_err(err)?,
                "input_states_2" => TensorRef::from_array_view(([2usize, 1, 640], self.s[1].as_slice())).map_err(err)?,
            ];
            let out = self.joint.run(inputs).map_err(err)?;
            let (_, scores) = out["outputs"].try_extract_tensor::<f32>().map_err(err)?;
            let (_, s1) = out["output_states_1"].try_extract_tensor::<f32>().map_err(err)?;
            let (_, s2) = out["output_states_2"].try_extract_tensor::<f32>().map_err(err)?;
            self.next = Some([s1.to_vec(), s2.to_vec()]);
            Ok(scores.to_vec())
        }

        fn keep(&mut self) {
            if let Some(s) = self.next.take() {
                self.s = s;
            }
        }
    }

    impl Decoder for Engine {
        fn decode(&mut self, audio: &[f32], _lang: Option<&str>, abort: Option<&AtomicBool>) -> Result<Decoded, String> {
            if audio.is_empty() {
                return Ok(Decoded::default());
            }
            let t = Instant::now();
            let text = self.transcribe(audio, abort)?;
            connect_core::logln!(
                "island: stt: parakeet decoded {:.1} s in {:.2} s",
                audio.len() as f32 / RATE as f32,
                t.elapsed().as_secs_f32()
            );
            Ok(Decoded { segments: vec![text], lang: None })
        }

        fn vad(&mut self, audio: &[f32]) -> Option<Vec<Span>> {
            engine::silero(self.vad.as_mut()?, audio)
        }

        fn cuts(&self) -> vad::Cuts {
            // Fast enough to re-read long pieces: cut only at real pauses,
            // and at most every 25 s, so a piece keeps its sentence.
            vad::Cuts { min_gap: RATE * 6 / 10, trailing_gap: RATE, min_chunk: RATE * 3, force_after: RATE * 25 }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mel_filters_are_librosas() {
        let w = mel_filters();
        assert_eq!(w.len(), 257 * BANDS);
        // Values from librosa.filters.mel(sr=16000, n_fft=512, n_mels=128,
        // norm="slaney") (onnx-asr's fbanks.npz, nemo128).
        let max = w.iter().cloned().fold(0.0f32, f32::max);
        assert!((max - 0.042366076).abs() < 1e-7, "{max}");
        // Every band has some weight, the DC bin none.
        for m in 0..BANDS {
            assert!((0..257).any(|k| w[k * BANDS + m] > 0.0), "band {m}");
            assert_eq!(w[m], 0.0);
        }
    }

    #[test]
    fn the_fft_matches_a_slow_dft() {
        let fft = Fft::new();
        let x: Vec<f64> = (0..N_FFT).map(|i| ((i * 37 % 101) as f64 / 50.0 - 1.0) * (i as f64 * 0.01).sin()).collect();
        let (mut re, mut im, mut out) = (vec![0.0; N_FFT], vec![0.0; N_FFT], vec![0.0f32; 257]);
        fft.magnitudes(&x, &mut re, &mut im, &mut out);
        for k in [0usize, 1, 5, 100, 256] {
            let (mut r, mut i) = (0.0f64, 0.0f64);
            for (n, v) in x.iter().enumerate() {
                let a = -2.0 * std::f64::consts::PI * (k * n) as f64 / N_FFT as f64;
                r += v * a.cos();
                i += v * a.sin();
            }
            assert!((out[k] as f64 - r.hypot(i)).abs() < 1e-4, "bin {k}");
        }
    }

    #[test]
    fn features_are_normalized_per_band() {
        let audio: Vec<f32> = (0..16_000).map(|i| (i as f32 * 0.05).sin() * 0.3 + ((i * 7919) % 13) as f32 * 0.001).collect();
        let fft = Fft::new();
        let (f, frames, len) = features(&audio, &mel_filters(), &fft).unwrap();
        assert_eq!((frames, len), (101, 100));
        for b in [0, 40, 127] {
            let row = &f[b * frames..b * frames + len];
            let mean: f32 = row.iter().sum::<f32>() / len as f32;
            assert!(mean.abs() < 1e-3, "band {b}: {mean}");
            assert_eq!(f[b * frames + len], 0.0, "the padding frame is zero");
        }
        assert!(features(&audio[..300], &mel_filters(), &fft).is_none());
    }

    #[test]
    fn tokens_become_text_as_onnx_asr_writes_it() {
        let v = Vocab::parse("<unk> 0\n▁При 1\nвет 2\n, 3\n▁мир 4\n▁ 5\n. 6\n▁GitHub 7\n<blk> 8\n").unwrap();
        assert_eq!(v.blank, 8);
        assert_eq!(v.len(), 9);
        assert_eq!(v.text(&[1, 2, 3, 4, 6]), "Привет, мир.");
        // A lone "▁" before punctuation goes; the space at the very start
        // too (only the first one, as onnx-asr's pattern).
        assert_eq!(v.text(&[1, 2, 5, 3, 7]), "Привет, GitHub");
        assert_eq!(v.text(&[5, 1]), " При");
        assert_eq!(v.text(&[]), "");
    }

    /// A joint network that plays a script: (token, duration) a step.
    struct Script {
        steps: Vec<(usize, usize)>,
        at: usize,
        seen: Vec<(usize, usize)>,
        kept: usize,
    }

    impl Joint for Script {
        fn step(&mut self, t: usize, last: usize) -> Result<Vec<f32>, String> {
            self.seen.push((t, last));
            let (tok, dur) = self.steps.get(self.at).copied().unwrap_or((4, 1));
            self.at += 1;
            let mut out = vec![0.0; 5 + 5];
            out[tok] = 1.0;
            out[5 + dur] = 1.0;
            Ok(out)
        }

        fn keep(&mut self) {
            self.kept += 1;
        }
    }

    #[test]
    fn tdt_greedy_follows_tokens_and_durations() {
        // vocab 5, blank 4. Frame 0: token 1 (dur 0), token 2 (dur 2) ->
        // frame 2: blank (dur 0) -> frame 3: token 3 (dur 1) -> frame 4: end.
        let mut j = Script { steps: vec![(1, 0), (2, 2), (4, 0), (3, 1)], at: 0, seen: vec![], kept: 0 };
        let tokens = greedy(&mut j, 4, 5, 4, &|| false).unwrap();
        assert_eq!(tokens, vec![1, 2, 3]);
        assert_eq!(j.seen, vec![(0, 4), (0, 1), (2, 2), (3, 2)]);
        assert_eq!(j.kept, 3);
        // Ten tokens on one frame with duration 0: then it moves on.
        let mut j = Script { steps: vec![(1, 0); 12], at: 0, seen: vec![], kept: 0 };
        let tokens = greedy(&mut j, 1, 5, 4, &|| false).unwrap();
        assert_eq!(tokens.len(), MAX_TOKENS_PER_STEP);
        // Stopped.
        let mut j = Script { steps: vec![], at: 0, seen: vec![], kept: 0 };
        assert_eq!(greedy(&mut j, 3, 5, 4, &|| true).unwrap_err(), super::super::stream::ABORTED);
    }

    /// Word error rate over a set, as the benchmark scores it (jiwer on
    /// lower case, "ё" as "е", punctuation as spaces).
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn wer(pairs: &[(String, String)]) -> f32 {
        let norm = |s: &str| -> Vec<String> {
            let low: String = s.to_lowercase().replace('ё', "е").chars().map(|c| if c.is_alphanumeric() || c.is_whitespace() || c == '\'' { c } else { ' ' }).collect();
            low.split_whitespace().map(String::from).collect()
        };
        let (mut edits, mut words) = (0usize, 0usize);
        for (r, h) in pairs {
            let (r, h) = (norm(r), norm(h));
            let mut row: Vec<usize> = (0..=h.len()).collect();
            for i in 1..=r.len() {
                let mut prev = row[0];
                row[0] = i;
                for j in 1..=h.len() {
                    let cur = row[j];
                    row[j] = (prev + usize::from(r[i - 1] != h[j - 1])).min(row[j] + 1).min(row[j - 1] + 1);
                    prev = cur;
                }
            }
            edits += row[h.len()];
            words += r.len();
        }
        edits as f32 / words.max(1) as f32
    }

    /// The proof of the port: the real model over the benchmark's clips
    /// against the onnx-asr reference.
    ///
    /// The int8 model is chaotic at the level of float noise: in Python
    /// itself, noise of 1e-5 on the features changes the text of about one
    /// clip in five, and so does ONNX Runtime 1.31 against 1.22. So the
    /// exact check runs on the float model of the same repository (noise
    /// of 1e-5 changes nothing there): with PARAKEET_FP32=1 (the default
    /// when its files are there) the texts must be the same as onnx-asr's
    /// on ONNX Runtime 1.22 (res/parakeet_fp32_ort122.json). On the int8
    /// model (PARAKEET_FP32=0, against res/parakeet.json) the texts may
    /// differ, the word error rate against the true transcripts may not
    /// (at most 1.5 points worse).
    ///
    /// Needs the model files in $PARAKEET_MODEL_DIR (default
    /// /tmp/parakeet-bench/dl/models/parakeet-onnx) and the clips and
    /// references in $PARAKEET_BENCH (default /tmp/parakeet-bench:
    /// q/<set>/NNN.wav + refs.tsv, res/*.json; $PARAKEET_REF overrides the
    /// reference). Sets: $PARAKEET_SETS (default "cs,golos_crowd"). Run in
    /// release, alone:
    ///
    /// `cargo test -p snth-connect --release -- --ignored stt_parakeet_matches --nocapture --test-threads 1`
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    #[ignore]
    fn stt_parakeet_matches_the_reference() {
        use std::path::PathBuf;
        use std::time::Instant;
        let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
        let models = PathBuf::from(env("PARAKEET_MODEL_DIR", "/tmp/parakeet-bench/dl/models/parakeet-onnx"));
        let bench = PathBuf::from(env("PARAKEET_BENCH", "/tmp/parakeet-bench"));
        let fp32 = env("PARAKEET_FP32", if models.join("encoder-model.onnx").exists() { "1" } else { "0" }) == "1";
        let default_ref = bench.join(if fp32 { "res/parakeet_fp32_ort122.json" } else { "res/parakeet.json" });
        let reference = PathBuf::from(env("PARAKEET_REF", &default_ref.to_string_lossy()));
        let reference: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&reference).unwrap()).unwrap();
        let q = if fp32 { "" } else { ".int8" };
        let t = Instant::now();
        let mut e = Engine::load(
            &models.join(format!("encoder-model{q}.onnx")),
            &models.join(format!("decoder_joint-model{q}.onnx")),
            &models.join("vocab.txt"),
            &models.join("no-vad.bin"),
        )
        .unwrap();
        println!("{} model, load + warm-up: {} ms", if fp32 { "fp32" } else { "int8" }, t.elapsed().as_millis());
        let (mut total, mut differ, mut audio_s, mut spent) = (0, 0, 0.0f32, 0.0f32);
        for set in env("PARAKEET_SETS", "cs,golos_crowd").split(',') {
            let hyps = reference[set]["hyp"].as_object().unwrap_or_else(|| panic!("no set {set}"));
            let truth: std::collections::HashMap<String, String> = std::fs::read_to_string(bench.join("q").join(set).join("refs.tsv"))
                .unwrap_or_default()
                .lines()
                .filter_map(|l| l.split_once('\t'))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            let (mut ours, mut theirs) = (Vec::new(), Vec::new());
            let mut names: Vec<&String> = hyps.keys().collect();
            names.sort();
            for name in names {
                let want = hyps[name].as_str().unwrap_or("");
                let b = std::fs::read(bench.join("q").join(set).join(name)).unwrap();
                let at = b.windows(4).position(|w| w == b"data").unwrap() + 8;
                let audio: Vec<f32> = b[at..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect();
                let t = Instant::now();
                let got = e.transcribe(&audio, None).unwrap();
                spent += t.elapsed().as_secs_f32();
                audio_s += audio.len() as f32 / 16_000.0;
                total += 1;
                if got != want {
                    differ += 1;
                    println!("DIFF {set}/{name}\n  ref: {want}\n  got: {got}");
                }
                if let Some(tr) = truth.get(name.as_str()) {
                    ours.push((tr.clone(), got));
                    theirs.push((tr.clone(), want.to_string()));
                }
            }
            let (w_ours, w_ref) = (wer(&ours), wer(&theirs));
            println!("{set}: WER {:.1}% here, {:.1}% in the reference", w_ours * 100.0, w_ref * 100.0);
            if !fp32 {
                assert!(w_ours <= w_ref + 0.015, "{set}: WER {w_ours} against {w_ref}");
            }
        }
        println!("{} of {total} clips differ; {audio_s:.0} s of audio in {spent:.1} s (RTF {:.3})", differ, spent / audio_s);
        assert!(total > 50, "the clips are there");
        if fp32 {
            assert!(differ <= 2, "{differ} of {total} differ");
        }
    }
}
