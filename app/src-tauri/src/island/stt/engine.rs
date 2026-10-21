//! whisper.cpp (whisper-rs, Metal) as the stream's decoder. Apple silicon
//! only (Cargo.toml); everything here runs on the speech thread (mod.rs),
//! never on the main thread or the async runtime.
//!
//! Speed on an M1 comes from two settings, measured on large-v3-turbo
//! q5_0 (tz-local-stt.md; the numbers are in the commit that added this):
//!
//!  - `audio_ctx` cut to the piece's length: whisper always encodes 30 s
//!    otherwise (about 2.5 s on an M1, per call), and the encoder is most
//!    of the cost (about 1.35 ms per 20 ms frame: 0.28 s for 3.7 s of
//!    audio, 0.35 s for 5 s). In steps of 64 frames, with a little room:
//!    a window much longer than the speech makes whisper repeat the text
//!    to fill it, and one shorter than 192 frames garbles it.
//!  - the language passed once it is known: finding it is a second
//!    encoder pass (+0.3..0.5 s).
//!
//! Timestamps stay on: without them whisper loops on short Russian pieces
//! ("Слушай, мне нужно... Слушай, мне нужно...").

use std::ffi::c_void;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState, WhisperVadContext, WhisperVadContextParams, WhisperVadParams};

use super::stream::{pick_lang, Decoded, Decoder, ABORTED};
use super::vad::{Span, RATE};

/// The smallest encoder size (in 20 ms frames, a multiple of 64, at
/// least 192 = 3.8 s) that holds this many samples with 0.16 s of room;
/// 0 (whisper's own 30 s windows) past 29 s.
pub fn audio_ctx(samples: usize) -> i32 {
    if samples >= RATE * 29 {
        return 0;
    }
    let need = (samples * 50 / RATE) as i32 + 8;
    ((need + 63) / 64 * 64).clamp(192, 1500)
}

/// Tokens one piece may take: generous for speech (Russian runs at about
/// 6 tokens a second), small enough that a loop stops early.
fn max_tokens(samples: usize) -> i32 {
    (samples * 12 / RATE) as i32 + 24
}

/// whisper.cpp's Silero voice detector, on its own (the Parakeet engine
/// uses it too). None when it does not load: the energy detector stands in.
pub(super) fn load_vad(vad_model: &Path) -> Option<WhisperVadContext> {
    whisper_rs::install_logging_hooks();
    vad_model.to_str().and_then(|p| {
        let mut params = WhisperVadContextParams::new();
        params.set_n_threads(1);
        params.set_use_gpu(false);
        match WhisperVadContext::new(p, params) {
            Ok(v) => Some(v),
            Err(e) => {
                connect_core::logln!("island: the voice detector did not load ({e}); using loudness");
                None
            }
        }
    })
}

/// Where the speech is, by Silero.
pub(super) fn silero(vad: &mut WhisperVadContext, audio: &[f32]) -> Option<Vec<Span>> {
    let mut params = WhisperVadParams::new();
    params.set_threshold(0.5);
    params.set_min_speech_duration(150);
    // Short, so the pauses at commas show (the stream may cut there).
    params.set_min_silence_duration(100);
    params.set_speech_pad(30);
    let segments = vad.segments_from_samples(params, audio).ok()?;
    let cs = |t: f32| ((t.max(0.0) * RATE as f32 / 100.0) as usize).min(audio.len());
    Some(
        (0..segments.num_segments())
            .filter_map(|i| segments.get_segment(i))
            .map(|s| (cs(s.start), cs(s.end)))
            .filter(|(a, b)| b > a)
            .collect(),
    )
}

pub struct Engine {
    _ctx: WhisperContext,
    state: WhisperState,
    vad: Option<WhisperVadContext>,
}

/// whisper.cpp asks this between steps of the encoder and the decoder:
/// true stops the decode.
unsafe extern "C" fn should_abort(user: *mut c_void) -> bool {
    if user.is_null() {
        return false;
    }
    (*(user as *const AtomicBool)).load(Ordering::Relaxed)
}

impl Engine {
    /// Load the model (and the voice detector, which may be missing: the
    /// energy detector stands in), then decode a moment of silence so the
    /// first real piece does not pay for Metal's setup.
    pub fn load(model: &Path, vad_model: &Path) -> Result<Engine, String> {
        whisper_rs::install_logging_hooks();
        let started = Instant::now();
        let path = model.to_str().ok_or("the model path is not UTF-8")?;
        let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default()).map_err(|e| format!("load: {e}"))?;
        let state = ctx.create_state().map_err(|e| format!("state: {e}"))?;
        let vad = load_vad(vad_model);
        let mut engine = Engine { _ctx: ctx, state, vad };
        let loaded = started.elapsed();
        let _ = engine.decode(&vec![0.0; RATE / 2], Some("en"), None);
        connect_core::logln!(
            "island: speech-to-text loaded in {} ms, warm in {} ms",
            loaded.as_millis(),
            started.elapsed().as_millis()
        );
        Ok(engine)
    }

    /// The piece's language, from whisper's odds (`pick_lang`). The same
    /// encoder pass whisper would make for "auto".
    fn detect(&mut self, audio: &[f32]) -> Result<(String, f32), String> {
        self.state.pcm_to_mel(audio, THREADS).map_err(|e| format!("mel: {e}"))?;
        let (best, probs) = self.state.lang_detect(0, THREADS).map_err(|e| format!("language: {e}"))?;
        let p = |code: &str| whisper_rs::get_lang_id(code).and_then(|i| probs.get(i as usize).copied()).unwrap_or(0.0);
        let best_code = whisper_rs::get_lang_str(best).unwrap_or("en");
        Ok(pick_lang(best_code, p(best_code), p("ru"), p("en")))
    }
}

/// CPU threads for whisper's own work (the M1 has four fast cores; the
/// model runs on the GPU).
const THREADS: usize = 4;

impl Decoder for Engine {
    fn decode(&mut self, audio: &[f32], lang: Option<&str>, abort: Option<&AtomicBool>) -> Result<Decoded, String> {
        if audio.is_empty() {
            return Ok(Decoded::default());
        }
        let detected = match lang {
            Some(_) => None,
            None => Some(self.detect(audio)?),
        };
        let lang = lang.map(String::from).or_else(|| detected.as_ref().map(|(l, _)| l.clone())).unwrap_or_else(|| "en".into());
        let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        p.set_n_threads(THREADS as i32);
        p.set_language(Some(&lang));
        p.set_translate(false);
        p.set_no_context(true);
        p.set_suppress_blank(true);
        p.set_suppress_nst(true);
        p.set_max_tokens(max_tokens(audio.len()));
        p.set_print_progress(false);
        p.set_print_realtime(false);
        p.set_print_special(false);
        p.set_print_timestamps(false);
        let ctx = audio_ctx(audio.len());
        if ctx > 0 {
            p.set_audio_ctx(ctx);
        }
        if let Some(flag) = abort {
            // The flag outlives the call: it is borrowed for all of it.
            unsafe {
                p.set_abort_callback(Some(should_abort));
                p.set_abort_callback_user_data(flag as *const AtomicBool as *mut c_void);
            }
        }
        let ran = self.state.full(p, audio);
        if abort.is_some_and(|a| a.load(Ordering::Relaxed)) {
            return Err(ABORTED.into());
        }
        ran.map_err(|e| format!("decode: {e}"))?;
        let segments = self.state.as_iter().map(|s| s.to_str_lossy().map(|t| t.into_owned()).unwrap_or_default()).collect();
        Ok(Decoded { segments, lang: detected })
    }

    fn vad(&mut self, audio: &[f32]) -> Option<Vec<Span>> {
        silero(self.vad.as_mut()?, audio)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_encoder_size_fits_the_piece() {
        assert_eq!(audio_ctx(RATE), 192);
        assert_eq!(audio_ctx(RATE * 36 / 10), 192, "3.6 s: 188 frames");
        assert_eq!(audio_ctx(RATE * 4), 256);
        assert_eq!(audio_ctx(RATE * 5), 320, "5 s: 258 frames");
        assert_eq!(audio_ctx(RATE * 10), 512);
        assert_eq!(audio_ctx(RATE * 28), 1408);
        assert_eq!(audio_ctx(RATE * 29) % 64, 0);
        assert_eq!(audio_ctx(RATE * 60), 0, "whisper's own windows");
        assert!(max_tokens(RATE * 10) >= 100);
    }
}
