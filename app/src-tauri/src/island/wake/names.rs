//! Is this the owner calling the synth? The names the island answers to,
//! and the check of what whisper heard at the start of a stretch of
//! speech.
//!
//! The text is compared as sound, not as spelling: every word becomes a
//! key (lower case, Cyrillic written in Latin letters, y and j as i,
//! accents and double letters dropped), so "Мия", "Миа", "Mia" and "Mía"
//! are the same key, `mia`. The key of what was heard may differ from the
//! name's by a few edits (`tolerance`): none for a short name, where one
//! letter makes another word ("Мир", "Майя"), one for five to seven
//! letters, two beyond.
//!
//! The name counts only at the start of what was said: as the first or
//! the second word ("Мия, ...", "Эй, Мия, ..."). Mentioned later ("я
//! вчера видел Мию") it is talk about the synth, not a call.

use crate::island::stt::filter;

/// One name or phrase the island answers to.
#[derive(Debug, Clone)]
struct Entry {
    /// The key of the whole name or phrase.
    key: String,
    /// How many words it is.
    words: usize,
}

/// The names of the main synth (and the owner's extra phrase).
#[derive(Debug, Clone, Default)]
pub struct Names {
    entries: Vec<Entry>,
    shown: Vec<String>,
}

/// The name, heard.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// The words heard as the name, as whisper wrote them.
    pub heard: String,
    /// 1 for the exact key, less for each edit.
    pub score: f32,
    /// What follows the name, as written (the request).
    pub rest: String,
}

/// How many edits a key of this length may be off by.
pub fn tolerance(len: usize) -> usize {
    match len {
        0..=4 => 0,
        5..=7 => 1,
        _ => 2,
    }
}

fn latin(c: char) -> Option<&'static str> {
    Some(match c {
        'а' => "a",
        'б' => "b",
        'в' => "v",
        'г' => "g",
        'д' => "d",
        'е' | 'ё' | 'э' => "e",
        'ж' => "zh",
        'з' => "z",
        'и' | 'й' | 'ы' => "i",
        'к' => "k",
        'л' => "l",
        'м' => "m",
        'н' => "n",
        'о' => "o",
        'п' => "p",
        'р' => "r",
        'с' => "s",
        'т' => "t",
        'у' => "u",
        'ф' => "f",
        'х' => "h",
        'ц' => "ts",
        'ч' => "ch",
        'ш' | 'щ' => "sh",
        'ъ' | 'ь' => "",
        'ю' => "iu",
        'я' => "ia",
        'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => "a",
        'è' | 'é' | 'ê' | 'ë' => "e",
        'ì' | 'í' | 'î' | 'ï' => "i",
        'ò' | 'ó' | 'ô' | 'ö' | 'õ' => "o",
        'ù' | 'ú' | 'û' | 'ü' => "u",
        'ñ' => "n",
        'ç' => "s",
        'y' | 'j' => "i",
        'w' => "v",
        'q' => "k",
        'x' => "ks",
        _ => return None,
    })
}

/// The sound key of a word (or of words run together).
pub fn key(word: &str) -> String {
    let mut raw = String::with_capacity(word.len());
    for c in word.chars().flat_map(char::to_lowercase) {
        if let Some(l) = latin(c) {
            raw.push_str(l);
        } else if c.is_ascii_alphanumeric() {
            raw.push(c);
        }
    }
    // Spellings of one sound.
    let raw = raw.replace("ph", "f").replace("th", "t").replace("ck", "k").replace("ee", "i").replace("ch", "\u{1}").replace('c', "k").replace('\u{1}', "ch");
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        if !out.ends_with(c) {
            out.push(c);
        }
    }
    out
}

/// Edits (insert, delete, substitute) from one key to the other.
pub fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let sub = prev[j - 1] + usize::from(a[i - 1] != b[j - 1]);
            cur[j] = sub.min(prev[j] + 1).min(cur[j - 1] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn cyrillic(name: &str) -> Vec<String> {
    let low = name.to_lowercase();
    if !low.chars().all(|c| c.is_ascii_alphabetic()) {
        return Vec::new();
    }
    let mut out = String::new();
    let b = low.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let two = low.get(i..i + 2).unwrap_or("");
        let (s, n) = match two {
            "sh" => ("ш", 2),
            "ch" => ("ч", 2),
            "zh" => ("ж", 2),
            "kh" => ("х", 2),
            "ts" => ("ц", 2),
            "ya" => ("я", 2),
            "yu" => ("ю", 2),
            _ => (
                match b[i] {
                    b'a' => "а",
                    b'b' => "б",
                    b'c' | b'k' | b'q' => "к",
                    b'd' => "д",
                    b'e' => "е",
                    b'f' => "ф",
                    b'g' => "г",
                    b'h' => "х",
                    b'i' | b'y' => "и",
                    b'j' => "дж",
                    b'l' => "л",
                    b'm' => "м",
                    b'n' => "н",
                    b'o' => "о",
                    b'p' => "п",
                    b'r' => "р",
                    b's' => "с",
                    b't' => "т",
                    b'u' => "у",
                    b'v' | b'w' => "в",
                    b'x' => "кс",
                    b'z' => "з",
                    _ => "",
                },
                1,
            ),
        };
        out.push_str(s);
        i += n;
    }
    let mut forms = vec![capitalize(&out)];
    // "Mia" is "Мия" as often as "Миа".
    if let Some(stem) = out.strip_suffix("иа") {
        forms.insert(0, capitalize(&format!("{stem}ия")));
    }
    forms
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The words of a text, normalized, each with the index of the
/// whitespace-separated token it came from.
fn words(text: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for (i, token) in text.split_whitespace().enumerate() {
        for w in filter::norm(token).split(' ').filter(|w| !w.is_empty()) {
            out.push((w.to_string(), i));
        }
    }
    out
}

impl Names {
    /// The synth's name (its first word: people call "Mia", not "Mia
    /// Sabry") with its other spellings, and the owner's extra phrase.
    pub fn new(synth_name: &str, extra: &str) -> Names {
        let mut names = Names::default();
        let first = synth_name.split(|c: char| c.is_whitespace() || c == '_' || c == '-').find(|w| !w.is_empty()).unwrap_or("");
        let first: String = first.chars().filter(|c| c.is_alphabetic()).collect();
        if key(&first).chars().count() >= 2 {
            let k = key(&first);
            names.entries.push(Entry { key: k.clone(), words: 1 });
            names.shown.push(capitalize(&first));
            if first.chars().all(|c| c.is_ascii_alphabetic()) {
                names.shown.extend(cyrillic(&first));
            } else {
                names.shown.push(capitalize(&k));
            }
        }
        let extra = extra.trim();
        let ws = words(extra);
        let k = key(&ws.iter().map(|(w, _)| w.as_str()).collect::<String>());
        if k.chars().count() >= 3 && !names.entries.iter().any(|e| e.key == k) {
            names.entries.push(Entry { key: k, words: ws.len() });
            names.shown.push(extra.to_string());
        }
        let mut seen = std::collections::HashSet::new();
        names.shown.retain(|s| seen.insert(s.to_lowercase()));
        names
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The spellings shown in Settings ("Mia", "Мия", "Миа").
    pub fn shown(&self) -> &[String] {
        &self.shown
    }

    /// The hint whisper gets when it decodes in `lang`: the name as it is
    /// written in that script ("Мия." for Russian, "Mia." for English),
    /// and the extra phrase.
    pub fn prompt(&self, lang: &str) -> String {
        let cyrillic = |s: &&String| s.chars().any(|c| matches!(c, 'а'..='я' | 'А'..='Я' | 'ё' | 'Ё'));
        let name = match lang {
            "ru" => self.shown.iter().find(cyrillic),
            _ => self.shown.iter().find(|s| !cyrillic(s)),
        }
        .or(self.shown.first());
        let mut parts: Vec<&str> = name.map(String::as_str).into_iter().collect();
        if self.entries.len() > 1 {
            if let Some(phrase) = self.shown.last().filter(|p| Some(*p) != name) {
                parts.push(phrase);
            }
        }
        if parts.is_empty() {
            return String::new();
        }
        format!("{}.", parts.join(". "))
    }

    /// The name at the start of this text (its first or second word), and
    /// what follows it. None for anything else.
    pub fn find(&self, text: &str) -> Option<Hit> {
        self.closest(text, 0)
    }

    /// Something at the start sounds almost like the name (it starts with
    /// the same sound and is two edits more off than a match): worth
    /// decoding again in another language.
    pub fn near(&self, text: &str) -> bool {
        self.closest(text, 2).is_some()
    }

    fn closest(&self, text: &str, slack: usize) -> Option<Hit> {
        let ws = words(text);
        let mut best: Option<(usize, usize, usize, usize)> = None; // (distance, start, end, key length)
        for e in &self.entries {
            for start in 0..2.min(ws.len()) {
                // The name as whisper split it: its own number of words,
                // or one more short piece ("Ми я").
                for len in [e.words, e.words + 1, e.words.saturating_sub(1)] {
                    if len == 0 || start + len > ws.len() {
                        continue;
                    }
                    if len > e.words && key(&ws[start + len - 1].0).len() > 3 {
                        continue;
                    }
                    let heard = key(&ws[start..start + len].iter().map(|(w, _)| w.as_str()).collect::<String>());
                    let d = distance(&heard, &e.key);
                    let same_start = heard.chars().next() == e.key.chars().next();
                    if d <= tolerance(e.key.len()) + slack && (slack == 0 || same_start) && best.is_none_or(|b| (d, start) < (b.0, b.1)) {
                        best = Some((d, start, start + len, e.key.len()));
                    }
                }
            }
        }
        let (d, start, end, len) = best?;
        let tokens: Vec<&str> = text.split_whitespace().collect();
        let (first_tok, last_tok) = (ws[start].1, ws[end - 1].1);
        let heard = tokens[first_tok..=last_tok].join(" ");
        let heard = heard.trim_matches(|c: char| !c.is_alphanumeric()).to_string();
        let rest = tokens[last_tok + 1..].join(" ");
        let rest = capitalize(rest.trim_start_matches(|c: char| !c.is_alphanumeric()));
        Some(Hit { heard, score: 1.0 - d as f32 / len.max(1) as f32, rest })
    }

    /// The request without the name in front: None when nothing but the
    /// name (or a sound or two) was said. A text without the name at its
    /// start (the device's model heard it differently) is kept whole.
    pub fn request(&self, text: &str) -> Option<String> {
        let rest = match self.find(text) {
            Some(hit) => hit.rest,
            None => text.trim().to_string(),
        };
        let letters = rest.chars().filter(|c| c.is_alphanumeric()).count();
        (letters >= 2).then_some(rest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mia() -> Names {
        Names::new("Mia", "")
    }

    #[test]
    fn spellings_of_one_name_have_one_key() {
        for w in ["Мия", "Миа", "Mia", "MIA", "Mía", "Miya", "Мийа"] {
            assert_eq!(key(w), "mia", "{w}");
        }
        assert_eq!(key("Саша"), key("Sasha"));
        assert_eq!(key("Алекс"), key("Alex"));
        assert_eq!(key("Кира"), key("Kira"));
        assert_ne!(key("Майя"), key("Мия"));
        assert_eq!(distance("mia", "maia"), 1);
        assert_eq!(distance("kitten", "sitting"), 3);
        assert_eq!(distance("", "abc"), 3);
    }

    #[test]
    fn the_names_shown_and_the_prompt() {
        assert_eq!(mia().shown(), &["Mia".to_string(), "Мия".into(), "Миа".into()]);
        assert_eq!(mia().prompt("ru"), "Мия.");
        assert_eq!(mia().prompt("en"), "Mia.");
        assert_eq!(Names::new("Мия", "").prompt("en"), "Mia.");
        assert_eq!(Names::new("Mia", "эй подруга").prompt("ru"), "Мия. эй подруга.");
        assert_eq!(Names::new("Мия", "").shown(), &["Мия".to_string(), "Mia".into()]);
        assert_eq!(Names::new("Mia Sabry", "").shown()[0], "Mia");
        assert_eq!(Names::new("Kira", "эй подруга").shown(), &["Kira".to_string(), "Кира".into(), "эй подруга".into()]);
        assert!(Names::new("", "").is_empty());
        assert!(Names::new("", "a").is_empty(), "too short to call");
    }

    #[test]
    fn the_name_at_the_start_wakes() {
        let n = mia();
        for (text, rest) in [
            ("Мия, открой почту.", "Открой почту."),
            ("Mia, open my mail.", "Open my mail."),
            ("Миа, найди билеты в Бангкок", "Найди билеты в Бангкок"),
            ("Эй, Мия, найди билеты", "Найди билеты"),
            ("Hey Mia, what's the time?", "What's the time?"),
            ("Ми я, открой почту", "Открой почту"),
            ("Мия.", ""),
            ("Мия!", ""),
            ("Mía, abre el correo", "Abre el correo"),
        ] {
            let hit = n.find(text).unwrap_or_else(|| panic!("{text}: no wake"));
            assert_eq!(hit.rest, rest, "{text}");
            assert!(hit.score > 0.99, "{text}: {}", hit.score);
        }
        assert_eq!(n.find("Мия, открой").unwrap().heard, "Мия");
    }

    #[test]
    fn the_name_later_in_a_sentence_does_not_wake() {
        let n = mia();
        for text in [
            "я вчера видел Мию",
            "я вчера видел Мия",
            "я говорил Мие вчера",
            "I talked to Mia yesterday",
            "Сегодня хорошая погода, пойдём гулять",
            "Мир такой большой",
            "Майя, привет",
            "Мило, что ты зашла",
            "Me and my friends",
        ] {
            assert_eq!(n.find(text), None, "{text}");
        }
        assert!(n.find("Мия").is_some());
        assert_eq!(n.find(""), None);
    }

    #[test]
    fn a_near_miss_is_worth_another_language() {
        let n = mia();
        for text in ["Ме, открой почту", "Миян Май Франс", "Мая, привет", "А, меня, найди"] {
            assert!(n.near(text), "{text}");
            assert!(n.find(text).is_none(), "{text}");
        }
        for text in ["Сегодня хорошая погода", "The quarterly report", "я вчера видел Мию"] {
            assert!(!n.near(text), "{text}");
        }
    }

    #[test]
    fn longer_names_take_a_slip_of_the_ear() {
        let n = Names::new("Alexandra", "");
        assert!(n.find("Aleksandra, open it").is_some());
        assert!(n.find("Александра, открой").is_some());
        assert!(n.find("Alexander, open it").is_some(), "two edits on nine letters");
        assert!(n.find("Alexis, open it").is_none());
        let n = Names::new("Kira", "");
        assert!(n.find("Кира, привет").is_some());
        assert!(n.find("Кора, привет").is_none(), "a short name is exact");
    }

    #[test]
    fn the_extra_phrase_wakes_too() {
        let n = Names::new("Mia", "эй подруга");
        let hit = n.find("Эй, подруга, что у нас на завтра?").unwrap();
        assert_eq!(hit.rest, "Что у нас на завтра?");
        assert!(n.find("Мия, привет").is_some(), "the name still works");
        assert!(n.find("вот моя подруга пришла").is_none());
    }

    #[test]
    fn the_request_is_the_text_without_the_name() {
        let n = mia();
        assert_eq!(n.request("Мия, открой почту."), Some("Открой почту.".into()));
        assert_eq!(n.request("Мия."), None);
        assert_eq!(n.request("Мия, а"), None, "a sound is not a request");
        assert_eq!(n.request("  "), None);
        assert_eq!(n.request("Открой почту"), Some("Открой почту".into()), "the name heard differently: kept whole");
    }
}
