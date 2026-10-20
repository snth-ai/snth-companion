//! The listener: the always-on microphone's audio in, "the name was said"
//! and "the request ended" out. Apart from the microphone and the models
//! so it can be tested: the voice detector and whisper come in as `Ears`.
//!
//! Listening, every 30 ms frame is weighed against the room's own noise
//! floor (falls at once, rises slowly). A run of loud frames is a stretch
//! that may be speech; it ends after `GAP` of quiet. A stretch is checked
//! once: when it has gone on for `CHECK`, or when it ended before that
//! ("Мия" alone). The check asks the voice detector (Silero) whether there
//! is speech at all, then whisper what the first `CHECK` of it says (in
//! Russian, then in English if Russian nearly heard the name: `LANGS`),
//! and `Names` whether that starts with the name. So in a quiet room
//! nothing but the loudness runs, and only the start of each stretch of
//! speech is ever decoded.
//!
//! After the wake the recording goes on until the voice detector hears
//! `TRAILING` of silence (looked at every `END_EVERY`), at most
//! `MAX_WAKE`. "Мия" alone waits `NAME_WAIT` for the request; nothing
//! more said, the wake is dropped. After a wake `COOLDOWN` of audio is
//! skipped, so the tail of the request does not wake it again.
//!
//! Time is counted in samples (16 kHz), from the first sample pushed.

use super::names::{Hit, Names};
use crate::island::stt::vad::{self, Span, RATE};

/// 30 ms.
const FRAME: usize = RATE * 30 / 1000;
/// Audio kept before the speech, so the start of the name is not cut.
pub const PREROLL: usize = RATE / 2;
/// The name check looks at this much of a stretch, from its start.
pub const CHECK: usize = RATE * 2;
/// Quiet that ends a stretch.
const GAP: usize = RATE * 4 / 10;
/// Loud audio a stretch needs before it is worth a check.
const MIN_LOUD: usize = RATE * 15 / 100;
/// The languages a check decodes in, in order, until one hears the name.
pub const LANGS: [&str; 2] = ["ru", "en"];
/// Less speech than this (by the voice detector) is not worth whisper.
const MIN_SPEECH: usize = RATE / 5;
/// Silence that ends the request.
pub const TRAILING: usize = RATE;
/// "Мия" alone: how long the request is waited for.
pub const NAME_WAIT: usize = RATE * 6;
/// The longest request.
pub const MAX_WAKE: usize = RATE * 60;
/// Audio skipped after a wake.
pub const COOLDOWN: usize = RATE * 2;
/// Audio kept while listening (a stretch, its pre-roll and a margin).
pub const KEEP: usize = RATE * 4;
/// How often the end of the request is looked for, and over how much.
const END_EVERY: usize = RATE * 3 / 10;
const END_WINDOW: usize = RATE * 2;
/// A frame is loud this many dB over the floor, and never below this.
const OVER_FLOOR: f32 = 10.0;
const LOUD_MIN_DB: f32 = -55.0;
/// How fast the floor rises, per frame (about 1.7 dB a second).
const FLOOR_RISE: f32 = 0.05;

/// The voice detector and whisper, as the listener uses them.
pub trait Ears {
    /// Where the speech is in this audio (Silero). None: it could not run
    /// (the loudness stands in).
    fn speech(&mut self, audio: &[f32]) -> Option<Vec<Span>>;
    /// What is said in this audio, decoded in this language ("ru",
    /// "en") with the names as a hint.
    fn hear(&mut self, audio: &[f32], lang: &str, prompt: &str) -> Result<String, String>;
}

/// The name was heard.
#[derive(Debug, Clone, PartialEq)]
pub struct Wake {
    /// Where the request's audio starts (absolute sample, the pre-roll
    /// included).
    pub from: usize,
    pub hit: Hit,
    /// The stretch ended with the name: the request is still to come.
    pub name_only: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Wake(Wake),
    /// The request ended: send it.
    Send(&'static str),
    /// Nothing followed the name: send nothing.
    Drop(&'static str),
}

#[derive(Debug, Clone, Copy)]
struct Stretch {
    start: usize,
    loud: usize,
    last_loud: usize,
    checked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Listening,
    Woke { at: usize, from: usize, name_only: bool, name_end: usize, last_speech: usize, next_end: usize },
    Cooldown { until: usize },
}

pub struct Listener {
    audio: Vec<f32>,
    /// The absolute index of `audio[0]`.
    start: usize,
    /// Frames are weighed up to here (absolute).
    framed: usize,
    floor: Option<f32>,
    stretch: Option<Stretch>,
    state: State,
    /// Name checks run (whisper calls), for the measurements.
    pub checks: u32,
}

impl Default for Listener {
    fn default() -> Self {
        Listener::new()
    }
}

fn frame_db(frame: &[f32]) -> f32 {
    let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len().max(1) as f32).sqrt();
    20.0 * rms.max(1e-7).log10()
}

impl Listener {
    pub fn new() -> Listener {
        Listener { audio: Vec::with_capacity(KEEP * 2), start: 0, framed: 0, floor: None, stretch: None, state: State::Listening, checks: 0 }
    }

    /// The absolute index of the next sample.
    pub fn now(&self) -> usize {
        self.start + self.audio.len()
    }

    pub fn woke(&self) -> bool {
        matches!(self.state, State::Woke { .. })
    }

    /// Audio before this (absolute) is no longer needed.
    pub fn keep_from(&self) -> usize {
        match self.state {
            State::Woke { from, .. } => from,
            _ => self.now().saturating_sub(KEEP),
        }
    }

    /// A wake ended (sent, dropped, cancelled): skip `COOLDOWN` of audio.
    pub fn cooldown(&mut self) {
        self.state = State::Cooldown { until: self.now() + COOLDOWN };
        self.stretch = None;
    }

    /// Paused: forget the audio and the room.
    pub fn reset(&mut self) {
        let now = self.now();
        self.audio.clear();
        self.start = now;
        self.framed = now;
        self.floor = None;
        self.stretch = None;
        self.state = State::Listening;
    }

    fn at(&self, abs: usize) -> usize {
        abs.saturating_sub(self.start).min(self.audio.len())
    }

    fn slice(&self, from: usize, to: usize) -> &[f32] {
        &self.audio[self.at(from)..self.at(to)]
    }

    fn trim(&mut self) {
        let keep = match self.state {
            State::Woke { from, .. } => from.max(self.now().saturating_sub(END_WINDOW + RATE)),
            _ => self.now().saturating_sub(KEEP),
        };
        // In batches: a second at a time at least.
        if keep > self.start + RATE {
            let n = self.at(keep);
            self.audio.drain(..n);
            self.start += n;
        }
    }

    /// New samples (16 kHz mono). Returns what happened, if anything.
    pub fn push(&mut self, samples: &[f32], ears: &mut dyn Ears, names: &Names) -> Option<Event> {
        self.audio.extend_from_slice(samples);
        let event = match self.state {
            State::Cooldown { until } => {
                if self.now() >= until {
                    self.state = State::Listening;
                    self.framed = self.now();
                }
                None
            }
            State::Listening => self.listen(ears, names),
            State::Woke { .. } => self.follow(ears),
        };
        self.trim();
        event
    }

    fn listen(&mut self, ears: &mut dyn Ears, names: &Names) -> Option<Event> {
        self.framed = self.framed.max(self.start);
        while self.framed + FRAME <= self.now() {
            let at = self.framed;
            let end = at + FRAME;
            let db = frame_db(self.slice(at, end));
            let floor = *self.floor.get_or_insert(db);
            let loud = db > (floor + OVER_FLOOR).max(LOUD_MIN_DB);
            self.floor = Some(if db < floor { db.max(-90.0) } else { (floor + FLOOR_RISE).min(db) });
            self.framed = end;
            if loud {
                match self.stretch.as_mut() {
                    Some(s) => {
                        s.loud += FRAME;
                        s.last_loud = end;
                    }
                    None => self.stretch = Some(Stretch { start: at, loud: FRAME, last_loud: end, checked: false }),
                }
            } else if let Some(s) = self.stretch {
                if end - s.last_loud >= GAP {
                    self.stretch = None;
                    if !s.checked && s.loud >= MIN_LOUD {
                        if let Some(e) = self.check(s, true, ears, names) {
                            return Some(e);
                        }
                    }
                }
            }
            if let Some(s) = self.stretch.as_mut() {
                if !s.checked && s.loud >= MIN_LOUD && end >= s.start + CHECK {
                    s.checked = true;
                    let s = *s;
                    if let Some(e) = self.check(s, false, ears, names) {
                        return Some(e);
                    }
                }
            }
        }
        None
    }

    /// Is this stretch the owner calling? `ended`: it is over already.
    fn check(&mut self, s: Stretch, ended: bool, ears: &mut dyn Ears, names: &Names) -> Option<Event> {
        let now = self.now();
        let w0 = s.start.saturating_sub(PREROLL).max(self.start);
        let w1 = (s.start + CHECK).min(now);
        let window = self.slice(w0, w1);
        let spans = ears.speech(window).unwrap_or_else(|| vad::energy_spans(window));
        if vad::speech_len(&spans) < MIN_SPEECH {
            return None;
        }
        let s0 = w0 + spans[0].0;
        let c0 = s0.saturating_sub(RATE / 5).max(self.start);
        let c1 = (s0 + CHECK).min(now);
        let mut hit = None;
        for lang in LANGS {
            self.checks += 1;
            let text = ears.hear(self.slice(c0, c1), lang, &names.prompt(lang)).ok()?;
            hit = names.find(&text);
            // The next language only for a near miss ("Ме, ..."): plain
            // talk is decoded once.
            if hit.is_some() || !names.near(&text) {
                break;
            }
        }
        let hit = hit?;
        let name_only = ended && !hit.rest.chars().any(char::is_alphanumeric);
        let from = s0.saturating_sub(PREROLL).max(self.start);
        self.stretch = None;
        self.state = State::Woke {
            at: now,
            from,
            name_only,
            name_end: s.last_loud,
            last_speech: if ended { s.last_loud } else { now },
            next_end: now + END_EVERY,
        };
        Some(Event::Wake(Wake { from, hit, name_only }))
    }

    fn follow(&mut self, ears: &mut dyn Ears) -> Option<Event> {
        let now = self.now();
        let State::Woke { at, from, mut name_only, name_end, mut last_speech, next_end } = self.state else {
            return None;
        };
        if now.saturating_sub(from) >= MAX_WAKE {
            self.cooldown();
            return Some(Event::Send("the 60 s limit"));
        }
        if now < next_end {
            return None;
        }
        let w0 = now.saturating_sub(END_WINDOW).max(self.start).max(from);
        let window = self.slice(w0, now);
        let spans = ears.speech(window).unwrap_or_else(|| vad::energy_spans(window));
        if let Some(last) = spans.last() {
            last_speech = last_speech.max(w0 + last.1);
        }
        // Speech that starts after the name: the request has come.
        if name_only && spans.iter().any(|sp| w0 + sp.0 > name_end + RATE / 10) {
            name_only = false;
        }
        self.state = State::Woke { at, from, name_only, name_end, last_speech, next_end: now + END_EVERY };
        if name_only {
            if now - at >= NAME_WAIT {
                self.cooldown();
                return Some(Event::Drop("nothing followed the name"));
            }
        } else if now.saturating_sub(last_speech) >= TRAILING {
            self.cooldown();
            return Some(Event::Send("a second of silence"));
        }
        None
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// A stand-in: the loudness is the voice detector, whisper says the
    /// next line of a script.
    pub struct FakeEars {
        pub said: VecDeque<&'static str>,
        /// The length of each stretch decoded (the first language only).
        pub heard: Vec<usize>,
        last: &'static str,
    }

    impl FakeEars {
        pub fn new(said: &[&'static str]) -> FakeEars {
            FakeEars { said: said.iter().copied().collect(), heard: Vec::new(), last: "" }
        }
    }

    impl Ears for FakeEars {
        fn speech(&mut self, audio: &[f32]) -> Option<Vec<Span>> {
            Some(vad::energy_spans(audio))
        }

        fn hear(&mut self, audio: &[f32], lang: &str, prompt: &str) -> Result<String, String> {
            assert_eq!(prompt, if lang == "ru" { "Мия." } else { "Mia." });
            // The same speech in either language: the next line once.
            if lang == LANGS[0] {
                self.heard.push(audio.len());
                self.last = self.said.pop_front().unwrap_or("");
            }
            Ok(self.last.to_string())
        }
    }

    pub fn names() -> Names {
        Names::new("Mia", "")
    }

    /// Feed a clip in 100 ms blocks, as the microphone does. The events,
    /// with the second each came at.
    pub fn run(l: &mut Listener, ears: &mut dyn Ears, clip: &[f32]) -> Vec<(f32, Event)> {
        let mut out = Vec::new();
        for block in clip.chunks(RATE / 10) {
            if let Some(e) = l.push(block, ears, &names()) {
                out.push((l.now() as f32 / RATE as f32, e));
            }
        }
        out
    }

    fn clip(secs: f32, on: &[(f32, f32)]) -> Vec<f32> {
        vad::tests::clip(secs, on)
    }

    fn secs(n: usize) -> f32 {
        (n as f32 / RATE as f32 * 10.0).round() / 10.0
    }

    #[test]
    fn a_quiet_room_wakes_nothing_and_decodes_nothing() {
        let mut l = Listener::new();
        let mut ears = FakeEars::new(&["Мия, открой почту"]);
        assert!(run(&mut l, &mut ears, &clip(30.0, &[])).is_empty());
        assert_eq!(l.checks, 0, "whisper never ran");
        assert!(l.audio.len() <= KEEP + RATE, "the audio kept is bounded: {}", l.audio.len());
    }

    #[test]
    fn the_name_and_a_request_wake_record_and_send_after_a_second_of_silence() {
        let mut l = Listener::new();
        let mut ears = FakeEars::new(&["Мия, найди билеты в Бангкок"]);
        // Speech 1.0-4.0 s, then silence.
        let ev = run(&mut l, &mut ears, &clip(8.0, &[(1.0, 4.0)]));
        assert_eq!(ev.len(), 2, "{ev:?}");
        let (t, Event::Wake(w)) = &ev[0] else { panic!("{ev:?}") };
        assert!((3.0..=3.2).contains(t), "checked once two seconds of it are there: {t}");
        assert_eq!(w.hit.rest, "Найди билеты в Бангкок");
        assert!(!w.name_only);
        assert!((0.4..=0.6).contains(&secs(w.from)), "half a second before the speech: {}", secs(w.from));
        let (t, e) = &ev[1];
        assert_eq!(*e, Event::Send("a second of silence"));
        assert!((5.0..=5.5).contains(t), "about a second after the speech ended: {t}");
        assert_eq!(ears.heard.len(), 1);
        assert!(ears.heard[0] <= CHECK + RATE / 5, "only the start is decoded");
    }

    #[test]
    fn the_name_alone_waits_for_the_request() {
        // "Мия", a pause of 2 s, the request.
        let mut l = Listener::new();
        let mut ears = FakeEars::new(&["Мия."]);
        let ev = run(&mut l, &mut ears, &clip(10.0, &[(1.0, 1.5), (3.5, 6.0)]));
        let (_, Event::Wake(w)) = &ev[0] else { panic!("{ev:?}") };
        assert!(w.name_only);
        let (t, e) = &ev[1];
        assert_eq!(*e, Event::Send("a second of silence"));
        assert!((7.0..=7.5).contains(t), "{t}");
        assert_eq!(ev.len(), 2, "{ev:?}");

        // "Мия" and then nothing: dropped after about 6 s.
        let mut l = Listener::new();
        let mut ears = FakeEars::new(&["Мия."]);
        let ev = run(&mut l, &mut ears, &clip(12.0, &[(1.0, 1.5)]));
        assert_eq!(ev.len(), 2, "{ev:?}");
        let (t, e) = &ev[1];
        assert_eq!(*e, Event::Drop("nothing followed the name"));
        assert!((7.8..=8.3).contains(t), "{t}");
    }

    #[test]
    fn other_speech_and_the_name_mid_sentence_do_not_wake() {
        let mut l = Listener::new();
        let mut ears = FakeEars::new(&["я вчера видел Мию", "Сегодня хорошая погода", "Мир такой большой"]);
        let ev = run(&mut l, &mut ears, &clip(14.0, &[(1.0, 3.0), (4.0, 7.5), (9.0, 10.0)]));
        assert!(ev.is_empty(), "{ev:?}");
        assert_eq!(ears.heard.len(), 3, "each stretch checked once");
        assert_eq!(l.checks, 4, "plain talk in one language, the near miss \"Мир\" in both");
    }

    #[test]
    fn a_long_request_is_cut_at_a_minute() {
        let mut l = Listener::new();
        let mut ears = FakeEars::new(&["Мия, слушай"]);
        let ev = run(&mut l, &mut ears, &clip(70.0, &[(1.0, 69.0)]));
        assert_eq!(ev.len(), 2, "{ev:?}");
        let (t, e) = &ev[1];
        assert_eq!(*e, Event::Send("the 60 s limit"));
        assert!((60.4..=60.7).contains(t), "{t}");
        assert!(l.audio.len() <= END_WINDOW + 3 * RATE, "the listener's own copy stays short: {}", l.audio.len());
    }

    #[test]
    fn after_a_wake_the_tail_does_not_wake_again() {
        let mut l = Listener::new();
        let mut ears = FakeEars::new(&["Мия, открой почту", "Мия, и ещё"]);
        // The request ends at 3.0; more speech at 4.5 (inside the
        // cooldown), and a new stretch at 9.0 that wakes again.
        let ev = run(&mut l, &mut ears, &clip(14.0, &[(1.0, 3.0), (4.5, 5.0), (9.0, 11.5)]));
        let wakes = ev.iter().filter(|(_, e)| matches!(e, Event::Wake(_))).count();
        assert_eq!(wakes, 2, "{ev:?}");
        assert_eq!(ears.heard.len(), 2, "the stretch inside the cooldown was never checked");
    }

    #[test]
    fn a_cancel_cools_down_and_a_pause_forgets() {
        let mut l = Listener::new();
        let mut ears = FakeEars::new(&["Мия, открой", "Мия, снова"]);
        let ev = run(&mut l, &mut ears, &clip(3.2, &[(1.0, 4.0)]));
        assert!(matches!(ev[0].1, Event::Wake(_)));
        assert!(l.woke());
        // The owner pressed Escape.
        l.cooldown();
        assert!(!l.woke());
        assert!(run(&mut l, &mut ears, &clip(1.5, &[(0.0, 1.0)])).is_empty(), "cooling down");
        l.reset();
        assert_eq!(l.audio.len(), 0);
        let ev = run(&mut l, &mut ears, &clip(5.0, &[(1.0, 2.0)]));
        assert!(matches!(ev[0].1, Event::Wake(_)), "{ev:?}");
    }
}
