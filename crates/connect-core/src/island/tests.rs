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
    }
}

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

    let mut text = OwnerMessage { client_id: "c-2".into(), mode: Mode::Text, text: "  find me a flight  ".into(), audio: None, images: vec![] };
    assert_eq!(text.params().unwrap()["text"], "find me a flight");
    text.text = "   ".into();
    assert_eq!(text.params().unwrap_err().code, "empty");

    // Images only with a screenshot.
    let mut wrong = voice(1000, 10);
    wrong.images = vec![Image { mime: "image/png".into(), data: vec![1], long_edge: 10 }];
    assert!(wrong.params().is_err());
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
    for (error, code) in [
        ("too_large: 9000000 bytes", "too_large"),
        ("no_owner_session", "no_owner_session"),
        ("busy: the last message is still running", "busy"),
        ("denied: unknown method \"owner.message\"", "outdated"),
        ("something else", "failed"),
    ] {
        let (core, mut q) = linked(&[("mia", BOTH)]);
        let shared = core.shared();
        let call = tokio::spawn(async move { send(&shared, "mia", &voice(4000, 64)).await });
        let sent = q[0].recv().await.unwrap();
        core.shared().slot("mia").unwrap().requests.resolve(&json!({"type": "response", "id": sent["id"], "error": error}));
        assert_eq!(call.await.unwrap().unwrap_err().code, code, "{error}");
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
    slot.requests.resolve(&json!({"type": "response", "id": asked["id"], "result": {"error": "invalid_face", "reason": "eyes.shape: one of round, oval"}}));
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
