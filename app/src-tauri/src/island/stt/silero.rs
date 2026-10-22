//! The Silero voice detector (Silero VAD v5, MIT) without whisper.cpp: the
//! official ONNX model on ONNX Runtime, for the Parakeet engine where
//! whisper.cpp is not in the build (Windows). On a Mac the hold keeps
//! whisper.cpp's port of the same model (engine.rs `silero`); this one is
//! compiled and tested there too, and used only for a `.onnx` file
//! (parakeet.rs `load_vad`).
//!
//! It is made to give what whisper.cpp 1.7.6 gives (whisper-rs-sys 0.14.1,
//! `whisper_vad_detect_speech` + `whisper_vad_segments_from_probs`), so a
//! hold is cut and judged silent the same way on both systems:
//!
//!  - `probs`: one probability per 512 samples (32 ms), the LSTM state
//!    carried from window to window and zeroed at the start of each call,
//!    the last window zero-padded. The ONNX model takes each window with
//!    64 samples of context in front and pads 64 on the right by
//!    reflection itself; whisper.cpp's graph takes the bare window and
//!    reflects 64 on both sides. So the context given here is the window's
//!    own reflection (`with_context`), which makes the model's input the
//!    same 640 samples as whisper.cpp's (`stt_silero_onnx_matches_whisper_cpp`
//!    checks the probabilities against whisper.cpp's on real speech).
//!  - `segments`: `whisper_vad_segments_from_probs` line by line (the
//!    hysteresis, the merge of gaps under 200 ms, the padding) with the
//!    hold's parameters (`HOLD`, the ones engine.rs gives whisper.cpp),
//!    then the same centisecond rounding on the way back to samples.

use super::vad::{Span, RATE};

/// Samples a probability covers (the model's window at 16 kHz).
pub const WINDOW: usize = 512;
/// Samples of context in front of each window.
pub const CONTEXT: usize = 64;

/// whisper.cpp's `whisper_vad_params`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub threshold: f32,
    pub min_speech_ms: i32,
    pub min_silence_ms: i32,
    pub max_speech_s: f32,
    pub speech_pad_ms: i32,
}

/// The hold's settings (engine.rs gives the same to whisper.cpp): a short
/// minimum silence, so the pauses at commas show (the stream may cut
/// there). The maximum speech length is whisper.cpp's default (none).
pub const HOLD: Params = Params { threshold: 0.5, min_speech_ms: 150, min_silence_ms: 100, max_speech_s: f32::MAX, speech_pad_ms: 30 };

/// The model's input for one window: 64 samples of context, then the 512.
/// The context is the window's reflection (`x[64], x[63], .., x[1]`), as
/// whisper.cpp pads it (`ggml_pad_reflect_1d(.., 64, 64)`).
pub fn with_context(window: &[f32; WINDOW], out: &mut [f32; CONTEXT + WINDOW]) {
    for i in 0..CONTEXT {
        out[i] = window[CONTEXT - i];
    }
    out[CONTEXT..].copy_from_slice(window);
}

/// The windows of `audio` as whisper.cpp makes them (the last one
/// zero-padded).
pub fn windows(audio: &[f32]) -> impl Iterator<Item = [f32; WINDOW]> + '_ {
    audio.chunks(WINDOW).map(|c| {
        let mut w = [0.0f32; WINDOW];
        w[..c.len()].copy_from_slice(c);
        w
    })
}

/// whisper.cpp's `samples_to_cs`.
fn samples_to_cs(samples: i32) -> i64 {
    ((samples as f64 / RATE as f64) * 100.0 + 0.5) as i64
}

/// Speech segments from the probabilities, in centiseconds (whisper.cpp's
/// `whisper_vad_segments_from_probs`, ported as it is, quirks included:
/// a `temp_end` of 0 means "none").
pub fn segments(probs: &[f32], p: &Params) -> Vec<(i64, i64)> {
    let n_window = WINDOW as i32;
    let sample_rate = RATE as i32;
    let n_probs = probs.len() as i32;
    let min_silence_samples = sample_rate * p.min_silence_ms / 1000;
    let audio_length_samples = n_probs * n_window;
    let min_speech_samples = sample_rate * p.min_speech_ms / 1000;
    let speech_pad_samples = sample_rate * p.speech_pad_ms / 1000;
    let max_speech_samples: i32 = if p.max_speech_s > 100_000.0 {
        i32::MAX / 2
    } else {
        let temp = sample_rate as i64 * (p.max_speech_s as i64) - n_window as i64 - 2 * speech_pad_samples as i64;
        let m = if temp > i32::MAX as i64 { i32::MAX / 2 } else { temp as i32 };
        if m < 0 {
            i32::MAX / 2
        } else {
            m
        }
    };
    let min_silence_samples_at_max_speech = sample_rate * 98 / 1000;
    let neg_threshold = if p.threshold - 0.15 < 0.01 { 0.01 } else { p.threshold - 0.15 };

    let mut speeches: Vec<(i32, i32)> = Vec::new();
    let mut is_speech_segment = false;
    let mut temp_end = 0i32;
    let mut prev_end = 0i32;
    let mut next_start = 0i32;
    let mut curr_speech_start = 0i32;
    let mut has_curr_speech = false;

    for (i, &curr_prob) in probs.iter().enumerate() {
        let curr_sample = n_window * i as i32;

        if curr_prob >= p.threshold && temp_end != 0 {
            temp_end = 0;
            if next_start < prev_end {
                next_start = curr_sample;
            }
        }

        if curr_prob >= p.threshold && !is_speech_segment {
            is_speech_segment = true;
            curr_speech_start = curr_sample;
            has_curr_speech = true;
            continue;
        }

        if is_speech_segment && (curr_sample - curr_speech_start) > max_speech_samples {
            if prev_end != 0 {
                speeches.push((curr_speech_start, prev_end));
                has_curr_speech = true;
                if next_start < prev_end {
                    is_speech_segment = false;
                    has_curr_speech = false;
                } else {
                    curr_speech_start = next_start;
                }
                prev_end = 0;
                next_start = 0;
                temp_end = 0;
            } else {
                speeches.push((curr_speech_start, curr_sample));
                prev_end = 0;
                next_start = 0;
                temp_end = 0;
                is_speech_segment = false;
                has_curr_speech = false;
                continue;
            }
        }

        if curr_prob < neg_threshold && is_speech_segment {
            if temp_end == 0 {
                temp_end = curr_sample;
            }
            if (curr_sample - temp_end) > min_silence_samples_at_max_speech {
                prev_end = temp_end;
            }
            if (curr_sample - temp_end) < min_silence_samples {
                continue;
            }
            if (temp_end - curr_speech_start) > min_speech_samples {
                speeches.push((curr_speech_start, temp_end));
            }
            prev_end = 0;
            next_start = 0;
            temp_end = 0;
            is_speech_segment = false;
            has_curr_speech = false;
            continue;
        }
    }

    if has_curr_speech && (audio_length_samples - curr_speech_start) > min_speech_samples {
        speeches.push((curr_speech_start, audio_length_samples));
    }

    // Merge segments with gaps under 200 ms.
    let max_merge_gap_samples = sample_rate * 200 / 1000;
    let mut i = 0;
    while speeches.len() > 1 && i + 1 < speeches.len() {
        if speeches[i + 1].0 - speeches[i].1 < max_merge_gap_samples {
            speeches[i].1 = speeches[i + 1].1;
            speeches.remove(i + 1);
        } else {
            i += 1;
        }
    }

    speeches.retain(|(s, e)| e - s >= min_speech_samples);

    let n = speeches.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        if i == 0 {
            speeches[i].0 = if speeches[i].0 > speech_pad_samples { speeches[i].0 - speech_pad_samples } else { 0 };
        }
        if i + 1 < n {
            let silence_duration = speeches[i + 1].0 - speeches[i].1;
            if silence_duration < 2 * speech_pad_samples {
                speeches[i].1 += silence_duration / 2;
                speeches[i + 1].0 = if speeches[i + 1].0 > silence_duration / 2 { speeches[i + 1].0 - silence_duration / 2 } else { 0 };
            } else {
                speeches[i].1 = (speeches[i].1 + speech_pad_samples).min(audio_length_samples);
                speeches[i + 1].0 = if speeches[i + 1].0 > speech_pad_samples { speeches[i + 1].0 - speech_pad_samples } else { 0 };
            }
        } else {
            speeches[i].1 = (speeches[i].1 + speech_pad_samples).min(audio_length_samples);
        }
        out.push((samples_to_cs(speeches[i].0), samples_to_cs(speeches[i].1)));
    }
    out
}

/// Centisecond segments as sample spans of a clip of `len` samples, the
/// way engine.rs reads whisper.cpp's (a float of centiseconds, clipped to
/// the clip, empty ones dropped).
pub fn spans(segments: &[(i64, i64)], len: usize) -> Vec<Span> {
    let cs = |t: f32| ((t.max(0.0) * RATE as f32 / 100.0) as usize).min(len);
    segments.iter().map(|(a, b)| (cs(*a as f32), cs(*b as f32))).filter(|(a, b)| b > a).collect()
}

#[cfg(device_stt)]
pub use onnx::Silero;

#[cfg(device_stt)]
mod onnx {
    use std::path::Path;

    use ort::session::builder::GraphOptimizationLevel;
    use ort::session::Session;
    use ort::value::TensorRef;

    use super::{segments, spans, windows, with_context, Params, CONTEXT, WINDOW};
    use crate::island::stt::vad::{Span, RATE};

    /// The LSTM's state: 2 x 1 x 128.
    const STATE: usize = 2 * 128;

    pub struct Silero {
        session: Session,
    }

    impl Silero {
        /// One thread, as whisper.cpp runs its detector (a window is a few
        /// thousand multiply-adds; more threads only cost their wake-ups).
        pub fn load(path: &Path) -> Result<Silero, String> {
            let session = Session::builder()
                .and_then(|b| b.with_optimization_level(GraphOptimizationLevel::Level3))
                .and_then(|b| b.with_intra_threads(1))
                .and_then(|b| b.with_inter_threads(1))
                .and_then(|b| b.commit_from_file(path))
                .map_err(|e| format!("voice detector: {e}"))?;
            Ok(Silero { session })
        }

        /// The speech probability of each 512-sample window.
        pub fn probs(&mut self, audio: &[f32]) -> Result<Vec<f32>, String> {
            self.probs_with(audio, false)
        }

        /// `previous`: the context is the 64 samples before the window, as
        /// Silero's own Python wrapper gives it, instead of whisper.cpp's
        /// reflection (only the parity test compares the two).
        pub(in crate::island::stt) fn probs_with(&mut self, audio: &[f32], previous: bool) -> Result<Vec<f32>, String> {
            let err = |e: ort::Error| format!("voice detector: {e}");
            let mut state = vec![0.0f32; STATE];
            let sr = [RATE as i64];
            let mut input = [0.0f32; CONTEXT + WINDOW];
            let mut out = Vec::with_capacity(audio.len().div_ceil(WINDOW));
            let mut before = [0.0f32; CONTEXT];
            for w in windows(audio) {
                with_context(&w, &mut input);
                if previous {
                    input[..CONTEXT].copy_from_slice(&before);
                    before.copy_from_slice(&w[WINDOW - CONTEXT..]);
                }
                let inputs = ort::inputs![
                    "input" => TensorRef::from_array_view(([1usize, CONTEXT + WINDOW], &input[..])).map_err(err)?,
                    "state" => TensorRef::from_array_view(([2usize, 1, 128], state.as_slice())).map_err(err)?,
                    "sr" => TensorRef::from_array_view(((), &sr[..])).map_err(err)?,
                ];
                let res = self.session.run(inputs).map_err(err)?;
                let (_, p) = res["output"].try_extract_tensor::<f32>().map_err(err)?;
                let (_, s) = res["stateN"].try_extract_tensor::<f32>().map_err(err)?;
                if s.len() != STATE {
                    return Err(format!("voice detector: a state of {} values", s.len()));
                }
                out.push(*p.first().ok_or("voice detector: no probability")?);
                state.copy_from_slice(s);
            }
            Ok(out)
        }

        /// Where the speech is, with these settings. None when the model
        /// failed (the loudness detector stands in).
        pub fn spans_with(&mut self, audio: &[f32], p: &Params) -> Option<Vec<Span>> {
            match self.probs(audio) {
                Ok(probs) => Some(spans(&segments(&probs, p), audio.len())),
                Err(e) => {
                    connect_core::logln!("island: the voice detector failed: {e}");
                    None
                }
            }
        }

        /// Where the speech is in a hold (`HOLD`).
        pub fn spans(&mut self, audio: &[f32]) -> Option<Vec<Span>> {
            self.spans_with(audio, &super::HOLD)
        }
    }

    /// The detector for an engine. None when it does not load: the
    /// loudness detector stands in (as with whisper.cpp's).
    pub fn load(path: &Path) -> Option<Silero> {
        match Silero::load(path) {
            Ok(s) => Some(s),
            Err(e) => {
                connect_core::logln!("island: the voice detector did not load ({e}); using loudness");
                None
            }
        }
    }
}

#[cfg(device_stt)]
pub use onnx::load;

#[cfg(test)]
mod tests {
    use super::*;

    /// Probabilities for `secs` of audio: speech (0.9) inside the ranges,
    /// quiet (0.05) elsewhere.
    fn probs(secs: f32, on: &[(f32, f32)]) -> Vec<f32> {
        let n = (secs * RATE as f32 / WINDOW as f32).ceil() as usize;
        (0..n)
            .map(|i| {
                let t = (i * WINDOW) as f32 / RATE as f32;
                if on.iter().any(|(a, b)| t >= *a && t < *b) {
                    0.9
                } else {
                    0.05
                }
            })
            .collect()
    }

    #[test]
    fn the_context_is_the_windows_reflection() {
        let mut w = [0.0f32; WINDOW];
        for (i, x) in w.iter_mut().enumerate() {
            *x = i as f32;
        }
        let mut out = [0.0f32; CONTEXT + WINDOW];
        with_context(&w, &mut out);
        assert_eq!(out[0], 64.0);
        assert_eq!(out[63], 1.0);
        assert_eq!(out[64], 0.0);
        assert_eq!(out[CONTEXT + WINDOW - 1], 511.0);
        // The last window is zero-padded.
        let ws: Vec<_> = windows(&[1.0; 600]).collect();
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[1][87], 1.0);
        assert_eq!(ws[1][88], 0.0);
    }

    #[test]
    fn the_holds_settings_are_whisper_cpps() {
        // engine.rs gives whisper.cpp these numbers; max speech stays at
        // whisper.cpp's default (FLT_MAX: no limit).
        assert_eq!((HOLD.threshold, HOLD.min_speech_ms, HOLD.min_silence_ms, HOLD.speech_pad_ms), (0.5, 150, 100, 30));
        assert_eq!(HOLD.max_speech_s, f32::MAX);
    }

    #[test]
    fn silence_has_no_segments() {
        assert!(segments(&[], &HOLD).is_empty());
        assert!(segments(&probs(3.0, &[]), &HOLD).is_empty());
        assert!(spans(&segments(&probs(3.0, &[]), &HOLD), RATE * 3).is_empty());
    }

    #[test]
    fn speech_is_found_padded_and_rounded_to_centiseconds() {
        // Speech 0.5..1.5 s: windows 16..47 (0.512 s .. 1.504 s).
        let segs = segments(&probs(3.0, &[(0.5, 1.5)]), &HOLD);
        // Start: window 16 = 8192 samples, less 480 of padding = 7712 ->
        // 48.2 cs -> 48. End: the first quiet window (47 = 24064) + 480 =
        // 24544 -> 153.4 cs -> 153.
        assert_eq!(segs, vec![(48, 153)]);
        assert_eq!(spans(&segs, RATE * 3), vec![(7680, 24480)]);
    }

    #[test]
    fn pauses_under_200_ms_are_merged_and_longer_ones_kept() {
        // A 128 ms pause: one segment.
        assert_eq!(segments(&probs(4.0, &[(0.5, 1.5), (1.628, 3.0)]), &HOLD).len(), 1);
        // A 400 ms pause: two.
        assert_eq!(segments(&probs(4.0, &[(0.5, 1.5), (1.9, 3.0)]), &HOLD).len(), 2);
    }

    #[test]
    fn a_blip_is_not_speech_and_a_short_dip_does_not_end_it() {
        // 64 ms of speech is under min_speech (150 ms).
        assert!(segments(&probs(2.0, &[(0.5, 0.564)]), &HOLD).is_empty());
        // A dip under min_silence (100 ms) inside speech: one segment.
        let mut p = probs(3.0, &[(0.5, 2.0)]);
        p[40] = 0.0;
        p[41] = 0.0;
        assert_eq!(segments(&p, &HOLD).len(), 1);
        // Between the thresholds (0.35..0.5) speech goes on.
        let mut p = probs(3.0, &[(0.5, 2.0)]);
        for x in &mut p[30..50] {
            *x = 0.4;
        }
        assert_eq!(segments(&p, &HOLD).len(), 1);
    }

    /// The ONNX detector against whisper.cpp's on real speech:
    ///
    ///  - the segment logic: this port, fed whisper.cpp's own
    ///    probabilities, gives whisper.cpp's segments exactly;
    ///  - the model: the ONNX probabilities against whisper.cpp's (and,
    ///    for comparison, with Silero's own context of the samples before);
    ///  - the result: the spans of a hold, ONNX against whisper.cpp.
    ///
    /// Needs in $SILERO_DIR (default /tmp/wds-models) `silero_vad.onnx`
    /// (model.rs SILERO_ONNX) and `ggml-silero-v5.1.2.bin` (model.rs VAD),
    /// and 16 kHz mono 16-bit clips under $SILERO_CLIPS (default
    /// /tmp/parakeet-bench/q, every .wav one folder down). Run:
    ///
    /// `cargo test -p snth-connect -- --ignored stt_silero_onnx_matches --nocapture --test-threads 1`
    #[cfg(whisper_stt)]
    #[test]
    #[ignore]
    fn stt_silero_onnx_matches_whisper_cpp() {
        use std::path::PathBuf;
        use std::time::Instant;
        let env = |k: &str, d: &str| PathBuf::from(std::env::var(k).unwrap_or_else(|_| d.to_string()));
        let dir = env("SILERO_DIR", "/tmp/wds-models");
        let clips_dir = env("SILERO_CLIPS", "/tmp/parakeet-bench/q");
        let mut wv = crate::island::stt::engine::load_vad(&dir.join(super::super::model::VAD.name)).expect("whisper.cpp's detector");
        let mut onnx = Silero::load(&dir.join(super::super::model::SILERO_ONNX.name)).expect("the ONNX detector");
        let mut clips: Vec<PathBuf> = std::fs::read_dir(&clips_dir)
            .unwrap()
            .flatten()
            .filter(|d| d.path().is_dir())
            .flat_map(|d| std::fs::read_dir(d.path()).unwrap().flatten().map(|f| f.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "wav"))
            .collect();
        clips.sort();
        let (mut n, mut windows_n, mut same_segs, mut same_spans) = (0, 0usize, 0, 0);
        let (mut worst, mut worst_prev, mut sum_diff, mut sum_prev) = (0.0f32, 0.0f32, 0.0f64, 0.0f64);
        let (mut flips, mut flips_prev) = (0usize, 0usize);
        let (mut t_w, mut t_o) = (0.0f32, 0.0f32);
        let (mut speech_w, mut speech_o, mut speech_both) = (0usize, 0usize, 0usize);
        for clip in &clips {
            let b = std::fs::read(clip).unwrap();
            let Some(at) = b.windows(4).position(|w| w == b"data") else { continue };
            let audio: Vec<f32> = b[at + 8..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect();
            if audio.len() < RATE / 2 {
                continue;
            }
            n += 1;
            // The segment logic on whisper.cpp's own probabilities.
            wv.detect_speech(&audio).unwrap();
            let pw = wv.probabilities().to_vec();
            let mut params = whisper_rs::WhisperVadParams::new();
            params.set_threshold(HOLD.threshold);
            params.set_min_speech_duration(HOLD.min_speech_ms);
            params.set_min_silence_duration(HOLD.min_silence_ms);
            params.set_speech_pad(HOLD.speech_pad_ms);
            let sw = wv.segments_from_probabilities(params).unwrap();
            let theirs: Vec<(f32, f32)> = (0..sw.num_segments()).filter_map(|i| sw.get_segment(i)).map(|s| (s.start, s.end)).collect();
            let ours: Vec<(f32, f32)> = segments(&pw, &HOLD).iter().map(|(a, b)| (*a as f32, *b as f32)).collect();
            assert_eq!(ours, theirs, "{}: the segment logic differs", clip.display());
            same_segs += 1;
            // The model.
            let t = Instant::now();
            let po = onnx.probs(&audio).unwrap();
            t_o += t.elapsed().as_secs_f32();
            let pp = onnx.probs_with(&audio, true).unwrap();
            assert_eq!(po.len(), pw.len());
            for i in 0..pw.len() {
                let (d, dp) = ((po[i] - pw[i]).abs(), (pp[i] - pw[i]).abs());
                worst = worst.max(d);
                worst_prev = worst_prev.max(dp);
                sum_diff += d as f64;
                sum_prev += dp as f64;
                flips += usize::from((po[i] >= HOLD.threshold) != (pw[i] >= HOLD.threshold));
                flips_prev += usize::from((pp[i] >= HOLD.threshold) != (pw[i] >= HOLD.threshold));
            }
            windows_n += pw.len();
            // The hold's spans.
            let t = Instant::now();
            let a = crate::island::stt::engine::silero(&mut wv, &audio).unwrap();
            t_w += t.elapsed().as_secs_f32();
            let o = onnx.spans(&audio).unwrap();
            let mask = |sp: &[Span]| {
                let mut m = vec![false; audio.len()];
                for (s, e) in sp {
                    m[*s..*e].iter_mut().for_each(|x| *x = true);
                }
                m
            };
            let (ma, mo) = (mask(&a), mask(&o));
            speech_w += ma.iter().filter(|x| **x).count();
            speech_o += mo.iter().filter(|x| **x).count();
            speech_both += ma.iter().zip(&mo).filter(|(x, y)| **x && **y).count();
            if a == o {
                same_spans += 1;
            } else {
                println!("{}: whisper.cpp {a:?}\n    onnx {o:?}", clip.display());
            }
        }
        println!("{n} clips, {windows_n} windows: segment logic identical on {same_segs}");
        println!(
            "probabilities, reflected context: max |dp| {worst:.4}, mean {:.5}, {flips} windows on the other side of 0.5",
            sum_diff / windows_n as f64
        );
        println!(
            "probabilities, previous samples as context: max |dp| {worst_prev:.4}, mean {:.5}, {flips_prev} windows on the other side of 0.5",
            sum_prev / windows_n as f64
        );
        println!(
            "spans identical on {same_spans} of {n}; speech whisper.cpp {:.1} s, onnx {:.1} s, both {:.1} s",
            speech_w as f32 / RATE as f32,
            speech_o as f32 / RATE as f32,
            speech_both as f32 / RATE as f32
        );
        println!("time: whisper.cpp {t_w:.2} s, onnx {t_o:.2} s");
        assert!(n >= 20, "the clips are there");
        assert!(worst < 0.05, "the ONNX model differs from whisper.cpp's: {worst}");
        assert!(same_spans * 10 >= n * 9, "spans differ on {} of {n}", n - same_spans);
    }

    #[test]
    fn speech_to_the_end_is_kept_to_the_end() {
        let p = probs(2.0, &[(0.5, 2.0)]);
        let segs = segments(&p, &HOLD);
        assert_eq!(segs.len(), 1);
        // To the end of the last window (63 * 512 = 32256 samples, 201.6
        // cs -> 202), clipped to the clip by `spans`.
        assert_eq!(segs[0].1, 202);
        assert_eq!(spans(&segs, RATE * 2)[0].1, RATE * 2);
    }
}
