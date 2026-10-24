//! The listener's ears: Silero and whisper tiny, both on the CPU.
//!
//! On Apple silicon both are whisper.cpp's (Silero as whisper.cpp ports
//! it). On Windows x64 whisper tiny is the same whisper.cpp, built for the
//! CPU alone (no Metal; AVX2 as the floor, .cargo/ggml-x64.cmake), and
//! Silero is the ONNX model on ONNX Runtime (stt/silero.rs), which gives
//! the same spans as whisper.cpp's (its parity test: 245 of 245 clips),
//! with the same settings (`silero::HOLD` is what is set here on a Mac).
//! So the same model hears the name, with the same prompt, on both
//! systems, and `names.rs` is tuned once.
//!
//! The tiny model is a separate small context next to the hold's big one
//! (stt/engine.rs on a Mac, Parakeet on Windows): the hold's engine keeps
//! its own load on demand and idle unload, and a name check never waits
//! for it. On the CPU, not Metal: a check is about two seconds of audio,
//! which the CPU decodes in tens of milliseconds, and the GPU stays free
//! for the hold that may follow.
//!
//! A check is one short decode in a given language with the name as the
//! initial prompt (whisper then writes the name the way it is spelled).
//! No language detection: it encodes a whole 30 s window (about 0.5 s on
//! the CPU), where a decode of two seconds takes 30 to 60 ms. The listener
//! asks Russian first, English only when Russian missed the name:
//! measured on `say` clips, Russian with "Мия." hears the name even in
//! English speech ("Мия, open my mail"), and English catches what Russian
//! garbles ("Эй, Мия" as "A, Miya").

use std::path::Path;
use std::time::Instant;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};
#[cfg(whisper_stt)]
use whisper_rs::{WhisperVadContext, WhisperVadContextParams, WhisperVadParams};

use super::listen::Ears;
use crate::island::stt::vad::audio_ctx;
#[cfg(whisper_stt)]
use crate::island::stt::vad::RATE;
use crate::island::stt::vad::Span;

/// whisper.cpp's Silero on a Mac.
#[cfg(whisper_stt)]
type Vad = WhisperVadContext;
/// The ONNX Silero on Windows (no whisper.cpp detector file there).
#[cfg(not(whisper_stt))]
type Vad = crate::island::stt::silero::Silero;

/// Tokens a check may take.
const MAX_TOKENS: i32 = 12;

/// CPU threads for a decode (WAKE_THREADS overrides, for measuring).
fn threads() -> usize {
    std::env::var("WAKE_THREADS").ok().and_then(|s| s.parse().ok()).unwrap_or(THREADS)
}

const THREADS: usize = 2;

pub struct WhisperEars {
    _ctx: WhisperContext,
    state: WhisperState,
    vad: Vad,
}

impl WhisperEars {
    pub fn load(tiny: &Path, vad: &Path) -> Result<WhisperEars, String> {
        whisper_rs::install_logging_hooks();
        let started = Instant::now();
        let mut params = WhisperContextParameters::default();
        params.use_gpu(false);
        let ctx = WhisperContext::new_with_params(tiny.to_str().ok_or("the model path is not UTF-8")?, params).map_err(|e| format!("load: {e}"))?;
        let state = ctx.create_state().map_err(|e| format!("state: {e}"))?;
        let vad = load_vad(vad)?;
        connect_core::logln!("island: wake: whisper tiny loaded in {} ms", started.elapsed().as_millis());
        Ok(WhisperEars { _ctx: ctx, state, vad })
    }
}

#[cfg(whisper_stt)]
fn load_vad(vad: &Path) -> Result<Vad, String> {
    let mut vp = WhisperVadContextParams::new();
    vp.set_n_threads(1);
    vp.set_use_gpu(false);
    WhisperVadContext::new(vad.to_str().ok_or("the model path is not UTF-8")?, vp).map_err(|e| format!("voice detector: {e}"))
}

#[cfg(not(whisper_stt))]
fn load_vad(vad: &Path) -> Result<Vad, String> {
    crate::island::stt::silero::Silero::load(vad)
}

/// Where the speech is: the ONNX Silero with the settings set below on a
/// Mac (`silero::HOLD` is the same four numbers).
#[cfg(not(whisper_stt))]
fn speech(vad: &mut Vad, audio: &[f32]) -> Option<Vec<Span>> {
    vad.spans(audio)
}

#[cfg(whisper_stt)]
fn speech(vad: &mut Vad, audio: &[f32]) -> Option<Vec<Span>> {
    let mut params = WhisperVadParams::new();
    params.set_threshold(0.5);
    params.set_min_speech_duration(150);
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

impl Ears for WhisperEars {
    fn speech(&mut self, audio: &[f32]) -> Option<Vec<Span>> {
        if audio.is_empty() {
            return Some(Vec::new());
        }
        speech(&mut self.vad, audio)
    }

    fn hear(&mut self, audio: &[f32], lang: &str, prompt: &str) -> Result<String, String> {
        if audio.is_empty() {
            return Ok(String::new());
        }
        let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        p.set_n_threads(threads() as i32);
        p.set_language(Some(lang));
        p.set_translate(false);
        p.set_no_context(true);
        p.set_single_segment(true);
        p.set_suppress_blank(true);
        p.set_suppress_nst(true);
        // The name and a word or two after it are all a check needs.
        p.set_max_tokens(MAX_TOKENS);
        // No retries at higher temperatures: on long speech whisper.cpp
        // otherwise decodes a piece up to six times (measured: 237 ms a
        // check instead of 40).
        p.set_temperature_inc(0.0);
        p.set_initial_prompt(prompt);
        p.set_print_progress(false);
        p.set_print_realtime(false);
        p.set_print_special(false);
        p.set_print_timestamps(false);
        let ctx = audio_ctx(audio.len());
        if ctx > 0 {
            p.set_audio_ctx(ctx);
        }
        self.state.full(p, audio).map_err(|e| format!("decode: {e}"))?;
        let text: Vec<String> = self.state.as_iter().map(|s| s.to_str_lossy().map(|t| t.into_owned()).unwrap_or_default()).collect();
        Ok(text.join(" ").trim().to_string())
    }
}

