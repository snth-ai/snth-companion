//! The gate, against the in-memory stub: risk classes, the per-app
//! switches, what a platform offers, the rule for sending a message,
//! bounded results, and what the journal may hold.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::stub::StubBackend;
use super::*;
use crate::cookies::MultiReader;
use crate::core::{Core, CoreConfig, CuaAttach};
use crate::grant::{Approval, ApprovalFuture, Approver, Duration_};
use crate::secret::MemSecretStore;
use crate::settings::Preset;

/// Answers from a queue (then `fallback`) and keeps every ask it got.
struct Script {
    answers: Mutex<VecDeque<Approval>>,
    fallback: Approval,
    seen: Mutex<Vec<ApprovalRequest>>,
}

impl Script {
    fn new(fallback: Approval) -> Arc<Script> {
        Arc::new(Script { answers: Mutex::new(VecDeque::new()), fallback, seen: Mutex::new(Vec::new()) })
    }
    fn next(&self, a: Approval) {
        self.answers.lock().unwrap().push_back(a);
    }
    fn asks(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
    fn last(&self) -> ApprovalRequest {
        self.seen.lock().unwrap().last().cloned().expect("no ask was made")
    }
}

impl Approver for Script {
    fn ask(&self, req: ApprovalRequest) -> ApprovalFuture {
        self.seen.lock().unwrap().push(req);
        let answer = self.answers.lock().unwrap().pop_front().unwrap_or(self.fallback);
        Box::pin(async move { answer })
    }
}

struct Env {
    core: Core,
    stub: Arc<StubBackend>,
    owner: Arc<Script>,
}

fn env_with(stub: StubBackend, fallback: Approval) -> Env {
    let stub = Arc::new(stub);
    let owner = Script::new(fallback);
    let core = Core::new(CoreConfig {
        settings_path: None,
        journal_path: None,
        approver: owner.clone(),
        cookies: Arc::new(MultiReader { chromium: vec![], firefox: vec![], keys: Arc::new(crate::cookies::fixtures::NoKey) }),
        apps: stub.clone(),
        secret: Arc::new(MemSecretStore::new()),
        hub: Arc::new(crate::UreqHubClient),
        app_version: "t".into(),
        cua_bin: "cua-driver-not-installed".into(),
        cua_private_root: None,
        cua_attach: CuaAttach::User,
    });
    Env { core, stub, owner }
}

/// A Mac with apps on in `mode`, Messages switched on too.
fn mac(mode: AppsMode, fallback: Approval) -> Env {
    let env = env_with(StubBackend::mac(), fallback);
    env.core.mutate_settings(|s| {
        s.apps.enabled = true;
        s.apps.mode = mode;
        s.apps.apps.insert("messages".into(), true);
    });
    env
}

impl Env {
    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        handle(&self.core.shared(), method, &params).await
    }
    fn did(&self, what: &str) -> usize {
        self.stub.done().iter().filter(|d| d.what == what).count()
    }
    fn journal(&self) -> String {
        serde_json::to_string(&self.core.recent_journal()).unwrap()
    }
}

fn send(to: &str, text: &str) -> Value {
    json!({"to": to, "text": text, "reason": "the owner asked me to"})
}

const ALWAYS: Approval = Approval { allow: true, duration: Duration_::Always };
const ONCE: Approval = Approval { allow: true, duration: Duration_::Once };
const SESSION: Approval = Approval { allow: true, duration: Duration_::Session };
const NO: Approval = Approval { allow: false, duration: Duration_::Once };

#[test]
fn every_catalog_action_is_listed_once_and_belongs_to_a_known_app() {
    for a in CATALOG {
        assert!(APPS.contains(&a.app), "{} is not in APPS", a.app);
        assert_eq!(CATALOG.iter().filter(|b| b.app == a.app && b.action == a.action).count(), 1);
    }
    for app in APPS {
        assert!(!actions_of(app).is_empty(), "{app} has no action");
    }
    // The risk classes, spelled out: a change of class is a policy change.
    let class = |app: &str, action: &str| find(app, action).unwrap().risk;
    for (app, action) in [
        ("messages", "recent"), ("contacts", "search"), ("calendar", "list"), ("reminders", "list"),
        ("notes", "search"), ("notes", "read"), ("shortcuts", "list"), ("clipboard", "read"),
    ] {
        assert_eq!(class(app, action), Risk::Read, "{app}.{action}");
    }
    for (app, action) in [
        ("messages", "send"), ("calendar", "create"), ("reminders", "create"), ("reminders", "complete"),
        ("notes", "create"), ("shortcuts", "run"), ("clipboard", "write"), ("open", "url"), ("open", "file"),
        ("open", "app"),
    ] {
        assert_eq!(class(app, action), Risk::Act, "{app}.{action}");
    }
    assert_eq!(class("notify", "show"), Risk::ActLight);
}

#[tokio::test]
async fn off_until_the_owner_turns_it_on() {
    let env = env_with(StubBackend::mac(), ALWAYS);
    let e = env.call("apps.calendar.list", json!({"start": "2026-10-13", "end": "2026-10-14"})).await.unwrap_err();
    assert_eq!(e, "denied: apps are turned off on this computer");
    assert!(env.stub.done().is_empty());
    // The synth is told the same in the hello: enabled false.
    let wire = wire_value(env.stub.as_ref(), &env.core.settings().apps);
    assert_eq!(wire["enabled"], false);
}

#[tokio::test]
async fn pause_refuses_everything() {
    let env = mac(AppsMode::Auto, ALWAYS);
    env.core.pause().await;
    let e = env.call("apps.clipboard.read", json!({})).await.unwrap_err();
    assert_eq!(e, "denied: apps are paused by the owner");
    assert!(env.stub.done().is_empty());
}

#[tokio::test]
async fn a_windows_computer_has_no_mac_app_anywhere() {
    let env = env_with(StubBackend::windows(), ALWAYS);
    env.core.apply_preset(Preset::Full);
    // What the owner sees.
    let ids: Vec<&str> = env.core.apps_views().iter().map(|v| v.id).collect();
    assert_eq!(ids, ["notify", "clipboard", "open"]);
    // What the synth is told.
    let wire = crate::wire::capabilities_value(&env.core.settings(), "", "ok", env.stub.as_ref());
    let text = wire["apps"].to_string();
    for mac_app in MAC_ONLY {
        assert!(!text.contains(mac_app), "{mac_app} is in the Windows hello: {text}");
    }
    assert_eq!(wire["apps"]["enabled"], true);
    assert_eq!(wire["apps"]["mode"], "auto");
    assert_eq!(wire["apps"]["apps"][0], json!({"id": "notify", "on": true, "actions": ["show"]}));
    // What happens if the synth asks anyway.
    for method in ["apps.calendar.list", "apps.messages.send", "apps.contacts.search", "apps.shortcuts.run"] {
        let e = env.call(method, json!({"query": "anna", "to": "+79161234567", "text": "x"})).await.unwrap_err();
        assert!(e.starts_with("unavailable: "), "{method}: {e}");
        assert!(e.contains("not available on this computer"), "{method}: {e}");
    }
    assert!(env.stub.done().is_empty());
    assert_eq!(env.owner.asks(), 0);
    // The three that exist work.
    env.call("apps.notify.show", json!({"title": "Done"})).await.unwrap();
    env.call("apps.clipboard.write", json!({"text": "copied"})).await.unwrap();
    assert_eq!(env.call("apps.clipboard.read", json!({})).await.unwrap()["text"], "copied");
    env.call("apps.open.url", json!({"url": "https://example.com/"})).await.unwrap();
}

#[tokio::test]
async fn a_mac_lists_every_app_with_messages_off_by_default() {
    let env = env_with(StubBackend::mac(), ALWAYS);
    env.core.apply_preset(Preset::Balanced);
    let views = env.core.apps_views();
    assert_eq!(views.iter().map(|v| v.id).collect::<Vec<_>>(), APPS);
    for v in &views {
        assert_eq!(v.on, v.id != "messages", "{}", v.id);
    }
    let e = env.call("apps.messages.recent", json!({})).await.unwrap_err();
    assert!(e.starts_with("denied: Messages is turned off on this computer"), "{e}");
    let e = env.call("apps.messages.send", send("+79161234567", "hi")).await.unwrap_err();
    assert!(e.starts_with("denied: Messages is turned off"), "{e}");
    assert_eq!(env.owner.asks(), 0, "a switched-off app must not even ask");
    assert!(env.stub.done().is_empty());
}

#[tokio::test]
async fn per_app_switches_hold() {
    let env = mac(AppsMode::Auto, ALWAYS);
    env.core.mutate_settings(|s| {
        s.apps.apps.insert("notes".into(), false);
    });
    let e = env.call("apps.notes.search", json!({"query": "trip"})).await.unwrap_err();
    assert!(e.starts_with("denied: Notes is turned off"), "{e}");
    let e = env.call("apps.notes.create", json!({"title": "x"})).await.unwrap_err();
    assert!(e.starts_with("denied: Notes is turned off"), "{e}");
    // Calendar is untouched by the Notes switch.
    env.call("apps.calendar.list", json!({"start": "2026-10-13", "end": "2026-10-20"})).await.unwrap();
    assert_eq!(env.did("calendar.list"), 1);
    assert_eq!(env.did("notes.search") + env.did("notes.create"), 0);
}

#[tokio::test]
async fn reading_never_asks_in_any_mode() {
    for mode in [AppsMode::Ask, AppsMode::AskNew, AppsMode::Auto] {
        let env = mac(mode, NO);
        env.call("apps.messages.recent", json!({"limit": 2})).await.unwrap();
        env.call("apps.contacts.search", json!({"query": "anna"})).await.unwrap();
        env.call("apps.calendar.list", json!({"start": "2026-10-13", "end": "2026-10-20"})).await.unwrap();
        env.call("apps.reminders.list", json!({})).await.unwrap();
        env.call("apps.notes.search", json!({"query": "trip"})).await.unwrap();
        env.call("apps.notes.read", json!({"id": "note-1"})).await.unwrap();
        env.call("apps.shortcuts.list", json!({})).await.unwrap();
        env.call("apps.clipboard.read", json!({})).await.unwrap();
        env.call("apps.notify.show", json!({"title": "Ping"})).await.unwrap();
        assert_eq!(env.owner.asks(), 0, "mode {mode:?} asked about a read");
    }
}

fn act_calls() -> Vec<(&'static str, Value)> {
    vec![
        ("apps.calendar.create", json!({"title": "Dentist", "start": "2026-10-13T15:00", "end": "2026-10-13T16:00"})),
        ("apps.reminders.create", json!({"title": "Buy oat milk"})),
        ("apps.reminders.complete", json!({"id": "rem-1"})),
        ("apps.notes.create", json!({"title": "Plan", "text": "first line"})),
        ("apps.shortcuts.run", json!({"name": "Morning Routine"})),
        ("apps.clipboard.write", json!({"text": "copied"})),
        ("apps.open.url", json!({"url": "https://example.com/a"})),
        ("apps.open.app", json!({"name": "Safari"})),
    ]
}

#[tokio::test]
async fn ask_mode_asks_before_every_change_and_remembers_nothing() {
    let env = mac(AppsMode::Ask, ALWAYS);
    for (method, params) in act_calls() {
        env.call(method, params.clone()).await.unwrap();
        env.call(method, params).await.unwrap();
    }
    assert_eq!(env.owner.asks(), act_calls().len() * 2, "the owner said always, and is still asked every time");
    assert!(env.core.grant_list().is_empty(), "ask mode must not store an answer");
    assert_eq!(env.owner.last().remember, Remember::Never);
}

#[tokio::test]
async fn ask_new_mode_asks_once_per_kind_of_change() {
    let env = mac(AppsMode::AskNew, ALWAYS);
    for (method, params) in act_calls() {
        env.call(method, params.clone()).await.unwrap();
        env.call(method, params).await.unwrap();
    }
    assert_eq!(env.owner.asks(), act_calls().len(), "one ask per kind, then remembered");
    // A remembered yes is for that kind only.
    let scopes: Vec<String> = env.core.grant_list().into_iter().map(|(_, s, _)| s).collect();
    for want in ["calendar.create", "shortcuts.run:Morning Routine", "open.url:example.com", "open.app:safari"] {
        assert!(scopes.iter().any(|s| s == want), "no grant {want} in {scopes:?}");
    }
    // Another site, another shortcut: asked again.
    env.call("apps.open.url", json!({"url": "https://other.example.org/"})).await.unwrap();
    env.call("apps.shortcuts.run", json!({"name": "Log Water"})).await.unwrap();
    assert_eq!(env.owner.asks(), act_calls().len() + 2);
}

#[tokio::test]
async fn auto_mode_does_not_ask_about_changes() {
    let env = mac(AppsMode::Auto, NO);
    for (method, params) in act_calls() {
        env.call(method, params).await.unwrap();
    }
    assert_eq!(env.owner.asks(), 0);
}

#[tokio::test]
async fn a_message_asks_every_time_in_every_mode() {
    for mode in [AppsMode::Ask, AppsMode::AskNew, AppsMode::Auto] {
        let env = mac(mode, ONCE);
        for i in 0..3 {
            env.call("apps.messages.send", send("+7 916 123-45-67", &format!("message {i}"))).await.unwrap();
        }
        assert_eq!(env.owner.asks(), 3, "mode {mode:?}: every send must ask");
        assert_eq!(env.did("messages.send"), 3);
        // The window gets the recipient and the WHOLE text, as it will go out.
        let ask = env.owner.last();
        assert_eq!(ask.capability, "apps");
        assert_eq!(ask.scope, "messages.send:+79161234567");
        assert!(ask.details.contains(&("to".to_string(), "+79161234567".to_string())));
        assert!(ask.details.contains(&("text".to_string(), "message 2".to_string())));
        assert_eq!(ask.reason, "the owner asked me to");
    }
}

#[tokio::test]
async fn the_whole_text_of_a_long_message_is_shown() {
    let env = mac(AppsMode::Auto, ONCE);
    let long = "word ".repeat(390); // 1950 characters
    env.call("apps.messages.send", send("anna@example.com", &long)).await.unwrap();
    let shown = env.owner.last().details.iter().find(|(k, _)| k == "text").unwrap().1.clone();
    assert_eq!(shown, long.trim());
    assert_eq!(env.stub.done()[0].detail, format!("anna@example.com|{}", long.trim()));
}

#[tokio::test]
async fn always_for_one_recipient_covers_only_that_recipient() {
    for mode in [AppsMode::AskNew, AppsMode::Auto] {
        let env = mac(mode, ONCE);
        env.owner.next(ALWAYS);
        env.call("apps.messages.send", send("+79161234567", "one")).await.unwrap();
        assert_eq!(env.owner.last().remember, Remember::AlwaysOnly);
        // The same person, spelled another way: no new ask.
        env.call("apps.messages.send", send("+7 (916) 123 45 67", "two")).await.unwrap();
        assert_eq!(env.owner.asks(), 1, "mode {mode:?}");
        // Anyone else: asked.
        env.call("apps.messages.send", send("+79160000000", "three")).await.unwrap();
        env.call("apps.messages.send", send("anna@example.com", "four")).await.unwrap();
        assert_eq!(env.owner.asks(), 3, "mode {mode:?}");
        assert_eq!(env.did("messages.send"), 4);
    }
}

#[tokio::test]
async fn a_message_grant_cannot_be_for_a_session_or_a_while() {
    let env = mac(AppsMode::AskNew, ONCE);
    // The owner's window offers only "this time" and "always for this
    // person"; an answer of another kind is treated as "this time".
    env.owner.next(SESSION);
    env.owner.next(Approval { allow: true, duration: Duration_::Until(i64::MAX / 2) });
    env.call("apps.messages.send", send("+79161234567", "one")).await.unwrap();
    env.call("apps.messages.send", send("+79161234567", "two")).await.unwrap();
    env.call("apps.messages.send", send("+79161234567", "three")).await.unwrap();
    assert_eq!(env.owner.asks(), 3);
    assert!(env.core.grant_list().is_empty());
}

#[tokio::test]
async fn in_ask_mode_not_even_always_is_kept_for_a_message() {
    let env = mac(AppsMode::Ask, ALWAYS);
    env.call("apps.messages.send", send("+79161234567", "one")).await.unwrap();
    env.call("apps.messages.send", send("+79161234567", "two")).await.unwrap();
    assert_eq!(env.owner.asks(), 2);
    assert_eq!(env.owner.last().remember, Remember::Never);
    assert!(env.core.grant_list().is_empty());
    // And an "always" given earlier, in another mode, is not used now.
    env.core.shared().grants().record(CAP, "messages.send:+79161234567", ALWAYS);
    env.call("apps.messages.send", send("+79161234567", "three")).await.unwrap();
    assert_eq!(env.owner.asks(), 3);
}

#[tokio::test]
async fn no_blanket_grant_covers_a_message() {
    let env = mac(AppsMode::Auto, NO);
    let grants = env.core.shared();
    // Everything a wide grant could be called.
    for scope in [
        "", "*", "apps", "messages", "messages.send", "messages.send:", "messages.send:*", "messages.*",
        "task", "all", "messages.send:+7916",
    ] {
        grants.grants().record(CAP, scope, ALWAYS);
    }
    for cap in ["*", "messages", "computer", "files"] {
        grants.grants().record(cap, "messages.send:+79161234567", ALWAYS);
    }
    let e = env.call("apps.messages.send", send("+79161234567", "hi")).await.unwrap_err();
    assert_eq!(e, "denied: the owner did not allow it");
    assert_eq!(env.owner.asks(), 1, "the owner must have been asked");
    assert_eq!(env.did("messages.send"), 0);
    // The synth cannot make such a grant itself either.
    let e = crate::rpc::call_for_test(
        &env.core.shared(),
        "permission.request",
        json!({"capability": "apps", "scope": "messages.send:+79161234567", "reason": "trust me"}),
    )
    .await
    .unwrap_err();
    assert!(e.starts_with("denied: "), "{e}");
    assert_eq!(env.owner.asks(), 1, "permission.request for apps must not reach the owner");
    assert!(!grants.grants().allows(CAP, "messages.send:+79161234567"));
}

#[tokio::test]
async fn a_refusal_stops_the_action_and_is_journaled_without_content() {
    let env = mac(AppsMode::AskNew, NO);
    let e = env
        .call("apps.calendar.create", json!({"title": "Secret meeting", "start": "2026-10-13T15:00", "end": "2026-10-13T16:00"}))
        .await
        .unwrap_err();
    assert_eq!(e, "denied: the owner did not allow it");
    assert_eq!(env.did("calendar.create"), 0);
    // The window had the title and the time.
    let ask = env.owner.last();
    assert!(ask.details.contains(&("title".to_string(), "Secret meeting".to_string())));
    assert!(ask.details.iter().any(|(k, v)| k == "when" && v == "2026-10-13 15:00 .. 2026-10-13 16:00"));
    let j = env.journal();
    assert!(j.contains("calendar.create") && j.contains("denied: owner"), "{j}");
    assert!(!j.contains("Secret meeting"), "{j}");
}

#[tokio::test]
async fn a_malformed_call_never_reaches_the_owner() {
    let env = mac(AppsMode::Ask, ALWAYS);
    for (method, params) in [
        ("apps.messages.send", json!({"to": "Anna", "text": "hi"})),
        ("apps.messages.send", json!({"to": "+79161234567"})),
        ("apps.calendar.create", json!({"title": "x", "start": "soon", "end": "later"})),
        ("apps.open.url", json!({"url": "file:///etc/passwd"})),
        ("apps.open.app", json!({"name": "../../bin/sh"})),
        ("apps.contacts.search", json!({"query": "a"})),
    ] {
        let e = env.call(method, params).await.unwrap_err();
        assert!(e.starts_with("bad request: ") || e.starts_with("denied: "), "{method}: {e}");
    }
    assert_eq!(env.owner.asks(), 0);
    assert!(env.stub.done().is_empty());
    let e = env.call("apps.calendar.erase_everything", json!({})).await.unwrap_err();
    assert!(e.starts_with("denied: unknown method"), "{e}");
    let e = env.call("apps.nonsense", json!({})).await.unwrap_err();
    assert!(e.starts_with("denied: unknown method"), "{e}");
}

#[tokio::test]
async fn a_contact_search_returns_matches_only_never_the_book() {
    // A backend that hands over the whole address book for any query.
    let mut stub = StubBackend::mac();
    stub.dump_contacts = true;
    let env = env_with(stub, ALWAYS);
    env.core.apply_preset(Preset::Full);
    let got = env.call("apps.contacts.search", json!({"query": "ann"})).await.unwrap();
    let names: Vec<&str> = got["contacts"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Anna Petrova", "Boris Annenkov"]);
    // By the digits of a number.
    let got = env.call("apps.contacts.search", json!({"query": "916 555 04"})).await.unwrap();
    assert_eq!(got["contacts"].as_array().unwrap().len(), 1);
    assert_eq!(got["contacts"][0]["name"], "Dmitri Orlov");
    // A cap on the count, flagged.
    let got = env.call("apps.contacts.search", json!({"query": "example", "limit": 2})).await.unwrap();
    assert_eq!(got["contacts"].as_array().unwrap().len(), 2);
    assert_eq!(got["truncated"], true);
    // Nothing that is not a real query gets through, whatever the backend.
    for q in ["", "*", "a", " ", "12"] {
        assert!(env.call("apps.contacts.search", json!({"query": q})).await.is_err(), "query {q:?}");
    }
}

#[test]
fn results_are_cut_to_the_limits_whatever_a_backend_returns() {
    // Messages: the count, the length of a text, and no attachment data.
    let many: Vec<Value> = (0..500)
        .map(|i| json!({"time": "t", "from": "x", "chat": "x", "text": "y".repeat(5000), "service": "SMS", "attachment_path": format!("/Users/x/secret-{i}.pdf")}))
        .collect();
    let req = Request::parse("messages", "recent", &json!({"limit": 100000})).unwrap();
    let (v, count) = bound(&req, json!({"messages": many}));
    let got = v["messages"].as_array().unwrap();
    assert_eq!(got.len(), request::MESSAGES_MAX);
    assert_eq!(count, Some(request::MESSAGES_MAX as u64));
    assert_eq!(got[0]["text"].as_str().unwrap().chars().count(), request::MESSAGE_TEXT_MAX);
    assert!(got[0].get("attachment_path").is_none(), "an attachment path leaked");
    assert_eq!(v["truncated"], true);

    let req = Request::parse("notes", "read", &json!({"id": "n"})).unwrap();
    let (v, _) = bound(&req, json!({"id": "n", "title": "t", "text": "z".repeat(50_000)}));
    assert_eq!(v["text"].as_str().unwrap().len(), request::NOTE_TEXT_MAX);
    assert_eq!(v["truncated"], true);

    let req = Request::parse("calendar", "list", &json!({"start": "2026-10-01", "end": "2026-10-02", "limit": 3})).unwrap();
    let events: Vec<Value> = (0..10).map(|i| json!({"id": i, "title": "e"})).collect();
    let (v, count) = bound(&req, json!({"events": events}));
    assert_eq!((v["events"].as_array().unwrap().len(), count), (3, Some(3)));

    let req = Request::parse("shortcuts", "run", &json!({"name": "x"})).unwrap();
    let (v, _) = bound(&req, json!({"output": "o".repeat(200_000)}));
    assert_eq!(v["output"].as_str().unwrap().len(), request::SHORTCUT_OUTPUT_MAX);
}

#[tokio::test]
async fn the_journal_has_counts_and_never_content() {
    let env = mac(AppsMode::Auto, ALWAYS);
    env.call("apps.messages.send", send("+79161234567", "MESSAGE-BODY-XYZ")).await.unwrap();
    env.call("apps.messages.recent", json!({})).await.unwrap();
    env.call("apps.contacts.search", json!({"query": "Petrova"})).await.unwrap();
    env.call("apps.calendar.create", json!({"title": "EVENT-TITLE-XYZ", "start": "2026-10-13T15:00", "end": "2026-10-13T16:00", "notes": "EVENT-NOTES-XYZ"})).await.unwrap();
    env.call("apps.calendar.list", json!({"start": "2026-10-13", "end": "2026-10-20"})).await.unwrap();
    env.call("apps.reminders.create", json!({"title": "REMINDER-XYZ"})).await.unwrap();
    env.call("apps.notes.create", json!({"title": "NOTE-TITLE-XYZ", "text": "NOTE-TEXT-XYZ"})).await.unwrap();
    env.call("apps.notes.read", json!({"title": "Trip plan"})).await.unwrap();
    env.call("apps.clipboard.write", json!({"text": "CLIPBOARD-XYZ"})).await.unwrap();
    env.call("apps.clipboard.read", json!({})).await.unwrap();
    env.call("apps.notify.show", json!({"title": "NOTIFY-XYZ", "body": "NOTIFY-BODY-XYZ"})).await.unwrap();
    env.call("apps.shortcuts.run", json!({"name": "Morning Routine", "input": "SHORTCUT-INPUT-XYZ"})).await.unwrap();

    let entries = env.core.recent_journal();
    assert_eq!(entries.len(), 12, "one line per call");
    assert!(entries.iter().all(|e| e.capability == "apps" && e.outcome == "ok"));
    let j = env.journal();
    for secret in [
        "XYZ", "+79161234567", "9161234567", "Petrova", "Anna", "anna@example.com", "Trip plan", "Lisbon",
        "Are we still on", "Dentist", "oat milk",
    ] {
        assert!(!j.contains(secret), "the journal holds {secret:?}: {j}");
    }
    // What it does hold: the app and action, the outcome, a count.
    let line = |action: &str| entries.iter().find(|e| e.action == action).unwrap_or_else(|| panic!("no line for {action}"));
    assert_eq!(line("messages.recent").count, Some(4));
    assert_eq!(line("contacts.search").count, Some(1));
    assert_eq!(line("calendar.list").count, Some(2));
    assert_eq!(line("messages.send").count, None);
    assert_eq!(line("messages.send").target, "");
    // A shortcut is named (the owner made it); its input is not there.
    assert_eq!(line("shortcuts.run").target, "Morning Routine");
}

#[tokio::test]
async fn notifications_never_ask_carry_the_synths_name_and_are_rate_limited() {
    let env = mac(AppsMode::Ask, NO);
    for i in 0..NOTIFY_PER_MINUTE {
        env.call("apps.notify.show", json!({"title": format!("n{i}"), "body": "b"})).await.unwrap();
    }
    assert_eq!(env.owner.asks(), 0);
    assert!(env.stub.done()[0].detail.starts_with("SNTH: n0|b"), "{:?}", env.stub.done()[0]);
    let e = env.call("apps.notify.show", json!({"title": "one too many"})).await.unwrap_err();
    assert!(e.starts_with("denied: too many notifications"), "{e}");
    assert_eq!(env.did("notify.show"), NOTIFY_PER_MINUTE);
}

#[tokio::test]
async fn a_shortcut_with_input_is_asked_about_even_when_the_shortcut_is_remembered() {
    let env = mac(AppsMode::AskNew, ALWAYS);
    env.call("apps.shortcuts.run", json!({"name": "Morning Routine"})).await.unwrap();
    env.call("apps.shortcuts.run", json!({"name": "Morning Routine"})).await.unwrap();
    assert_eq!(env.owner.asks(), 1);
    // The same shortcut, now with input: a remembered yes does not cover
    // it, and this yes is not remembered either.
    env.call("apps.shortcuts.run", json!({"name": "Morning Routine", "input": "rm -rf ~"})).await.unwrap();
    env.call("apps.shortcuts.run", json!({"name": "Morning Routine", "input": "rm -rf ~"})).await.unwrap();
    assert_eq!(env.owner.asks(), 3);
    let ask = env.owner.last();
    assert_eq!(ask.remember, Remember::Never);
    assert!(ask.details.contains(&("input".to_string(), "rm -rf ~".to_string())), "the window must show the input");
    assert!(ask.details.contains(&("name".to_string(), "Morning Routine".to_string())));
}

#[tokio::test]
async fn only_a_document_inside_a_shared_folder_opens() {
    let dir = std::env::temp_dir().join(format!("connect-apps-open-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let shared = dir.join("shared");
    let outside = dir.join("outside");
    std::fs::create_dir_all(shared.join("Trip")).unwrap();
    std::fs::create_dir_all(shared.join("Evil.app")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(shared.join("report.pdf"), b"%PDF").unwrap();
    std::fs::write(shared.join("run.command"), b"#!/bin/sh\n").unwrap();
    std::fs::write(shared.join("tool.txt"), b"x").unwrap();
    std::fs::write(outside.join("secret.pdf"), b"%PDF").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(shared.join("tool.txt"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.pdf"), shared.join("link.pdf")).unwrap();
    }

    let env = mac(AppsMode::Auto, ALWAYS);
    let path = |p: PathBuf| json!({"path": p.to_string_lossy()});

    // No shared folder yet: nothing opens.
    let e = env.call("apps.open.file", path(shared.join("report.pdf"))).await.unwrap_err();
    assert!(e.starts_with("denied: no folder is shared"), "{e}");

    env.core.add_root(shared.clone(), false);
    let ok = env.call("apps.open.file", path(shared.join("report.pdf"))).await.unwrap();
    assert_eq!(ok["opened"], true);
    env.call("apps.open.file", path(shared.join("Trip"))).await.unwrap();
    assert_eq!(env.did("open.file"), 2);

    for (bad, why) in [
        (outside.join("secret.pdf"), "denied: "),
        (shared.join("run.command"), "denied: files of type"),
        (shared.join("Evil.app"), "denied: a folder whose name"),
        (shared.join("missing.pdf"), "not found: "),
        (PathBuf::from("relative.pdf"), "denied: "),
    ] {
        let e = env.call("apps.open.file", path(bad.clone())).await.unwrap_err();
        assert!(e.starts_with(why), "{}: {e}", bad.display());
    }
    #[cfg(unix)]
    {
        let e = env.call("apps.open.file", path(shared.join("tool.txt"))).await.unwrap_err();
        assert!(e.contains("marked as a program"), "{e}");
        // A link that points out of the shared folder is followed first.
        let e = env.call("apps.open.file", path(shared.join("link.pdf"))).await.unwrap_err();
        assert!(e.starts_with("denied: ") && e.contains("outside the shared folders"), "{e}");
    }
    assert_eq!(env.did("open.file"), 2, "nothing else was opened");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_decision_table() {
    use Decision::*;
    let read = Request::parse("calendar", "list", &json!({"start": "2026-10-01", "end": "2026-10-02"})).unwrap();
    let act = Request::parse("notes", "create", &json!({"title": "t"})).unwrap();
    let msg = Request::parse("messages", "send", &json!({"to": "+79161234567", "text": "t"})).unwrap();
    let note = Request::parse("notify", "show", &json!({"title": "t"})).unwrap();
    for mode in [AppsMode::Ask, AppsMode::AskNew, AppsMode::Auto] {
        assert_eq!(decide(mode, Risk::Read, &read), Pass);
        assert_eq!(decide(mode, Risk::ActLight, &note), Pass);
        // A message is asked about whatever the mode and whatever risk a
        // caller passes in.
        for risk in [Risk::Read, Risk::Act, Risk::ActLight] {
            assert!(matches!(decide(mode, risk, &msg), Ask { ref scope, .. } if scope == "messages.send:+79161234567"));
        }
    }
    assert_eq!(decide(AppsMode::Auto, Risk::Act, &act), Pass);
    assert_eq!(decide(AppsMode::Ask, Risk::Act, &act), Ask { scope: "notes.create".into(), remember: Remember::Never });
    assert_eq!(decide(AppsMode::AskNew, Risk::Act, &act), Ask { scope: "notes.create".into(), remember: Remember::Any });
    assert_eq!(
        decide(AppsMode::Auto, Risk::Act, &msg),
        Ask { scope: "messages.send:+79161234567".into(), remember: Remember::AlwaysOnly }
    );
}

#[test]
fn the_hello_shape_of_apps() {
    let stub = StubBackend::mac();
    let mut s = AppsSettings { enabled: true, mode: AppsMode::AskNew, ..AppsSettings::default() };
    s.apps.insert("reminders".into(), false);
    let v = wire_value(&stub, &s);
    assert_eq!(v["mode"], "ask_new");
    let apps = v["apps"].as_array().unwrap();
    assert_eq!(apps.len(), 9);
    assert_eq!(apps[0], json!({"id": "messages", "on": false, "actions": ["recent", "send"]}));
    assert_eq!(apps[2], json!({"id": "calendar", "on": true, "actions": ["list", "create"]}));
    assert_eq!(apps[3]["on"], false);
    assert_eq!(apps[8], json!({"id": "open", "on": true, "actions": ["url", "file", "app"]}));
}

#[tokio::test]
async fn open_app_takes_a_bundle_id() {
    // The synth brings an app's window to the owner's desktop with it
    // (Finder lives outside /Applications); the name is still what the
    // owner sees and what the policy keys on.
    let env = mac(AppsMode::Auto, ALWAYS);
    let out = env.call("apps.open.app", json!({"name": "Finder", "bundle_id": "com.apple.finder"})).await.unwrap();
    assert_eq!(out["bundle_id"], "com.apple.finder");
    let done = env.stub.done();
    assert!(done.iter().any(|d| format!("{d:?}").contains("Finder (com.apple.finder)")), "{done:?}");
    let e = env.call("apps.open.app", json!({"name": "Finder", "bundle_id": "-a"})).await.unwrap_err();
    assert!(e.contains("bundle_id must be"), "{e}");
}
