//! One hold, recognized while the key is down.
//!
//! The audio grows while the owner talks. A few times a second
//! (`Streamer::step`) the part not committed yet is looked at: when it
//! has a pause (vad.rs `cut`), the piece before the pause is decoded once
//! and its text committed, and that audio is never decoded again. What is
//! left after the cut is decoded too, as a preview for the island: after
//! every 0.7 s of new speech, and once more as soon as the speech stops.
//! So a step decodes a few seconds at most, however long the hold runs
//! (two minutes at most).
//!
//! On release (`finish`) only the piece after the last commit is decoded,
//! and usually not even that: people stop talking a moment before they
//! let go, and the preview made when the speech stopped is the text.
//! Until the language is known each piece is asked for it (`pick_lang`);
//! the first sure answer on a piece with enough speech is kept for the
//! rest of the hold. On a second of speech whisper may be sure of the
//! wrong one (Russian heard as English at 0.9), and a wrong language
//! makes whisper translate, so the bar is high.
//!
//! The decoder is a trait so the stitching and the cutting are tested
//! without a model (the engine is engine.rs).

use std::sync::atomic::AtomicBool;

use super::filter;
use super::vad::{self, Span, RATE};

/// What a decoder makes of a piece of audio.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Decoded {
    /// Whisper's segments, as written.
    pub segments: Vec<String>,
    /// The language it heard ("ru", "en") and how sure it was (0..1),
    /// when it was asked to find it.
    pub lang: Option<(String, f32)>,
}

pub trait Decoder {
    /// Transcribe 16 kHz mono audio. `lang` None: find the language.
    /// `abort`: set when the result is no longer wanted (the key was let
    /// go); then Err(ABORTED) as soon as the engine notices.
    ///
    /// No prompt with the text said before: whisper (large-v3-turbo) then
    /// writes the prompt again at the start of the piece.
    fn decode(&mut self, audio: &[f32], lang: Option<&str>, abort: Option<&AtomicBool>) -> Result<Decoded, String>;

    /// Where the speech is by the voice detector itself (Silero). None
    /// when it could not run (its model did not load, it failed).
    fn vad(&mut self, audio: &[f32]) -> Option<Vec<Span>>;

    /// Where the stream may cut a hold into pieces (vad.rs).
    fn cuts(&self) -> vad::Cuts {
        vad::Cuts::default()
    }

    /// Where the speech is: the voice detector, else the loudness
    /// detector.
    fn speech(&mut self, audio: &[f32]) -> Vec<Span> {
        self.vad(audio).unwrap_or_else(|| vad::energy_spans(audio))
    }
}

pub const ABORTED: &str = "aborted";

/// The voice detector's verdict on a whole hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vad {
    /// Speech was heard (by either detector).
    Speech,
    /// The voice detector went over the whole recording and found no
    /// speech, and neither did the loudness detector.
    Silence,
    /// There is no verdict: the voice detector could not run, or there
    /// was too little audio to judge.
    NotRun,
}

/// How a hold ended on the speech thread. What is sent is decided by
/// `release::decide`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heard {
    /// What was said, empty when nothing could be read.
    pub text: String,
    pub vad: Vad,
}

/// Less audio than this (a hold whose microphone started late) is not
/// judged silent or not.
const MIN_JUDGE: usize = RATE / 10;

/// A step looks only when there is this much new audio.
const MIN_STEP: usize = RATE / 10;
/// A preview only after this much more audio than the last one.
const PREVIEW_EVERY: usize = RATE * 7 / 10;
/// A preview only of this much speech or more: on less, whisper guesses
/// (and loops).
const PREVIEW_SPEECH: usize = RATE;
/// The language is taken from a piece with at least this much speech.
const LANG_SPEECH: usize = RATE * 3 / 2;
/// A piece with less speech than this is not decoded.
const MIN_PIECE_SPEECH: usize = RATE / 5;
/// The preview is the final text when the speech ended at least this long
/// before the end of the audio it was made of.
const PREVIEW_QUIET: usize = RATE * 15 / 100;

/// The language is kept only when whisper is at least this sure.
const LANG_SURE: f32 = 0.8;

/// The language to decode a piece in, from whisper's guess (`best`, how
/// sure it is) and its odds for Russian and English, the two expected
/// ones. A Russian speaker is often heard as one of the neighbours
/// (Ukrainian, Belarusian, ...): that is Russian. Another language wins
/// only when whisper is sure of it; else the likelier of Russian and
/// English.
pub fn pick_lang(best: &str, p_best: f32, p_ru: f32, p_en: f32) -> (String, f32) {
    match best {
        "ru" | "en" => (best.to_string(), p_best),
        "uk" | "be" | "bg" | "sr" | "mk" | "kk" => ("ru".to_string(), (p_best + p_ru).min(1.0)),
        _ if p_best >= LANG_SURE => (best.to_string(), p_best),
        _ if p_ru >= p_en => ("ru".to_string(), p_ru),
        _ => ("en".to_string(), p_en),
    }
}

#[derive(Debug, Default)]
pub struct Streamer {
    audio: Vec<f32>,
    /// Committed text, piece by piece.
    committed: Vec<String>,
    /// Audio up to here is committed.
    at: usize,
    lang: Option<String>,
    preview: String,
    /// The audio length the preview was made at.
    preview_len: usize,
    /// Audio length at the last step.
    stepped: usize,
    /// Some piece had speech in it.
    heard: bool,
    /// `finish` took the preview as the last piece.
    pub reused: bool,
    pub cuts: vad::Cuts,
}

impl Streamer {
    pub fn new() -> Streamer {
        Streamer::default()
    }

    /// More audio from the microphone.
    pub fn push(&mut self, samples: &[f32]) {
        self.audio.extend_from_slice(samples);
    }

    pub fn len(&self) -> usize {
        self.audio.len()
    }

    /// Audio not committed yet (what the release still has to decode).
    pub fn pending(&self) -> usize {
        self.audio.len() - self.at
    }

    /// The language found so far.
    pub fn lang(&self) -> Option<&str> {
        self.lang.as_deref()
    }

    /// The text so far, for the island: committed pieces and the preview.
    pub fn live(&self) -> String {
        let mut parts: Vec<&str> = self.committed.iter().map(String::as_str).collect();
        parts.push(&self.preview);
        filter::stitch(&parts)
    }

    fn clean(segments: Vec<String>) -> String {
        let kept: Vec<String> = filter::dedupe(segments).iter().filter_map(|s| filter::segment(s)).collect();
        filter::stitch(&kept)
    }

    /// Decode one piece; learn the language from it when it has enough
    /// speech.
    fn decode_piece(&mut self, dec: &mut dyn Decoder, from: usize, to: usize, speech: usize, abort: Option<&AtomicBool>) -> Result<String, String> {
        let d = dec.decode(&self.audio[from..to], self.lang.as_deref(), abort)?;
        if self.lang.is_none() && speech >= LANG_SPEECH {
            self.lang = d.lang.filter(|(_, p)| *p >= LANG_SURE).map(|(l, _)| l);
        }
        Ok(Self::clean(d.segments))
    }

    /// One look while the key is held: commit what is before a pause,
    /// refresh the preview. Ok(true) when the live text changed.
    pub fn step(&mut self, dec: &mut dyn Decoder, abort: &AtomicBool) -> Result<bool, String> {
        if self.audio.len() < self.stepped + MIN_STEP {
            return Ok(false);
        }
        self.stepped = self.audio.len();
        let before = self.live();
        let pending = self.audio.len() - self.at;
        let spans = dec.speech(&self.audio[self.at..]);
        let at = self.at;
        let audio = &self.audio;
        let cut = vad::cut(&spans, pending, &self.cuts, |from, to| vad::quietest(&audio[at..], from, to));
        let mut rest_spans = spans.clone();
        if let Some(cut) = cut {
            let piece = vad::speech_len(&vad::within(&spans, 0, cut));
            if piece >= MIN_PIECE_SPEECH {
                let text = self.decode_piece(dec, self.at, self.at + cut, piece, Some(abort))?;
                self.heard = true;
                if !text.is_empty() {
                    self.committed.push(text);
                }
            }
            self.at += cut;
            self.preview.clear();
            self.preview_len = 0;
            rest_spans = vad::within(&spans, cut, pending);
        }
        // A preview after enough new speech, or as soon as the speech
        // stopped (that one is usually the final text).
        let rest = vad::speech_len(&rest_spans);
        let pending = self.audio.len() - self.at;
        let previewed = self.preview_len.saturating_sub(self.at);
        let last_end = rest_spans.last().map(|s| s.1).unwrap_or(0);
        let unseen = last_end > previewed.saturating_sub(PREVIEW_QUIET) || self.preview.is_empty();
        let stopped = last_end + PREVIEW_QUIET <= pending;
        let grown = self.audio.len() >= self.preview_len + PREVIEW_EVERY;
        if rest >= PREVIEW_SPEECH && unseen && (grown || stopped) {
            let end = self.audio.len();
            self.preview = self.decode_piece(dec, self.at, end, rest, Some(abort))?;
            self.preview_len = end;
        }
        Ok(self.live() != before)
    }

    /// The key was let go: decode what is left and say how the hold ended.
    /// Usually the owner stops talking a moment before letting go: when
    /// the last preview already holds all of the speech, it is the text
    /// and nothing is decoded.
    pub fn finish(&mut self, dec: &mut dyn Decoder) -> Result<Heard, String> {
        let rest_spans = dec.speech(&self.audio[self.at..]);
        let rest = vad::speech_len(&rest_spans);
        let previewed = self.preview_len.saturating_sub(self.at);
        let covered = !self.preview.is_empty() && rest_spans.iter().all(|(_, e)| e + PREVIEW_QUIET <= previewed);
        self.reused = rest >= MIN_PIECE_SPEECH && covered;
        if rest >= MIN_PIECE_SPEECH {
            let end = self.audio.len();
            let text = if covered { std::mem::take(&mut self.preview) } else { self.decode_piece(dec, self.at, end, rest, None)? };
            self.heard = true;
            if !text.is_empty() {
                self.committed.push(text);
            }
            self.at = end;
        }
        self.preview.clear();
        let text = filter::whole(&filter::stitch(&self.committed));
        let vad = if !text.is_empty() || self.heard { Vad::Speech } else { self.verdict(dec) };
        Ok(Heard { text, vad })
    }

    /// Silence only when the voice detector itself went over the whole
    /// recording and neither it nor the loudness detector heard speech.
    fn verdict(&self, dec: &mut dyn Decoder) -> Vad {
        if self.audio.len() < MIN_JUDGE {
            return Vad::NotRun;
        }
        let Some(spans) = dec.vad(&self.audio) else {
            return Vad::NotRun;
        };
        let energy = vad::speech_len(&vad::energy_spans(&self.audio));
        if vad::speech_len(&spans) < vad::MIN_SPEECH && energy < vad::MIN_SPEECH {
            Vad::Silence
        } else {
            Vad::Speech
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island::stt::vad::tests::clip;
    use std::sync::atomic::Ordering;

    /// A decoder that reads the "words" off the clip: each speech span is
    /// one word, named by where it starts (tenths of a second), so a test
    /// knows what any piece should say.
    #[derive(Default)]
    struct Fake {
        calls: Vec<(usize, Option<String>)>,
        lang: &'static str,
        sure: f32,
        /// The text it writes for a piece without words.
        on_silence: Option<&'static str>,
        abort_at_call: Option<usize>,
        /// The voice detector could not run (its model is missing).
        no_vad: bool,
    }

    impl Decoder for Fake {
        fn decode(&mut self, audio: &[f32], lang: Option<&str>, abort: Option<&AtomicBool>) -> Result<Decoded, String> {
            self.calls.push((audio.len(), lang.map(String::from)));
            if self.abort_at_call == Some(self.calls.len()) && abort.is_some_and(|a| a.load(Ordering::Relaxed)) {
                return Err(ABORTED.into());
            }
            let spans = vad::energy_spans(audio);
            let mut words: Vec<String> = spans.iter().map(|(s, e)| format!("w{}", (s + e) / 2 * 10 / RATE)).collect();
            if words.is_empty() {
                words = self.on_silence.map(|s| vec![s.to_string()]).unwrap_or_default();
            }
            Ok(Decoded { segments: vec![words.join(" ")], lang: lang.is_none().then(|| (self.lang.to_string(), self.sure)) })
        }

        fn vad(&mut self, audio: &[f32]) -> Option<Vec<Span>> {
            (!self.no_vad).then(|| vad::energy_spans(audio))
        }
    }

    /// Play a clip into the streamer second by second, as the engine does.
    fn play(s: &mut Streamer, dec: &mut Fake, audio: &[f32]) {
        let never = AtomicBool::new(false);
        for chunk in audio.chunks(RATE) {
            s.push(chunk);
            s.step(dec, &never).unwrap();
        }
    }

    #[test]
    fn pieces_are_committed_at_pauses_and_each_is_decoded_once() {
        // Four "words" with pauses between them.
        let audio = clip(9.0, &[(0.5, 1.5), (2.2, 3.4), (4.2, 5.6), (6.4, 7.6)]);
        let mut dec = Fake { lang: "ru", sure: 1.0, ..Fake::default() };
        let mut s = Streamer::new();
        play(&mut s, &mut dec, &audio);
        let committed: usize = s.at;
        assert!(committed > 0, "something was committed while the key was held");
        // What finish decodes is only what came after the last commit.
        let before = dec.calls.len();
        let out = s.finish(&mut dec).unwrap();
        assert_eq!(out.vad, Vad::Speech);
        let text = out.text;
        assert_eq!(text.split(' ').count(), 4, "{text}");
        let tail = dec.calls[before..].iter().map(|c| c.0).sum::<usize>();
        assert!(tail <= audio.len() - committed, "only the tail is decoded at the end");
        assert!(tail <= 3 * RATE, "the tail is short: {tail}");
    }

    #[test]
    fn the_language_is_found_once_and_kept() {
        let audio = clip(8.0, &[(0.2, 2.2), (2.8, 4.5), (5.2, 7.0)]);
        let mut dec = Fake { lang: "ru", sure: 0.95, ..Fake::default() };
        let mut s = Streamer::new();
        play(&mut s, &mut dec, &audio);
        s.finish(&mut dec).unwrap();
        assert_eq!(s.lang(), Some("ru"));
        let asked: Vec<Option<String>> = dec.calls.iter().map(|c| c.1.clone()).collect();
        let first_known = asked.iter().position(Option::is_some).expect("a language was set");
        assert!(asked[first_known..].iter().all(|l| l.as_deref() == Some("ru")), "{asked:?}");
    }

    #[test]
    fn an_unsure_language_is_asked_again() {
        let audio = clip(6.0, &[(0.2, 2.2), (2.8, 4.5), (5.0, 5.8)]);
        let mut dec = Fake { lang: "en", sure: 0.6, ..Fake::default() };
        let mut s = Streamer::new();
        play(&mut s, &mut dec, &audio);
        s.finish(&mut dec).unwrap();
        assert_eq!(s.lang(), None);
        assert!(dec.calls.iter().all(|c| c.1.is_none()), "every piece is asked: {:?}", dec.calls);
    }

    #[test]
    fn the_language_is_russian_or_english_unless_whisper_is_sure() {
        assert_eq!(pick_lang("ru", 0.6, 0.6, 0.3), ("ru".into(), 0.6));
        assert_eq!(pick_lang("en", 0.9, 0.02, 0.9), ("en".into(), 0.9));
        // A neighbour of Russian is Russian.
        assert_eq!(pick_lang("uk", 0.5, 0.4, 0.05).0, "ru");
        assert!(pick_lang("uk", 0.5, 0.4, 0.05).1 > 0.85);
        // Another language when whisper is sure of it.
        assert_eq!(pick_lang("de", 0.9, 0.01, 0.05), ("de".into(), 0.9));
        // Unsure: the likelier of the two.
        assert_eq!(pick_lang("es", 0.4, 0.1, 0.3), ("en".into(), 0.3));
        assert_eq!(pick_lang("ja", 0.3, 0.25, 0.2), ("ru".into(), 0.25));
    }

    #[test]
    fn a_silent_hold_is_silent_and_says_nothing() {
        let audio = clip(4.0, &[]);
        let mut dec = Fake { lang: "ru", sure: 1.0, on_silence: Some("Продолжение следует..."), ..Fake::default() };
        let mut s = Streamer::new();
        play(&mut s, &mut dec, &audio);
        assert_eq!(s.finish(&mut dec).unwrap(), Heard { text: String::new(), vad: Vad::Silence });
        assert!(dec.calls.is_empty(), "silence is never decoded");
    }

    #[test]
    fn without_the_voice_detector_there_is_no_silence_verdict() {
        // The same silent hold, but the voice detector could not run: the
        // loudness detector alone never drops a hold.
        let audio = clip(4.0, &[]);
        let mut dec = Fake { lang: "ru", sure: 1.0, no_vad: true, ..Fake::default() };
        let mut s = Streamer::new();
        play(&mut s, &mut dec, &audio);
        assert_eq!(s.finish(&mut dec).unwrap(), Heard { text: String::new(), vad: Vad::NotRun });
        // Speech without the voice detector is still heard (by loudness).
        let mut dec = Fake { lang: "ru", sure: 1.0, no_vad: true, ..Fake::default() };
        let mut s = Streamer::new();
        play(&mut s, &mut dec, &clip(3.0, &[(0.3, 2.0)]));
        let out = s.finish(&mut dec).unwrap();
        assert!(!out.text.is_empty() && out.vad == Vad::Speech, "{out:?}");
    }

    #[test]
    fn too_little_audio_is_not_judged() {
        // A hold whose microphone started late: a few samples only.
        let mut dec = Fake { lang: "ru", sure: 1.0, ..Fake::default() };
        let mut s = Streamer::new();
        s.push(&clip(0.05, &[]));
        assert_eq!(s.finish(&mut dec).unwrap().vad, Vad::NotRun);
        let mut s = Streamer::new();
        assert_eq!(s.finish(&mut dec).unwrap().vad, Vad::NotRun, "no audio at all");
    }

    #[test]
    fn speech_that_reads_as_a_known_hallucination_goes_as_audio() {
        // The decoder hears only "Спасибо." in a hold that has sound.
        struct Thanks;
        impl Decoder for Thanks {
            fn decode(&mut self, _: &[f32], _: Option<&str>, _: Option<&AtomicBool>) -> Result<Decoded, String> {
                Ok(Decoded { segments: vec![" Спасибо.".into()], lang: Some(("ru".into(), 1.0)) })
            }
            fn vad(&mut self, audio: &[f32]) -> Option<Vec<Span>> {
                Some(vad::energy_spans(audio))
            }
        }
        let mut s = Streamer::new();
        s.push(&clip(2.0, &[(0.3, 1.5)]));
        assert_eq!(s.finish(&mut Thanks).unwrap(), Heard { text: String::new(), vad: Vad::Speech });
    }

    #[test]
    fn a_step_cut_short_by_the_release_leaves_nothing_half_done() {
        let audio = clip(6.0, &[(0.3, 1.8), (2.5, 4.2), (4.9, 5.8)]);
        let mut dec = Fake { lang: "ru", sure: 1.0, abort_at_call: Some(2), ..Fake::default() };
        let mut s = Streamer::new();
        let abort = AtomicBool::new(false);
        let mut aborted = false;
        for chunk in audio.chunks(RATE) {
            s.push(chunk);
            if dec.calls.len() == 1 {
                abort.store(true, Ordering::Relaxed);
            }
            match s.step(&mut dec, &abort) {
                Ok(_) => {}
                Err(e) => {
                    assert_eq!(e, ABORTED);
                    aborted = true;
                    break;
                }
            }
        }
        assert!(aborted);
        // Everything still adds up at the end.
        let rest = audio.len() - s.len();
        s.push(&audio[audio.len() - rest..]);
        let text = s.finish(&mut dec).unwrap().text;
        assert_eq!(text.split(' ').count(), 3, "{text}");
    }

    #[test]
    fn the_live_text_grows_while_the_key_is_held() {
        let audio = clip(5.0, &[(0.2, 1.6), (2.3, 3.6)]);
        let mut dec = Fake { lang: "ru", sure: 1.0, ..Fake::default() };
        let mut s = Streamer::new();
        let never = AtomicBool::new(false);
        let mut seen = Vec::new();
        for chunk in audio.chunks(RATE) {
            s.push(chunk);
            if s.step(&mut dec, &never).unwrap() {
                seen.push(s.live());
            }
        }
        assert!(!seen.is_empty());
        assert!(seen.last().unwrap().split(' ').count() >= 2, "{seen:?}");
    }
}
