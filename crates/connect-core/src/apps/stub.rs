//! An in-memory stand-in for the apps of a computer.
//!
//! Used by the tests of the gate, by `connectd run --apps-stub` (the
//! loopback script drives every rpc path through it) and by nothing that
//! ships. It holds a small made-up address book, calendar and so on,
//! answers every catalog action from them, and records what it was asked
//! to do. Nothing here touches a real app, the clipboard or the screen.

use std::sync::Mutex;

use serde_json::{json, Value};

use super::request::Request;
use super::{AppError, Backend, Call, CallFuture, Offer, Permission, PermissionKind, PermissionState, APPS, MAC_ONLY};

/// What the stub did, in order: `<app>.<action>` plus the one fact a test
/// wants to see (a recipient, a title, a name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    pub what: String,
    pub detail: String,
}

pub struct StubBackend {
    apps: Vec<&'static str>,
    done: Mutex<Vec<Done>>,
    clipboard: Mutex<String>,
    /// When set, `contacts.search` returns the whole address book
    /// whatever the query (a misbehaving backend, to prove the gate
    /// filters).
    pub dump_contacts: bool,
}

impl StubBackend {
    /// A stub that offers exactly these apps (unknown ids are ignored).
    pub fn offering(apps: &[&str]) -> StubBackend {
        StubBackend {
            apps: APPS.iter().copied().filter(|a| apps.contains(a)).collect(),
            done: Mutex::new(Vec::new()),
            clipboard: Mutex::new("stub clipboard text".into()),
            dump_contacts: false,
        }
    }

    /// Everything a Mac offers.
    pub fn mac() -> StubBackend {
        StubBackend::offering(APPS)
    }

    /// What a Windows computer offers: no Mac app.
    pub fn windows() -> StubBackend {
        let apps: Vec<&str> = APPS.iter().copied().filter(|a| !MAC_ONLY.contains(a)).collect();
        StubBackend::offering(&apps)
    }

    /// By name, for a command line: "mac", "windows" or "none".
    pub fn named(kind: &str) -> Option<StubBackend> {
        match kind {
            "mac" | "macos" => Some(StubBackend::mac()),
            "windows" | "win" => Some(StubBackend::windows()),
            "none" => Some(StubBackend::offering(&[])),
            _ => None,
        }
    }

    pub fn done(&self) -> Vec<Done> {
        self.done.lock().unwrap().clone()
    }

    pub fn clipboard(&self) -> String {
        self.clipboard.lock().unwrap().clone()
    }

    fn note(&self, what: &str, detail: &str) {
        self.done.lock().unwrap().push(Done { what: what.into(), detail: detail.into() });
    }

    fn contacts() -> Vec<Value> {
        vec![
            json!({"name": "Anna Petrova", "organization": "Blue Finch Studio", "phones": ["+7 916 555-01-01"], "emails": ["anna@example.com"]}),
            json!({"name": "Boris Annenkov", "organization": "", "phones": ["+7 903 555-02-02"], "emails": []}),
            json!({"name": "Clara Diaz", "organization": "Harbor Clinic", "phones": ["+34 600 555 003"], "emails": ["clara@example.org"]}),
            json!({"name": "Dmitri Orlov", "organization": "", "phones": ["+7 916 555-04-04"], "emails": ["d.orlov@example.com"]}),
            json!({"name": "Eve Stone", "organization": "Stonework", "phones": [], "emails": ["eve@example.net"]}),
        ]
    }

    fn answer(&self, req: Request, synth: &str) -> Result<Value, AppError> {
        match req {
            Request::MessagesRecent { chat, limit } => {
                self.note("messages.recent", chat.as_deref().unwrap_or(""));
                let all = [
                    json!({"time": "2026-10-12T09:15:00Z", "from": "+79165550101", "chat": "+79165550101", "text": "Are we still on for Thursday?", "service": "iMessage"}),
                    json!({"time": "2026-10-12T09:10:00Z", "from": "me", "chat": "+79165550101", "text": "Running ten minutes late", "service": "iMessage"}),
                    json!({"time": "2026-10-11T18:02:00Z", "from": "+79035550202", "chat": "+79035550202", "text": "The parcel is at the door", "service": "SMS", "attachments": 1}),
                    json!({"time": "2026-10-11T12:00:00Z", "from": "clara@example.org", "chat": "clara@example.org", "text": "See you at the clinic", "service": "iMessage"}),
                ];
                let messages: Vec<Value> = all
                    .into_iter()
                    .filter(|m| chat.as_deref().is_none_or(|c| m["chat"] == c))
                    .take(limit)
                    .collect();
                Ok(json!({"messages": messages}))
            }
            Request::MessagesSend { to, text, service } => {
                self.note("messages.send", &format!("{to}|{text}"));
                Ok(json!({"sent": true, "to": to, "service": service.label()}))
            }
            Request::ContactsSearch { query, .. } => {
                self.note("contacts.search", &query);
                let contacts: Vec<Value> = Self::contacts()
                    .into_iter()
                    .filter(|c| self.dump_contacts || super::contact_matches(c, &query))
                    .collect();
                Ok(json!({"contacts": contacts}))
            }
            Request::CalendarList { query, .. } => {
                self.note("calendar.list", "");
                let all = [
                    json!({"id": "ev-1", "title": "Dentist", "start": "2026-10-13T15:00:00+03:00", "end": "2026-10-13T16:00:00+03:00", "all_day": false, "calendar": "Home", "location": "Harbor Clinic", "notes": ""}),
                    json!({"id": "ev-2", "title": "Team sync", "start": "2026-10-14T10:00:00+03:00", "end": "2026-10-14T10:30:00+03:00", "all_day": false, "calendar": "Work", "location": "", "notes": "Bring the numbers"}),
                ];
                let q = query.unwrap_or_default().to_lowercase();
                let events: Vec<Value> =
                    all.into_iter().filter(|e| q.is_empty() || e.to_string().to_lowercase().contains(&q)).collect();
                Ok(json!({"events": events}))
            }
            Request::CalendarCreate { title, calendar, .. } => {
                self.note("calendar.create", &title);
                Ok(json!({"id": "ev-new", "title": title, "calendar": calendar.unwrap_or_else(|| "Home".into())}))
            }
            Request::RemindersList { .. } => {
                self.note("reminders.list", "");
                Ok(json!({"reminders": [
                    {"id": "rem-1", "title": "Buy oat milk", "list": "Groceries", "due": "", "completed": false, "notes": ""},
                    {"id": "rem-2", "title": "Call the landlord", "list": "Reminders", "due": "2026-10-13T12:00:00+03:00", "completed": false, "notes": "About the heating"},
                ]}))
            }
            Request::RemindersCreate { title, list, .. } => {
                self.note("reminders.create", &title);
                Ok(json!({"id": "rem-new", "title": title, "list": list.unwrap_or_else(|| "Reminders".into())}))
            }
            Request::RemindersComplete { id } => {
                self.note("reminders.complete", &id);
                if id != "rem-1" && id != "rem-2" {
                    return Err(AppError::NotFound(format!("no reminder with id {id}")));
                }
                Ok(json!({"id": id, "title": "Buy oat milk", "completed": true}))
            }
            Request::NotesSearch { query, .. } => {
                self.note("notes.search", &query);
                Ok(json!({"notes": [
                    {"id": "note-1", "title": "Trip plan", "folder": "Notes", "modified": "2026-10-10T08:00:00+03:00", "snippet": "Lisbon in May: flights, a room near the river"},
                ]}))
            }
            Request::NotesRead { id, title } => {
                self.note("notes.read", id.as_deref().or(title.as_deref()).unwrap_or(""));
                Ok(json!({"id": "note-1", "title": "Trip plan", "folder": "Notes", "modified": "2026-10-10T08:00:00+03:00", "text": "Lisbon in May: flights, a room near the river, the tile museum."}))
            }
            Request::NotesCreate { title, .. } => {
                self.note("notes.create", &title);
                Ok(json!({"id": "note-new", "title": title}))
            }
            Request::ShortcutsList => {
                self.note("shortcuts.list", "");
                Ok(json!({"shortcuts": ["Morning Routine", "Log Water"]}))
            }
            Request::ShortcutsRun { name, input } => {
                self.note("shortcuts.run", &format!("{name}|{input}"));
                if name != "Morning Routine" && name != "Log Water" {
                    return Err(AppError::NotFound(format!("no shortcut named {name:?}")));
                }
                Ok(json!({"name": name, "output": format!("ran with {} bytes of input", input.len()), "duration_ms": 12}))
            }
            Request::NotifyShow { title, body } => {
                self.note("notify.show", &format!("{synth}: {title}|{body}"));
                Ok(json!({"shown": true}))
            }
            Request::ClipboardRead => {
                self.note("clipboard.read", "");
                Ok(json!({"text": self.clipboard()}))
            }
            Request::ClipboardWrite { text } => {
                self.note("clipboard.write", &text);
                *self.clipboard.lock().unwrap() = text.clone();
                Ok(json!({"bytes": text.len()}))
            }
            Request::OpenUrl { url, .. } => {
                self.note("open.url", &url);
                Ok(json!({"opened": true}))
            }
            Request::OpenFile { path } => {
                self.note("open.file", &path.to_string_lossy());
                Ok(json!({"opened": true, "path": path.to_string_lossy()}))
            }
            Request::OpenApp { name } => {
                self.note("open.app", &name);
                Ok(json!({"opened": true, "name": name}))
            }
        }
    }
}

impl Backend for StubBackend {
    fn offered(&self) -> Vec<Offer> {
        self.apps.iter().map(|a| Offer::all(a)).collect()
    }

    fn call(&self, call: Call) -> CallFuture {
        let offered = self.apps.contains(&app_of(&call.request));
        let result = if offered {
            self.answer(call.request, &call.synth_name)
        } else {
            // The gate never lets this through; a stub that is called
            // for an app it does not offer says so loudly.
            Err(AppError::Failed("the stub was called for an app it does not offer".into()))
        };
        Box::pin(async move { result })
    }

    fn permissions(&self, app: &str) -> Vec<Permission> {
        if MAC_ONLY.contains(&app) && app != "shortcuts" && self.apps.contains(&app) {
            vec![Permission { kind: PermissionKind::Automation, state: PermissionState::Granted }]
        } else {
            Vec::new()
        }
    }
}

/// The app a checked request belongs to.
pub fn app_of(req: &Request) -> &'static str {
    match req {
        Request::MessagesRecent { .. } | Request::MessagesSend { .. } => "messages",
        Request::ContactsSearch { .. } => "contacts",
        Request::CalendarList { .. } | Request::CalendarCreate { .. } => "calendar",
        Request::RemindersList { .. } | Request::RemindersCreate { .. } | Request::RemindersComplete { .. } => "reminders",
        Request::NotesSearch { .. } | Request::NotesRead { .. } | Request::NotesCreate { .. } => "notes",
        Request::ShortcutsList | Request::ShortcutsRun { .. } => "shortcuts",
        Request::NotifyShow { .. } => "notify",
        Request::ClipboardRead | Request::ClipboardWrite { .. } => "clipboard",
        Request::OpenUrl { .. } | Request::OpenFile { .. } | Request::OpenApp { .. } => "open",
    }
}
