//! What whisper says that nobody said, and how the pieces of one hold
//! become one text.
//!
//! Whisper was trained on subtitles: on silence, noise or a cut-off word
//! it likes to write the subtitler's credit or a sign-off ("Продолжение
//! следует...", "Субтитры сделал DimaTorzok", "Thanks for watching!"). Two
//! lists:
//!
//!  - `NEVER_SAID`: lines nobody says to their synth while holding a key.
//!    A segment that is one of them goes, wherever it is.
//!  - `ALONE_SUSPECT`: short lines a person may well say ("Спасибо.",
//!    "Thank you.", "Okay.") but whisper also invents on noise. They go
//!    only when they are the whole text. The voice is not lost then: the
//!    app sends the audio instead (an empty text on audio that was not
//!    silent falls back to the synth's speech-to-text).
//!
//! It also folds a phrase stuck in a loop ("слушай, слушай, слушай...")
//! into one, and strips the quotes whisper sometimes puts around a whole
//! sentence.

/// Lines whisper writes on silence that a person holding the key never
/// says. Compared after `norm` (lower case, `ё` as `е`, only letters,
/// digits and single spaces), as a part of the segment.
const NEVER_SAID: &[&str] = &[
    "продолжение следует",
    "dimatorzok",
    "субтитры сделал",
    "субтитры создавал",
    "субтитры делал",
    "субтитры подготовил",
    "редактор субтитров",
    "корректор а егорова",
    "amara org",
    "подписывайтесь на канал",
    "ставьте лайки",
    "спасибо за просмотр",
    "смотрите продолжение",
    "thanks for watching",
    "thank you for watching",
    "please subscribe",
    "like and subscribe",
    "subtitles by",
];

/// Short lines whisper invents on noise, dropped only when they are all
/// there is. After `norm`.
const ALONE_SUSPECT: &[&str] = &[
    "",
    "you",
    "thank you",
    "thank you very much",
    "thanks",
    "bye",
    "bye bye",
    "okay",
    "ok",
    "so",
    "the end",
    "music",
    "спасибо",
    "спасибо большое",
    "пока",
    "до свидания",
    "музыка",
    "аплодисменты",
    "смех",
    "ну",
];

/// Lower case, `ё` as `е`, every run of non-letters as one space, trimmed.
pub fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars().flat_map(char::to_lowercase) {
        let c = if c == 'ё' { 'е' } else { c };
        if c.is_alphanumeric() {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(c);
        } else {
            space = true;
        }
    }
    out
}

/// A segment that is only a sound label: "[Music]", "(музыка)", "*laughs*",
/// "♪♪".
fn is_label(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return true;
    }
    let wrapped = |open: char, close: char| t.starts_with(open) && t.ends_with(close);
    wrapped('[', ']') || wrapped('(', ')') || wrapped('*', '*') || t.chars().all(|c| !c.is_alphanumeric())
}

/// One segment as whisper wrote it: kept (cleaned) or dropped.
pub fn segment(s: &str) -> Option<String> {
    if is_label(s) {
        return None;
    }
    let n = norm(s);
    if NEVER_SAID.iter().any(|p| n.contains(p)) {
        return None;
    }
    let t = collapse_repeats(s.trim());
    (!t.is_empty()).then_some(t)
}

/// A phrase of one to twelve words said three or more times in a row, as
/// whisper does when it is stuck, kept once.
pub fn collapse_repeats(s: &str) -> String {
    let words: Vec<&str> = s.split_whitespace().collect();
    let keys: Vec<String> = words.iter().map(|w| norm(w)).collect();
    let mut out: Vec<&str> = Vec::with_capacity(words.len());
    let mut i = 0;
    'outer: while i < words.len() {
        for n in 1..=12usize {
            if i + 3 * n > words.len() {
                break;
            }
            let same = |a: usize, b: usize| (0..n).all(|k| keys[a + k] == keys[b + k] && !keys[a + k].is_empty());
            let mut reps = 1;
            while i + (reps + 1) * n <= words.len() && same(i, i + reps * n) {
                reps += 1;
            }
            if reps >= 3 {
                // Keep the last copy: it has the sentence's own ending
                // punctuation when there is one.
                out.extend_from_slice(&words[i + (reps - 1) * n..i + reps * n]);
                i += reps * n;
                continue 'outer;
            }
        }
        out.push(words[i]);
        i += 1;
    }
    out.join(" ")
}

/// Quotes around the whole text («...», "...") that are not quoting
/// anything inside it.
fn unquote(s: &str) -> &str {
    for (open, close) in [('«', '»'), ('"', '"'), ('“', '”')] {
        if let Some(inner) = s.strip_prefix(open).and_then(|r| r.strip_suffix(close)) {
            if !inner.contains(open) && !inner.contains(close) {
                return inner.trim();
            }
        }
    }
    s
}

/// A piece's segments without a segment that only repeats the one before
/// it: with the encoder window cut to the piece (engine.rs) whisper
/// sometimes fills the rest of the window with the same sentence again.
pub fn dedupe(segments: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(segments.len());
    for s in segments {
        let n = norm(&s);
        if out.last().is_some_and(|prev| norm(prev) == n && !n.is_empty()) {
            continue;
        }
        out.push(s);
    }
    out
}

/// The pieces of one hold, in order, as one text.
pub fn stitch<S: AsRef<str>>(parts: &[S]) -> String {
    let mut out = String::new();
    for p in parts {
        let p = p.as_ref().trim();
        if p.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(p);
    }
    out
}

/// The whole text of a hold, last check: "" when it is only a line whisper
/// invents on noise.
pub fn whole(text: &str) -> String {
    let t = unquote(text.trim());
    let n = norm(t);
    if ALONE_SUSPECT.contains(&n.as_str()) || NEVER_SAID.iter().any(|p| n == *p) {
        return String::new();
    }
    t.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtitle_credits_go_wherever_they_are() {
        assert_eq!(segment("Продолжение следует..."), None);
        assert_eq!(segment(" Субтитры сделал DimaTorzok"), None);
        assert_eq!(segment("Редактор субтитров А.Синецкая Корректор А.Егорова"), None);
        assert_eq!(segment("Thanks for watching!"), None);
        assert_eq!(segment("[Music]"), None);
        assert_eq!(segment("(музыка)"), None);
        assert_eq!(segment("♪ ♪"), None);
        assert_eq!(segment("..."), None);
        assert_eq!(segment(" Найди мне рейс. ").as_deref(), Some("Найди мне рейс."));
        // "Продолжение" alone is a word a person may say.
        assert_eq!(segment("Продолжение отчёта пришли").as_deref(), Some("Продолжение отчёта пришли"));
    }

    #[test]
    fn short_suspects_go_only_when_they_are_all_there_is() {
        for s in ["Thank you.", " you", "Спасибо.", "Спасибо большое!", "«Пока»", "Okay.", "", "  ", "Продолжение следует..."] {
            assert_eq!(whole(s), "", "{s:?}");
        }
        assert_eq!(whole("Спасибо, Мия, это то, что нужно."), "Спасибо, Мия, это то, что нужно.");
        assert_eq!(whole("Thank you, move it to Friday."), "Thank you, move it to Friday.");
    }

    #[test]
    fn quotes_around_the_whole_text_go() {
        assert_eq!(whole("«Слушай, найди отчёт по курьерам»."), "«Слушай, найди отчёт по курьерам».", "a period after: not a wrap");
        assert_eq!(whole("«Слушай, найди отчёт по курьерам»"), "Слушай, найди отчёт по курьерам");
        assert_eq!(whole("«Слушай» и «найди»"), "«Слушай» и «найди»", "quotes inside stay");
    }

    #[test]
    fn a_loop_is_kept_once() {
        assert_eq!(collapse_repeats("Слушай, слушай, слушай, слушай, слушай,"), "слушай,");
        assert_eq!(collapse_repeats("the, the, the, the"), "the");
        assert_eq!(collapse_repeats("я тебе я тебе я тебе скажу"), "я тебе скажу");
        assert_eq!(collapse_repeats("да да, конечно"), "да да, конечно", "twice is speech");
        assert_eq!(collapse_repeats("one two three four"), "one two three four");
        assert_eq!(collapse_repeats(""), "");
    }

    #[test]
    fn a_segment_that_repeats_the_one_before_goes() {
        let segs = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            dedupe(segs(&[" Слушай, найди отчёт по курьерам.", " Слушай, найди отчёт по курьерам.", " Проверь суммы."])),
            segs(&[" Слушай, найди отчёт по курьерам.", " Проверь суммы."])
        );
        assert_eq!(dedupe(segs(&["Привет, Мия.", "привет мия", "Привет, Мия!"])), segs(&["Привет, Мия."]));
        assert_eq!(dedupe(segs(&["Да.", "Нет.", "Да."])), segs(&["Да.", "Нет.", "Да."]), "only one right after the other");
    }

    #[test]
    fn pieces_join_with_one_space() {
        assert_eq!(stitch(&["  Привет, Мия.", "", "Посмотри на экран. "]), "Привет, Мия. Посмотри на экран.");
        assert_eq!(stitch::<&str>(&[]), "");
    }

    #[test]
    fn norm_keeps_only_words() {
        assert_eq!(norm("  Ещё, раз!! «Да» "), "еще раз да");
        assert_eq!(norm("A.Егорова"), "a егорова");
    }
}
