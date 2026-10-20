//! What a released hold sends, decided in one place.
//!
//! The rule (tz-local-stt.md): a hold is never lost. A hold of at least
//! 300 ms (hold.rs) sends something, the text recognized here or the
//! audio exactly as on the synth's path, unless the voice detector went
//! over the whole recording and found no speech in it. "The engine had
//! nothing yet" is never silence: an engine that is loading, failed, was
//! too slow or whose thread is gone sends the audio, and so does a voice
//! detector that could not run. A silent screenshot + voice hold still
//! sends its screenshot, with no voice.
//!
//! `decide` is pure so every combination is tested; the island (mod.rs)
//! gathers its inputs and acts on the answer, with one log line each.

use super::stream::Vad;
use super::RELEASE_WAIT;

/// Where the speech engine is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EngineState {
    /// Not loaded (never yet, or unloaded after an idle while).
    Unloaded = 0,
    Loading = 1,
    Ready = 2,
    /// The last load failed.
    Failed = 3,
    /// The speech thread ended (a panic): the next hold starts it again.
    Gone = 4,
}

impl EngineState {
    pub fn from_u8(v: u8) -> EngineState {
        match v {
            1 => EngineState::Loading,
            2 => EngineState::Ready,
            3 => EngineState::Failed,
            4 => EngineState::Gone,
            _ => EngineState::Unloaded,
        }
    }

    /// Why the audio goes while the engine is like this.
    pub fn reason(self) -> &'static str {
        match self {
            EngineState::Unloaded => "the model was not loaded",
            EngineState::Loading => "the model was still loading",
            EngineState::Ready => "the engine answered nothing",
            EngineState::Failed => "the model did not load",
            EngineState::Gone => "the speech thread was gone",
        }
    }
}

/// What the island knows when the speech side is done with a hold (or
/// gave up on it).
#[derive(Debug, Clone, Copy)]
pub struct AtRelease<'a> {
    /// The engine at the moment of the decision.
    pub engine: EngineState,
    /// The voice detector's verdict over the whole recording (NotRun
    /// when the speech thread did not answer).
    pub vad: Vad,
    /// The text recognized (None when the speech thread did not answer).
    pub text: Option<&'a str>,
    /// No answer within `RELEASE_WAIT` of the release.
    pub timed_out: bool,
    /// The speech thread answered with an error instead.
    pub error: Option<&'a str>,
    /// A screenshot was taken for this hold (screen_voice).
    pub picture: bool,
}

/// What the hold sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Release {
    /// The text recognized here (`voice_transcript`, no audio).
    Text(String),
    /// The audio, as on the synth's path, and why.
    Audio(String),
    /// A silent screenshot + voice hold: the screenshot alone.
    PictureOnly,
    /// The voice detector heard no speech in the whole recording and
    /// there is no screenshot: nothing goes.
    Silent,
}

pub fn decide(r: &AtRelease) -> Release {
    let answered = !r.timed_out && r.error.is_none();
    if answered {
        if let Some(text) = r.text.map(str::trim).filter(|t| !t.is_empty()) {
            return Release::Text(text.to_string());
        }
    }
    if r.timed_out {
        return Release::Audio(match r.engine {
            EngineState::Ready => format!("no text within {:.1} s of the release", RELEASE_WAIT.as_secs_f32()),
            other => other.reason().to_string(),
        });
    }
    if let Some(e) = r.error {
        return Release::Audio(e.to_string());
    }
    // An answer with no text: only a ready engine's voice detector may
    // call the hold silent.
    if r.engine != EngineState::Ready {
        return Release::Audio(r.engine.reason().to_string());
    }
    match r.vad {
        Vad::Silence if r.picture => Release::PictureOnly,
        Vad::Silence => Release::Silent,
        Vad::NotRun => Release::Audio("the voice detector could not run".into()),
        Vad::Speech => Release::Audio("no text was read from the speech".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENGINES: [EngineState; 5] = [EngineState::Unloaded, EngineState::Loading, EngineState::Ready, EngineState::Failed, EngineState::Gone];
    const VADS: [Vad; 3] = [Vad::Speech, Vad::Silence, Vad::NotRun];
    const TEXTS: [Option<&str>; 4] = [None, Some(""), Some("  "), Some("Найди мне рейс")];
    const ERRORS: [Option<&str>; 2] = [None, Some("decode: failed")];

    fn at<'a>(engine: EngineState, vad: Vad, text: Option<&'a str>, timed_out: bool, error: Option<&'a str>, picture: bool) -> AtRelease<'a> {
        AtRelease { engine, vad, text, timed_out, error, picture }
    }

    /// Every combination: text only from an answer with text, nothing
    /// (or the screenshot alone) only from a ready engine's voice
    /// detector that heard silence, the audio in every other case.
    #[test]
    fn every_combination_sends_something_unless_the_vad_heard_silence() {
        let mut seen = 0;
        for engine in ENGINES {
            for vad in VADS {
                for text in TEXTS {
                    for timed_out in [false, true] {
                        for error in ERRORS {
                            for picture in [false, true] {
                                let r = at(engine, vad, text, timed_out, error, picture);
                                let got = decide(&r);
                                seen += 1;
                                let has_text = text.is_some_and(|t| !t.trim().is_empty());
                                let answered = !timed_out && error.is_none();
                                let silent = answered && !has_text && engine == EngineState::Ready && vad == Vad::Silence;
                                let want = if answered && has_text {
                                    Release::Text(text.unwrap().trim().to_string())
                                } else if silent && picture {
                                    Release::PictureOnly
                                } else if silent {
                                    Release::Silent
                                } else {
                                    match got {
                                        Release::Audio(ref why) => {
                                            assert!(!why.is_empty(), "{r:?}: a reason for the log");
                                            got.clone()
                                        }
                                        ref other => panic!("{r:?}: must send the audio, got {other:?}"),
                                    }
                                };
                                assert_eq!(got, want, "{r:?}");
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(seen, 5 * 3 * 4 * 2 * 2 * 2);
    }

    #[test]
    fn the_reasons_say_what_happened() {
        let silent = |engine| at(engine, Vad::Silence, Some(""), false, None, false);
        // The first hold after the download: the model still loading at
        // the release. Never "silence".
        assert_eq!(decide(&at(EngineState::Loading, Vad::NotRun, None, true, None, false)), Release::Audio("the model was still loading".into()));
        assert_eq!(decide(&silent(EngineState::Loading)), Release::Audio("the model was still loading".into()));
        assert_eq!(decide(&at(EngineState::Unloaded, Vad::NotRun, None, true, None, true)), Release::Audio("the model was not loaded".into()));
        assert_eq!(decide(&at(EngineState::Ready, Vad::NotRun, None, true, None, false)), Release::Audio("no text within 2.5 s of the release".into()));
        assert_eq!(decide(&at(EngineState::Gone, Vad::NotRun, None, false, Some("the speech thread is gone"), false)), Release::Audio("the speech thread is gone".into()));
        assert_eq!(decide(&silent(EngineState::Failed)), Release::Audio("the model did not load".into()));
        // A ready engine.
        assert_eq!(decide(&silent(EngineState::Ready)), Release::Silent);
        assert_eq!(decide(&at(EngineState::Ready, Vad::Silence, None, false, None, true)), Release::PictureOnly);
        assert_eq!(decide(&at(EngineState::Ready, Vad::NotRun, Some(""), false, None, false)), Release::Audio("the voice detector could not run".into()));
        assert_eq!(decide(&at(EngineState::Ready, Vad::Speech, Some(""), false, None, false)), Release::Audio("no text was read from the speech".into()));
        assert_eq!(decide(&at(EngineState::Ready, Vad::Speech, Some(" Привет "), false, None, true)), Release::Text("Привет".into()));
    }

    #[test]
    fn the_engine_state_round_trips() {
        for e in ENGINES {
            assert_eq!(EngineState::from_u8(e as u8), e);
        }
        assert_eq!(EngineState::from_u8(200), EngineState::Unloaded);
    }
}
