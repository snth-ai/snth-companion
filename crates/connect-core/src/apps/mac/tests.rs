//! The Mac backend without the Mac's apps: a stand-in `osascript` (and
//! `shortcuts`, `pbcopy`...) records what it was started with and prints
//! a canned answer, and the Messages reader runs on a database built
//! here. No test in this file talks to a real app, reads the owner's
//! data, shows a notification or touches the clipboard.
//!
//! The one thing that runs for real is `/usr/bin/osascript` on scripts
//! that name NO application (the shared handlers), which needs no
//! permission and touches nothing.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};

use super::*;
use crate::apps::notify::{Notifier, NotifyFuture};

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// A stand-in program. It writes its arguments (NUL separated) to
/// `<name>.args`, its stdin to `<name>.stdin` and its environment to
/// `<name>.env`, prints `<name>.out`, prints `<name>.err` to stderr and
/// exits with the code in `<name>.code`.
const FAKE: &str = r#"#!/bin/sh
me="$0"
: > "$me.args"
for a in "$@"; do printf '%s\0' "$a" >> "$me.args"; done
cat > "$me.stdin"
env > "$me.env"
[ -f "$me.err" ] && cat "$me.err" >&2
[ -f "$me.out" ] && cat "$me.out"
exit "$(cat "$me.code" 2>/dev/null || echo 0)"
"#;

/// A stand-in `shortcuts`: `list` prints two names; `run <name>` writes
/// what it was given to the `--output-path` file.
const FAKE_SHORTCUTS: &str = r#"#!/bin/sh
me="$0"
if [ "$1" = "list" ]; then
  printf 'Morning Routine\nLog Water\n'
  exit 0
fi
printf '%s\n' "$@" > "$me.args"
out=""; in=""
while [ $# -gt 0 ]; do
  case "$1" in
    --output-path) out="$2"; shift ;;
    --input-path) in="$2"; shift ;;
  esac
  shift
done
if [ -n "$in" ]; then printf 'got: ' > "$out"; cat "$in" >> "$out"; else printf 'no input' > "$out"; fi
exit 0
"#;

struct Silent;
impl Notifier for Silent {
    fn show(&self, _title: String, _body: String) -> NotifyFuture {
        Box::pin(async { Ok(()) })
    }
}

struct Fixture {
    dir: PathBuf,
    backend: MacBackend,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "connect-apps-mac-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = |name: &str, body: &str| -> PathBuf {
            let p = dir.join(name);
            std::fs::write(&p, body).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        let backend = MacBackend {
            osascript: exe("osascript", FAKE),
            shortcuts: exe("shortcuts", FAKE_SHORTCUTS),
            pbcopy: exe("pbcopy", FAKE),
            pbpaste: exe("pbpaste", FAKE),
            open: exe("open", FAKE),
            chat_db: dir.join("chat.db"),
            notifier: Arc::new(Silent),
        };
        Fixture { dir, backend }
    }

    /// What the stand-in `program` prints next.
    fn answer(&self, program: &str, out: &str) {
        std::fs::write(self.dir.join(format!("{program}.out")), out).unwrap();
    }

    fn fail(&self, program: &str, stderr: &str) {
        std::fs::write(self.dir.join(format!("{program}.err")), stderr).unwrap();
        std::fs::write(self.dir.join(format!("{program}.code")), "1").unwrap();
    }

    fn args(&self, program: &str) -> Vec<String> {
        let raw = std::fs::read(self.dir.join(format!("{program}.args"))).unwrap_or_default();
        let mut parts: Vec<String> = raw.split(|b| *b == 0).map(|p| String::from_utf8_lossy(p).to_string()).collect();
        parts.pop(); // the piece after the last NUL
        parts
    }

    fn stdin(&self, program: &str) -> String {
        std::fs::read_to_string(self.dir.join(format!("{program}.stdin"))).unwrap_or_default()
    }

    fn env(&self, program: &str) -> String {
        std::fs::read_to_string(self.dir.join(format!("{program}.env"))).unwrap_or_default()
    }

    async fn call(&self, app: &str, action: &str, params: Value) -> Result<Value, AppError> {
        let request = Request::parse(app, action, &params)?;
        self.backend.call(Call { request, synth_name: "Mia".into() }).await
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn rec(fields: &[&str]) -> String {
    format!("{}{RS}", fields.join(&FS.to_string()))
}

const ALL_SCRIPTS: &[(&str, &str)] = &[
    ("contacts search", CONTACTS_SEARCH),
    ("contacts phones", CONTACTS_PHONES),
    ("calendar list", CALENDAR_LIST),
    ("calendar create", CALENDAR_CREATE),
    ("reminders list", REMINDERS_LIST),
    ("reminders create", REMINDERS_CREATE),
    ("reminders complete", REMINDERS_COMPLETE),
    ("notes search", NOTES_SEARCH),
    ("notes read", NOTES_READ),
    ("notes create", NOTES_CREATE),
    ("messages send", MESSAGES_SEND),
];

#[test]
fn scripts_are_constants_that_take_arguments() {
    for (name, script) in ALL_SCRIPTS {
        assert!(script.is_ascii(), "{name}: a script must be plain ASCII");
        assert!(script.contains("on run argv"), "{name}: values must arrive as arguments");
        assert!(!script.contains("do shell script"), "{name}: no shell");
        assert!(!script.contains("%s") && !script.contains("{0}"), "{name}: nothing is formatted into a script");
        assert_eq!(script.matches("tell application").count(), 1, "{name}: one app per script");
    }
    assert!(PRELUDE.is_ascii() && !PRELUDE.contains("tell application"));
    // Every app that needs Automation has a probe and a bundle id.
    for app in ["messages", "contacts", "calendar", "reminders", "notes"] {
        assert!(probe_script(app).is_some() && bundle_id(app).is_some(), "{app}");
    }
    assert!(bundle_id("shortcuts").is_none() && probe_script("clipboard").is_none());
}

/// Run PRELUDE + `body` under the real osascript. Only for scripts that
/// name no application.
fn pure_osascript(body: &str, args: &[&str]) -> String {
    use std::io::Write;
    assert!(!body.contains("application"), "this helper is for scripts without an app");
    let mut child = crate::proc::std_command("/usr/bin/osascript")
        .arg("-")
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(format!("{PRELUDE}\n{body}").as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "osascript failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string()
}

#[test]
fn the_shared_handlers_work_in_real_applescript() {
    // A date built from arguments prints back as the same local time,
    // also on the 31st -> a 30-day month (the day is set to 1 first).
    let body = "on run argv\n\treturn my iso(my argDate(argv, 1)) & (character id 31) & my iso(my argDate(argv, 7))\nend run\n";
    let out = pure_osascript(body, &["2026", "2", "28", "23", "59", "7", "2027", "11", "30", "0", "0", "0"]);
    assert_eq!(out, format!("2026-02-28T23:59:07{FS}2027-11-30T00:00:00"));

    let body = "on run argv\n\tset FS to character id 31\n\treturn my txt(missing value) & FS & my txt(42) & FS & my joinList({\"a\", \"b\"}, character id 29) & FS & my joinList({}, \"|\") & FS & my iso(missing value) & FS & (true as string) & FS & my pad2(5)\nend run\n";
    let out = pure_osascript(body, &[]);
    assert_eq!(out, format!("{FS}42{FS}a{GS}b{FS}{FS}{FS}true{FS}05"));

    // Arguments arrive exactly as given: quotes, a leading dash, line
    // breaks, another alphabet. Nothing needs escaping.
    let body = "on run argv\n\treturn (item 1 of argv) & (character id 31) & (item 2 of argv) & (character id 31) & (count of argv)\nend run\n";
    let tricky = "\" & (do shell script \"id\") & \"";
    let out = pure_osascript(body, &[tricky, "-x привет\nline two", ""]);
    assert_eq!(out, format!("{tricky}{FS}-x привет\nline two{FS}3"));
}

#[tokio::test]
async fn what_the_synth_wrote_is_an_argument_never_script_text() {
    let fx = Fixture::new();
    fx.answer("osascript", &format!("uid-1{FS}Dentist{FS}Home"));
    let hostile = "Dentist\" & (do shell script \"touch /tmp/pwned\") & \"";
    let got = fx
        .call(
            "calendar",
            "create",
            json!({"title": hostile, "start": "2026-10-13T15:00", "end": "2026-10-13T16:30", "calendar": "Home", "location": "Harbor \"Clinic\"", "notes": "line one\nline two", "all_day": false}),
        )
        .await
        .unwrap();
    assert_eq!(got, json!({"id": "uid-1", "title": "Dentist", "calendar": "Home"}));
    let args = fx.args("osascript");
    assert_eq!(
        args,
        [
            "-", "2026", "10", "13", "15", "0", "0", "2026", "10", "13", "16", "30", "0", "Home", hostile,
            "Harbor \"Clinic\"", "line one\nline two", "false"
        ]
    );
    let source = fx.stdin("osascript");
    assert!(source.contains("tell application \"Calendar\""));
    assert!(!source.contains("pwned") && !source.contains("Dentist") && !source.contains("Harbor"), "a value got into the script text");
    assert_eq!(source, format!("{PRELUDE}\n{CALENDAR_CREATE}"));
}

#[tokio::test]
async fn calendar_list_sorts_filters_and_gives_offsets() {
    let fx = Fixture::new();
    let out = [
        rec(&["u2", "Team sync", "2026-10-14T10:00:00", "2026-10-14T10:30:00", "Work", "", "Bring the numbers", "false"]),
        rec(&["u1", "Dentist", "2026-10-13T15:00:00", "2026-10-13T16:00:00", "Home", "Harbor Clinic", "", "false"]),
        rec(&["u3", "Birthday", "2026-10-15T00:00:00", "2026-10-16T00:00:00", "Home", "", "", "true"]),
        "broken record".to_string(),
    ]
    .concat();
    fx.answer("osascript", &out);
    let got = fx.call("calendar", "list", json!({"start": "2026-10-13", "end": "2026-10-20"})).await.unwrap();
    let events = got["events"].as_array().unwrap();
    assert_eq!(events.iter().map(|e| e["title"].as_str().unwrap()).collect::<Vec<_>>(), ["Dentist", "Team sync", "Birthday"]);
    assert_eq!(events[2]["all_day"], true);
    // RFC 3339 with this Mac's offset.
    let start = events[0]["start"].as_str().unwrap();
    assert!(start.starts_with("2026-10-13T15:00:00") && start.len() == 25, "{start}");
    assert_eq!(got["truncated"], false);
    let args = fx.args("osascript");
    assert_eq!(&args[1..7], ["2026", "10", "13", "0", "0", "0"]);
    assert_eq!(&args[7..13], ["2026", "10", "20", "0", "0", "0"]);
    assert_eq!(args[13], "", "no calendar filter");

    // A text filter and a limit, done here.
    let got = fx.call("calendar", "list", json!({"start": "2026-10-13", "end": "2026-10-20", "query": "clinic"})).await.unwrap();
    assert_eq!(got["events"].as_array().unwrap().len(), 1);
    let got = fx.call("calendar", "list", json!({"start": "2026-10-13", "end": "2026-10-20", "limit": 2})).await.unwrap();
    assert_eq!(got["events"].as_array().unwrap().len(), 2);
    assert_eq!(got["truncated"], true);
}

#[tokio::test]
async fn script_errors_become_labels_without_the_apps_words() {
    let fx = Fixture::new();
    for (stderr, want) in [
        ("0:1: execution error: Not authorized to send Apple events to Calendar. (-1743)\n", "unavailable: SNTH Connect is not allowed to control Calendar"),
        ("execution error: Calendar got an error: Can't get calendar 1 whose name = \"Secret Project\". (-1719)\n", "not found: no calendar named \"Work\""),
        ("execution error: Calendar got an error: Application isn't running. (-600)\n", "unavailable: Calendar is not running or did not answer"),
        ("12:20: syntax error: Expected end of line but found identifier. (-2741)\n", "the Calendar script does not work on this version of macOS (error -2741)"),
        ("execution error: Calendar got an error: The event \"Therapy with Dr. K\" is read only. (-10006)\n", "Calendar reported error -10006"),
        ("something went wrong\n", "Calendar could not do it"),
    ] {
        fx.fail("osascript", stderr);
        let e = fx
            .call("calendar", "list", json!({"start": "2026-10-13", "end": "2026-10-20", "calendar": "Work"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(e.starts_with(want), "{stderr:?} -> {e}");
        assert!(!e.contains("Secret Project") && !e.contains("Therapy"), "the error quotes the app: {e}");
    }
    // A locked note is the owner's lock, so it reads as a refusal.
    fx.fail("osascript", "execution error: locked (9001)\n");
    let e = fx.call("notes", "read", json!({"title": "Diary"})).await.unwrap_err().to_string();
    assert_eq!(e, "denied: that note is locked with a password");
}

#[tokio::test]
async fn contacts_search_by_name_and_by_number() {
    let fx = Fixture::new();
    let out = [
        rec(&["Anna Petrova", "Blue Finch", &format!("+7 916 555-01-01{GS}+7 495 555-01-02"), "anna@example.com"]),
        rec(&["Anna Petrova", "Blue Finch", &format!("+7 916 555-01-01{GS}+7 495 555-01-02"), "anna@example.com"]),
        rec(&["Annabel Lee", "missing value", "", ""]),
    ]
    .concat();
    fx.answer("osascript", &out);
    let got = fx.call("contacts", "search", json!({"query": "Anna"})).await.unwrap();
    let c = got["contacts"].as_array().unwrap();
    assert_eq!(c.len(), 2, "the same person twice is one result");
    assert_eq!(c[0]["phones"], json!(["+7 916 555-01-01", "+7 495 555-01-02"]));
    assert_eq!(c[0]["emails"], json!(["anna@example.com"]));
    assert_eq!(fx.args("osascript")[1], "Anna");
    assert!(fx.stdin("osascript").contains("tell application \"Contacts\"") && !fx.stdin("osascript").contains("Anna"));

    // By number: the script hands over names and numbers, the match is
    // made here, and only the match is returned.
    let out = [
        rec(&["Anna Petrova", &format!("+7 916 555-01-01{GS}+7 495 555-01-02")]),
        rec(&["Boris Annenkov", "+7 903 555-02-02"]),
        rec(&["Clara Diaz", "+34 600 555 003"]),
    ]
    .concat();
    fx.answer("osascript", &out);
    let got = fx.call("contacts", "search", json!({"query": "+7 (903) 555"})).await.unwrap();
    let c = got["contacts"].as_array().unwrap();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0]["name"], "Boris Annenkov");
    assert_eq!(fx.args("osascript"), ["-"], "the number is not sent to the script at all");
}

#[tokio::test]
async fn reminders_and_notes_round_trip_through_their_scripts() {
    let fx = Fixture::new();
    fx.answer(
        "osascript",
        &[
            rec(&["x-apple-reminder://1", "Buy oat milk", "Groceries", "", "false", "missing value"]),
            rec(&["x-apple-reminder://2", "Call the landlord", "Reminders", "2026-10-13T12:00:00", "false", "About the heating"]),
        ]
        .concat(),
    );
    let got = fx.call("reminders", "list", json!({"list": "Groceries"})).await.unwrap();
    assert_eq!(got["reminders"].as_array().unwrap().len(), 2);
    assert_eq!(got["reminders"][0]["due"], "");
    assert!(got["reminders"][1]["due"].as_str().unwrap().starts_with("2026-10-13T12:00:00"));
    assert_eq!(&fx.args("osascript")[1..], ["Groceries", "false", "51"]);

    fx.answer("osascript", &format!("rid{FS}Pay rent{FS}Reminders"));
    let got = fx.call("reminders", "create", json!({"title": "Pay rent", "due": "2026-11-01T09:00", "notes": "bank app"})).await.unwrap();
    assert_eq!(got, json!({"id": "rid", "title": "Pay rent", "list": "Reminders"}));
    assert_eq!(&fx.args("osascript")[1..], ["Pay rent", "", "bank app", "true", "2026", "11", "1", "9", "0", "0"]);

    fx.answer("osascript", &format!("rid{FS}Pay rent"));
    let got = fx.call("reminders", "complete", json!({"id": "rid"})).await.unwrap();
    assert_eq!(got["completed"], true);

    fx.answer("osascript", &rec(&["n1", "Trip plan", "Notes", "2026-10-10T08:00:00", "Lisbon in May:\n flights,   a room"]));
    let got = fx.call("notes", "search", json!({"query": "Lisbon"})).await.unwrap();
    assert_eq!(got["notes"][0]["snippet"], "Lisbon in May: flights, a room");

    // The text of a note may itself hold the separator characters after
    // the fourth field: it is taken whole.
    fx.answer("osascript", &format!("n1{FS}Trip plan{FS}Notes{FS}2026-10-10T08:00:00{FS}line one\nline{FS}two"));
    let got = fx.call("notes", "read", json!({"id": "n1"})).await.unwrap();
    assert_eq!(got["text"], format!("line one\nline{FS}two"));
    assert_eq!(&fx.args("osascript")[1..], ["n1", ""]);

    fx.answer("osascript", &format!("n2{FS}Plan"));
    fx.call("notes", "create", json!({"title": "Plan", "text": "a <b> & c\n\nlast"})).await.unwrap();
    assert_eq!(&fx.args("osascript")[1..], ["Plan", "<div>a &lt;b&gt; &amp; c</div><div><br></div><div>last</div>", ""]);
}

#[tokio::test]
async fn a_message_goes_out_as_two_arguments() {
    let fx = Fixture::new();
    fx.answer("osascript", "ok");
    let got = fx.call("messages", "send", json!({"to": "+7 916 123-45-67", "text": "See you at 5\n\"Anna\"", "service": "sms"})).await.unwrap();
    assert_eq!(got, json!({"sent": true, "to": "+79161234567", "service": "sms"}));
    assert_eq!(&fx.args("osascript")[1..], ["+79161234567", "See you at 5\n\"Anna\"", "sms"]);
    let source = fx.stdin("osascript");
    assert!(source.contains("tell application \"Messages\"") && !source.contains("See you"));
}

fn chat_db(path: &Path) {
    let c = rusqlite::Connection::open(path).unwrap();
    c.execute_batch(
        "CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT);
         CREATE TABLE chat (ROWID INTEGER PRIMARY KEY, chat_identifier TEXT, display_name TEXT);
         CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER);
         CREATE TABLE message (ROWID INTEGER PRIMARY KEY, date INTEGER, text TEXT, attributedBody BLOB,
            is_from_me INTEGER, handle_id INTEGER, service TEXT, cache_has_attachments INTEGER,
            associated_message_type INTEGER, item_type INTEGER);
         INSERT INTO handle VALUES (1, '+79165550101'), (2, 'clara@example.org');
         INSERT INTO chat VALUES (1, '+79165550101', ''), (2, 'clara@example.org', ''), (3, 'chat123', 'Family');",
    )
    .unwrap();
    // 2026-10-12T09:00:00Z in nanoseconds since 2001.
    let base: i64 = (1_791_795_600 - 978_307_200) * 1_000_000_000;
    let ns = |minutes: i64| base + minutes * 60 * 1_000_000_000;
    // The archived form newer systems use instead of the text column.
    let mut archived = b"\x04\x0bstreamtyped\x81\xe8\x03\x84\x01@\x84\x84\x84\x12NSAttributedString\x00\x84\x84\x08NSObject\x00\x85\x92\x84\x84\x84\x08NSString\x01\x94\x84\x01+".to_vec();
    let body = "Текст из архива";
    archived.push(body.len() as u8);
    archived.extend_from_slice(body.as_bytes());
    archived.extend_from_slice(b"\x86\x84\x02iI\x01\x0f");
    // rowid, date, text, body, from_me, handle, service, attach, reaction, item_type, chat
    type Row<'a> = (i64, i64, Option<&'a str>, Option<Vec<u8>>, i64, i64, &'a str, i64, i64, i64, i64);
    let rows: Vec<Row> = vec![
        (1, ns(0), Some("Are we still on for Thursday?"), None, 0, 1, "iMessage", 0, 0, 0, 1),
        (2, ns(1), Some("Yes, at five"), None, 1, 1, "iMessage", 0, 0, 0, 1),
        (3, ns(2), None, Some(archived), 0, 1, "iMessage", 0, 0, 0, 1),
        (4, ns(3), Some("Loved \"Yes, at five\""), None, 0, 1, "iMessage", 0, 2000, 0, 1),
        (5, ns(4), Some("\u{FFFC}"), None, 0, 2, "iMessage", 1, 0, 0, 2),
        (6, ns(5), None, None, 0, 2, "iMessage", 0, 0, 0, 2),
        (7, ns(6), Some("See you at the clinic"), None, 0, 2, "iMessage", 0, 0, 0, 2),
        (8, ns(7), Some("Dinner on Sunday"), None, 0, 1, "iMessage", 0, 0, 0, 3),
        (9, ns(8), None, None, 0, 1, "iMessage", 0, 0, 1, 3),
    ];
    for (rowid, date, text, body, from_me, handle, service, attach, reaction, item, chat) in rows {
        c.execute(
            "INSERT INTO message VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![rowid, date, text, body, from_me, handle, service, attach, reaction, item],
        )
        .unwrap();
        c.execute("INSERT INTO chat_message_join VALUES (?1, ?2)", rusqlite::params![chat, rowid]).unwrap();
    }
}

#[tokio::test]
async fn messages_are_read_from_the_history_database() {
    let fx = Fixture::new();
    chat_db(&fx.backend.chat_db);

    // Across conversations, newest first; the reaction, the empty row
    // and the group event are not messages.
    let got = fx.call("messages", "recent", json!({})).await.unwrap();
    let m = got["messages"].as_array().unwrap();
    let texts: Vec<&str> = m.iter().map(|x| x["text"].as_str().unwrap()).collect();
    assert_eq!(
        texts,
        ["Dinner on Sunday", "See you at the clinic", "[attachment]", "Текст из архива", "Yes, at five", "Are we still on for Thursday?"]
    );
    assert_eq!(m[0]["chat"], "chat123");
    assert_eq!(m[0]["chat_name"], "Family");
    assert_eq!(m[2]["attachments"], 1);
    assert!(m[2].get("path").is_none());
    assert_eq!(m[4]["from"], "me");
    assert_eq!(m[5]["from"], "+79165550101");
    assert_eq!(m[5]["time"], "2026-10-12T09:00:00Z");
    assert_eq!(got["truncated"], false);

    // One conversation, by the number as a person writes it.
    let got = fx.call("messages", "recent", json!({"chat": "+7 (916) 555-01-01"})).await.unwrap();
    let m = got["messages"].as_array().unwrap();
    assert_eq!(m.len(), 4);
    assert!(m.iter().all(|x| x["chat"] == "+79165550101" || x["chat"] == "chat123"));

    // A hard cap on the count.
    let got = fx.call("messages", "recent", json!({"limit": 2})).await.unwrap();
    assert_eq!(got["messages"].as_array().unwrap().len(), 2);
    assert_eq!(got["truncated"], true);

    // A filter is a value of the query, not a piece of it.
    let got = fx.call("messages", "recent", json!({"chat": "chat123"})).await.unwrap();
    assert_eq!(got["messages"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn without_the_history_reading_says_what_is_missing() {
    let fx = Fixture::new();
    let e = fx.call("messages", "recent", json!({})).await.unwrap_err().to_string();
    assert_eq!(e, "unavailable: Messages has no history on this Mac");
    // A file that cannot be read stands for "no Full Disk Access".
    std::fs::write(&fx.backend.chat_db, b"").unwrap();
    std::fs::set_permissions(&fx.backend.chat_db, std::fs::Permissions::from_mode(0o000)).unwrap();
    let e = fx.call("messages", "recent", json!({})).await.unwrap_err().to_string();
    assert!(e.starts_with("unavailable: reading messages needs Full Disk Access"), "{e}");
    assert_eq!(full_disk_state(&fx.backend.chat_db), PermissionState::Denied);
    assert_eq!(full_disk_state(&fx.dir.join("nothing-here")), PermissionState::Unknown);
    std::fs::set_permissions(&fx.backend.chat_db, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(full_disk_state(&fx.backend.chat_db), PermissionState::Granted);
}

#[test]
fn archived_text_lengths() {
    let wrap = |len_bytes: &[u8], text: &str| {
        let mut b = b"....NSString\x01\x94\x84\x01+".to_vec();
        b.extend_from_slice(len_bytes);
        b.extend_from_slice(text.as_bytes());
        b.extend_from_slice(b"\x86\x84");
        b
    };
    assert_eq!(attributed_text(&wrap(&[5], "hello")).as_deref(), Some("hello"));
    let long = "x".repeat(300);
    assert_eq!(attributed_text(&wrap(&[0x81, 0x2c, 0x01], &long)).as_deref(), Some(long.as_str()));
    assert_eq!(attributed_text(b"no marker here"), None);
    assert_eq!(attributed_text(&wrap(&[200], "short")), None, "a length past the end is not text");
    assert_eq!(attributed_text(b"NSString"), None);
}

#[tokio::test]
async fn shortcuts_run_by_exact_name_with_private_files() {
    let fx = Fixture::new();
    let got = fx.call("shortcuts", "list", json!({})).await.unwrap();
    assert_eq!(got["shortcuts"], json!(["Morning Routine", "Log Water"]));

    let got = fx.call("shortcuts", "run", json!({"name": "Log Water", "input": "250 ml"})).await.unwrap();
    assert_eq!(got["output"], "got: 250 ml");
    assert_eq!(got["name"], "Log Water");
    let args = std::fs::read_to_string(fx.dir.join("shortcuts.args")).unwrap();
    let lines: Vec<&str> = args.lines().collect();
    assert_eq!(&lines[..3], ["run", "Log Water", "--output-path"]);
    // The temporary files are gone afterwards.
    for l in lines.iter().filter(|l| l.contains("snth-shortcut-")) {
        assert!(!Path::new(l).exists(), "{l} was left behind");
    }

    let got = fx.call("shortcuts", "run", json!({"name": "Morning Routine"})).await.unwrap();
    assert_eq!(got["output"], "no input");

    // A name that is not a shortcut is never handed to the tool.
    let _ = std::fs::remove_file(fx.dir.join("shortcuts.args"));
    let e = fx.call("shortcuts", "run", json!({"name": "list"})).await.unwrap_err().to_string();
    assert_eq!(e, "not found: no shortcut named \"list\"");
    assert!(!fx.dir.join("shortcuts.args").exists());
}

#[tokio::test]
async fn clipboard_open_and_notify_use_the_small_tools() {
    let fx = Fixture::new();
    fx.answer("pbpaste", "скопировано");
    let got = fx.call("clipboard", "read", json!({})).await.unwrap();
    assert_eq!(got["text"], "скопировано");
    // Without a UTF-8 locale the real tools mangle such text.
    assert!(fx.env("pbpaste").contains("LC_ALL=en_US.UTF-8"));

    fx.call("clipboard", "write", json!({"text": "новый текст"})).await.unwrap();
    assert_eq!(fx.stdin("pbcopy"), "новый текст");
    assert!(fx.env("pbcopy").contains("LC_ALL=en_US.UTF-8"));

    fx.call("open", "url", json!({"url": "https://example.com/a?b=1"})).await.unwrap();
    assert_eq!(fx.args("open"), ["https://example.com/a?b=1"]);
    fx.call("open", "app", json!({"name": "Visual Studio Code"})).await.unwrap();
    assert_eq!(fx.args("open"), ["-a", "Visual Studio Code"]);
    std::fs::write(fx.dir.join("open.code"), "1").unwrap();
    let e = fx.call("open", "app", json!({"name": "Nope"})).await.unwrap_err().to_string();
    assert_eq!(e, "not found: no app named \"Nope\" on this Mac");

    assert_eq!(fx.call("notify", "show", json!({"title": "Done", "body": "All set"})).await.unwrap(), json!({"shown": true}));
}

#[test]
fn a_mac_offers_every_app() {
    let fx = Fixture::new();
    let ids: Vec<&str> = fx.backend.offered().iter().map(|o| o.app).collect();
    assert_eq!(ids, APPS);
    // Which system permissions each app shows on the Apps screen.
    let kinds = |app: &str| fx.backend.permissions(app).iter().map(|p| p.kind).collect::<Vec<_>>();
    assert_eq!(kinds("messages"), [PermissionKind::Automation, PermissionKind::FullDisk]);
    assert_eq!(kinds("calendar"), [PermissionKind::Automation]);
    assert!(kinds("shortcuts").is_empty() && kinds("clipboard").is_empty() && kinds("open").is_empty());
}

#[test]
fn the_automation_check_asks_the_system_without_prompting() {
    // An app that does not exist cannot be running and cannot raise a
    // prompt: the system answers "no such process", which is Unknown.
    // This proves the call links and returns.
    assert_eq!(automation_state("ai.snth.connect.test.no-such-app"), PermissionState::Unknown);
}

#[test]
fn note_text_becomes_simple_html() {
    assert_eq!(note_html("a\n\nb < c"), "<div>a</div><div><br></div><div>b &lt; c</div>");
    assert_eq!(note_html(""), "<div><br></div>");
}

#[test]
fn error_numbers_are_read_from_the_last_line() {
    assert_eq!(script_error_number("x\n34:56: execution error: Notes got an error: Can't get note. (-1728)\n"), Some(-1728));
    assert_eq!(script_error_number("no number"), None);
    assert_eq!(script_error_number(""), None);
}
