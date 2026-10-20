//! The emotion in a voice, by emotion2vec+ base (Ma et al., "emotion2vec:
//! Self-Supervised Pre-Training for Speech Emotion Representation",
//! arXiv:2312.15185; the model by Alibaba's FunASR team, under the FunASR
//! Model License 1.1, which allows use and redistribution with this
//! attribution and the model's name kept).
//!
//! The model file is our ONNX export of `emotion2vec/emotion2vec_plus_base`
//! (the encoder, the mean over frames, the classifier and the softmax in
//! one graph; 16 kHz mono float32 [1, n] in, nine probabilities out), its
//! matrix products quantized to int8 (118 MB; the same top label as the
//! float model on 95% of the eval clips).
//!
//! Measured on Russian speech (Sber's Dusha, 1020 clips, tz-device-tone
//! notes): about 46% on four balanced classes (chance 25%). The model is
//! often sure when it says angry or happy (precision about 0.75 at
//! probability 0.8 or more on the natural mix, 0.88 on a balanced one) and
//! says "sad" about flat neutral Russian speech (precision 0.12). So only
//! angry and happy are ever said (`Emotion::mood`), and only above
//! `SAY_AT`; sad, fearful and the rest never.

/// The model's labels, in its output order.
pub const LABELS: [&str; 9] = ["angry", "disgusted", "fearful", "happy", "neutral", "other", "sad", "surprised", "unk"];
const ANGRY: usize = 0;
const HAPPY: usize = 3;
/// An emotion is said only at this probability or more (calibrated on the
/// eval set, see above), and called "medium" confidence from `SURE_AT`.
pub const SAY_AT: f32 = 0.8;
pub const SURE_AT: f32 = 0.95;

/// What the model heard in a hold: its probabilities, averaged over the
/// pieces it was run on (weighted by their speech).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Emotion {
    pub probs: [f32; 9],
    /// Speech the model heard, in seconds.
    pub secs: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    Irritated,
    Cheerful,
}

impl Emotion {
    /// The mood worth saying, and whether the model is fairly sure of it.
    pub fn mood(&self) -> Option<(Mood, bool)> {
        let (m, p) = if self.probs[ANGRY] >= self.probs[HAPPY] { (Mood::Irritated, self.probs[ANGRY]) } else { (Mood::Cheerful, self.probs[HAPPY]) };
        (p >= SAY_AT).then_some((m, p >= SURE_AT))
    }

    /// The top label and its probability (the log line).
    pub fn top(&self) -> (&'static str, f32) {
        let (i, p) = self.probs.iter().enumerate().fold((0, 0.0f32), |best, (i, p)| if *p > best.1 { (i, *p) } else { best });
        (LABELS[i], p)
    }
}

/// The weighted mean of the pieces' probabilities.
pub fn average(pieces: &[([f32; 9], f32)]) -> Option<Emotion> {
    let total: f32 = pieces.iter().map(|(_, w)| w).sum();
    if pieces.is_empty() || total <= 0.0 {
        return None;
    }
    let mut probs = [0.0; 9];
    for (p, w) in pieces {
        for (acc, x) in probs.iter_mut().zip(p) {
            *acc += x * w / total;
        }
    }
    Some(Emotion { probs, secs: total })
}

/// Runs the model on 16 kHz mono audio.
pub trait Classify: Send {
    fn classify(&mut self, audio: &[f32]) -> Result<[f32; 9], String>;
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use onnx::Model;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod onnx {
    use std::path::Path;

    use ort::session::builder::GraphOptimizationLevel;
    use ort::session::{RunOptions, Session};
    use ort::value::TensorRef;

    /// CPU threads for the model (the M1 has four fast cores; whisper
    /// works on the GPU meanwhile).
    const THREADS: usize = 4;

    pub struct Model {
        session: Session,
        /// Each run gives its working memory back (a 6 s piece needs about
        /// 200 MB, mostly the first convolutions; kept, it would stay with
        /// the app until the model is unloaded).
        shrink: RunOptions,
    }

    impl Model {
        pub fn load(path: &Path) -> Result<Model, String> {
            let session = Session::builder()
                .and_then(|b| b.with_optimization_level(GraphOptimizationLevel::Level3))
                .and_then(|b| b.with_intra_threads(THREADS))
                .and_then(|b| b.commit_from_file(path))
                .map_err(|e| format!("emotion model: {e}"))?;
            let mut shrink = RunOptions::new().map_err(|e| e.to_string())?;
            shrink.add_config_entry("memory.enable_memory_arena_shrinkage", "cpu:0").map_err(|e| e.to_string())?;
            Ok(Model { session, shrink })
        }
    }

    impl super::Classify for Model {
        fn classify(&mut self, audio: &[f32]) -> Result<[f32; 9], String> {
            let input = TensorRef::from_array_view(([1usize, audio.len()], audio)).map_err(|e| e.to_string())?;
            let out = self.session.run_with_options(ort::inputs!["audio" => input], &self.shrink).map_err(|e| e.to_string())?;
            let (_, probs) = out["probs"].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
            let mut p = [0.0; 9];
            if probs.len() != p.len() {
                return Err(format!("the emotion model gave {} values", probs.len()));
            }
            p.copy_from_slice(probs);
            Ok(p)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probs(i: usize, p: f32) -> [f32; 9] {
        let mut a = [0.0; 9];
        a[i] = p;
        a[4] += 1.0 - p;
        a
    }

    #[test]
    fn only_a_sure_angry_or_happy_is_a_mood() {
        let e = |i, p| Emotion { probs: probs(i, p), secs: 3.0 };
        assert_eq!(e(ANGRY, 0.79).mood(), None);
        assert_eq!(e(ANGRY, 0.8).mood(), Some((Mood::Irritated, false)));
        assert_eq!(e(HAPPY, 0.96).mood(), Some((Mood::Cheerful, true)));
        assert_eq!(e(6, 0.99).mood(), None, "sad is never said");
        assert_eq!(e(2, 0.99).mood(), None, "nor fearful");
        assert_eq!(e(6, 0.99).top(), ("sad", 0.99));
    }

    #[test]
    fn pieces_average_by_their_speech() {
        assert_eq!(average(&[]), None);
        let e = average(&[(probs(ANGRY, 1.0), 3.0), (probs(ANGRY, 0.0), 1.0)]).unwrap();
        assert!((e.probs[ANGRY] - 0.75).abs() < 1e-6);
        assert!((e.probs.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert_eq!(e.secs, 4.0);
    }
}
