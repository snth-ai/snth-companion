//! The tone as one short Russian line for the synth (owner.message
//! `voice_tone`): what stands out against the owner's usual manner, and
//! an emotion only when the model is sure of it.
//!
//! «Темп выше обычного, голос громче и выше обычного; похоже на
//! раздражение (уверенность низкая).»
//!
//! With fewer than `baseline::MIN_HOLDS` holds learned nothing is called
//! usual or unusual: only wide absolute bands are used (very fast, very
//! slow, long pauses, monotone). Nothing notable: «Обычная манера, ничего
//! необычного.» once the baseline exists, else nothing at all (the field
//! is left out).

use super::baseline::{semitones, Baseline};
use super::emotion::{Emotion, Mood};
use super::prosody::Features;

/// The line is never longer than this.
pub const MAX_CHARS: usize = 300;
/// This many spreads from the usual is unusual.
const Z: f32 = 1.2;
/// The smallest spread each feature is judged with.
const FLOOR_RATE: f32 = 0.4;
const FLOOR_PAUSE: f32 = 0.05;
const FLOOR_LOUD: f32 = 2.0;
const FLOOR_PITCH: f32 = 1.0;
const FLOOR_RANGE: f32 = 1.0;
/// Absolute bands, before there is a baseline (syllables per second of
/// speech; Russian conversation runs at about 4 to 6).
const FAST: f32 = 7.0;
const SLOW: f32 = 2.8;
const LONG_PAUSE: f32 = 2.0;
const MANY_PAUSES: f32 = 0.35;
const MONOTONE: f32 = 2.5;
const LIVELY: f32 = 12.0;
/// "Almost no pauses" needs this much speech to mean anything.
const NO_PAUSE_SPEECH: f32 = 4.0;

pub const NOTHING_NOTABLE: &str = "Обычная манера, ничего необычного.";

/// Above (+1), below (-1) or within (0) the usual.
fn side(z: Option<f32>) -> i8 {
    match z {
        Some(z) if z >= Z => 1,
        Some(z) if z <= -Z => -1,
        _ => 0,
    }
}

pub fn describe(f: &Features, base: &Baseline, emotion: Option<&Emotion>) -> String {
    let mut parts: Vec<String> = Vec::new();
    let known = base.holds >= super::baseline::MIN_HOLDS;
    if f.judgeable() {
        if known {
            relative(f, base, &mut parts);
        } else {
            absolute(f, &mut parts);
        }
    }
    let mood = emotion.and_then(Emotion::mood).map(|(m, sure)| {
        let what = match m {
            Mood::Irritated => "похоже на раздражение",
            Mood::Cheerful => "звучит весело",
        };
        format!("{what} (уверенность {})", if sure { "средняя" } else { "низкая" })
    });
    let mut line = parts.join(", ");
    if let Some(m) = mood {
        if !line.is_empty() {
            line.push_str("; ");
        }
        line.push_str(&m);
    }
    if line.is_empty() {
        return if known && f.judgeable() { NOTHING_NOTABLE.to_string() } else { String::new() };
    }
    line.push('.');
    let mut chars = line.chars();
    let first = chars.next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    let out: String = first + chars.as_str();
    out.chars().take(MAX_CHARS).collect()
}

fn relative(f: &Features, b: &Baseline, parts: &mut Vec<String>) {
    match side(f.rate.and_then(|r| b.rate.z(r, FLOOR_RATE))) {
        1 => parts.push("темп выше обычного".into()),
        -1 => parts.push("темп ниже обычного".into()),
        _ => {}
    }
    let pz = b.pause_ratio.z(f.pause_ratio, FLOOR_PAUSE);
    if f.pauses == 0 && f.speech_secs >= NO_PAUSE_SPEECH && pz.is_some_and(|z| z <= -1.0) {
        parts.push("пауз почти нет".into());
    } else {
        match side(pz) {
            1 => parts.push("пауз больше обычного".into()),
            -1 => parts.push("пауз меньше обычного".into()),
            _ => {}
        }
    }
    let mut voice = Vec::new();
    match side(b.loud_db.z(f.loud_db, FLOOR_LOUD)) {
        1 => voice.push("громче"),
        -1 => voice.push("тише"),
        _ => {}
    }
    match side(f.f0.and_then(|hz| b.f0_st.z(semitones(hz), FLOOR_PITCH))) {
        1 => voice.push("выше"),
        -1 => voice.push("ниже"),
        _ => {}
    }
    if !voice.is_empty() {
        parts.push(format!("голос {} обычного", voice.join(" и ")));
    }
    match side(f.f0_range.and_then(|r| b.f0_range.z(r, FLOOR_RANGE))) {
        1 => parts.push("интонация живее обычного".into()),
        -1 => parts.push("интонация ровнее обычного".into()),
        _ => {}
    }
}

fn absolute(f: &Features, parts: &mut Vec<String>) {
    match f.rate {
        Some(r) if r >= FAST => parts.push("говорит быстро".into()),
        Some(r) if r <= SLOW => parts.push("говорит медленно".into()),
        _ => {}
    }
    if f.longest_pause >= LONG_PAUSE {
        parts.push("с долгими паузами".into());
    } else if f.pause_ratio >= MANY_PAUSES {
        parts.push("много пауз".into());
    } else if f.pauses == 0 && f.speech_secs >= NO_PAUSE_SPEECH + 2.0 {
        parts.push("почти без пауз".into());
    }
    match f.f0_range {
        Some(r) if r <= MONOTONE => parts.push("говорит монотонно".into()),
        Some(r) if r >= LIVELY => parts.push("очень живая интонация".into()),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::super::emotion::LABELS;
    use super::*;

    fn usual() -> Features {
        Features { secs: 5.0, speech_secs: 4.5, pauses: 1, longest_pause: 0.4, pause_ratio: 0.1, loud_db: -26.0, loud_sd: 4.0, f0: Some(140.0), f0_range: Some(6.0), rate: Some(5.0) }
    }

    /// A baseline of `n` holds around `usual()`, with a little spread.
    fn learned(n: u32) -> Baseline {
        let mut b = Baseline::default();
        for i in 0..n {
            let wobble = if i % 2 == 0 { 1.0 } else { -1.0 };
            let f = Features {
                rate: Some(5.0 + 0.3 * wobble),
                loud_db: -26.0 + 1.0 * wobble,
                f0: Some(140.0 * 2f32.powf(0.5 * wobble / 12.0)),
                f0_range: Some(6.0 + 0.5 * wobble),
                pause_ratio: 0.1 + 0.02 * wobble,
                ..usual()
            };
            b.add(&f);
        }
        b
    }

    fn mood(label: &str, p: f32) -> Emotion {
        let mut probs = [0.0; 9];
        let i = LABELS.iter().position(|l| *l == label).unwrap();
        probs[i] = p;
        probs[4] = 1.0 - p;
        Emotion { probs, secs: 4.0 }
    }

    #[test]
    fn the_usual_manner_says_so_once_there_is_a_baseline() {
        assert_eq!(describe(&usual(), &learned(10), None), NOTHING_NOTABLE);
        // Before that: nothing at all (the field is left out).
        assert_eq!(describe(&usual(), &learned(3), None), "");
        // Too little speech: nothing, baseline or not.
        let short = Features { speech_secs: 0.5, ..usual() };
        assert_eq!(describe(&short, &learned(10), None), "");
    }

    #[test]
    fn what_stands_out_is_said_against_the_usual() {
        let b = learned(10);
        let f = Features { rate: Some(7.5), loud_db: -18.0, f0: Some(190.0), pauses: 0, pause_ratio: 0.0, ..usual() };
        assert_eq!(describe(&f, &b, None), "Темп выше обычного, пауз почти нет, голос громче и выше обычного.");
        let f = Features { rate: Some(3.0), loud_db: -33.0, f0_range: Some(2.0), pause_ratio: 0.4, pauses: 4, ..usual() };
        assert_eq!(describe(&f, &b, None), "Темп ниже обычного, пауз больше обычного, голос тише обычного, интонация ровнее обычного.");
    }

    #[test]
    fn without_a_baseline_only_wide_bands_count() {
        let b = learned(2);
        // Louder and higher mean nothing without the usual to compare with.
        let f = Features { loud_db: -10.0, f0: Some(250.0), ..usual() };
        assert_eq!(describe(&f, &b, None), "");
        let f = Features { rate: Some(7.4), longest_pause: 2.5, pauses: 2, f0_range: Some(1.8), ..usual() };
        assert_eq!(describe(&f, &b, None), "Говорит быстро, с долгими паузами, говорит монотонно.");
        let f = Features { rate: Some(2.2), speech_secs: 7.0, pauses: 0, pause_ratio: 0.0, ..usual() };
        assert_eq!(describe(&f, &b, None), "Говорит медленно, почти без пауз.");
    }

    #[test]
    fn an_emotion_only_when_the_model_is_sure() {
        let b = learned(10);
        assert_eq!(describe(&usual(), &b, Some(&mood("angry", 0.6))), NOTHING_NOTABLE);
        assert_eq!(describe(&usual(), &b, Some(&mood("angry", 0.85))), "Похоже на раздражение (уверенность низкая).");
        assert_eq!(describe(&usual(), &b, Some(&mood("happy", 0.97))), "Звучит весело (уверенность средняя).");
        // Sad and fearful are never said (on Russian speech the model
        // calls a flat voice sad: 12% precision on the eval set).
        assert_eq!(describe(&usual(), &b, Some(&mood("sad", 0.99))), NOTHING_NOTABLE);
        let f = Features { rate: Some(7.5), ..usual() };
        assert_eq!(describe(&f, &b, Some(&mood("angry", 0.9))), "Темп выше обычного; похоже на раздражение (уверенность низкая).");
        // Even with too little speech for the manner, a sure emotion goes.
        let short = Features { speech_secs: 0.8, ..usual() };
        assert_eq!(describe(&short, &learned(2), Some(&mood("happy", 0.9))), "Звучит весело (уверенность низкая).");
    }

    #[test]
    fn the_line_is_short() {
        let b = learned(10);
        let f = Features { rate: Some(9.0), loud_db: -10.0, f0: Some(250.0), f0_range: Some(14.0), pause_ratio: 0.5, pauses: 6, ..usual() };
        let line = describe(&f, &b, Some(&mood("angry", 0.99)));
        assert!(line.chars().count() <= MAX_CHARS, "{line}");
        assert!(!line.contains('\u{2014}'));
    }
}
