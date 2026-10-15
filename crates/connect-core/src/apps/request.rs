//! One `apps.*` call, checked and bounded before anything else looks at it.
//!
//! Everything the synth sends is untrusted. `Request::parse` is the only
//! way to build a request, so every backend gets values that are already
//! the right kind, the right size and free of control characters, and the
//! approval window shows exactly the text that will be sent.

use std::path::PathBuf;

use serde_json::Value;

use super::time::When;
use super::AppError;

// --- limits (connect.md: apps) ---

pub const MESSAGES_DEFAULT: usize = 20;
pub const MESSAGES_MAX: usize = 50;
pub const MESSAGE_TEXT_MAX: usize = 2000;
pub const CONTACTS_DEFAULT: usize = 10;
pub const CONTACTS_MAX: usize = 25;
/// A contact search needs this many letters or digits (CJK names are
/// often two characters long).
pub const CONTACT_QUERY_MIN: usize = 2;
/// A search by number needs this many digits, so "1" cannot match every
/// phone in the address book.
pub const CONTACT_DIGITS_MIN: usize = 4;
pub const EVENTS_DEFAULT: usize = 50;
pub const EVENTS_MAX: usize = 100;
/// The longest range one calendar.list may cover.
pub const EVENT_RANGE_MAX_DAYS: i64 = 92;
pub const REMINDERS_DEFAULT: usize = 50;
pub const REMINDERS_MAX: usize = 200;
pub const NOTES_DEFAULT: usize = 20;
pub const NOTES_MAX: usize = 50;
pub const NOTE_QUERY_MIN: usize = 2;
pub const NOTE_TEXT_MAX: usize = 20_000;
pub const TITLE_MAX: usize = 300;
pub const BODY_MAX: usize = 4000;
pub const NAME_MAX: usize = 120;
pub const SHORTCUT_INPUT_MAX: usize = 16 * 1024;
pub const SHORTCUT_OUTPUT_MAX: usize = 64 * 1024;
pub const SHORTCUTS_MAX: usize = 200;
pub const CLIPBOARD_MAX: usize = 100_000;
pub const NOTIFY_TITLE_MAX: usize = 80;
pub const NOTIFY_BODY_MAX: usize = 300;
pub const URL_MAX: usize = 2000;

/// Which messaging service a send goes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgService {
    IMessage,
    Sms,
}

impl MsgService {
    pub fn label(self) -> &'static str {
        match self {
            MsgService::IMessage => "imessage",
            MsgService::Sms => "sms",
        }
    }
}

/// A checked call. Field values are final: bounded, trimmed, cleaned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    MessagesRecent { chat: Option<String>, limit: usize },
    MessagesSend { to: String, text: String, service: MsgService },
    ContactsSearch { query: String, limit: usize },
    CalendarList { start: When, end: When, calendar: Option<String>, query: Option<String>, limit: usize },
    CalendarCreate { title: String, start: When, end: When, calendar: Option<String>, location: String, notes: String, all_day: bool },
    RemindersList { list: Option<String>, include_completed: bool, limit: usize },
    RemindersCreate { title: String, due: Option<When>, list: Option<String>, notes: String },
    RemindersComplete { id: String },
    NotesSearch { query: String, folder: Option<String>, limit: usize },
    NotesRead { id: Option<String>, title: Option<String> },
    NotesCreate { title: String, text: String, folder: Option<String> },
    ShortcutsList,
    ShortcutsRun { name: String, input: String },
    NotifyShow { title: String, body: String },
    ClipboardRead,
    ClipboardWrite { text: String },
    OpenUrl { url: String, host: String },
    /// `path` is as the synth wrote it; the gate replaces it with the
    /// resolved path inside a shared folder before a backend sees it.
    OpenFile { path: PathBuf },
    /// `bundle_id` (macOS, optional): open exactly that app (`open -b`);
    /// the name stays the one the owner sees and the policy keys on.
    OpenApp { name: String, bundle_id: Option<String> },
}

/// Characters that never belong in text a person will read or send:
/// controls, and the invisible direction and joining marks that can make
/// a text look like something else in the approval window.
fn hidden(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}'
        )
}

/// One line of text: hidden characters dropped, line breaks and tabs
/// turned into spaces, whitespace trimmed.
pub fn clean_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\n' || c == '\r' || c == '\t' {
            out.push(' ');
        } else if !hidden(c) {
            out.push(c);
        }
    }
    out.trim().to_string()
}

/// Text that may span lines: hidden characters dropped, line breaks kept
/// as `\n`.
pub fn clean_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.replace("\r\n", "\n").chars() {
        if c == '\n' || c == '\r' {
            out.push('\n');
        } else if c == '\t' {
            out.push(' ');
        } else if !hidden(c) {
            out.push(c);
        }
    }
    out.trim_matches(|c: char| c == '\n' || c == ' ').to_string()
}

/// Cut to at most `max` characters (not bytes), on a character boundary.
pub fn cut(s: &str, max: usize) -> (String, bool) {
    match s.char_indices().nth(max) {
        Some((at, _)) => (s[..at].to_string(), true),
        None => (s.to_string(), false),
    }
}

fn str_param<'a>(p: &'a Value, key: &str) -> &'a str {
    p.get(key).and_then(Value::as_str).unwrap_or("")
}

fn bool_param(p: &Value, key: &str) -> bool {
    p.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn limit_param(p: &Value, default: usize, max: usize) -> usize {
    match p.get("limit").and_then(Value::as_u64) {
        Some(0) | None => default,
        Some(n) => (n as usize).min(max),
    }
}

/// A required one-line field of at most `max` characters. Longer is a
/// refusal, not a silent cut: what is created must be what was asked.
fn line(p: &Value, key: &str, max: usize) -> Result<String, AppError> {
    let v = clean_line(str_param(p, key));
    if v.is_empty() {
        return Err(AppError::BadRequest(format!("{key} is required")));
    }
    if v.chars().count() > max {
        return Err(AppError::BadRequest(format!("{key} is longer than {max} characters")));
    }
    Ok(v)
}

fn opt_line(p: &Value, key: &str, max: usize) -> Result<Option<String>, AppError> {
    let v = clean_line(str_param(p, key));
    if v.is_empty() {
        return Ok(None);
    }
    if v.chars().count() > max {
        return Err(AppError::BadRequest(format!("{key} is longer than {max} characters")));
    }
    Ok(Some(v))
}

fn text(p: &Value, key: &str, max: usize, required: bool) -> Result<String, AppError> {
    let v = clean_text(str_param(p, key));
    if v.is_empty() && required {
        return Err(AppError::BadRequest(format!("{key} is required")));
    }
    if v.chars().count() > max {
        return Err(AppError::BadRequest(format!("{key} is longer than {max} characters")));
    }
    Ok(v)
}

/// A phone number or an email address, in the one spelling used for the
/// send, for the approval window and for a remembered "always":
/// `+79161234567` (digits with an optional leading plus) or a lowercased
/// email. Anything else (a name, a group) is refused.
pub fn normalize_recipient(raw: &str) -> Result<String, AppError> {
    let bad = || {
        AppError::BadRequest(
            "to must be one phone number (like +79161234567) or one email address; look a name up with contacts first".into(),
        )
    };
    let v = clean_line(raw);
    if v.is_empty() || v.chars().count() > 100 {
        return Err(bad());
    }
    if v.contains('@') {
        let lower = v.to_lowercase();
        let mut parts = lower.split('@');
        let (user, host) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let ok = parts.next().is_none()
            && !user.is_empty()
            && host.contains('.')
            && !host.starts_with('.')
            && !host.ends_with('.')
            && lower
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '@' | '.' | '_' | '-' | '+'));
        return if ok { Ok(lower) } else { Err(bad()) };
    }
    let mut digits = String::new();
    for (i, c) in v.chars().enumerate() {
        match c {
            '+' if i == 0 => digits.push('+'),
            '0'..='9' => digits.push(c),
            ' ' | '-' | '(' | ')' | '.' => {}
            _ => return Err(bad()),
        }
    }
    let count = digits.chars().filter(char::is_ascii_digit).count();
    if !(3..=15).contains(&count) {
        return Err(bad());
    }
    Ok(digits)
}

/// The conversation filter of messages.recent: a phone number or email
/// (normalized like a recipient), or a chat identifier as an earlier
/// result returned it.
fn chat_filter(raw: &str) -> Result<Option<String>, AppError> {
    let v = clean_line(raw);
    if v.is_empty() {
        return Ok(None);
    }
    if let Ok(handle) = normalize_recipient(&v) {
        return Ok(Some(handle));
    }
    let ok = v.chars().count() <= 200
        && v.chars().all(|c| c.is_alphanumeric() || matches!(c, ';' | '+' | '-' | '_' | '.' | '@' | ':'));
    if ok {
        Ok(Some(v))
    } else {
        Err(AppError::BadRequest(
            "chat must be a phone number, an email address, or a chat value from an earlier result".into(),
        ))
    }
}

/// An http or https address with a real host. Everything else (file:,
/// tel:, app schemes that can act on their own) is refused.
pub fn check_url(raw: &str) -> Result<(String, String), AppError> {
    let v = clean_line(raw);
    if v.chars().count() > URL_MAX {
        return Err(AppError::BadRequest(format!("url is longer than {URL_MAX} characters")));
    }
    let lower = v.to_ascii_lowercase();
    let rest = if lower.starts_with("https://") {
        &v[8..]
    } else if lower.starts_with("http://") {
        &v[7..]
    } else {
        return Err(AppError::Denied("only http and https addresses can be opened".into()));
    };
    if v.chars().any(char::is_whitespace) {
        return Err(AppError::BadRequest("url must not contain spaces".into()));
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // No user:password@ part: it is how a link pretends to be another site.
    if authority.contains('@') {
        return Err(AppError::Denied("addresses with a user name before the site are not opened".into()));
    }
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or("").to_string()
    } else {
        authority.split(':').next().unwrap_or("").to_string()
    };
    if host.is_empty() || host.starts_with('-') {
        return Err(AppError::BadRequest("url has no site name".into()));
    }
    Ok((v, crate::core::registrable_domain(&host)))
}

/// An app name as people say it ("Safari", "Visual Studio Code"): never
/// a path, never an option.
fn app_name(raw: &str) -> Result<String, AppError> {
    let v = clean_line(raw);
    let bad = v.is_empty()
        || v.chars().count() > NAME_MAX
        || v.starts_with('-')
        || v.starts_with('.')
        || v.chars().any(|c| matches!(c, '/' | '\\' | ':' | '"' | '<' | '>' | '|' | '*' | '?'));
    if bad {
        return Err(AppError::BadRequest(
            "name must be the plain name of an app, for example Safari".into(),
        ));
    }
    Ok(v)
}

/// An app's bundle id, when given: reverse-DNS letters, digits, dots and
/// hyphens only (`com.apple.finder`), at least one dot, never a path or a
/// flag. Empty = none.
fn bundle_id(raw: &str) -> Result<Option<String>, AppError> {
    let v = raw.trim();
    if v.is_empty() {
        return Ok(None);
    }
    let bad = v.len() > 155
        || !v.contains('.')
        || v.starts_with(['.', '-'])
        || v.ends_with('.')
        || v.contains("..")
        || !v.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if bad {
        return Err(AppError::BadRequest(
            "bundle_id must be an app's bundle id, for example com.apple.finder".into(),
        ));
    }
    Ok(Some(v.to_string()))
}

fn letters_or_digits(s: &str) -> usize {
    s.chars().filter(|c| c.is_alphanumeric()).count()
}

impl Request {
    /// Check and bound the params of `<app>.<action>`.
    pub fn parse(app: &str, action: &str, p: &Value) -> Result<Request, AppError> {
        Ok(match (app, action) {
            ("messages", "recent") => Request::MessagesRecent {
                chat: chat_filter(str_param(p, "chat"))?,
                limit: limit_param(p, MESSAGES_DEFAULT, MESSAGES_MAX),
            },
            ("messages", "send") => {
                let service = match str_param(p, "service").to_ascii_lowercase().as_str() {
                    "" | "imessage" => MsgService::IMessage,
                    "sms" => MsgService::Sms,
                    _ => return Err(AppError::BadRequest("service must be imessage or sms".into())),
                };
                Request::MessagesSend {
                    to: normalize_recipient(str_param(p, "to"))?,
                    text: text(p, "text", MESSAGE_TEXT_MAX, true)?,
                    service,
                }
            }
            ("contacts", "search") => {
                let query = clean_line(str_param(p, "query"));
                let digits = query.chars().filter(char::is_ascii_digit).count();
                let letters = query.chars().filter(|c| c.is_alphabetic()).count();
                let enough = if letters == 0 {
                    digits >= CONTACT_DIGITS_MIN
                } else {
                    letters_or_digits(&query) >= CONTACT_QUERY_MIN
                };
                if !enough || query.chars().count() > NAME_MAX {
                    return Err(AppError::BadRequest(format!(
                        "query must be a name of at least {CONTACT_QUERY_MIN} letters, an email, or at least {CONTACT_DIGITS_MIN} digits of a phone number; the address book is never listed whole"
                    )));
                }
                Request::ContactsSearch { query, limit: limit_param(p, CONTACTS_DEFAULT, CONTACTS_MAX) }
            }
            ("calendar", "list") => {
                let start = When::parse("start", str_param(p, "start"))?;
                let end = When::parse("end", str_param(p, "end"))?;
                let span = end.local().ordinal() - start.local().ordinal();
                if span <= 0 {
                    return Err(AppError::BadRequest("end must be after start".into()));
                }
                if span > EVENT_RANGE_MAX_DAYS * 86_400 {
                    return Err(AppError::BadRequest(format!(
                        "the range is longer than {EVENT_RANGE_MAX_DAYS} days; ask for a shorter one"
                    )));
                }
                Request::CalendarList {
                    start,
                    end,
                    calendar: opt_line(p, "calendar", NAME_MAX)?,
                    query: opt_line(p, "query", NAME_MAX)?,
                    limit: limit_param(p, EVENTS_DEFAULT, EVENTS_MAX),
                }
            }
            ("calendar", "create") => {
                let start = When::parse("start", str_param(p, "start"))?;
                let end = When::parse("end", str_param(p, "end"))?;
                if end.local().ordinal() <= start.local().ordinal() {
                    return Err(AppError::BadRequest("end must be after start".into()));
                }
                Request::CalendarCreate {
                    title: line(p, "title", TITLE_MAX)?,
                    start,
                    end,
                    calendar: opt_line(p, "calendar", NAME_MAX)?,
                    location: opt_line(p, "location", TITLE_MAX)?.unwrap_or_default(),
                    notes: text(p, "notes", BODY_MAX, false)?,
                    all_day: bool_param(p, "all_day"),
                }
            }
            ("reminders", "list") => Request::RemindersList {
                list: opt_line(p, "list", NAME_MAX)?,
                include_completed: bool_param(p, "include_completed"),
                limit: limit_param(p, REMINDERS_DEFAULT, REMINDERS_MAX),
            },
            ("reminders", "create") => {
                let due = match str_param(p, "due").trim() {
                    "" => None,
                    v => Some(When::parse("due", v)?),
                };
                Request::RemindersCreate {
                    title: line(p, "title", TITLE_MAX)?,
                    due,
                    list: opt_line(p, "list", NAME_MAX)?,
                    notes: text(p, "notes", BODY_MAX, false)?,
                }
            }
            ("reminders", "complete") => Request::RemindersComplete { id: line(p, "id", 300)? },
            ("notes", "search") => {
                let query = clean_line(str_param(p, "query"));
                if letters_or_digits(&query) < NOTE_QUERY_MIN || query.chars().count() > NAME_MAX {
                    return Err(AppError::BadRequest(format!(
                        "query must have at least {NOTE_QUERY_MIN} letters or digits; notes are never listed whole"
                    )));
                }
                Request::NotesSearch {
                    query,
                    folder: opt_line(p, "folder", NAME_MAX)?,
                    limit: limit_param(p, NOTES_DEFAULT, NOTES_MAX),
                }
            }
            ("notes", "read") => {
                let id = opt_line(p, "id", 300)?;
                let title = opt_line(p, "title", TITLE_MAX)?;
                if id.is_none() && title.is_none() {
                    return Err(AppError::BadRequest("id or title is required".into()));
                }
                Request::NotesRead { id, title }
            }
            ("notes", "create") => Request::NotesCreate {
                title: line(p, "title", TITLE_MAX)?,
                text: text(p, "text", NOTE_TEXT_MAX, false)?,
                folder: opt_line(p, "folder", NAME_MAX)?,
            },
            ("shortcuts", "list") => Request::ShortcutsList,
            ("shortcuts", "run") => {
                let name = line(p, "name", NAME_MAX)?;
                if name.starts_with('-') {
                    return Err(AppError::BadRequest("name must be the exact name of a shortcut".into()));
                }
                let input = clean_text(str_param(p, "input"));
                if input.len() > SHORTCUT_INPUT_MAX {
                    return Err(AppError::BadRequest(format!(
                        "input is larger than {SHORTCUT_INPUT_MAX} bytes"
                    )));
                }
                Request::ShortcutsRun { name, input }
            }
            ("notify", "show") => Request::NotifyShow {
                title: line(p, "title", NOTIFY_TITLE_MAX)?,
                body: opt_line(p, "body", NOTIFY_BODY_MAX)?.unwrap_or_default(),
            },
            ("clipboard", "read") => Request::ClipboardRead,
            ("clipboard", "write") => Request::ClipboardWrite { text: text(p, "text", CLIPBOARD_MAX, true)? },
            ("open", "url") => {
                let (url, host) = check_url(str_param(p, "url"))?;
                Request::OpenUrl { url, host }
            }
            ("open", "file") => {
                let raw = str_param(p, "path").trim();
                if raw.is_empty() {
                    return Err(AppError::BadRequest("path is required".into()));
                }
                if raw.chars().any(hidden) {
                    return Err(AppError::BadRequest("path has characters that cannot be in a file name".into()));
                }
                Request::OpenFile { path: PathBuf::from(raw) }
            }
            ("open", "app") => Request::OpenApp {
                name: app_name(str_param(p, "name"))?,
                bundle_id: bundle_id(str_param(p, "bundle_id"))?,
            },
            _ => return Err(AppError::Denied(format!("unknown method apps.{app}.{action}"))),
        })
    }
}

/// File types `open.file` hands to their default app. A list of what is
/// allowed, not of what is forbidden: opening a script, an app bundle, a
/// shortcut file, an installer or a disk image RUNS it, which would turn
/// "open a document" into "run a program" for a synth that can also
/// write files into a shared folder.
pub const OPENABLE_EXTENSIONS: &[&str] = &[
    // text and documents
    "txt", "md", "rtf", "pdf", "doc", "docx", "odt", "pages", "xls", "xlsx", "ods", "numbers", "csv",
    "tsv", "ppt", "pptx", "odp", "key", "epub", "log", "json", "xml", "yaml", "yml", "ics", "vcf",
    // pictures
    "png", "jpg", "jpeg", "gif", "heic", "heif", "webp", "bmp", "tif", "tiff",
    // sound and video
    "mp3", "m4a", "aac", "wav", "flac", "ogg", "mp4", "m4v", "mov", "mkv", "webm", "avi",
];

/// May this resolved path be handed to the system's "open"? A folder is
/// fine (it opens in the file manager) unless its name has an extension:
/// on macOS such a folder can be a bundle that runs. A file needs an
/// extension from the list.
pub fn openable(path: &std::path::Path, is_dir: bool) -> Result<(), AppError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if is_dir {
        if ext.is_empty() {
            return Ok(());
        }
        return Err(AppError::Denied(
            "a folder whose name ends like a file type is not opened (it may be an app or a package)".into(),
        ));
    }
    if OPENABLE_EXTENSIONS.contains(&ext.as_str()) {
        Ok(())
    } else {
        Err(AppError::Denied(format!(
            "files of type {:?} are not opened: only documents, pictures, sound and video (opening a program or a script would run it)",
            if ext.is_empty() { "none".to_string() } else { format!(".{ext}") }
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(app: &str, action: &str, p: Value) -> Result<Request, AppError> {
        Request::parse(app, action, &p)
    }

    #[test]
    fn contacts_search_needs_a_real_query() {
        for q in ["", " ", "a", "*", "%", "..", "1", "123", "+7", "  x  "] {
            let r = parse("contacts", "search", json!({"query": q}));
            assert!(matches!(r, Err(AppError::BadRequest(_))), "query {q:?} was accepted");
        }
        for q in ["an", "Анна", "李明", "+7 916", "1234", "anna@example.com"] {
            assert!(parse("contacts", "search", json!({"query": q})).is_ok(), "query {q:?} was refused");
        }
    }

    #[test]
    fn limits_are_capped() {
        let Request::ContactsSearch { limit, .. } =
            parse("contacts", "search", json!({"query": "anna", "limit": 100000})).unwrap()
        else {
            panic!()
        };
        assert_eq!(limit, CONTACTS_MAX);
        let Request::MessagesRecent { limit, chat } = parse("messages", "recent", json!({"limit": 9999})).unwrap()
        else {
            panic!()
        };
        assert_eq!((limit, chat), (MESSAGES_MAX, None));
        let Request::MessagesRecent { limit, .. } = parse("messages", "recent", json!({})).unwrap() else {
            panic!()
        };
        assert_eq!(limit, MESSAGES_DEFAULT);
        let Request::RemindersList { limit, .. } = parse("reminders", "list", json!({"limit": 0})).unwrap()
        else {
            panic!()
        };
        assert_eq!(limit, REMINDERS_DEFAULT);
    }

    #[test]
    fn recipients_have_one_spelling() {
        assert_eq!(normalize_recipient(" +7 (916) 123-45-67 ").unwrap(), "+79161234567");
        assert_eq!(normalize_recipient("8 916 1234567").unwrap(), "89161234567");
        assert_eq!(normalize_recipient("Anna@Example.COM").unwrap(), "anna@example.com");
        for bad in ["", "Anna", "anna@", "@example.com", "a@b", "+7916x", "12", "1234567890123456", "a@b.c, d@e.f", "chat123456"] {
            assert!(normalize_recipient(bad).is_err(), "{bad:?} was accepted as a recipient");
        }
    }

    #[test]
    fn a_message_is_sent_exactly_as_shown() {
        let Request::MessagesSend { to, text, service } = parse(
            "messages",
            "send",
            json!({"to": "+7 916 123 45 67", "text": "  hi\u{202E}dlrow\u{200B}\r\nline two\u{0007}  "}),
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(to, "+79161234567");
        // Direction overrides, zero-width marks and controls are gone, so
        // the window cannot show one thing while another is sent.
        assert_eq!(text, "hidlrow\nline two");
        assert_eq!(service, MsgService::IMessage);
        let long = "x".repeat(MESSAGE_TEXT_MAX + 1);
        assert!(parse("messages", "send", json!({"to": "+79161234567", "text": long})).is_err());
        assert!(parse("messages", "send", json!({"to": "+79161234567", "text": " \n "})).is_err());
        assert!(parse("messages", "send", json!({"to": "+79161234567", "text": "x", "service": "fax"})).is_err());
    }

    #[test]
    fn only_web_addresses_open() {
        for bad in [
            "file:///etc/passwd", "tel:+79161234567", "facetime://x", "javascript:alert(1)",
            "x-apple.systempreferences:x", "shortcuts://run-shortcut?name=x", "ftp://example.com/",
            "https://", "https://-x.com", "https://user:pw@example.com/", "https://example.com/a b",
        ] {
            assert!(parse("open", "url", json!({"url": bad})).is_err(), "{bad:?} was accepted");
        }
        let Request::OpenUrl { url, host } =
            parse("open", "url", json!({"url": "HTTPS://Maps.Google.com/maps?q=1"})).unwrap()
        else {
            panic!()
        };
        assert_eq!(url, "HTTPS://Maps.Google.com/maps?q=1");
        assert_eq!(host, "google.com");
    }

    #[test]
    fn app_names_are_names() {
        for bad in ["", "-a", "../Terminal", "/Applications/Safari.app", "C:\\x.exe", ".hidden", "a|b"] {
            assert!(parse("open", "app", json!({"name": bad})).is_err(), "{bad:?} was accepted");
        }
        assert!(parse("open", "app", json!({"name": "Visual Studio Code"})).is_ok());
    }

    #[test]
    fn bundle_ids_are_bundle_ids() {
        match parse("open", "app", json!({"name": "Finder", "bundle_id": "com.apple.finder"})).unwrap() {
            Request::OpenApp { name, bundle_id } => {
                assert_eq!(name, "Finder");
                assert_eq!(bundle_id.as_deref(), Some("com.apple.finder"));
            }
            other => panic!("{other:?}"),
        }
        match parse("open", "app", json!({"name": "Finder", "bundle_id": ""})).unwrap() {
            Request::OpenApp { bundle_id, .. } => assert!(bundle_id.is_none()),
            other => panic!("{other:?}"),
        }
        for bad in ["-a", "finder", "/Applications/Safari.app", "com.apple..x", ".com.x", "com.apple.finder;rm", "com apple", "com.x."] {
            assert!(
                parse("open", "app", json!({"name": "Finder", "bundle_id": bad})).is_err(),
                "{bad:?} was accepted"
            );
        }
    }

    #[test]
    fn only_documents_open() {
        use std::path::Path;
        for ok in ["/x/report.pdf", "/x/Photo.JPG", "/x/notes.md", "/x/song.mp3"] {
            assert!(openable(Path::new(ok), false).is_ok(), "{ok} was refused");
        }
        for bad in [
            "/x/run.command", "/x/a.sh", "/x/a.app", "/x/a.scpt", "/x/a.workflow", "/x/a.shortcut",
            "/x/setup.exe", "/x/a.bat", "/x/a.ps1", "/x/a.lnk", "/x/a.pkg", "/x/a.dmg", "/x/a.jar",
            "/x/a.html", "/x/a.webloc", "/x/a.terminal", "/x/Makefile", "/x/a.desktop", "/x/a.js",
        ] {
            assert!(openable(Path::new(bad), false).is_err(), "{bad} would be opened");
        }
        assert!(openable(Path::new("/x/Photos"), true).is_ok());
        assert!(openable(Path::new("/x/Evil.app"), true).is_err());
    }

    #[test]
    fn calendar_ranges_are_bounded() {
        assert!(parse("calendar", "list", json!({"start": "2026-10-01", "end": "2026-10-31"})).is_ok());
        assert!(parse("calendar", "list", json!({"start": "2026-10-31", "end": "2026-10-01"})).is_err());
        assert!(parse("calendar", "list", json!({"start": "2026-01-01", "end": "2027-01-01"})).is_err());
        assert!(parse("calendar", "list", json!({"start": "soon", "end": "later"})).is_err());
        assert!(parse(
            "calendar",
            "create",
            json!({"title": "Dentist", "start": "2026-10-13T15:00", "end": "2026-10-13T16:00"})
        )
        .is_ok());
        assert!(parse(
            "calendar",
            "create",
            json!({"title": "", "start": "2026-10-13T15:00", "end": "2026-10-13T16:00"})
        )
        .is_err());
    }

    #[test]
    fn shortcuts_and_notes_are_checked() {
        assert!(parse("shortcuts", "run", json!({"name": "--help"})).is_err());
        assert!(parse("shortcuts", "run", json!({"name": "Morning"})).is_ok());
        assert!(parse("shortcuts", "run", json!({"name": "x", "input": "y".repeat(SHORTCUT_INPUT_MAX + 1)})).is_err());
        assert!(parse("notes", "search", json!({"query": "a"})).is_err());
        assert!(parse("notes", "read", json!({})).is_err());
        assert!(parse("notes", "read", json!({"title": "Plan"})).is_ok());
    }

    #[test]
    fn unknown_methods_are_refused() {
        assert!(matches!(parse("calendar", "delete", json!({})), Err(AppError::Denied(_))));
        assert!(matches!(parse("mail", "send", json!({})), Err(AppError::Denied(_))));
    }

    #[test]
    fn cut_counts_characters() {
        assert_eq!(cut("привет", 3), ("при".to_string(), true));
        assert_eq!(cut("hi", 3), ("hi".to_string(), false));
    }
}
