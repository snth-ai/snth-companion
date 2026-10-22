//! The island: the synth's face at the top of the screen and the owner's
//! hold-to-talk keys (connect.md, "Island"; TZ tz-island.md). The window,
//! the keys, the microphone and the screenshot are the shell's; the core
//! holds what the link carries and what survives a restart:
//!
//!  - the owner's island settings (on/off, the main synth, three keys),
//!    kept in settings.json next to the policy but outside it: no preset
//!    touches them and the synth is told only whether the island is on
//!    (`capabilities.island` in the hello);
//!  - `owner.message`, the one request that hands the synth a voice note,
//!    a screenshot or a line of text as the owner's own message. A voice
//!    note goes as audio, or (on-device speech-to-text, tz-local-stt.md)
//!    as the text the app recognized itself (`voice_transcript`, `stt`
//!    "device"), only to a synth that announces `island_device_stt`. A
//!    screenshot + voice hold in which nothing was said sends the
//!    screenshot alone (mode `screen_voice`, no voice; every synth with
//!    the island takes pictures without a voice);
//!  - the owner's choice of speech-to-text and the synth's one-time offer
//!    of it (`IslandSettings::stt`, `stt_offer`);
//!  - wake by name (`IslandSettings::wake`, `wake_phrase`): the owner's
//!    own, never sent to the synth;
//!  - each synth's face (FaceConfig v1), cached per synth: `face.get` at
//!    link time, `face.set` from the owner's editor, `island.event
//!    kind=face` when the synth changed it itself;
//!  - `island.event`, the rpc the synth calls with the life of a message
//!    (accepted, working, done with a summary, error) and face changes.
//!
//! Protocol additions are additive (no proto bump): an old synth does not
//! announce the `island` feature and is sent nothing; an old app does not
//! announce the capability and is called with nothing.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::core::{CoreEvent, LinkState, Shared};
use crate::slot::LinkSlot;

/// The hello_ack feature of a synth that takes owner.message, face.get,
/// face.set and calls island.event.
pub const FEATURE: &str = "island";
/// The hello_ack feature of a synth that takes a voice message as the
/// text the app recognized itself (owner.message `voice_transcript`, `stt`
/// "device", no audio). Without it the app sends the voice as audio, even
/// with on-device speech-to-text switched on.
pub const FEATURE_DEVICE_STT: &str = "island_device_stt";
/// The hello_ack feature of a synth that takes, with such a transcript,
/// the tone of the voice estimated on this computer (owner.message
/// `voice_tone`). Without it the tone is left out.
pub const FEATURE_DEVICE_TONE: &str = "island_device_tone";
/// `capabilities.island.version`.
pub const VERSION: u32 = 1;

/// Limits of one owner.message (the TZ, section 2).
pub const MAX_VOICE_MS: u64 = 120_000;
pub const MAX_IMAGES: usize = 2;
pub const MAX_IMAGE_EDGE: u32 = 2560;
/// The whole request line, envelope included.
pub const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;
/// A FaceConfig is at most this long as JSON. The app checks nothing
/// deeper: the synth validates, the renderer clamps.
pub const MAX_FACE_BYTES: usize = 4096;
/// The longest line of text the owner may type.
pub const MAX_TEXT_CHARS: usize = 4000;
/// The longest transcript of a voice message recognized on this computer.
pub const MAX_TRANSCRIPT_CHARS: usize = 8000;
/// The longest voice tone (the protocol's limit; the app writes far less).
pub const MAX_TONE_CHARS: usize = 600;
/// `owner.message` `stt`: the transcript was recognized on this computer.
pub const STT_DEVICE: &str = "device";
/// The send error of a transcript to a synth that does not take one (the
/// link came back with an older synth): the caller sends the audio.
pub const NO_DEVICE_STT: &str = "no_device_stt";

/// How long owner.message may take to be accepted. Longer than the other
/// requests: up to 8 MB go up first, over whatever line the owner has.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(90);
/// After the link dropped mid-send: how long to wait for it to come back
/// before the one retry with the same client_id.
const RETRY_WAIT: Duration = Duration::from_secs(10);

/// The summary the island shows is cut to this many characters (the
/// synth sends at most 140; a longer one is not trusted to fit).
const MAX_SUMMARY_CHARS: usize = 200;

// --- settings ---

/// The three hold keys, as accelerators ("Alt+Space"). An empty string is
/// a key the owner switched off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hotkeys {
    #[serde(default = "default_voice")]
    pub voice: String,
    #[serde(default = "default_screen_voice")]
    pub screen_voice: String,
    #[serde(default = "default_text")]
    pub text: String,
}

/// Which key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyKind {
    Voice,
    ScreenVoice,
    Text,
}

impl HotkeyKind {
    pub const ALL: [HotkeyKind; 3] = [HotkeyKind::Voice, HotkeyKind::ScreenVoice, HotkeyKind::Text];

    pub fn parse(s: &str) -> Option<HotkeyKind> {
        match s {
            "voice" => Some(HotkeyKind::Voice),
            "screen_voice" => Some(HotkeyKind::ScreenVoice),
            "text" => Some(HotkeyKind::Text),
            _ => None,
        }
    }
}

/// The defaults of one system.
///
/// macOS: Option+Space (voice), Option+Shift+Space (screenshot + voice),
/// Control+Option+Space (text), as the TZ asks.
///
/// Windows (and Linux, where the island is not shown): Alt+Space opens the
/// window menu of the focused window and Ctrl+Space switches the input
/// method in East Asian layouts, so neither may be taken. The Win key
/// combinations belong to the shell (Win+Space switches the layout,
/// Win+Alt+... is the Game Bar). Ctrl+Alt+Space is free in Windows and
/// in the common apps, and Ctrl+Alt+Shift+Space follows it like the Mac
/// pair does. Text is Ctrl+Alt+Enter: the same left hand, and Enter is the
/// key that sends the line after it.
pub fn default_hotkeys(os: &str) -> Hotkeys {
    if os == "macos" {
        Hotkeys {
            voice: "Alt+Space".into(),
            screen_voice: "Alt+Shift+Space".into(),
            text: "Control+Alt+Space".into(),
        }
    } else {
        Hotkeys {
            voice: "Control+Alt+Space".into(),
            screen_voice: "Control+Alt+Shift+Space".into(),
            text: "Control+Alt+Enter".into(),
        }
    }
}

fn this_os() -> &'static str {
    std::env::consts::OS
}

fn default_voice() -> String {
    default_hotkeys(this_os()).voice
}
fn default_screen_voice() -> String {
    default_hotkeys(this_os()).screen_voice
}
fn default_text() -> String {
    default_hotkeys(this_os()).text
}

impl Default for Hotkeys {
    fn default() -> Self {
        default_hotkeys(this_os())
    }
}

impl Hotkeys {
    pub fn get(&self, kind: HotkeyKind) -> &str {
        match kind {
            HotkeyKind::Voice => &self.voice,
            HotkeyKind::ScreenVoice => &self.screen_voice,
            HotkeyKind::Text => &self.text,
        }
    }

    pub fn set(&mut self, kind: HotkeyKind, accelerator: String) {
        match kind {
            HotkeyKind::Voice => self.voice = accelerator,
            HotkeyKind::ScreenVoice => self.screen_voice = accelerator,
            HotkeyKind::Text => self.text = accelerator,
        }
    }

    /// Which other key (of these three) already uses this combination.
    /// Compared without regard to case and to the order of modifiers.
    pub fn taken_by(&self, kind: HotkeyKind, accelerator: &str) -> Option<HotkeyKind> {
        let want = canonical(accelerator);
        if want.is_empty() {
            return None;
        }
        HotkeyKind::ALL
            .into_iter()
            .filter(|k| *k != kind)
            .find(|k| canonical(self.get(*k)) == want)
    }

    /// Trim, and switch off a key that repeats an earlier one (a file
    /// edited by hand); the first of the two keeps it.
    pub fn normalize(&mut self) {
        let mut seen: Vec<String> = Vec::new();
        for kind in HotkeyKind::ALL {
            let value = self.get(kind).trim().to_string();
            let key = canonical(&value);
            if !key.is_empty() && seen.contains(&key) {
                self.set(kind, String::new());
            } else {
                if !key.is_empty() {
                    seen.push(key);
                }
                self.set(kind, value);
            }
        }
    }
}

/// An accelerator in one comparable form: lower case, modifiers sorted
/// and spelled one way, the key last. "" for an empty one.
pub fn canonical(accelerator: &str) -> String {
    let mut mods: Vec<&'static str> = Vec::new();
    let mut key = String::new();
    for token in accelerator.split('+').map(str::trim).filter(|t| !t.is_empty()) {
        let m = match token.to_ascii_lowercase().as_str() {
            "alt" | "option" => Some("alt"),
            "shift" => Some("shift"),
            "control" | "ctrl" => Some("control"),
            "command" | "cmd" | "super" | "meta" => Some("super"),
            "commandorcontrol" | "cmdorctrl" | "cmdorcontrol" | "commandorctrl" => {
                Some(if this_os() == "macos" { "super" } else { "control" })
            }
            _ => None,
        };
        match m {
            Some(m) => {
                if !mods.contains(&m) {
                    mods.push(m);
                }
            }
            None => key = token.to_ascii_lowercase().trim_start_matches("key").to_string(),
        }
    }
    if key.is_empty() && mods.is_empty() {
        return String::new();
    }
    mods.sort_unstable();
    let mut out = mods.join("+");
    if !out.is_empty() {
        out.push('+');
    }
    out.push_str(&key);
    out
}

/// Who turns the owner's voice into text (tz-local-stt.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SttMode {
    /// The voice goes to the synth as audio; the synth's speech-to-text
    /// hears the tone of voice too. The default.
    #[default]
    Synth,
    /// The app recognizes the speech itself while the key is held and
    /// sends the text: faster, the voice stays on this computer.
    Device,
}

/// Which on-device engine recognizes the speech (with `SttMode::Device`).
/// A build without whisper (Windows) runs Parakeet for `Whisper` and keeps
/// the stored value as it is (the app's `stt::effective`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SttEngine {
    /// whisper large-v3-turbo: keeps English words in Latin letters. The
    /// default, and in a settings file written before the choice existed.
    #[default]
    Whisper,
    /// NVIDIA Parakeet TDT 0.6B v3: much faster, better on short commands.
    Parakeet,
}

/// The synth's offer to recognize speech on this computer, shown once on
/// the island. Pending on a fresh install and in a settings file written
/// before the offer existed, so an owner who has the island on already
/// sees it once after the update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SttOffer {
    /// Not shown yet.
    #[default]
    Pending,
    /// On the island, not answered yet (shown again after a restart).
    Shown,
    Accepted,
    /// "Not now": never asked again (the switch stays in Settings).
    Declined,
}

impl SttOffer {
    /// The offer still waits for an answer.
    pub fn open(self) -> bool {
        matches!(self, SttOffer::Pending | SttOffer::Shown)
    }

    /// The island showed it.
    pub fn shown(self) -> SttOffer {
        match self {
            SttOffer::Pending => SttOffer::Shown,
            other => other,
        }
    }

    /// The owner answered on the island. An answer to an offer that is no
    /// longer open changes nothing.
    pub fn answered(self, accept: bool) -> SttOffer {
        match (self.open(), accept) {
            (true, true) => SttOffer::Accepted,
            (true, false) => SttOffer::Declined,
            (false, _) => self,
        }
    }

    /// The owner chose in Settings: that answers an open offer too.
    pub fn chosen_in_settings(self, mode: SttMode) -> SttOffer {
        self.answered(mode == SttMode::Device)
    }
}

/// Which screen edge the island keeps to (the owner's choice), or the
/// middle of a side edge (with `Left` or `Right` only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IslandVertical {
    Top,
    Middle,
    Bottom,
}

/// Where along that edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IslandHorizontal {
    Left,
    #[serde(alias = "centre")]
    Center,
    Right,
}

/// Where the island sits on the screen: eight spots (the middle of the
/// screen is not one). None on an axis is this system's default (macOS:
/// top centre, merged with the notch; Windows: bottom centre above the
/// taskbar, under a taskbar on the top edge), and so is a settings file
/// written before the choice existed. Middle with the centre (bad data)
/// is the default as a whole (the app's `geometry::Spot`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IslandPosition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertical: Option<IslandVertical>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horizontal: Option<IslandHorizontal>,
}

impl IslandPosition {
    /// "top", "middle", "bottom" or "" / "default" (this system's
    /// default).
    pub fn parse_vertical(s: &str) -> Result<Option<IslandVertical>, String> {
        match s {
            "" | "default" => Ok(None),
            "top" => Ok(Some(IslandVertical::Top)),
            "middle" => Ok(Some(IslandVertical::Middle)),
            "bottom" => Ok(Some(IslandVertical::Bottom)),
            _ => Err("bad_position".into()),
        }
    }

    /// "left", "center" ("centre"), "right" or "" / "default".
    pub fn parse_horizontal(s: &str) -> Result<Option<IslandHorizontal>, String> {
        match s {
            "" | "default" => Ok(None),
            "left" => Ok(Some(IslandHorizontal::Left)),
            "center" | "centre" => Ok(Some(IslandHorizontal::Center)),
            "right" => Ok(Some(IslandHorizontal::Right)),
            _ => Err("bad_position".into()),
        }
    }

    /// The middle of the screen, which is not a spot: middle with the
    /// centre, the default centre included.
    pub fn is_screen_middle(&self) -> bool {
        self.vertical == Some(IslandVertical::Middle) && matches!(self.horizontal, None | Some(IslandHorizontal::Center))
    }
}

/// The owner's island. Off on a fresh install and in a settings file
/// written before the island existed (a test feature for now).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IslandSettings {
    #[serde(default)]
    pub enabled: bool,
    /// The synth the keys talk to. Empty, or a synth that is no longer
    /// linked: the first linked synth (`main_synth_of`).
    #[serde(default)]
    pub main_synth: String,
    #[serde(default)]
    pub hotkeys: Hotkeys,
    /// Speech-to-text for the hold keys. The owner's own, like the keys:
    /// the synth only sees it in the messages (`stt`).
    #[serde(default)]
    pub stt: SttMode,
    #[serde(default)]
    pub stt_offer: SttOffer,
    /// The on-device engine (its own model download).
    #[serde(default)]
    pub stt_engine: SttEngine,
    /// Wake by name (tz-wake-word.md): the island listens for the main
    /// synth's name and takes what follows as a hold. Off by default.
    #[serde(default)]
    pub wake: bool,
    /// The owner's extra wake phrase, next to the synth's name. Empty:
    /// the name alone.
    #[serde(default)]
    pub wake_phrase: String,
    /// With speech recognized here: estimate the tone of the voice here
    /// too (owner.message `voice_tone`). On by default, and in a settings
    /// file written before the switch existed.
    #[serde(default = "yes")]
    pub stt_tone: bool,
    /// Where the island sits (`IslandPosition`): this system's default
    /// unless the owner chose a spot.
    #[serde(default)]
    pub position: IslandPosition,
}

fn yes() -> bool {
    true
}

impl Default for IslandSettings {
    fn default() -> Self {
        IslandSettings {
            enabled: false,
            main_synth: String::new(),
            hotkeys: Hotkeys::default(),
            stt: SttMode::default(),
            stt_offer: SttOffer::default(),
            stt_engine: SttEngine::default(),
            wake: false,
            wake_phrase: String::new(),
            stt_tone: true,
            position: IslandPosition::default(),
        }
    }
}

/// The longest extra wake phrase, in characters.
pub const WAKE_PHRASE_MAX: usize = 40;

impl IslandSettings {
    pub fn normalize(&mut self) {
        self.main_synth = self.main_synth.trim().to_string();
        self.hotkeys.normalize();
        let phrase = self.wake_phrase.split_whitespace().collect::<Vec<_>>().join(" ");
        self.wake_phrase = phrase.chars().take(WAKE_PHRASE_MAX).collect();
    }

    /// The synth the keys talk to among these links: the chosen one while
    /// it is linked, else the first link.
    pub fn main_synth_of(&self, links: &[Arc<LinkSlot>]) -> Option<Arc<LinkSlot>> {
        links
            .iter()
            .find(|l| !self.main_synth.is_empty() && l.synth_id == self.main_synth)
            .or_else(|| links.first())
            .cloned()
    }
}

/// `capabilities.island`, only while the island is on. A synth that sees
/// it may call island.event and offers the synth its face tool.
pub fn capability(enabled: bool) -> Option<Value> {
    enabled.then(|| json!({"enabled": true, "version": VERSION}))
}

/// The capabilities object with the island in it when it is on.
pub fn with_capability(mut caps: Value, enabled: bool) -> Value {
    if let (Some(cap), Some(obj)) = (capability(enabled), caps.as_object_mut()) {
        obj.insert("island".into(), cap);
    }
    caps
}

// --- owner.message ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Voice,
    ScreenVoice,
    Text,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Voice => "voice",
            Mode::ScreenVoice => "screen_voice",
            Mode::Text => "text",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Audio {
    /// "audio/ogg", "audio/mp4" or "audio/wav".
    pub mime: String,
    pub data: Vec<u8>,
    pub ms: u64,
}

#[derive(Debug, Clone)]
pub struct Image {
    /// "image/jpeg" or "image/png".
    pub mime: String,
    pub data: Vec<u8>,
    /// The long edge in pixels (the shell scaled it down already).
    pub long_edge: u32,
}

/// One message from the owner to the synth.
#[derive(Debug, Clone)]
pub struct OwnerMessage {
    /// A fresh uuid per message: the synth answers a repeat of it with the
    /// same message_id and no second turn.
    pub client_id: String,
    pub mode: Mode,
    pub text: String,
    pub audio: Option<Audio>,
    pub images: Vec<Image>,
    /// voice and screen_voice: the text recognized on this computer, sent
    /// instead of the audio (`stt` "device"). Only to a synth with
    /// `FEATURE_DEVICE_STT`; never together with `audio`.
    pub voice_transcript: Option<String>,
    /// With `voice_transcript` only: the tone of the voice estimated on
    /// this computer. Sent only to a synth with `FEATURE_DEVICE_TONE`
    /// (`send` leaves it out otherwise); cut to `MAX_TONE_CHARS`.
    pub voice_tone: Option<String>,
}

/// A fresh client_id: a random UUID (version 4).
pub fn new_client_id() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let mut hex = String::with_capacity(32);
    for x in b {
        use std::fmt::Write;
        let _ = write!(hex, "{x:02x}");
    }
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// Why a message could not go, as a short code the UI has words for, and
/// the detail behind it (diagnostics).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SendError {
    /// "empty", "too_large", "too_long", "offline", "paused", "outdated",
    /// "no_owner_session", "busy", "no_synth", "failed".
    pub code: String,
    pub detail: String,
}

impl SendError {
    pub fn new(code: &str, detail: impl Into<String>) -> SendError {
        SendError { code: code.into(), detail: detail.into() }
    }
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

const AUDIO_MIMES: [&str; 3] = ["audio/ogg", "audio/mp4", "audio/wav"];
const IMAGE_MIMES: [&str; 2] = ["image/jpeg", "image/png"];

/// Base64 grows 4/3, rounded up to whole groups.
fn b64_len(n: usize) -> usize {
    n.div_ceil(3) * 4
}

impl OwnerMessage {
    /// The request params, checked against every limit of the protocol.
    pub fn params(&self) -> Result<Value, SendError> {
        if self.client_id.trim().is_empty() {
            return Err(SendError::new("failed", "a message needs a client_id"));
        }
        let text = self.text.trim();
        if text.chars().count() > MAX_TEXT_CHARS {
            return Err(SendError::new("too_large", format!("text longer than {MAX_TEXT_CHARS} characters")));
        }
        let transcript = self.voice_transcript.as_deref().map(str::trim);
        if transcript.is_some_and(|t| t.chars().count() > MAX_TRANSCRIPT_CHARS) {
            return Err(SendError::new("too_large", format!("a transcript longer than {MAX_TRANSCRIPT_CHARS} characters")));
        }
        let transcript = transcript.filter(|t| !t.is_empty());
        match self.mode {
            Mode::Text if text.is_empty() => return Err(SendError::new("empty", "no text")),
            Mode::Text if self.audio.is_some() => return Err(SendError::new("failed", "a text message carries no voice")),
            Mode::Text if self.voice_transcript.is_some() => {
                return Err(SendError::new("failed", "a text message carries no voice transcript"));
            }
            Mode::Voice | Mode::ScreenVoice if self.audio.is_some() && self.voice_transcript.is_some() => {
                return Err(SendError::new("failed", "a voice message carries the audio or its transcript, not both"));
            }
            Mode::Voice if self.audio.is_none() && transcript.is_none() => {
                return Err(SendError::new("empty", "no voice"));
            }
            // A screenshot + voice hold in which nothing was said sends
            // the screenshot alone (the synth takes pictures without a
            // voice).
            Mode::ScreenVoice if self.audio.is_none() && transcript.is_none() && self.images.is_empty() => {
                return Err(SendError::new("empty", "no voice and no screenshot"));
            }
            _ => {}
        }
        if self.mode != Mode::ScreenVoice && !self.images.is_empty() {
            return Err(SendError::new("failed", "only screen_voice carries images"));
        }
        if self.images.len() > MAX_IMAGES {
            return Err(SendError::new("too_large", format!("more than {MAX_IMAGES} images")));
        }
        let mut images = Vec::new();
        for image in &self.images {
            if !IMAGE_MIMES.contains(&image.mime.as_str()) {
                return Err(SendError::new("failed", format!("image type {} is not taken", image.mime)));
            }
            if image.long_edge > MAX_IMAGE_EDGE || image.data.is_empty() {
                return Err(SendError::new("too_large", format!("an image of {} px (max {MAX_IMAGE_EDGE})", image.long_edge)));
            }
            images.push(json!({"mime": image.mime, "b64": base64::engine::general_purpose::STANDARD.encode(&image.data)}));
        }
        let mut params = json!({
            "client_id": self.client_id,
            "mode": self.mode.as_str(),
            "text": text,
        });
        if let Some(audio) = &self.audio {
            if !AUDIO_MIMES.contains(&audio.mime.as_str()) {
                return Err(SendError::new("failed", format!("audio type {} is not taken", audio.mime)));
            }
            if audio.ms > MAX_VOICE_MS {
                return Err(SendError::new("too_long", format!("{} ms of voice (max {MAX_VOICE_MS})", audio.ms)));
            }
            if audio.data.is_empty() {
                return Err(SendError::new("empty", "the recording is empty"));
            }
            // Counted before encoding, so an oversized recording is not
            // turned into megabytes of base64 for nothing.
            let raw: usize = b64_len(audio.data.len()) + self.images.iter().map(|i| b64_len(i.data.len())).sum::<usize>();
            if raw > MAX_REQUEST_BYTES {
                return Err(SendError::new("too_large", format!("{raw} bytes of media (max {MAX_REQUEST_BYTES})")));
            }
            params["audio"] = json!({
                "mime": audio.mime,
                "b64": base64::engine::general_purpose::STANDARD.encode(&audio.data),
                "ms": audio.ms,
            });
        }
        if let Some(t) = transcript {
            params["voice_transcript"] = json!(t);
            params["stt"] = json!(STT_DEVICE);
        }
        if let Some(tone) = self.voice_tone.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            if transcript.is_none() {
                return Err(SendError::new("failed", "a voice tone goes only with a transcript"));
            }
            params["voice_tone"] = json!(tone.chars().take(MAX_TONE_CHARS).collect::<String>());
        }
        if !images.is_empty() {
            params["images"] = Value::Array(images);
        }
        // The whole request line as the control stream will carry it.
        let envelope = json!({"type": "request", "id": u64::MAX, "method": "owner.message", "params": params});
        let size = serde_json::to_vec(&envelope).map(|b| b.len() + 1).unwrap_or(usize::MAX);
        if size > MAX_REQUEST_BYTES {
            return Err(SendError::new("too_large", format!("a request of {size} bytes (max {MAX_REQUEST_BYTES})")));
        }
        Ok(params)
    }
}

/// The synth's error for owner.message, as one of the UI's codes. The
/// synth answers `{"error":"<code>","reason":"<text>"}` (requests.rs makes
/// that "<code>: <text>"); the code is matched exactly, the text is only
/// detail. Codes: too_large, no_owner_session, busy, bad_request.
pub fn send_error_of(e: &str) -> SendError {
    let code = e.split(':').next().unwrap_or("").trim();
    match code {
        "too_large" | "no_owner_session" | "busy" => return SendError::new(code, e),
        "bad_request" => return SendError::new("failed", e),
        _ => {}
    }
    let lower = e.to_ascii_lowercase();
    if lower.contains("needs an update") || lower.contains("unknown method") {
        return SendError::new("outdated", e);
    }
    if lower.starts_with("unavailable: not linked") || lower.contains("link to the synth dropped") || lower.contains("did not answer in time") {
        return SendError::new("offline", e);
    }
    SendError::new("failed", e)
}

/// The synth's answer: `{"accepted":true,"message_id":"..."}`.
pub fn accepted_of(answer: &Value) -> Result<String, SendError> {
    if let Some(e) = answer.get("error").and_then(Value::as_str) {
        return Err(send_error_of(e));
    }
    if answer.get("accepted").and_then(Value::as_bool) != Some(true) {
        return Err(SendError::new("failed", "the synth did not accept the message"));
    }
    let id = answer.get("message_id").and_then(Value::as_str).map(str::trim).unwrap_or("");
    if id.is_empty() {
        return Err(SendError::new("failed", "the synth accepted without a message_id"));
    }
    Ok(clip(id, 128))
}

/// Does this link's synth take island requests?
pub fn supports(slot: &LinkSlot) -> bool {
    slot.synth
        .read()
        .unwrap()
        .as_ref()
        .is_some_and(|ack| ack.has(FEATURE) && ack.has(crate::requests::FEATURE))
}

/// Does this link's synth take a voice message as text recognized here?
pub fn supports_device_stt(slot: &LinkSlot) -> bool {
    supports(slot) && slot.synth.read().unwrap().as_ref().is_some_and(|ack| ack.has(FEATURE_DEVICE_STT))
}

/// Does this link's synth take the tone estimated here with a transcript?
pub fn supports_device_tone(slot: &LinkSlot) -> bool {
    supports_device_stt(slot) && slot.synth.read().unwrap().as_ref().is_some_and(|ack| ack.has(FEATURE_DEVICE_TONE))
}

/// Send one message to this synth. The answer comes before the synth's
/// turn (the synth's iron rule), so this waits only for "accepted".
///
/// When the link drops before the answer, the message is sent once more
/// with the same client_id as soon as the link is back (at most
/// `RETRY_WAIT` later): the synth runs it once either way.
pub async fn send(shared: &Arc<Shared>, synth_id: &str, msg: &OwnerMessage) -> Result<String, SendError> {
    let slot = shared.slot(synth_id).ok_or_else(|| SendError::new("no_synth", synth_id))?;
    let params = msg.params()?;
    let mut retried = false;
    loop {
        if shared.paused() {
            return Err(SendError::new("paused", "the owner paused SNTH Connect"));
        }
        if slot.state() != LinkState::Linked {
            return Err(SendError::new("offline", slot.detail()));
        }
        if !supports(&slot) {
            return Err(SendError::new("outdated", "the synth does not announce the island"));
        }
        // The link came back with an older synth: the transcript means
        // nothing to it. The caller sends the audio instead.
        if msg.voice_transcript.is_some() && !supports_device_stt(&slot) {
            return Err(SendError::new(NO_DEVICE_STT, "the synth does not take a transcript"));
        }
        // The tone only to a synth that takes it (the link may have come
        // back with another version since the last try).
        let mut params = params.clone();
        if !supports_device_tone(&slot) {
            if let Some(p) = params.as_object_mut() {
                p.remove("voice_tone");
            }
        }
        match slot.requests.call_within("owner.message", params, SEND_TIMEOUT).await {
            Ok(answer) => return accepted_of(&answer),
            Err(e) if !retried && e.contains("dropped") => {
                retried = true;
                let deadline = tokio::time::Instant::now() + RETRY_WAIT;
                while tokio::time::Instant::now() < deadline && (slot.state() != LinkState::Linked || !supports(&slot)) {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
            Err(e) => return Err(send_error_of(&e)),
        }
    }
}

// --- faces ---

/// A face is a JSON object of at most `MAX_FACE_BYTES`. Nothing deeper is
/// checked here (the synth validates, the renderer clamps).
pub fn face_ok(face: &Value) -> bool {
    face.is_object() && serde_json::to_vec(face).is_ok_and(|b| b.len() <= MAX_FACE_BYTES)
}

/// Keep this synth's face (None = the synth has none: the default preset
/// of its id). Emits `CoreEvent::Face` when it changed.
pub(crate) fn store_face(shared: &Shared, synth_id: &str, face: Option<Value>) {
    if synth_id.is_empty() {
        return;
    }
    let changed = {
        let mut p = shared.persisted.write().unwrap();
        let before = p.faces.get(synth_id).cloned();
        match face.clone() {
            Some(f) => {
                p.faces.insert(synth_id.to_string(), f);
            }
            None => {
                p.faces.remove(synth_id);
            }
        }
        let changed = before != p.faces.get(synth_id).cloned();
        if changed {
            shared.save_persisted(&p);
        }
        changed
    };
    // Every answer counts as news for the UI the first time it arrives
    // (a face that was never read is shown as the default until then).
    if changed || !shared.faces_read.lock().unwrap().contains(synth_id) {
        shared.faces_read.lock().unwrap().insert(synth_id.to_string());
        shared.emit(CoreEvent::Face { synth_id: synth_id.to_string() });
    }
}

/// `face.get` of one synth into the cache. Err with the reason when the
/// synth could not be asked (the cache stays as it was).
pub async fn refresh_face(shared: &Arc<Shared>, slot: &Arc<LinkSlot>) -> Result<(), String> {
    if !supports(slot) {
        return Err("unavailable: the synth does not announce the island".into());
    }
    let answer = slot.requests.call("face.get", json!({})).await?;
    let face = answer.get("face").cloned().unwrap_or(Value::Null);
    if face.is_null() {
        store_face(shared, &slot.synth_id, None);
    } else if face_ok(&face) {
        store_face(shared, &slot.synth_id, Some(face));
    } else {
        return Err("the synth's face is not a FaceConfig of at most 4 KB".into());
    }
    Ok(())
}

/// What `face.set` answered.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum FaceSet {
    Saved { face: Value },
    Invalid { reason: String },
}

/// The owner changed the face in the editor: `face.set` on the synth (the
/// source of truth), then the cache.
pub async fn set_face(shared: &Arc<Shared>, synth_id: &str, face: Value, patch: bool) -> Result<FaceSet, String> {
    let slot = shared.slot(synth_id).ok_or("unknown_synth")?;
    if !face_ok(&face) {
        return Ok(FaceSet::Invalid { reason: format!("a face is a JSON object of at most {MAX_FACE_BYTES} bytes") });
    }
    if slot.state() != LinkState::Linked {
        return Err("offline".into());
    }
    if !supports(&slot) {
        return Err("outdated".into());
    }
    let answer = match slot.requests.call("face.set", json!({"face": face, "patch": patch})).await {
        Ok(answer) => answer,
        // The synth may also refuse as a response error.
        Err(e) if e.starts_with("invalid_face") => {
            return Ok(FaceSet::Invalid { reason: e.trim_start_matches("invalid_face").trim_start_matches(':').trim().to_string() });
        }
        Err(e) => return Err(e),
    };
    if answer.get("error").and_then(Value::as_str) == Some("invalid_face") {
        let reason = answer.get("reason").and_then(Value::as_str).unwrap_or("").to_string();
        return Ok(FaceSet::Invalid { reason: clip(&reason, 400) });
    }
    let saved = answer.get("face").cloned().unwrap_or(Value::Null);
    if !face_ok(&saved) {
        return Err("the synth answered without a face".into());
    }
    store_face(shared, synth_id, Some(saved.clone()));
    Ok(FaceSet::Saved { face: saved })
}

// --- island.event ---

/// One event of the synth about a message the owner sent from here, or a
/// change of its face.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IslandEvent {
    pub synth_id: String,
    /// "accepted", "working", "done", "error".
    pub kind: String,
    pub message_id: String,
    /// done: a short summary of the answer; error: a short text. Plain
    /// text, the synth's own words (rendered as text, never as markup).
    pub summary: String,
    /// done: where the answer went ("telegram", "tguser", "imessage"...).
    pub channel: String,
}

const KINDS: [&str; 5] = ["accepted", "working", "done", "error", "face"];

/// The rpc `island.event` (rpc.rs). Answered `{"ok":true}`; the synth
/// ignores errors, so a refusal is only a labeled line for its log.
pub(crate) fn handle_event(shared: &Arc<Shared>, params: &Value) -> Result<Value, String> {
    if !shared.island().enabled {
        return Err("denied: the island is off on this computer".into());
    }
    let synth_id = shared.current_synth_id();
    if synth_id.is_empty() {
        return Err("denied: not from a linked synth".into());
    }
    let kind = params.get("kind").and_then(Value::as_str).unwrap_or("");
    if !KINDS.contains(&kind) {
        return Err(format!("bad request: unknown island event kind {kind:?}"));
    }
    if kind == "face" {
        let face = params.get("face").cloned().unwrap_or(Value::Null);
        if face.is_null() {
            store_face(shared, &synth_id, None);
        } else if face_ok(&face) {
            store_face(shared, &synth_id, Some(face));
        } else {
            return Err(format!("bad request: params.face must be a FaceConfig object of at most {MAX_FACE_BYTES} bytes"));
        }
        return Ok(json!({"ok": true}));
    }
    let text = |key: &str, max: usize| params.get(key).and_then(Value::as_str).map(|s| clip(s, max)).unwrap_or_default();
    let channel: String = text("channel", 32)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    shared.emit(CoreEvent::Island(IslandEvent {
        synth_id,
        kind: kind.to_string(),
        message_id: text("message_id", 128),
        summary: text("summary", MAX_SUMMARY_CHARS),
        channel,
    }));
    Ok(json!({"ok": true}))
}

/// At most `max` characters, control characters as spaces, trimmed.
fn clip(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).map(|c| if c.is_control() { ' ' } else { c }).collect();
    out = out.trim().to_string();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

/// What the core keeps per synth for the island, as the UI sees it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct IslandView {
    pub settings: IslandSettings,
    /// The synth the keys talk to right now (None: no link).
    pub main_synth: Option<String>,
    /// synth_id -> its face; absent = the default preset of its id.
    pub faces: BTreeMap<String, Value>,
    /// synth_id -> it takes island requests (announced the feature).
    pub supported: BTreeMap<String, bool>,
    /// synth_id -> it takes a voice message as text recognized here.
    pub device_stt: BTreeMap<String, bool>,
}

#[cfg(test)]
mod tests;
