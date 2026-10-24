//! The island in the core: settings and their migration, the keys,
//! owner.message (limits, the answer, the retry), faces, island.event.

use std::sync::Arc;

use serde_json::{json, Value};

use super::*;
use crate::core::{Core, CoreEvent, LinkState};
use crate::grant::Approval;
use crate::pairing::Pending;
use crate::settings::{self, LinkSource};
use crate::testutil;
use crate::wire::HelloAck;

fn pending(id: &str, name: &str) -> Pending {
    Pending {
        synth_id: id.into(),
        synth_url: format!("https://{id}.snth.ai"),
        token: format!("tok-{id}"),
        synth_name: name.into(),
        source: LinkSource::Account,
        hub_id: String::new(),
        linked_to_account: None,
        reason: None,
    }
}

/// A core linked to these synths; each link "up" with the given features,
/// its request queue returned so a test can play the synth.
fn linked(ids: &[(&str, &[&str])]) -> (Core, Vec<tokio::sync::mpsc::UnboundedReceiver<Value>>) {
    let core = testutil::core(testutil::Owner::new(Approval::deny()));
    let shared = core.shared();
    let mut queues = Vec::new();
    for (id, features) in ids {
        shared.add_link(&pending(id, id)).unwrap();
        let slot = shared.slot(id).unwrap();
        let (rx, _) = slot.requests.attach();
        slot.requests.set_supported(features.contains(&crate::requests::FEATURE));
        shared.set_synth(
            &slot,
            HelloAck {
                synth_id: id.to_string(),
                synth_name: id.to_string(),
                synth_version: "t".into(),
                features: features.iter().map(|f| f.to_string()).collect(),
            },
        );
        shared.set_slot_state(&slot, LinkState::Linked, "linked");
        queues.push(rx);
    }
    (core, queues)
}

const BOTH: &[&str] = &["requests", "island"];

fn voice(ms: u64, bytes: usize) -> OwnerMessage {
    OwnerMessage {
        client_id: "c-1".into(),
        mode: Mode::Voice,
        text: String::new(),
        audio: Some(Audio { mime: "audio/wav".into(), data: vec![1; bytes], ms }),
        images: vec![],
        voice_transcript: None,
        voice_tone: None,
        voice_reply: None,
    }
}

/// A voice message as text recognized on this computer.
fn spoken(text: &str) -> OwnerMessage {
    OwnerMessage { audio: None, voice_transcript: Some(text.into()), ..voice(0, 0) }
}

const WITH_STT: &[&str] = &["requests", "island", "island_device_stt"];

// --- settings ---

#[test]
fn the_island_is_off_with_this_systems_keys_on_a_fresh_install_and_in_an_old_file() {
    let fresh = settings::Persisted::fresh("d".into());
    assert!(!fresh.island.enabled);
    assert_eq!(fresh.island.hotkeys, default_hotkeys(std::env::consts::OS));

    // A file of 0.1.13: no island, no faces.
    let old = json!({"rev": 7, "settings": crate::settings::Settings::default(), "device_id": "d"});
    let dir = std::env::temp_dir().join(format!("connect-island-old-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
    let p = settings::load(&path, || "x".into()).unwrap();
    assert_eq!(p.rev, 7);
    assert!(!p.island.enabled);
    assert_eq!(p.island.main_synth, "");
    assert_eq!(p.island.hotkeys, Hotkeys::default());
    assert!(p.faces.is_empty());

    // Half a hotkeys object: the missing keys get their defaults, a key
    // switched off stays off, a key given twice keeps the first.
    let half = json!({"rev": 1, "settings": crate::settings::Settings::default(), "device_id": "d",
        "island": {"enabled": true, "hotkeys": {"voice": " Alt+KeyV ", "text": "alt+keyv"}}});
    std::fs::write(&path, serde_json::to_vec(&half).unwrap()).unwrap();
    let p = settings::load(&path, || "x".into()).unwrap();
    assert!(p.island.enabled);
    assert_eq!(p.island.hotkeys.voice, "Alt+KeyV");
    assert_eq!(p.island.hotkeys.screen_voice, Hotkeys::default().screen_voice);
    assert_eq!(p.island.hotkeys.text, "", "the same combination twice: the second is switched off");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn speech_goes_to_the_synth_and_the_offer_waits_on_a_fresh_install_and_in_an_old_file() {
    let fresh = settings::Persisted::fresh("d".into());
    assert_eq!(fresh.island.stt, SttMode::Synth, "device speech-to-text is off by default");
    assert_eq!(fresh.island.stt_offer, SttOffer::Pending);

    // A file of 0.1.18: the island on, nothing about speech-to-text. The
    // offer is pending, so it shows once after the update.
    let old = json!({"rev": 3, "settings": crate::settings::Settings::default(), "device_id": "d",
        "island": {"enabled": true, "main_synth": "mia"}});
    let dir = std::env::temp_dir().join(format!("connect-island-stt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
    let p = settings::load(&path, || "x".into()).unwrap();
    assert!(p.island.enabled);
    assert_eq!(p.island.stt, SttMode::Synth);
    assert_eq!(p.island.stt_offer, SttOffer::Pending);

    // Written and read back.
    let mut p = p;
    p.island.stt = SttMode::Device;
    p.island.stt_offer = SttOffer::Accepted;
    settings::save(&path, &p).unwrap();
    let raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(raw["island"]["stt"], "device");
    assert_eq!(raw["island"]["stt_offer"], "accepted");
    let back = settings::load(&path, || "x".into()).unwrap();
    assert_eq!((back.island.stt, back.island.stt_offer), (SttMode::Device, SttOffer::Accepted));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wake_by_name_is_off_by_default_and_the_phrase_is_tidied() {
    let fresh = settings::Persisted::fresh("d".into());
    assert!(!fresh.island.wake, "wake by name is off by default");
    assert_eq!(fresh.island.wake_phrase, "");
    // A file of 0.1.20 knows nothing of it.
    let old: crate::island::IslandSettings = serde_json::from_value(json!({"enabled": true, "stt": "device"})).unwrap();
    assert!(!old.wake);
    let mut s = crate::island::IslandSettings { wake: true, wake_phrase: "  эй   подруга \n".into(), ..old };
    s.normalize();
    assert_eq!(s.wake_phrase, "эй подруга");
    s.wake_phrase = "я".repeat(100);
    s.normalize();
    assert_eq!(s.wake_phrase.chars().count(), crate::island::WAKE_PHRASE_MAX);
    let raw = serde_json::to_value(&s).unwrap();
    assert_eq!(raw["wake"], true);
}

#[test]
fn the_offer_is_answered_once() {
    use SttOffer::*;
    assert!(Pending.open() && Shown.open() && !Accepted.open() && !Declined.open());
    assert_eq!(Pending.shown(), Shown);
    assert_eq!(Shown.shown(), Shown);
    assert_eq!(Declined.shown(), Declined, "showing never reopens an answered offer");
    assert_eq!(Shown.answered(true), Accepted);
    assert_eq!(Shown.answered(false), Declined);
    assert_eq!(Pending.answered(false), Declined);
    // "Not now" never asks again: a late click changes nothing.
    assert_eq!(Declined.answered(true), Declined);
    assert_eq!(Accepted.answered(false), Accepted);
    // A choice in Settings answers an open offer.
    assert_eq!(Shown.chosen_in_settings(SttMode::Device), Accepted);
    assert_eq!(Pending.chosen_in_settings(SttMode::Synth), Declined);
    assert_eq!(Declined.chosen_in_settings(SttMode::Device), Declined);
}

#[test]
fn the_defaults_avoid_the_keys_windows_keeps_for_itself() {
    let mac = default_hotkeys("macos");
    assert_eq!((mac.voice.as_str(), mac.screen_voice.as_str(), mac.text.as_str()), ("Alt+Space", "Alt+Shift+Space", "Control+Alt+Space"));
    let win = default_hotkeys("windows");
    for key in [&win.voice, &win.screen_voice, &win.text] {
        let c = canonical(key);
        assert_ne!(c, "alt+space", "Alt+Space is the window menu");
        assert_ne!(c, "control+space", "Ctrl+Space switches the input method");
        assert!(!c.contains("super"), "Win combinations belong to the shell: {key}");
    }
    let mut all = vec![canonical(&win.voice), canonical(&win.screen_voice), canonical(&win.text)];
    all.dedup();
    assert_eq!(all.len(), 3);
}

#[test]
fn combinations_compare_whatever_the_spelling() {
    assert_eq!(canonical("Shift+Alt+Space"), canonical("alt+shift+space"));
    assert_eq!(canonical("Option+Shift+Space"), "alt+shift+space");
    assert_eq!(canonical("Ctrl+Alt+KeyV"), "alt+control+v");
    assert_eq!(canonical(""), "");
    let keys = default_hotkeys("macos");
    assert_eq!(keys.taken_by(HotkeyKind::Text, "shift+alt+space"), Some(HotkeyKind::ScreenVoice));
    assert_eq!(keys.taken_by(HotkeyKind::Voice, "Alt+Space"), None, "a key does not conflict with itself");
    assert_eq!(keys.taken_by(HotkeyKind::Voice, ""), None);
}

#[tokio::test]
async fn switching_the_island_on_tells_the_synth_and_the_keys_do_not() {
    let (core, _q) = linked(&[("mia", BOTH)]);
    let rev = core.shared().settings_rev();
    let told = core.shared().settings_tx.subscribe();
    core.set_island(|i| i.hotkeys.voice = "Alt+KeyV".into());
    assert_eq!(core.shared().settings_rev(), rev, "the keys are the owner's own business");
    assert!(!told.has_changed().unwrap());

    core.set_island(|i| i.enabled = true);
    assert_eq!(core.shared().settings_rev(), rev + 1);
    assert!(told.has_changed().unwrap(), "every link sends its settings again");

    let caps = with_capability(json!({"net": {}}), core.island().enabled);
    assert_eq!(caps["island"], json!({"enabled": true, "version": 1}));
    assert!(with_capability(json!({"net": {}}), false).get("island").is_none(), "no key at all while off");
}

#[tokio::test]
async fn the_main_synth_is_the_chosen_one_while_it_is_linked() {
    let (core, _q) = linked(&[("mia", BOTH), ("maya", BOTH)]);
    assert_eq!(core.island_main().unwrap().synth_id, "mia", "the first link by default");
    core.set_island(|i| i.main_synth = "maya".into());
    assert_eq!(core.island_main().unwrap().synth_id, "maya");
    core.set_island(|i| i.main_synth = "gone".into());
    assert_eq!(core.island_main().unwrap().synth_id, "mia");
}

// --- owner.message ---

#[test]
fn a_message_is_built_within_every_limit() {
    let p = voice(5_000, 1000).params().unwrap();
    assert_eq!(p["mode"], "voice");
    assert_eq!(p["audio"]["mime"], "audio/wav");
    assert_eq!(p["audio"]["ms"], 5000);
    assert_eq!(p["client_id"], "c-1");
    assert!(p.get("images").is_none());

    assert_eq!(voice(MAX_VOICE_MS + 1, 10).params().unwrap_err().code, "too_long");
    assert_eq!(voice(1000, 0).params().unwrap_err().code, "empty");
    // 6.5 MB of voice is 8.7 MB of base64.
    assert_eq!(voice(100_000, 6_500_000).params().unwrap_err().code, "too_large");

    let mut shot = voice(3000, 100);
    shot.mode = Mode::ScreenVoice;
    shot.images = vec![Image { mime: "image/jpeg".into(), data: vec![9; 50], long_edge: 2560 }];
    let p = shot.params().unwrap();
    assert_eq!(p["images"][0]["mime"], "image/jpeg");
    assert_eq!(p["images"].as_array().unwrap().len(), 1);
    shot.images[0].long_edge = 2561;
    assert_eq!(shot.params().unwrap_err().code, "too_large");
    shot.images[0].long_edge = 2000;
    shot.images = vec![shot.images[0].clone(), shot.images[0].clone(), shot.images[0].clone()];
    assert_eq!(shot.params().unwrap_err().code, "too_large");

    let mut text = OwnerMessage { client_id: "c-2".into(), mode: Mode::Text, text: "  find me a flight  ".into(), audio: None, images: vec![], voice_transcript: None, voice_tone: None, voice_reply: None };
    assert_eq!(text.params().unwrap()["text"], "find me a flight");
    text.text = "   ".into();
    assert_eq!(text.params().unwrap_err().code, "empty");

    // Images only with a screenshot.
    let mut wrong = voice(1000, 10);
    wrong.images = vec![Image { mime: "image/png".into(), data: vec![1], long_edge: 10 }];
    assert!(wrong.params().is_err());
}

#[test]
fn a_voice_message_carries_the_audio_or_the_text_recognized_here() {
    let p = spoken("  Найди мне рейс в пятницу  ").params().unwrap();
    assert_eq!(p["mode"], "voice");
    assert_eq!(p["voice_transcript"], "Найди мне рейс в пятницу");
    assert_eq!(p["stt"], "device");
    assert!(p.get("audio").is_none(), "no audio with a transcript");

    // The audio path is as before: no transcript, no stt.
    let p = voice(2000, 10).params().unwrap();
    assert!(p.get("voice_transcript").is_none() && p.get("stt").is_none());

    // A screenshot rides with the transcript as with the audio.
    let mut shot = spoken("что это за ошибка");
    shot.mode = Mode::ScreenVoice;
    shot.images = vec![Image { mime: "image/jpeg".into(), data: vec![9; 50], long_edge: 1200 }];
    let p = shot.params().unwrap();
    assert_eq!(p["mode"], "screen_voice");
    assert_eq!(p["images"].as_array().unwrap().len(), 1);
    assert_eq!(p["stt"], "device");

    assert_eq!(spoken("   ").params().unwrap_err().code, "empty");
    assert_eq!(spoken(&"а".repeat(MAX_TRANSCRIPT_CHARS)).params().unwrap()["voice_transcript"].as_str().unwrap().chars().count(), MAX_TRANSCRIPT_CHARS);
    assert_eq!(spoken(&"а".repeat(MAX_TRANSCRIPT_CHARS + 1)).params().unwrap_err().code, "too_large");
    let both = OwnerMessage { voice_transcript: Some("hi".into()), ..voice(1000, 10) };
    assert_eq!(both.params().unwrap_err().code, "failed", "never both");
    let text = OwnerMessage { mode: Mode::Text, text: "hi".into(), ..spoken("hi") };
    assert_eq!(text.params().unwrap_err().code, "failed", "a text message has no transcript");
}

#[test]
fn a_silent_screenshot_hold_sends_the_screenshot_alone() {
    let shot = Image { mime: "image/png".into(), data: vec![9; 50], long_edge: 1200 };
    let alone = OwnerMessage { mode: Mode::ScreenVoice, audio: None, voice_transcript: None, images: vec![shot.clone()], ..voice(0, 0) };
    let p = alone.params().unwrap();
    assert_eq!(p["mode"], "screen_voice");
    assert_eq!(p["images"].as_array().unwrap().len(), 1);
    assert!(p.get("audio").is_none() && p.get("voice_transcript").is_none() && p.get("stt").is_none());
    assert_eq!(p["text"], "");
    // Without the screenshot there is nothing to send.
    let nothing = OwnerMessage { images: vec![], ..alone.clone() };
    assert_eq!(nothing.params().unwrap_err().code, "empty");
    // A voice hold still needs its voice.
    let voice_only = OwnerMessage { mode: Mode::Voice, audio: None, voice_transcript: None, images: vec![], ..voice(0, 0) };
    assert_eq!(voice_only.params().unwrap_err().code, "empty");
}

#[tokio::test]
async fn a_transcript_goes_only_to_a_synth_that_takes_it() {
    let (core, mut q) = linked(&[("new", WITH_STT), ("old", BOTH)]);
    let shared = core.shared();
    assert!(supports_device_stt(&shared.slot("new").unwrap()));
    assert!(!supports_device_stt(&shared.slot("old").unwrap()));

    // An older synth: nothing goes, and the code tells the app to send
    // the audio instead.
    let e = send(&shared, "old", &spoken("привет")).await.unwrap_err();
    assert_eq!(e.code, NO_DEVICE_STT);
    assert!(q[1].try_recv().is_err());

    let call = tokio::spawn({
        let shared = shared.clone();
        async move { send(&shared, "new", &spoken("привет")).await }
    });
    let sent = q[0].recv().await.unwrap();
    assert_eq!(sent["params"]["voice_transcript"], "привет");
    assert_eq!(sent["params"]["stt"], "device");
    shared.slot("new").unwrap().requests.resolve(&json!({"type": "response", "id": sent["id"], "result": {"accepted": true, "message_id": "m-9"}}));
    assert_eq!(call.await.unwrap().unwrap(), "m-9");

    let view = core.island_view();
    assert_eq!(view.device_stt.get("new"), Some(&true));
    assert_eq!(view.device_stt.get("old"), Some(&false));
}

#[tokio::test]
async fn a_message_waits_only_for_accepted() {
    let (core, mut q) = linked(&[("mia", BOTH)]);
    let shared = core.shared();
    let call = tokio::spawn(async move { send(&shared, "mia", &voice(4000, 64)).await });
    let sent = q[0].recv().await.unwrap();
    assert_eq!(sent["method"], "owner.message");
    assert_eq!(sent["params"]["client_id"], "c-1");
    assert_eq!(sent["params"]["audio"]["b64"].as_str().unwrap().len(), 88);
    core.shared()
        .slot("mia")
        .unwrap()
        .requests
        .resolve(&json!({"type": "response", "id": sent["id"], "result": {"accepted": true, "message_id": "m-77"}}));
    assert_eq!(call.await.unwrap().unwrap(), "m-77");
}

#[tokio::test]
async fn the_synths_refusals_reach_the_island_as_codes() {
    // The synth's shape: the code in `error`, the words in `reason`.
    for (error, reason, code) in [
        ("too_large", "9000000 bytes", "too_large"),
        ("no_owner_session", "", "no_owner_session"),
        ("busy", "the last message is still running", "busy"),
        ("bad_request", "audio.mime", "failed"),
        ("denied: unknown method \"owner.message\"", "", "outdated"),
        ("something else", "", "failed"),
        // Words that look like a code are not one.
        ("failed", "the queue is busy", "failed"),
    ] {
        let (core, mut q) = linked(&[("mia", BOTH)]);
        let shared = core.shared();
        let call = tokio::spawn(async move { send(&shared, "mia", &voice(4000, 64)).await });
        let sent = q[0].recv().await.unwrap();
        core.shared()
            .slot("mia")
            .unwrap()
            .requests
            .resolve(&json!({"type": "response", "id": sent["id"], "error": error, "reason": reason}));
        let e = call.await.unwrap().unwrap_err();
        assert_eq!(e.code, code, "{error}");
        if !reason.is_empty() {
            assert!(e.detail.contains(reason), "{e}");
        }
    }
}

#[tokio::test]
async fn an_old_synth_or_a_link_that_is_down_is_sent_nothing() {
    let (core, mut q) = linked(&[("old", &["requests"]), ("down", BOTH)]);
    let shared = core.shared();
    assert_eq!(send(&shared, "old", &voice(4000, 64)).await.unwrap_err().code, "outdated");
    assert!(q[0].try_recv().is_err());
    let down = shared.slot("down").unwrap();
    shared.set_slot_state(&down, LinkState::Reconnecting, "dropped");
    assert_eq!(send(&shared, "down", &voice(4000, 64)).await.unwrap_err().code, "offline");
    assert!(q[1].try_recv().is_err());
    assert_eq!(send(&shared, "nobody", &voice(4000, 64)).await.unwrap_err().code, "no_synth");
    core.pause().await;
    let up = shared.slot("down").unwrap();
    shared.set_slot_state(&up, LinkState::Linked, "linked");
    assert_eq!(send(&shared, "down", &voice(4000, 64)).await.unwrap_err().code, "paused");
}

#[tokio::test]
async fn a_message_cut_off_by_a_dropped_link_goes_again_with_the_same_client_id() {
    let (core, mut q) = linked(&[("mia", BOTH)]);
    let shared = core.shared();
    let slot = shared.slot("mia").unwrap();
    let call = tokio::spawn({
        let shared = shared.clone();
        async move { send(&shared, "mia", &voice(4000, 64)).await }
    });
    let first = q[0].recv().await.unwrap();
    // The link drops before the answer, then comes back.
    let (mut again, _) = slot.requests.attach();
    slot.requests.set_supported(true);
    let second = again.recv().await.unwrap();
    assert_eq!(second["params"]["client_id"], first["params"]["client_id"]);
    slot.requests.resolve(&json!({"type": "response", "id": second["id"], "result": {"accepted": true, "message_id": "m-1"}}));
    assert_eq!(call.await.unwrap().unwrap(), "m-1");
}

// --- faces ---

fn a_face() -> Value {
    json!({"v": 1, "preset": "cat", "eyes": {"shape": "cat"}})
}

#[tokio::test]
async fn faces_are_read_set_and_cached_per_synth() {
    let (core, mut q) = linked(&[("mia", BOTH), ("maya", BOTH)]);
    let mut events = core.subscribe();
    let shared = core.shared();
    let slot = shared.slot("mia").unwrap();

    let read = tokio::spawn({
        let (shared, slot) = (shared.clone(), slot.clone());
        async move { refresh_face(&shared, &slot).await }
    });
    let asked = q[0].recv().await.unwrap();
    assert_eq!(asked["method"], "face.get");
    slot.requests.resolve(&json!({"type": "response", "id": asked["id"], "result": {"face": a_face(), "presets": []}}));
    read.await.unwrap().unwrap();
    assert_eq!(core.face_of("mia"), Some(a_face()));
    assert_eq!(core.face_of("maya"), None, "per synth");
    assert!(matches!(events.try_recv(), Ok(CoreEvent::Face { synth_id }) if synth_id == "mia"));

    // The owner's editor: a patch.
    let set = tokio::spawn({
        let shared = shared.clone();
        async move { set_face(&shared, "mia", json!({"mouth": {"style": "cat"}}), true).await }
    });
    let asked = q[0].recv().await.unwrap();
    assert_eq!(asked["method"], "face.set");
    assert_eq!(asked["params"]["patch"], true);
    let merged = json!({"v": 1, "preset": "cat", "eyes": {"shape": "cat"}, "mouth": {"style": "cat"}});
    slot.requests.resolve(&json!({"type": "response", "id": asked["id"], "result": {"face": merged}}));
    assert_eq!(set.await.unwrap().unwrap(), FaceSet::Saved { face: merged.clone() });
    assert_eq!(core.face_of("mia"), Some(merged.clone()));

    // A face the synth refuses: the cache stays.
    let set = tokio::spawn({
        let shared = shared.clone();
        async move { set_face(&shared, "mia", json!({"eyes": {"shape": "square"}}), true).await }
    });
    let asked = q[0].recv().await.unwrap();
    slot.requests.resolve(&json!({"type": "response", "id": asked["id"], "error": "invalid_face", "reason": "eyes.shape: one of round, oval"}));
    assert_eq!(set.await.unwrap().unwrap(), FaceSet::Invalid { reason: "eyes.shape: one of round, oval".into() });
    assert_eq!(core.face_of("mia"), Some(merged));

    // Too big is refused before it is sent.
    let big = json!({"v": 1, "preset": "x".repeat(MAX_FACE_BYTES)});
    assert!(matches!(set_face(&shared, "mia", big, false).await.unwrap(), FaceSet::Invalid { .. }));
    assert!(q[0].try_recv().is_err());

    // Unlinking a synth forgets its face.
    shared.wipe_slot(&slot);
    assert_eq!(core.face_of("mia"), None);
}

#[tokio::test]
async fn faces_survive_a_restart() {
    let dir = std::env::temp_dir().join(format!("connect-island-faces-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    let mut p = settings::Persisted::fresh("d".into());
    p.faces.insert("mia".into(), a_face());
    p.island.enabled = true;
    settings::save(&path, &p).unwrap();
    let back = settings::load(&path, || "x".into()).unwrap();
    assert_eq!(back.faces.get("mia"), Some(&a_face()));
    assert!(back.island.enabled);
    let _ = std::fs::remove_dir_all(&dir);
}

// --- island.event ---

async fn event_from(core: &Core, synth: &str, params: Value) -> Result<Value, String> {
    let shared = core.shared();
    let slot = shared.slot(synth).unwrap();
    crate::slot::PEER.scope(slot, async move { crate::rpc::call_for_test(&shared, "island.event", params).await }).await
}

#[tokio::test]
async fn island_events_reach_the_ui_tagged_with_their_synth() {
    let (core, _q) = linked(&[("mia", BOTH), ("maya", BOTH)]);
    let mut events = core.subscribe();

    // Off: refused, nothing shown.
    let off = event_from(&core, "maya", json!({"kind": "done", "message_id": "m"})).await.unwrap_err();
    assert!(off.starts_with("denied: "), "{off}");
    core.set_island(|i| i.enabled = true);
    while events.try_recv().is_ok() {}

    let ok = event_from(
        &core,
        "maya",
        json!({"kind": "done", "message_id": "m-9", "summary": "Нашла рейс\nна 9:40", "channel": "telegram<b>"}),
    )
    .await
    .unwrap();
    assert_eq!(ok, json!({"ok": true}));
    match events.try_recv() {
        Ok(CoreEvent::Island(e)) => {
            assert_eq!(e.synth_id, "maya");
            assert_eq!(e.kind, "done");
            assert_eq!(e.message_id, "m-9");
            assert_eq!(e.summary, "Нашла рейс на 9:40");
            assert_eq!(e.channel, "telegramb");
        }
        other => panic!("{other:?}"),
    }

    let long = "a".repeat(500);
    event_from(&core, "mia", json!({"kind": "error", "message_id": "m", "summary": long})).await.unwrap();
    match events.try_recv() {
        Ok(CoreEvent::Island(e)) => assert_eq!(e.summary.chars().count(), MAX_SUMMARY_CHARS + 1),
        other => panic!("{other:?}"),
    }

    assert!(event_from(&core, "mia", json!({"kind": "explode"})).await.is_err());

    // A face the synth gave itself.
    event_from(&core, "mia", json!({"kind": "face", "face": a_face()})).await.unwrap();
    assert_eq!(core.face_of("mia"), Some(a_face()));
    assert!(matches!(events.try_recv(), Ok(CoreEvent::Face { synth_id }) if synth_id == "mia"));
    assert!(event_from(&core, "mia", json!({"kind": "face", "face": [1, 2]})).await.is_err());
    assert_eq!(core.face_of("mia"), Some(a_face()));
}

#[test]
fn client_ids_are_uuids_and_never_repeat() {
    let a = new_client_id();
    assert_eq!(a.len(), 36);
    assert_eq!(&a[14..15], "4");
    assert_ne!(a, new_client_id());
}

#[test]
fn the_view_says_who_takes_island_requests() {
    let (core, _q) = linked(&[("mia", BOTH), ("old", &["requests"])]);
    let view = core.island_view();
    assert_eq!(view.main_synth.as_deref(), Some("mia"));
    assert_eq!(view.supported.get("mia"), Some(&true));
    assert_eq!(view.supported.get("old"), Some(&false));
    let _: Arc<crate::slot::LinkSlot> = core.island_main().unwrap();
}

// --- the tone estimated here ---

const WITH_TONE: &[&str] = &["requests", "island", "island_device_stt", "island_device_tone"];

fn toned(text: &str, tone: &str) -> OwnerMessage {
    OwnerMessage { voice_tone: Some(tone.into()), ..spoken(text) }
}

#[test]
fn a_voice_tone_rides_only_with_a_transcript() {
    let p = toned("найди рейс", "  Темп выше обычного.  ").params().unwrap();
    assert_eq!(p["voice_tone"], "Темп выше обычного.");
    assert_eq!(p["stt"], "device");
    // An empty tone is left out.
    let p = toned("найди рейс", "   ").params().unwrap();
    assert!(p.get("voice_tone").is_none());
    // Never with the audio, never alone.
    let with_audio = OwnerMessage { voice_tone: Some("громко".into()), ..voice(1000, 10) };
    assert_eq!(with_audio.params().unwrap_err().code, "failed");
    // A long one is cut to the protocol's limit, the message still goes.
    let p = toned("да", &"я".repeat(MAX_TONE_CHARS + 50)).params().unwrap();
    assert_eq!(p["voice_tone"].as_str().unwrap().chars().count(), MAX_TONE_CHARS);
}

#[test]
fn the_tone_switch_is_on_by_default_and_in_an_old_file() {
    assert!(IslandSettings::default().stt_tone);
    let old: IslandSettings = serde_json::from_value(json!({"enabled": true, "stt": "device"})).unwrap();
    assert!(old.stt_tone);
    let off: IslandSettings = serde_json::from_value(json!({"stt_tone": false})).unwrap();
    assert!(!off.stt_tone);
}

#[test]
fn the_engine_is_whisper_by_default_and_in_an_old_file() {
    assert_eq!(IslandSettings::default().stt_engine, SttEngine::Whisper);
    let old: IslandSettings = serde_json::from_value(json!({"enabled": true, "stt": "device"})).unwrap();
    assert_eq!(old.stt_engine, SttEngine::Whisper);
    let p: IslandSettings = serde_json::from_value(json!({"stt_engine": "parakeet"})).unwrap();
    assert_eq!(p.stt_engine, SttEngine::Parakeet);
    assert_eq!(serde_json::to_value(&p).unwrap()["stt_engine"], "parakeet");
}

#[test]
fn the_island_position_is_the_system_default_until_the_owner_chooses() {
    // A fresh install and a file written before the choice existed.
    assert_eq!(IslandSettings::default().position, IslandPosition::default());
    let old: IslandSettings = serde_json::from_value(json!({"enabled": true, "stt": "device"})).unwrap();
    assert_eq!(old.position, IslandPosition { vertical: None, horizontal: None });
    // The default writes nothing, so an older app reads the file as it was.
    let saved = serde_json::to_value(&old).unwrap();
    assert_eq!(saved["position"], json!({}));
    // A chosen spot round-trips; "centre" is read as "center"; one axis alone is fine.
    let p: IslandSettings = serde_json::from_value(json!({"position": {"vertical": "bottom", "horizontal": "right"}})).unwrap();
    assert_eq!(p.position.vertical, Some(IslandVertical::Bottom));
    assert_eq!(p.position.horizontal, Some(IslandHorizontal::Right));
    assert_eq!(serde_json::to_value(&p).unwrap()["position"], json!({"vertical": "bottom", "horizontal": "right"}));
    let c: IslandSettings = serde_json::from_value(json!({"position": {"horizontal": "centre"}})).unwrap();
    assert_eq!(c.position, IslandPosition { vertical: None, horizontal: Some(IslandHorizontal::Center) });
    // What the command takes.
    assert_eq!(IslandPosition::parse_vertical("top"), Ok(Some(IslandVertical::Top)));
    assert_eq!(IslandPosition::parse_vertical("default"), Ok(None));
    assert_eq!(IslandPosition::parse_vertical(""), Ok(None));
    assert_eq!(IslandPosition::parse_vertical("middle"), Ok(Some(IslandVertical::Middle)));
    assert!(IslandPosition::parse_vertical("centre").is_err());
    // The middle of the screen is not a spot; the middle of a side is.
    let mid = |h| IslandPosition { vertical: Some(IslandVertical::Middle), horizontal: h };
    assert!(mid(None).is_screen_middle());
    assert!(mid(Some(IslandHorizontal::Center)).is_screen_middle());
    assert!(!mid(Some(IslandHorizontal::Left)).is_screen_middle());
    assert!(!mid(Some(IslandHorizontal::Right)).is_screen_middle());
    assert!(!IslandPosition::default().is_screen_middle());
    assert_eq!(IslandPosition::parse_horizontal("left"), Ok(Some(IslandHorizontal::Left)));
    assert_eq!(IslandPosition::parse_horizontal("centre"), Ok(Some(IslandHorizontal::Center)));
    assert!(IslandPosition::parse_horizontal("up").is_err());
}

#[tokio::test]
async fn the_tone_goes_only_to_a_synth_that_takes_it() {
    let (core, mut q) = linked(&[("new", WITH_TONE), ("stt", WITH_STT)]);
    let shared = core.shared();
    assert!(supports_device_tone(&shared.slot("new").unwrap()));
    assert!(!supports_device_tone(&shared.slot("stt").unwrap()));
    for (i, id) in ["new", "stt"].into_iter().enumerate() {
        let call = tokio::spawn({
            let shared = shared.clone();
            async move { send(&shared, id, &toned("привет", "Голос тише обычного.")).await }
        });
        let sent = q[i].recv().await.unwrap();
        assert_eq!(sent["params"]["voice_transcript"], "привет");
        if id == "new" {
            assert_eq!(sent["params"]["voice_tone"], "Голос тише обычного.");
        } else {
            assert!(sent["params"].get("voice_tone").is_none(), "a synth without the feature gets no tone");
        }
        shared.slot(id).unwrap().requests.resolve(&json!({"type": "response", "id": sent["id"], "result": {"accepted": true, "message_id": "m-1"}}));
        assert_eq!(call.await.unwrap().unwrap(), "m-1");
    }
}

// --- the synth's voice reply (tz-voice-reply.md) ---

const WITH_VOICE: &[&str] = &["requests", "island", "island_device_stt", "island_voice_reply"];

#[test]
fn the_voice_reply_is_always_by_default_and_in_an_old_file() {
    assert_eq!(IslandSettings::default().voice_reply(), VoiceReply::Always);
    let old: IslandSettings = serde_json::from_value(json!({"enabled": true, "stt": "device"})).unwrap();
    assert_eq!(old.voice_reply, None);
    assert_eq!(old.voice_reply(), VoiceReply::Always);
    // The default writes nothing, so an older app reads the file as it was.
    assert!(serde_json::to_value(&old).unwrap().get("voice_reply").is_none());
    for (s, v) in [("always", VoiceReply::Always), ("auto", VoiceReply::Auto), ("off", VoiceReply::Off)] {
        let set: IslandSettings = serde_json::from_value(json!({"voice_reply": s})).unwrap();
        assert_eq!(set.voice_reply(), v);
        assert_eq!(serde_json::to_value(&set).unwrap()["voice_reply"], s);
        assert_eq!(VoiceReply::parse(s), Some(v));
        assert_eq!(v.as_str(), s);
    }
    assert_eq!(VoiceReply::parse("loud"), None);
}

#[test]
fn the_voice_reply_rides_only_with_a_voice_message() {
    let with = |m: OwnerMessage, r: VoiceReply| OwnerMessage { voice_reply: Some(r), ..m };
    // The audio, and the text recognized here.
    assert_eq!(with(voice(1000, 10), VoiceReply::Always).params().unwrap()["voice_reply"], "always");
    assert_eq!(with(spoken("привет"), VoiceReply::Auto).params().unwrap()["voice_reply"], "auto");
    assert_eq!(with(spoken("привет"), VoiceReply::Off).params().unwrap()["voice_reply"], "off");
    let shot = Image { mime: "image/jpeg".into(), data: vec![1; 10], long_edge: 100 };
    let screen = OwnerMessage { mode: Mode::ScreenVoice, images: vec![shot], ..spoken("что тут") };
    assert_eq!(with(screen, VoiceReply::Always).params().unwrap()["voice_reply"], "always");
    // Not set: not sent (an old app, byte for byte).
    assert!(voice(1000, 10).params().unwrap().get("voice_reply").is_none());
    // A line of text is always answered in text.
    let text = OwnerMessage { mode: Mode::Text, text: "hi".into(), audio: None, voice_transcript: None, ..voice(0, 0) };
    assert!(with(text, VoiceReply::Always).params().unwrap().get("voice_reply").is_none());
}

#[tokio::test]
async fn the_voice_reply_goes_only_to_a_synth_that_announces_it() {
    let (core, mut q) = linked(&[("new", WITH_VOICE), ("old", WITH_STT)]);
    let shared = core.shared();
    assert!(supports_voice_reply(&shared.slot("new").unwrap()));
    assert!(!supports_voice_reply(&shared.slot("old").unwrap()));
    let view = core.island_view();
    assert_eq!(view.voice_reply.get("new"), Some(&true));
    assert_eq!(view.voice_reply.get("old"), Some(&false));
    for (i, id) in ["new", "old"].into_iter().enumerate() {
        let call = tokio::spawn({
            let shared = shared.clone();
            async move { send(&shared, id, &OwnerMessage { voice_reply: Some(VoiceReply::Always), ..spoken("привет") }).await }
        });
        let sent = q[i].recv().await.unwrap();
        if id == "new" {
            assert_eq!(sent["params"]["voice_reply"], "always");
        } else {
            assert!(sent["params"].get("voice_reply").is_none(), "a synth without the feature gets no voice_reply");
        }
        shared.slot(id).unwrap().requests.resolve(&json!({"type": "response", "id": sent["id"], "result": {"accepted": true, "message_id": "m-1"}}));
        assert_eq!(call.await.unwrap().unwrap(), "m-1");
    }
}

#[test]
fn speech_types_are_read_with_their_usual_aliases() {
    assert_eq!(speech_mime("audio/mpeg"), Some("audio/mpeg"));
    assert_eq!(speech_mime("Audio/MP3"), Some("audio/mpeg"));
    assert_eq!(speech_mime("audio/wav"), Some("audio/wav"));
    assert_eq!(speech_mime("audio/x-wav"), Some("audio/wav"));
    assert_eq!(speech_mime("audio/ogg; codecs=opus"), Some("audio/ogg"));
    assert_eq!(speech_mime("audio/opus"), Some("audio/ogg"));
    assert_eq!(speech_mime("audio/aac"), None);
    assert_eq!(speech_mime(""), None);
}

#[tokio::test]
async fn a_spoken_answer_reaches_the_shell_with_its_audio() {
    use base64::Engine;
    let (core, _q) = linked(&[("mia", WITH_VOICE)]);
    let mut events = core.subscribe();
    core.set_island(|i| i.enabled = true);
    while events.try_recv().is_ok() {}

    let audio = vec![0xffu8, 0xfb, 0x90, 0x44, 1, 2, 3];
    let b64 = base64::engine::general_purpose::STANDARD.encode(&audio);
    let ok = event_from(
        &core,
        "mia",
        json!({"kind": "speech", "message_id": "m-9", "audio": {"mime": "audio/mpeg", "data": b64}, "text": "Нашла рейс\nна 9:40", "duration_ms": 2100}),
    )
    .await
    .unwrap();
    assert_eq!(ok, json!({"ok": true}));
    match events.try_recv() {
        Ok(CoreEvent::IslandSpeech(s)) => {
            assert_eq!(s.synth_id, "mia");
            assert_eq!(s.message_id, "m-9");
            assert_eq!(s.mime, "audio/mpeg");
            assert_eq!(*s.audio.0, audio);
            assert_eq!(s.text, "Нашла рейс на 9:40");
            assert_eq!(s.duration_ms, Some(2100));
            // Never printed: the debug line says the size only.
            assert!(format!("{s:?}").contains("<7 bytes of audio>"));
        }
        other => panic!("{other:?}"),
    }

    // "b64" (owner.message's own key) is read too, and an alias of the type.
    event_from(&core, "mia", json!({"kind": "speech", "message_id": "m", "audio": {"mime": "audio/x-wav", "b64": "AAEC"}})).await.unwrap();
    assert!(matches!(events.try_recv(), Ok(CoreEvent::IslandSpeech(s)) if s.mime == "audio/wav" && s.text.is_empty() && s.duration_ms.is_none()));

    // Refused, with a reason for the synth's log.
    let bad_type = event_from(&core, "mia", json!({"kind": "speech", "audio": {"mime": "audio/aac", "data": "AAEC"}})).await.unwrap_err();
    assert!(bad_type.contains("not played here"), "{bad_type}");
    let no_audio = event_from(&core, "mia", json!({"kind": "speech", "message_id": "m"})).await.unwrap_err();
    assert!(no_audio.starts_with("bad request"), "{no_audio}");
    let not_b64 = event_from(&core, "mia", json!({"kind": "speech", "audio": {"mime": "audio/mpeg", "data": "%%%"}})).await.unwrap_err();
    assert!(not_b64.contains("base64"), "{not_b64}");
    let huge = "A".repeat(MAX_SPEECH_BYTES / 3 * 4 + 400);
    let too_big = event_from(&core, "mia", json!({"kind": "speech", "audio": {"mime": "audio/mpeg", "data": huge}})).await.unwrap_err();
    assert!(too_big.starts_with("too_large"), "{too_big}");
    assert!(events.try_recv().is_err(), "nothing refused reaches the shell");

    // About 90 s of mp3 (1.5 MB, 2 MB of base64) fits.
    let big = base64::engine::general_purpose::STANDARD.encode(vec![7u8; 1_500_000]);
    event_from(&core, "mia", json!({"kind": "speech", "message_id": "m", "audio": {"mime": "audio/mpeg", "data": big}})).await.unwrap();
    assert!(matches!(events.try_recv(), Ok(CoreEvent::IslandSpeech(s)) if s.audio.0.len() == 1_500_000));

    // The island switched off since the message went: the answer still
    // plays (the shell decides); every other kind is refused.
    core.set_island(|i| i.enabled = false);
    while events.try_recv().is_ok() {}
    event_from(&core, "mia", json!({"kind": "speech", "message_id": "m", "audio": {"mime": "audio/mpeg", "data": "AAEC"}})).await.unwrap();
    assert!(matches!(events.try_recv(), Ok(CoreEvent::IslandSpeech(_))));
    assert!(event_from(&core, "mia", json!({"kind": "done", "message_id": "m"})).await.unwrap_err().starts_with("denied: "));
}

/// The shell's sink answers within the synth's rpc: taken (playback goes
/// on after the answer) or refused with its reason, which the synth reads
/// as "this computer will not play it" (under auto: a voice note to the
/// chat instead).
#[tokio::test]
async fn the_shell_answers_a_spoken_answer_within_the_rpc() {
    let (core, _q) = linked(&[("mia", WITH_VOICE)]);
    core.set_island(|i| i.enabled = true);
    let mut events = core.subscribe();
    while events.try_recv().is_ok() {}
    let taken = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen = taken.clone();
    core.set_speech_sink(Arc::new(move |s: IslandSpeech| {
        if s.message_id == "om_refused" {
            return Err("unavailable: a call, the microphone is in use".into());
        }
        seen.lock().unwrap().push(s.message_id);
        Ok(())
    }));
    let ev = |id: &str| json!({"kind": "speech", "message_id": id, "audio": {"mime": "audio/ogg", "data": "AAEC"}, "text": "hi"});
    assert_eq!(event_from(&core, "mia", ev("om_1")).await.unwrap(), json!({"ok": true}));
    let refused = event_from(&core, "mia", ev("om_refused")).await.unwrap_err();
    assert!(refused.starts_with("unavailable: a call"), "{refused}");
    assert_eq!(*taken.lock().unwrap(), vec!["om_1".to_string()]);
    // With a sink nothing goes out as an event.
    assert!(events.try_recv().is_err());
    // Whatever the synth sizes to the advertised line (its room: the line
    // less its slack and the text, as base64) is within the limit here.
    let synth_room = (crate::rpc::MAX_REQUEST_LINE - 1024) / 4 * 3;
    assert!(synth_room <= MAX_SPEECH_BYTES && b64_len(MAX_SPEECH_BYTES) <= crate::rpc::MAX_REQUEST_LINE);
}
