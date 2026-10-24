//! Observe in the core: clean strings, the categories and the
//! exclusions, the call decision (the app's own microphone never counts),
//! the place by network, the messages and what a link sends when.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::sources::*;
use super::*;
use crate::core::LinkState;
use crate::grant::Approval;
use crate::pairing::Pending;
use crate::settings::LinkSource;
use crate::testutil;
use crate::wire::HelloAck;

// --- clean strings ---

#[test]
fn strings_are_cleaned_as_the_relay_wants_them() {
    assert_eq!(clean("  registry.go \t-\n hub  ", 200), "registry.go - hub");
    assert_eq!(clean("<b>bold</b>", 200), "bbold/b");
    assert_eq!(clean("a\u{0}b\u{7}c\u{1b}[0m", 200), "abc[0m");
    // Bidi overrides and isolates, zero-width characters, the BOM.
    assert_eq!(clean("\u{202E}gpj.exe\u{202C} x\u{200B}y\u{2066}z\u{2069}\u{FEFF}", 200), "gpj.exe xyz");
    assert_eq!(clean("   \n\t ", 200), "");
    assert_eq!(clean("", 10), "");
}

#[test]
fn strings_are_cut_in_utf8_bytes_on_a_character_boundary() {
    // "дом" is 6 bytes: 5 leaves two letters, never half of one.
    assert_eq!(clean("дом", 5), "до");
    assert_eq!(clean("дом", 6), "дом");
    // A 4-byte emoji does not fit in 3.
    assert_eq!(clean("ab😀", 5), "ab");
    // A space is never left at the end of a cut.
    assert_eq!(clean("abc def", 4), "abc");
    let long = "я".repeat(200);
    let out = clean(&long, MAX_TITLE_BYTES);
    assert_eq!(out.len(), MAX_TITLE_BYTES);
    assert!(out.chars().all(|c| c == 'я'));
}

// --- categories ---

#[test]
fn apps_get_their_category() {
    let cases = [
        ("com.microsoft.VSCode", Category::Code),
        ("com.todesktop.230313mzl4w4u92", Category::Code),
        ("com.apple.dt.Xcode", Category::Code),
        ("com.jetbrains.intellij", Category::Code),
        ("com.jetbrains.goland", Category::Code),
        ("com.apple.Terminal", Category::Terminal),
        ("com.googlecode.iterm2", Category::Terminal),
        ("dev.warp.Warp-Stable", Category::Terminal),
        ("com.mitchellh.ghostty", Category::Terminal),
        ("com.apple.Safari", Category::Browser),
        ("com.google.Chrome", Category::Browser),
        ("com.google.Chrome.canary", Category::Browser),
        ("company.thebrowser.Browser", Category::Browser),
        ("org.mozilla.firefox", Category::Browser),
        ("ru.keepcoder.Telegram", Category::Chat),
        ("com.tinyspeck.slackmacgap", Category::Chat),
        ("com.apple.MobileSMS", Category::Chat),
        ("com.apple.mail", Category::Mail),
        ("com.microsoft.Outlook", Category::Mail),
        ("us.zoom.xos", Category::Meeting),
        ("com.apple.FaceTime", Category::Meeting),
        ("com.microsoft.teams2", Category::Meeting),
        ("com.spotify.client", Category::Music),
        ("org.videolan.vlc", Category::Video),
        ("notion.id", Category::Docs),
        ("com.figma.Desktop", Category::Design),
        ("com.valvesoftware.steam", Category::Game),
        // Windows file names.
        ("Code.exe", Category::Code),
        ("Cursor.exe", Category::Code),
        ("WindowsTerminal.exe", Category::Terminal),
        ("pwsh.exe", Category::Terminal),
        ("msedge.exe", Category::Browser),
        ("chrome.exe", Category::Browser),
        ("Telegram.exe", Category::Chat),
        ("OUTLOOK.EXE", Category::Mail),
        ("Zoom.exe", Category::Meeting),
        ("ms-teams.exe", Category::Meeting),
        ("WINWORD.EXE", Category::Docs),
        ("Spotify.exe", Category::Music),
    ];
    for (id, want) in cases {
        assert_eq!(category_of(id), want, "{id}");
    }
}

#[test]
fn an_unknown_app_is_other_and_a_family_never_covers_a_longer_word() {
    assert_eq!(category_of("com.example.thing"), Category::Other);
    assert_eq!(category_of(""), Category::Other);
    assert_eq!(category_of("something.exe"), Category::Other);
    // `com.apple.mail` covers its helpers, not another app that starts
    // with the same letters.
    assert_eq!(category_of("com.apple.mail.helper"), Category::Mail);
    assert_eq!(category_of("com.apple.mailx"), Category::Other);
    assert_eq!(category_of("com.jetbrainsx.thing"), Category::Other);
}

#[test]
fn windows_programs_have_a_name_to_show() {
    assert_eq!(windows_app_name("Code.exe"), "Visual Studio Code");
    assert_eq!(windows_app_name("WINWORD.EXE"), "Word");
    assert_eq!(windows_app_name("Telegram.exe"), "Telegram");
    assert_eq!(windows_app_name("noext"), "noext");
}

// --- exclusions ---

fn front(id: &str, name: &str, title: Option<&str>) -> Front {
    Front { id: id.into(), name: name.into(), title: title.map(String::from) }
}

#[test]
fn password_managers_are_always_excluded() {
    for f in [
        front("com.1password.1password", "1Password", Some("Vault")),
        front("com.agilebits.onepassword7", "1Password 7", None),
        front("com.bitwarden.desktop", "Bitwarden", None),
        front("com.apple.keychainaccess", "Keychain Access", None),
        front("com.apple.Passwords", "Passwords", None),
        front("org.keepassxc.keepassxc", "KeePassXC", None),
        front("Bitwarden.exe", "Bitwarden", None),
        front("KeePass.exe", "KeePass", None),
        // An unknown build, by its name.
        front("com.example.lastpass-beta", "LastPass Beta", None),
        front("", "Связка ключей", None),
    ] {
        assert!(excluded(&f, &[]), "{f:?}");
    }
}

#[test]
fn the_passwords_page_of_system_settings_is_excluded_and_its_other_pages_are_not() {
    assert!(excluded(&front("com.apple.systempreferences", "System Settings", Some("Passwords")), &[]));
    assert!(excluded(&front("com.apple.systempreferences", "Системные настройки", Some("Пароли")), &[]));
    assert!(!excluded(&front("com.apple.systempreferences", "System Settings", Some("Wi-Fi")), &[]));
}

#[test]
fn private_browser_windows_are_excluded_and_a_document_called_incognito_is_not() {
    assert!(excluded(&front("org.mozilla.firefox", "Firefox", Some("Bank - Mozilla Firefox Private Browsing")), &[]));
    assert!(excluded(&front("com.google.Chrome", "Google Chrome", Some("New Tab - Google Chrome (Incognito)")), &[]));
    assert!(excluded(&front("msedge.exe", "Microsoft Edge", Some("New InPrivate tab - Microsoft Edge")), &[]));
    assert!(excluded(&front("com.google.Chrome", "Google Chrome", Some("Новая вкладка - Режим инкогнито")), &[]));
    assert!(!excluded(&front("com.google.Chrome", "Google Chrome", Some("GitHub")), &[]));
    assert!(!excluded(&front("com.apple.Notes", "Notes", Some("Incognito mode notes")), &[]));
}

#[test]
fn the_owners_own_exclusions_match_by_id_or_by_name() {
    let own = vec![
        AppMark { id: "com.apple.MobileSMS".into(), name: "Messages".into() },
        AppMark { id: String::new(), name: "Banking".into() },
    ];
    assert!(excluded(&front("com.apple.mobilesms", "Messages", Some("Mom")), &own));
    assert!(excluded(&front("com.bank.app", "banking", None), &own));
    assert!(!excluded(&front("ru.keepcoder.Telegram", "Telegram", None), &own));
    // A mark with an id does not match another app by name.
    assert!(!excluded(&front("com.other.messages", "Messages", None), &own));
}

#[test]
fn an_excluded_app_goes_as_private_without_a_title() {
    let mut t = ActivityTracker::default();
    let v = t.update(Some(&front("com.1password.1password", "1Password", Some("Bank login"))), &[], 100).unwrap();
    assert_eq!(v, json!({"app": "private", "category": "private", "since": rfc3339(100)}));
    // The owner's own exclusion too.
    let own = vec![AppMark { id: "com.apple.mail".into(), name: "Mail".into() }];
    let v = t.update(Some(&front("com.apple.mail", "Mail", Some("Re: salary"))), &own, 110).unwrap();
    assert_eq!(v["app"], "private");
    assert!(v.get("title").is_none());
}

#[test]
fn the_activity_is_the_app_its_title_and_since_when() {
    let mut t = ActivityTracker::default();
    let code = front("com.microsoft.VSCode", "Visual Studio Code", Some("registry.go - hub"));
    let v = t.update(Some(&code), &[], 100).unwrap();
    assert_eq!(v, json!({"app": "Visual Studio Code", "title": "registry.go - hub", "category": "code", "since": rfc3339(100)}));
    // Same app and title later: the same since.
    assert_eq!(t.update(Some(&code), &[], 130).unwrap()["since"], rfc3339(100));
    // Another title: since moves.
    let other = front("com.microsoft.VSCode", "Visual Studio Code", Some("main.go - hub"));
    assert_eq!(t.update(Some(&other), &[], 140).unwrap()["since"], rfc3339(140));
    // No title (no Accessibility): no title key.
    let bare = front("com.apple.Terminal", "Terminal", None);
    let v = t.update(Some(&bare), &[], 150).unwrap();
    assert!(v.get("title").is_none());
    assert_eq!(v["category"], "terminal");
    // Nothing in front, or the lock screen: nothing to report.
    assert_eq!(t.update(None, &[], 160), None);
    assert_eq!(t.update(Some(&front("com.apple.loginwindow", "loginwindow", None)), &[], 170), None);
    // A long, dirty title is cleaned and cut.
    let dirty = front("com.google.Chrome", "Google <Chrome>", Some(&format!("\u{202E}{}", "x".repeat(400))));
    let v = t.update(Some(&dirty), &[], 180).unwrap();
    assert_eq!(v["app"], "Google Chrome");
    assert_eq!(v["title"].as_str().unwrap().len(), MAX_TITLE_BYTES);
}

// --- calls ---

fn mic(recorders: Option<&[&str]>, device_running: bool, own_capture: bool, running: &[&str]) -> MicSample {
    MicSample {
        recorders: recorders.map(|r| r.iter().map(|s| s.to_string()).collect()),
        device_running,
        own_capture,
        running: running.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn the_apps_own_microphone_is_never_a_call() {
    // Without the list of recorders (macOS before 14): the device runs,
    // but it is this app's own wake listener. Zoom running changes
    // nothing.
    assert_eq!(decide_call(&mic(None, true, true, &["us.zoom.xos"]), false), None);
    assert_eq!(decide_call(&mic(None, true, true, &[]), true), None);
    // The same device running with this app's microphone closed: Zoom.
    assert_eq!(decide_call(&mic(None, true, false, &["us.zoom.xos"]), false), Some(CallApp::Zoom));
    // With the list (macOS 14, Windows) this app is never in it: wake by
    // name listening while nobody else records is no call.
    assert_eq!(decide_call(&mic(Some(&[]), true, true, &["us.zoom.xos"]), false), None);
    // And another app's call is seen while this app listens too.
    assert_eq!(decide_call(&mic(Some(&["us.zoom.xos"]), true, true, &["us.zoom.xos"]), false), Some(CallApp::Zoom));
}

#[test]
fn a_call_is_the_recording_call_app() {
    let cases: [(&[&str], CallApp); 9] = [
        (&["us.zoom.xos"], CallApp::Zoom),
        (&["com.apple.FaceTime"], CallApp::Facetime),
        (&["ru.keepcoder.Telegram"], CallApp::Telegram),
        (&["net.whatsapp.WhatsApp"], CallApp::Whatsapp),
        (&["com.tinyspeck.slackmacgap.helper"], CallApp::Slack),
        (&["com.microsoft.teams2"], CallApp::Teams),
        (&["com.hnc.Discord.helper.Renderer"], CallApp::Discord),
        (&["Zoom.exe"], CallApp::Zoom),
        (&["MSTeams"], CallApp::Teams),
    ];
    for (rec, want) in cases {
        assert_eq!(decide_call(&mic(Some(rec), true, false, &[]), false), Some(want), "{rec:?}");
    }
}

#[test]
fn meet_is_a_browser_recording_with_a_meet_window_seen() {
    let chrome = mic(Some(&["com.google.Chrome.helper"]), true, false, &[]);
    assert_eq!(decide_call(&chrome, true), Some(CallApp::Meet));
    // A browser recording without Meet and no call app: not a call.
    assert_eq!(decide_call(&chrome, false), None);
    // ...with a call app running: a call of an app we cannot name.
    let with_slack = mic(Some(&["com.google.Chrome.helper"]), true, false, &["com.tinyspeck.slackmacgap"]);
    assert_eq!(decide_call(&with_slack, false), Some(CallApp::Other));
    // A dictation while nothing else runs: not a call.
    assert_eq!(decide_call(&mic(Some(&["com.apple.SpeechRecognitionCore"]), true, false, &[]), false), None);
    // Meet seen but the recorder is not a browser: the call app decides.
    assert_eq!(decide_call(&mic(Some(&["com.apple.VoiceMemos"]), true, false, &[]), true), None);
    // Without the list, Meet seen and the device running counts.
    assert_eq!(decide_call(&mic(None, true, false, &[]), true), Some(CallApp::Meet));
}

#[test]
fn meet_titles_are_told_from_other_words() {
    for t in ["Meet - abc-defg-hij", "Meet: abc-defg-hij", "Meet", "abc-defg-hij - Google Meet", "meet.google.com/abc"] {
        assert!(meet_title(t), "{t}");
    }
    for t in ["Meetup - events", "Team meeting notes", "Meeting"] {
        assert!(!meet_title(t), "{t}");
    }
}

#[test]
fn since_is_when_the_call_began() {
    let mut t = CallTracker::default();
    let idle = mic(Some(&[]), false, false, &["us.zoom.xos"]);
    let zoom = mic(Some(&["us.zoom.xos"]), true, false, &["us.zoom.xos"]);
    assert_eq!(t.update(&idle, None, 100), json!({"in_call": false}));
    assert_eq!(t.update(&zoom, None, 110), json!({"in_call": true, "since": rfc3339(110), "app": "zoom"}));
    assert_eq!(t.update(&zoom, None, 170)["since"], rfc3339(110));
    assert_eq!(t.update(&idle, None, 180), json!({"in_call": false}));
    assert_eq!(t.update(&zoom, None, 200)["since"], rfc3339(200));
}

#[test]
fn a_meet_call_stays_meet_after_the_window_went_to_the_back_and_is_forgotten_later() {
    let mut t = CallTracker::default();
    let chrome = mic(Some(&["com.google.Chrome.helper"]), true, false, &[]);
    let meet = front("com.google.Chrome", "Google Chrome", Some("Meet - abc-defg-hij"));
    let code = front("com.microsoft.VSCode", "Visual Studio Code", Some("x"));
    assert_eq!(t.update(&chrome, Some(&meet), 100)["app"], "meet");
    // Two hours of a call with the editor in front: still Meet.
    assert_eq!(t.update(&chrome, Some(&code), 7300)["app"], "meet");
    // After the call, a browser's microphone much later is not Meet.
    assert_eq!(t.update(&mic(Some(&[]), false, false, &[]), Some(&code), 7400)["in_call"], false);
    assert_eq!(t.update(&chrome, Some(&code), 7400 + MEET_MEMORY_SECS + 1)["in_call"], false);
}

// --- places ---

#[test]
fn a_named_network_is_a_place_and_its_name_never_goes() {
    let mut s = ObserveSettings::default();
    s.name_network("Keenetic-5G", "  дом ").unwrap();
    s.name_network("GymFree", "зал").unwrap();
    let home = place_of(&Wifi::Network("Keenetic-5G".into()), &s.places).unwrap();
    assert_eq!(home, json!({"name": "дом", "state": "present"}));
    assert!(!home.to_string().contains("Keenetic"));
    // An unnamed network, no network, no permission: nothing.
    assert_eq!(place_of(&Wifi::Network("Cafe".into()), &s.places), None);
    assert_eq!(place_of(&Wifi::NotConnected, &s.places), None);
    assert_eq!(place_of(&Wifi::Denied, &s.places), None);
    // SSIDs are matched exactly.
    assert_eq!(place_of(&Wifi::Network("keenetic-5g".into()), &s.places), None);
}

#[test]
fn networks_are_renamed_and_forgotten() {
    let mut s = ObserveSettings::default();
    s.name_network("Home", "дом").unwrap();
    s.name_network("Home", "квартира").unwrap();
    assert_eq!(s.places, vec![NamedNetwork { ssid: "Home".into(), name: "квартира".into() }]);
    // An empty name forgets the network.
    s.name_network("Home", "   ").unwrap();
    assert!(s.places.is_empty());
    s.name_network("Home", "дом").unwrap();
    assert!(s.forget_network("Home"));
    assert!(!s.forget_network("Home"));
    assert_eq!(s.name_network("", "x").unwrap_err(), "bad_network");
    // A long name is cut to what the relay keeps.
    s.name_network("Home", &"д".repeat(100)).unwrap();
    assert_eq!(s.places[0].name.len(), MAX_PLACE_BYTES);
}

#[test]
fn switches_and_exclusions_are_tidy() {
    let mut s = ObserveSettings::default();
    s.set_on("mia", CALL, true).unwrap();
    s.set_on("mia", ACTIVITY, true).unwrap();
    s.set_on("maya", PLACE, true).unwrap();
    assert_eq!(s.set_on("mia", "health.today", true).unwrap_err(), "unknown_resource");
    assert!(s.is_on("mia", CALL) && !s.is_on("maya", CALL));
    assert_eq!(s.wanted(&["mia".into()]), BTreeSet::from([CALL.to_string(), ACTIVITY.to_string()]));
    assert_eq!(s.wanted(&["mia".into(), "maya".into()]).len(), 3);
    s.set_on("maya", PLACE, false).unwrap();
    assert!(!s.on.contains_key("maya"));

    assert!(s.exclude("com.apple.MobileSMS", "Messages"));
    assert!(!s.exclude("com.apple.mobilesms", "Messages"));
    assert!(!s.exclude("", "  "));
    assert!(s.unexclude("com.apple.mobilesms", ""));
    assert!(s.excluded.is_empty());

    // A file edited by hand.
    let mut hand: ObserveSettings = serde_json::from_value(json!({
        "on": {"mia": ["device.call", "nonsense"], "": ["device.call"], "lev": []},
        "excluded": [{"id": "a"}, {"id": "A", "name": "dup"}, {"name": ""}],
        "places": [{"ssid": "x", "name": " дом "}, {"ssid": "x", "name": "dup"}, {"ssid": "", "name": "y"}]
    }))
    .unwrap();
    hand.normalize();
    assert_eq!(hand.on, BTreeMap::from([("mia".to_string(), BTreeSet::from([CALL.to_string()]))]));
    assert_eq!(hand.excluded.len(), 1);
    assert_eq!(hand.places, vec![NamedNetwork { ssid: "x".into(), name: "дом".into() }]);
}

// --- messages and what goes when ---

#[test]
fn observe_messages_are_encoded_as_connect_md_says() {
    let v = json!({"in_call": true, "since": "2026-10-24T10:00:00Z", "app": "zoom"});
    let at = 1_792_836_002; // 2026-10-24T10:00:02Z
    assert_eq!(rfc3339(at), "2026-10-24T10:00:02Z");
    assert_eq!(
        message(CALL, Some(&v), at),
        json!({"type": "observe", "resource": "device.call", "value": v, "observed_at": "2026-10-24T10:00:02Z"})
    );
    assert_eq!(message(CALL, None, at), json!({"type": "observe", "resource": "device.call", "value": null}));
    assert_eq!(serde_json::to_string(&message(CALL, None, at)).unwrap(), r#"{"resource":"device.call","type":"observe","value":null}"#);
    // The items carry no id, name or at.
    let a = activity_value("Visual Studio Code", Some("registry.go - hub"), Category::Code, Some(at));
    for k in ["id", "name", "at"] {
        assert!(a.get(k).is_none());
        assert!(call_value(Some((CallApp::Zoom, at))).get(k).is_none());
    }
}

fn values(list: &[(&str, Value)]) -> BTreeMap<String, Report> {
    list.iter().map(|(r, v)| (r.to_string(), Report { value: v.clone(), observed_at: 100 })).collect()
}

fn on(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn offered(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_link_sends_on_change_again_on_the_minute_and_null_once_on_switch_off() {
    let all = offered(&[PLACE, CALL, ACTIVITY]);
    let call = json!({"in_call": false});
    let mut sent = Sent::new();
    // Right after hello_ack: the value.
    let m = plan(&all, &on(&[CALL]), &values(&[(CALL, call.clone())]), &[], &mut sent, true, false);
    assert_eq!(m, vec![message(CALL, Some(&call), 100)]);
    // Nothing changed: nothing.
    assert!(plan(&all, &on(&[CALL]), &values(&[(CALL, call.clone())]), &[], &mut sent, false, false).is_empty());
    // The minute: the same value again.
    assert_eq!(plan(&all, &on(&[CALL]), &values(&[(CALL, call.clone())]), &[], &mut sent, true, false).len(), 1);
    // A change.
    let busy = json!({"in_call": true, "since": rfc3339(90), "app": "zoom"});
    assert_eq!(plan(&all, &on(&[CALL]), &values(&[(CALL, busy.clone())]), &[], &mut sent, false, false), vec![message(CALL, Some(&busy), 100)]);
    // Switched off: null once, then nothing, not even on the minute.
    let off = plan(&all, &on(&[]), &values(&[(CALL, busy.clone())]), &[], &mut sent, false, false);
    assert_eq!(off, vec![message(CALL, None, 0)]);
    assert!(plan(&all, &on(&[]), &values(&[(CALL, busy)]), &[], &mut sent, true, false).is_empty());
}

#[test]
fn a_source_with_nothing_to_say_clears_its_report_once() {
    let all = offered(&[PLACE]);
    let home = json!({"name": "дом", "state": "present"});
    let mut sent = Sent::new();
    // An unknown network from the start: nothing (the synth has nothing).
    assert!(plan(&all, &on(&[PLACE]), &values(&[]), &[], &mut sent, true, false).is_empty());
    assert_eq!(plan(&all, &on(&[PLACE]), &values(&[(PLACE, home.clone())]), &[], &mut sent, false, false).len(), 1);
    // Left home for a network without a name: null once.
    assert_eq!(plan(&all, &on(&[PLACE]), &values(&[]), &[], &mut sent, false, false), vec![message(PLACE, None, 0)]);
    assert!(plan(&all, &on(&[PLACE]), &values(&[]), &[], &mut sent, true, false).is_empty());
}

#[test]
fn only_what_the_synth_offers_goes_and_nothing_while_paused() {
    let call = json!({"in_call": false});
    let mut sent = Sent::new();
    // Not offered (no grant): nothing, even switched on.
    assert!(plan(&offered(&[PLACE]), &on(&[CALL]), &values(&[(CALL, call.clone())]), &[], &mut sent, true, false).is_empty());
    // Offered: sent.
    assert_eq!(plan(&offered(&[CALL]), &on(&[CALL]), &values(&[(CALL, call.clone())]), &[], &mut sent, true, false).len(), 1);
    // Paused: nothing, not even null for a switch-off, and nothing is
    // forgotten.
    assert!(plan(&offered(&[CALL]), &on(&[]), &values(&[(CALL, call.clone())]), &[], &mut sent, true, true).is_empty());
    assert_eq!(sent.get(CALL), Some(&Some(call.clone())));
    // Resumed with the source off by now: the null goes.
    assert_eq!(plan(&offered(&[CALL]), &on(&[]), &values(&[(CALL, call)]), &[], &mut sent, true, false), vec![message(CALL, None, 0)]);
}

#[test]
fn a_value_is_fitted_to_the_synths_descriptor() {
    let d = Descriptor::parse(&json!({
        "resource": "device.activity",
        "max_bytes": 120,
        "value": {"properties": {"app": {"type": "string", "maxLength": 64}, "title": {"type": "string", "maxLength": 10}}}
    }))
    .unwrap();
    let v = json!({"app": "Code", "title": "registry.go - hub", "category": "code"});
    assert_eq!(fit(ACTIVITY, &v, Some(&d)).unwrap()["title"], "registry.g");
    let big = json!({"app": "Code", "category": "code", "since": "x".repeat(200)});
    assert_eq!(fit(ACTIVITY, &big, Some(&d)), None);
    // Without a descriptor: the default limit.
    assert!(fit(ACTIVITY, &big, None).is_some());
    assert_eq!(fit(ACTIVITY, &json!("not an object"), None), None);
}

/// The hub's real descriptor of device.call, with keys the app does not
/// know.
fn call_descriptor() -> Value {
    json!({
        "descriptor_version": 1,
        "resource": "device.call",
        "revision": 2,
        "order": 14,
        "title": {"ru": "звонок", "en": "call"},
        "description": {"ru": "Идёт ли звонок на устройстве и в каком приложении.", "en": "Whether a call runs on the device and in which app."},
        "producers": ["device"],
        "scope": "device",
        "value": {"type": "object", "required": ["in_call"], "properties": {"in_call": {"type": "boolean"}, "since": {"type": "string", "format": "date-time"}, "app": {"type": "string", "enum": ["zoom", "meet", "other"]}}},
        "max_bytes": 8192,
        "ttl": {"max_seconds": 300},
        "rate": {"min_interval_seconds": 15},
        "privacy": {"class": "flag_only", "consent": {"ru": "Синт узнаёт только, идёт ли звонок.", "en": "The synth learns only whether a call runs."}},
        "state_keys": ["devices[].id"],
        "something_new": {"x": 1}
    })
}

#[test]
fn descriptors_are_read_and_unknown_keys_ignored() {
    let list = parse_resources(&json!({"resources": [call_descriptor(), {"no": "resource"}, {"resource": "place.current", "title": {"en": "place"}}, call_descriptor()]}));
    assert_eq!(list.len(), 2);
    let d = &list[0];
    assert_eq!(d.resource, CALL);
    assert_eq!(d.revision, 2);
    assert_eq!(d.title, Words { en: "call".into(), ru: "звонок".into() });
    assert_eq!(d.consent.en, "The synth learns only whether a call runs.");
    assert_eq!(d.privacy_class, "flag_only");
    assert_eq!(d.max_bytes, 8192);
    // place.current has no consent: empty words, the app's own copy.
    assert_eq!(list[1].consent, Words::default());
    assert!(parse_resources(&json!({})).is_empty());
}

#[test]
fn hello_ack_carries_the_feature_and_the_resources() {
    let ack: HelloAck = serde_json::from_value(json!({
        "type": "hello_ack", "synth_id": "mia", "synth_name": "Mia",
        "features": ["requests", "observe"],
        "observe_resources": ["place.current", "device.call", "device.activity", "device.call"]
    }))
    .unwrap();
    assert!(ack.has(FEATURE));
    let core = testutil::core(testutil::Owner::new(Approval::deny()));
    let shared = core.shared();
    shared.add_link(&pending("mia")).unwrap();
    let slot = shared.slot("mia").unwrap();
    note_ack(&slot, &ack);
    assert_eq!(slot.observe.lock().unwrap().offered, offered(&[PLACE, CALL, ACTIVITY]));
    // An older synth: no feature, nothing offered (an old app's ack
    // without the key reads the same).
    let old: HelloAck = serde_json::from_value(json!({"type": "hello_ack", "features": ["requests"], "observe_resources": ["device.call"]})).unwrap();
    note_ack(&slot, &old);
    let o = slot.observe.lock().unwrap().clone();
    assert!(!o.feature && o.offered.is_empty());
}

// --- the settings on disk ---

#[test]
fn observe_is_off_in_an_old_file_and_survives_a_restart() {
    let old: crate::settings::Persisted = serde_json::from_value(serde_json::to_value(crate::settings::Persisted::fresh("dev-1".into())).unwrap()).unwrap();
    assert!(old.observe.is_empty());
    // Nothing is written while there is nothing to keep.
    assert!(serde_json::to_value(&old).unwrap().get("observe").is_none());

    let dir = std::env::temp_dir().join(format!("observe-test-{}", std::process::id()));
    let path = dir.join("settings.json");
    let mut p = crate::settings::Persisted::fresh("dev-1".into());
    p.observe.set_on("mia", CALL, true).unwrap();
    p.observe.name_network("Home", "дом").unwrap();
    crate::settings::save(&path, &p).unwrap();
    let back = crate::settings::load(&path, || "new".into()).unwrap();
    assert_eq!(back.observe, p.observe);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn unlinking_a_synth_forgets_its_switches() {
    let core = testutil::core(testutil::Owner::new(Approval::deny()));
    let shared = core.shared();
    shared.add_link(&pending("mia")).unwrap();
    shared.add_link(&pending("maya")).unwrap();
    core.set_observe(|o| {
        o.set_on("mia", CALL, true).unwrap();
        o.set_on("maya", PLACE, true).unwrap();
    });
    assert_eq!(core.observe_wanted().len(), 2);
    core.unpair_synth("mia").await;
    assert_eq!(core.observe_wanted(), on(&[PLACE]));
    assert!(!core.observe_settings().on.contains_key("mia"));
}

// --- a link ---

fn pending(id: &str) -> Pending {
    Pending {
        synth_id: id.into(),
        synth_url: format!("https://{id}.snth.ai"),
        token: format!("tok-{id}"),
        synth_name: id.into(),
        source: LinkSource::Account,
        hub_id: String::new(),
        linked_to_account: None,
        reason: None,
    }
}

/// A core with one link up to a synth that takes observe; its writer
/// queue returned so the test reads what the app sends.
fn observed_link(resources: &[&str]) -> (crate::Core, Arc<LinkSlot>, tokio::sync::mpsc::UnboundedReceiver<Value>) {
    let core = testutil::core(testutil::Owner::new(Approval::deny()));
    let shared = core.shared();
    shared.add_link(&pending("mia")).unwrap();
    let slot = shared.slot("mia").unwrap();
    let (rx, generation) = slot.requests.attach();
    slot.requests.set_supported(true);
    let ack = HelloAck {
        synth_id: "mia".into(),
        synth_name: "Mia".into(),
        synth_version: "t".into(),
        features: vec!["requests".into(), FEATURE.into()],
        observe_resources: resources.iter().map(|s| s.to_string()).collect(),
    };
    note_ack(&slot, &ack);
    shared.set_synth(&slot, ack);
    shared.set_slot_state(&slot, LinkState::Linked, "linked");
    let out = slot.requests.sender(generation).unwrap();
    tokio::spawn(run_link(shared.clone(), slot.clone(), out));
    (core, slot, rx)
}

/// The next observe message (the requests in between are answered or
/// skipped by the caller's `answer`).
async fn next_observe(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Value>, slot: &LinkSlot, answer: Option<&Value>) -> Value {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("nothing sent").expect("queue closed");
        if msg["type"] == "request" {
            assert_eq!(msg["method"], "observe.resources");
            if let Some(a) = answer {
                slot.requests.resolve(&json!({"type": "response", "id": msg["id"], "result": a}));
            }
            continue;
        }
        return msg;
    }
}

#[tokio::test]
async fn a_link_asks_for_the_descriptors_sends_at_once_and_clears_on_switch_off() {
    let (core, slot, mut rx) = {
        let (core, slot, rx) = observed_link(&[CALL, PLACE]);
        core.set_observe(|o| o.set_on("mia", CALL, true).unwrap());
        (core, slot, rx)
    };
    core.observe_report(CALL, Some(json!({"in_call": false})));
    let answer = json!({"resources": [call_descriptor()]});
    let first = next_observe(&mut rx, &slot, Some(&answer)).await;
    assert_eq!(first["resource"], CALL);
    assert_eq!(first["value"], json!({"in_call": false}));
    assert!(first["observed_at"].as_str().unwrap().ends_with('Z'));
    // The request for the descriptors (it may have gone before the
    // value, then `next_observe` answered it already).
    if slot.observe.lock().unwrap().descriptors.is_empty() {
        if let Ok(Some(req)) = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            assert_eq!(req["method"], "observe.resources");
            slot.requests.resolve(&json!({"type": "response", "id": req["id"], "result": answer}));
        }
    }
    // The descriptors arrived: kept with the link, the resources follow.
    for _ in 0..100 {
        if !slot.observe.lock().unwrap().descriptors.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(slot.observe.lock().unwrap().offered, offered(&[CALL]));
    // A change goes at once.
    core.observe_report(CALL, Some(json!({"in_call": true, "app": "zoom", "since": rfc3339(1)})));
    assert_eq!(next_observe(&mut rx, &slot, None).await["value"]["app"], "zoom");
    // Switched off: null.
    core.set_observe(|o| o.set_on("mia", CALL, false).unwrap());
    let off = next_observe(&mut rx, &slot, None).await;
    assert_eq!(off, json!({"type": "observe", "resource": CALL, "value": null}));
}

#[tokio::test(start_paused = true)]
async fn a_link_repeats_every_minute_and_says_nothing_while_paused() {
    let (core, slot, mut rx) = observed_link(&[CALL]);
    core.set_observe(|o| o.set_on("mia", CALL, true).unwrap());
    core.observe_report(CALL, Some(json!({"in_call": false})));
    let first = next_observe(&mut rx, &slot, None).await;
    assert_eq!(first["value"], json!({"in_call": false}));
    // A minute later: the same value again.
    tokio::time::advance(REPEAT_EVERY + Duration::from_millis(10)).await;
    let again = next_observe(&mut rx, &slot, None).await;
    assert_eq!(again["value"], json!({"in_call": false}));
    // Paused: a change and the minute go by without a word.
    core.pause().await;
    core.observe_report(CALL, Some(json!({"in_call": true, "app": "zoom"})));
    tokio::time::advance(REPEAT_EVERY + Duration::from_millis(10)).await;
    tokio::task::yield_now().await;
    while let Ok(m) = rx.try_recv() {
        assert_ne!(m["type"], "observe", "sent while paused: {m}");
    }
    // Resumed: the current value at once.
    core.resume();
    assert_eq!(next_observe(&mut rx, &slot, None).await["value"]["app"], "zoom");
}

// --- a round of the sources ---

struct FakeMic(Arc<Mutex<MicSample>>);
impl MicProbe for FakeMic {
    fn sample(&mut self) -> MicSample {
        self.0.lock().unwrap().clone()
    }
}
struct FakeFront(Arc<Mutex<Option<Front>>>);
impl FrontProbe for FakeFront {
    fn front(&mut self) -> Option<Front> {
        self.0.lock().unwrap().clone()
    }
}
struct FakeWifi(Arc<Mutex<Wifi>>, Arc<Mutex<usize>>);
impl WifiProbe for FakeWifi {
    fn read(&mut self) -> Wifi {
        *self.1.lock().unwrap() += 1;
        self.0.lock().unwrap().clone()
    }
}

#[test]
fn a_round_reads_only_the_sources_that_are_on_and_clears_one_switched_off() {
    let m = Arc::new(Mutex::new(mic(Some(&["us.zoom.xos"]), true, false, &["us.zoom.xos"])));
    let f = Arc::new(Mutex::new(Some(front("com.microsoft.VSCode", "Visual Studio Code", Some("registry.go - hub")))));
    let w = Arc::new(Mutex::new(Wifi::Network("Home".into())));
    let reads = Arc::new(Mutex::new(0));
    let mut s = Sampler::new(Box::new(FakeMic(m.clone())), Box::new(FakeFront(f.clone())), Box::new(FakeWifi(w.clone(), reads.clone())));
    let mut settings = ObserveSettings::default();
    settings.name_network("Home", "дом").unwrap();
    let t0 = Instant::now();

    let out = s.tick(t0, 100, &on(&[CALL, ACTIVITY, PLACE]), &settings);
    let got: BTreeMap<&str, Option<Value>> = out.into_iter().collect();
    assert_eq!(got[CALL].as_ref().unwrap()["app"], "zoom");
    assert_eq!(got[ACTIVITY].as_ref().unwrap()["title"], "registry.go - hub");
    assert_eq!(got[PLACE], Some(json!({"name": "дом", "state": "present"})));
    assert_eq!(s.recent()[0].id, "com.microsoft.VSCode");

    // The network is read every WIFI_EVERY, the place worked out every
    // round: a network named in between counts at once.
    *w.lock().unwrap() = Wifi::Network("Gym".into());
    let out = s.tick(t0 + TICK, 102, &on(&[PLACE]), &settings);
    assert_eq!(out, vec![(CALL, None), (ACTIVITY, None), (PLACE, Some(json!({"name": "дом", "state": "present"})))]);
    assert_eq!(*reads.lock().unwrap(), 1);
    let out = s.tick(t0 + WIFI_EVERY, 110, &on(&[PLACE]), &settings);
    assert_eq!(out, vec![(PLACE, None)]);
    settings.name_network("Gym", "зал").unwrap();
    let out = s.tick(t0 + WIFI_EVERY + TICK, 112, &on(&[PLACE]), &settings);
    assert_eq!(out, vec![(PLACE, Some(json!({"name": "зал", "state": "present"})))]);
    // All off: one None, then nothing at all.
    assert_eq!(s.tick(t0 + WIFI_EVERY * 2, 120, &on(&[]), &settings), vec![(PLACE, None)]);
    assert!(s.tick(t0 + WIFI_EVERY * 3, 130, &on(&[]), &settings).is_empty());
    assert_eq!(s.wifi(), &Wifi::Unknown);
}
