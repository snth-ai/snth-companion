//! The three sources of `observe`, apart from the system: what counts as a
//! call, which app is in front and what kind of app it is, which named
//! place this Wi-Fi network is. The shell reads the system through the
//! three small probes below (`MicProbe`, `FrontProbe`, `WifiProbe`); every
//! decision is made here, so it is tested without a microphone, a window
//! or a network.
//!
//! Nothing here sends anything: `Sampler::tick` turns one round of probe
//! readings into the current value of each source (None = nothing to
//! report), and the shell hands those to `Core::observe_report`.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use super::{clean, rfc3339, AppMark, NamedNetwork, ObserveSettings, ACTIVITY, CALL, MAX_APP_BYTES, MAX_PLACE_BYTES, MAX_TITLE_BYTES, PLACE, PRIVATE};

/// How often the shell runs a round (`Sampler::tick`): the front app and
/// the microphone are read every round.
pub const TICK: Duration = Duration::from_secs(2);
/// The Wi-Fi network is read this often (the place itself is worked out
/// every round, so a network the owner just named counts at once). Short
/// enough to stand in for a "network changed" notification.
pub const WIFI_EVERY: Duration = Duration::from_secs(10);
/// A browser window with Google Meet in front this recently makes a
/// browser's microphone a Meet call.
pub const MEET_MEMORY_SECS: i64 = 600;
/// The apps the owner saw in front lately, for the exclusion picker.
pub const RECENT_APPS: usize = 12;

// --- categories ---

/// `device.activity` `category`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Code,
    Terminal,
    Browser,
    Chat,
    Mail,
    Meeting,
    Video,
    Music,
    Docs,
    Design,
    Game,
    Other,
    Private,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Code => "code",
            Category::Terminal => "terminal",
            Category::Browser => "browser",
            Category::Chat => "chat",
            Category::Mail => "mail",
            Category::Meeting => "meeting",
            Category::Video => "video",
            Category::Music => "music",
            Category::Docs => "docs",
            Category::Design => "design",
            Category::Game => "game",
            Category::Other => "other",
            Category::Private => "private",
        }
    }
}

/// macOS bundle ids, lower case. An entry also covers the ids under it
/// (`com.jetbrains` covers `com.jetbrains.intellij`), never a longer word
/// (`com.apple.mail` does not cover `com.apple.mailx`).
const MAC_APPS: &[(&str, Category)] = &[
    // code
    ("com.microsoft.vscode", Category::Code),
    ("com.microsoft.vscodeinsiders", Category::Code),
    ("com.vscodium", Category::Code),
    ("com.todesktop.230313mzl4w4u92", Category::Code), // Cursor
    ("com.exafunction.windsurf", Category::Code),
    ("dev.zed.zed", Category::Code),
    ("dev.zed.zed-preview", Category::Code),
    ("com.apple.dt.xcode", Category::Code),
    ("com.jetbrains", Category::Code),
    ("com.google.android.studio", Category::Code),
    ("com.sublimetext", Category::Code),
    ("com.panic.nova", Category::Code),
    ("com.barebones.bbedit", Category::Code),
    ("com.github.githubclient", Category::Code),
    ("com.sublimemerge", Category::Code),
    ("com.fournova.tower3", Category::Code),
    // terminal
    ("com.apple.terminal", Category::Terminal),
    ("com.googlecode.iterm2", Category::Terminal),
    ("dev.warp.warp-stable", Category::Terminal),
    ("dev.warp.warp", Category::Terminal),
    ("com.mitchellh.ghostty", Category::Terminal),
    ("net.kovidgoyal.kitty", Category::Terminal),
    ("org.alacritty", Category::Terminal),
    ("io.alacritty", Category::Terminal),
    ("com.github.wez.wezterm", Category::Terminal),
    ("co.zeit.hyper", Category::Terminal),
    // browser
    ("com.apple.safari", Category::Browser),
    ("com.apple.safaritechnologypreview", Category::Browser),
    ("com.google.chrome", Category::Browser),
    ("company.thebrowser.browser", Category::Browser), // Arc
    ("org.mozilla.firefox", Category::Browser),
    ("org.mozilla.firefoxdeveloperedition", Category::Browser),
    ("com.microsoft.edgemac", Category::Browser),
    ("com.brave.browser", Category::Browser),
    ("com.operasoftware.opera", Category::Browser),
    ("com.vivaldi.vivaldi", Category::Browser),
    ("org.chromium.chromium", Category::Browser),
    ("app.zen-browser.zen", Category::Browser),
    ("com.kagi.kagimacos", Category::Browser), // Orion
    // chat
    ("ru.keepcoder.telegram", Category::Chat),
    ("org.telegram.desktop", Category::Chat),
    ("net.whatsapp.whatsapp", Category::Chat),
    ("desktop.whatsapp", Category::Chat),
    ("com.tinyspeck.slackmacgap", Category::Chat),
    ("com.hnc.discord", Category::Chat),
    ("com.apple.mobilesms", Category::Chat), // Messages
    ("org.whispersystems.signal-desktop", Category::Chat),
    ("com.viber.osx", Category::Chat),
    ("com.facebook.archon", Category::Chat), // Messenger
    ("jp.naver.line.mac", Category::Chat),
    // mail
    ("com.apple.mail", Category::Mail),
    ("com.microsoft.outlook", Category::Mail),
    ("com.readdle.smartemail-mac", Category::Mail), // Spark
    ("com.readdle.spark", Category::Mail),
    ("com.superhuman.electron", Category::Mail),
    ("com.freron.mailmate", Category::Mail),
    ("it.bloop.airmail2", Category::Mail),
    ("com.mimestream.mimestream", Category::Mail),
    ("org.mozilla.thunderbird", Category::Mail),
    // meeting
    ("us.zoom.xos", Category::Meeting),
    ("com.apple.facetime", Category::Meeting),
    ("com.microsoft.teams", Category::Meeting),
    ("com.microsoft.teams2", Category::Meeting),
    ("com.skype.skype", Category::Meeting),
    ("com.cisco.webexmeetingsapp", Category::Meeting),
    ("com.webex.meetingmanager", Category::Meeting),
    // video
    ("com.apple.tv", Category::Video),
    ("com.apple.quicktimeplayerx", Category::Video),
    ("org.videolan.vlc", Category::Video),
    ("com.colliderli.iina", Category::Video),
    ("com.firecore.infuse", Category::Video),
    // music
    ("com.apple.music", Category::Music),
    ("com.spotify.client", Category::Music),
    ("com.apple.podcasts", Category::Music),
    ("com.tidal.desktop", Category::Music),
    ("com.apple.garageband10", Category::Music),
    ("com.apple.logic10", Category::Music),
    // docs
    ("com.apple.iwork.pages", Category::Docs),
    ("com.apple.iwork.numbers", Category::Docs),
    ("com.apple.iwork.keynote", Category::Docs),
    ("com.microsoft.word", Category::Docs),
    ("com.microsoft.excel", Category::Docs),
    ("com.microsoft.powerpoint", Category::Docs),
    ("com.microsoft.onenote.mac", Category::Docs),
    ("notion.id", Category::Docs),
    ("md.obsidian", Category::Docs),
    ("com.apple.notes", Category::Docs),
    ("com.apple.textedit", Category::Docs),
    ("com.apple.preview", Category::Docs),
    ("com.apple.ibooksx", Category::Docs),
    ("com.adobe.reader", Category::Docs),
    ("com.adobe.acrobat.pro", Category::Docs),
    ("net.shinyfrog.bear", Category::Docs),
    ("com.lukilabs.lukiapp", Category::Docs), // Craft
    ("com.ulyssesapp.mac", Category::Docs),
    ("com.literatureandlatte.scrivener3", Category::Docs),
    // design
    ("com.figma.desktop", Category::Design),
    ("com.bohemiancoding.sketch3", Category::Design),
    ("com.adobe.photoshop", Category::Design),
    ("com.adobe.illustrator", Category::Design),
    ("com.adobe.indesign", Category::Design),
    ("com.adobe.aftereffects", Category::Design),
    ("com.adobe.premierepro", Category::Design),
    ("com.adobe.lightroomcc", Category::Design),
    ("com.seriflabs", Category::Design), // Affinity
    ("com.pixelmatorteam", Category::Design),
    ("com.apple.finalcutpro", Category::Design),
    ("com.blackmagic-design.davinciresolve", Category::Design),
    ("org.blenderfoundation.blender", Category::Design),
    ("com.canva.canvaeditor", Category::Design),
    // game
    ("com.valvesoftware.steam", Category::Game),
    ("com.epicgames.epicgameslauncher", Category::Game),
    ("net.battle.app", Category::Game),
    ("com.apple.chess", Category::Game),
    ("org.openemu.openemu", Category::Game),
];

/// Windows program file names, lower case.
const WIN_APPS: &[(&str, Category)] = &[
    // code
    ("code.exe", Category::Code),
    ("code - insiders.exe", Category::Code),
    ("vscodium.exe", Category::Code),
    ("cursor.exe", Category::Code),
    ("windsurf.exe", Category::Code),
    ("zed.exe", Category::Code),
    ("devenv.exe", Category::Code),
    ("idea64.exe", Category::Code),
    ("pycharm64.exe", Category::Code),
    ("webstorm64.exe", Category::Code),
    ("goland64.exe", Category::Code),
    ("clion64.exe", Category::Code),
    ("rider64.exe", Category::Code),
    ("phpstorm64.exe", Category::Code),
    ("rubymine64.exe", Category::Code),
    ("datagrip64.exe", Category::Code),
    ("rustrover64.exe", Category::Code),
    ("studio64.exe", Category::Code),
    ("sublime_text.exe", Category::Code),
    ("notepad++.exe", Category::Code),
    ("githubdesktop.exe", Category::Code),
    // terminal
    ("windowsterminal.exe", Category::Terminal),
    ("wt.exe", Category::Terminal),
    ("openconsole.exe", Category::Terminal),
    ("conhost.exe", Category::Terminal),
    ("cmd.exe", Category::Terminal),
    ("powershell.exe", Category::Terminal),
    ("pwsh.exe", Category::Terminal),
    ("wezterm-gui.exe", Category::Terminal),
    ("alacritty.exe", Category::Terminal),
    ("mintty.exe", Category::Terminal),
    ("warp.exe", Category::Terminal),
    ("putty.exe", Category::Terminal),
    // browser
    ("chrome.exe", Category::Browser),
    ("msedge.exe", Category::Browser),
    ("firefox.exe", Category::Browser),
    ("brave.exe", Category::Browser),
    ("opera.exe", Category::Browser),
    ("vivaldi.exe", Category::Browser),
    ("arc.exe", Category::Browser),
    ("zen.exe", Category::Browser),
    // chat
    ("telegram.exe", Category::Chat),
    ("whatsapp.exe", Category::Chat),
    ("whatsapp.root.exe", Category::Chat),
    ("slack.exe", Category::Chat),
    ("discord.exe", Category::Chat),
    ("signal.exe", Category::Chat),
    ("viber.exe", Category::Chat),
    ("messenger.exe", Category::Chat),
    ("line.exe", Category::Chat),
    // mail
    ("outlook.exe", Category::Mail),
    ("olk.exe", Category::Mail), // the new Outlook
    ("hxoutlook.exe", Category::Mail),
    ("thunderbird.exe", Category::Mail),
    ("mailbird.exe", Category::Mail),
    // meeting
    ("zoom.exe", Category::Meeting),
    ("ms-teams.exe", Category::Meeting),
    ("teams.exe", Category::Meeting),
    ("skype.exe", Category::Meeting),
    ("webex.exe", Category::Meeting),
    ("ciscocollabhost.exe", Category::Meeting),
    // video
    ("vlc.exe", Category::Video),
    ("mpc-hc64.exe", Category::Video),
    ("mpv.exe", Category::Video),
    ("potplayermini64.exe", Category::Video),
    ("video.ui.exe", Category::Video),
    // music
    ("spotify.exe", Category::Music),
    ("itunes.exe", Category::Music),
    ("applemusic.exe", Category::Music),
    ("tidal.exe", Category::Music),
    ("foobar2000.exe", Category::Music),
    ("aimp.exe", Category::Music),
    // docs
    ("winword.exe", Category::Docs),
    ("excel.exe", Category::Docs),
    ("powerpnt.exe", Category::Docs),
    ("onenote.exe", Category::Docs),
    ("notion.exe", Category::Docs),
    ("obsidian.exe", Category::Docs),
    ("notepad.exe", Category::Docs),
    ("wordpad.exe", Category::Docs),
    ("acrord32.exe", Category::Docs),
    ("acrobat.exe", Category::Docs),
    ("sumatrapdf.exe", Category::Docs),
    // design
    ("figma.exe", Category::Design),
    ("photoshop.exe", Category::Design),
    ("illustrator.exe", Category::Design),
    ("indesign.exe", Category::Design),
    ("afterfx.exe", Category::Design),
    ("adobe premiere pro.exe", Category::Design),
    ("blender.exe", Category::Design),
    ("resolve.exe", Category::Design),
    ("canva.exe", Category::Design),
    ("inkscape.exe", Category::Design),
    ("paintdotnet.exe", Category::Design),
    ("mspaint.exe", Category::Design),
    // game
    ("steam.exe", Category::Game),
    ("epicgameslauncher.exe", Category::Game),
    ("battle.net.exe", Category::Game),
    ("riotclientux.exe", Category::Game),
];

/// Names for the Windows programs whose file name says little; the rest
/// show their file name without `.exe`.
const WIN_NAMES: &[(&str, &str)] = &[
    ("code.exe", "Visual Studio Code"),
    ("code - insiders.exe", "Visual Studio Code Insiders"),
    ("devenv.exe", "Visual Studio"),
    ("idea64.exe", "IntelliJ IDEA"),
    ("pycharm64.exe", "PyCharm"),
    ("webstorm64.exe", "WebStorm"),
    ("studio64.exe", "Android Studio"),
    ("sublime_text.exe", "Sublime Text"),
    ("windowsterminal.exe", "Windows Terminal"),
    ("wt.exe", "Windows Terminal"),
    ("cmd.exe", "Command Prompt"),
    ("powershell.exe", "PowerShell"),
    ("pwsh.exe", "PowerShell"),
    ("chrome.exe", "Google Chrome"),
    ("msedge.exe", "Microsoft Edge"),
    ("firefox.exe", "Firefox"),
    ("winword.exe", "Word"),
    ("excel.exe", "Excel"),
    ("powerpnt.exe", "PowerPoint"),
    ("olk.exe", "Outlook"),
    ("ms-teams.exe", "Microsoft Teams"),
    ("acrord32.exe", "Adobe Acrobat Reader"),
    ("explorer.exe", "File Explorer"),
    ("afterfx.exe", "After Effects"),
    ("paintdotnet.exe", "Paint.NET"),
    ("mspaint.exe", "Paint"),
];

/// `id` is `key`, or an id under it (`key.` + more).
fn under(id: &str, key: &str) -> bool {
    id == key || (id.len() > key.len() && id.starts_with(key) && id.as_bytes()[key.len()] == b'.')
}

fn lookup<T: Copy>(table: &[(&str, T)], id: &str) -> Option<T> {
    let id = id.trim().to_lowercase();
    table.iter().find(|(key, _)| under(&id, key)).map(|(_, v)| *v)
}

/// What kind of app this is, by its id: a macOS bundle id or a Windows
/// program's file name. `Other` when the app is not in the tables.
pub fn category_of(id: &str) -> Category {
    let lower = id.trim().to_lowercase();
    if lower.ends_with(".exe") {
        return WIN_APPS.iter().find(|(k, _)| *k == lower).map(|(_, c)| *c).unwrap_or(Category::Other);
    }
    lookup(MAC_APPS, &lower).unwrap_or(Category::Other)
}

/// The name a Windows program is shown by: a known one's own name, else
/// its file name without `.exe`.
pub fn windows_app_name(exe: &str) -> String {
    let lower = exe.trim().to_lowercase();
    if let Some((_, name)) = WIN_NAMES.iter().find(|(k, _)| *k == lower) {
        return (*name).to_string();
    }
    let trimmed = exe.trim();
    match trimmed.len().checked_sub(4) {
        Some(cut) if lower.ends_with(".exe") && trimmed.is_char_boundary(cut) => trimmed[..cut].to_string(),
        _ => trimmed.to_string(),
    }
}

// --- the front app and the exclusions ---

/// The app in front, as a probe reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Front {
    /// macOS: the bundle id. Windows: the program's file name ("Code.exe").
    pub id: String,
    /// The name the system shows (macOS `localizedName`).
    pub name: String,
    /// The focused window's title, where the system lets the app read it
    /// (macOS: Accessibility).
    pub title: Option<String>,
}

/// Reads the app in front. None while nothing useful is in front (the
/// screen is locked, the login window).
pub trait FrontProbe: Send {
    fn front(&mut self) -> Option<Front>;
}

/// Password managers: never named, never their window title. macOS bundle
/// ids and Windows file names, lower case.
const EXCLUDED_IDS: &[&str] = &[
    "com.1password.1password",
    "com.agilebits.onepassword",
    "com.agilebits.onepassword7",
    "com.agilebits.onepassword-osx",
    "com.bitwarden.desktop",
    "com.apple.keychainaccess",
    "com.apple.passwords",
    "com.lastpass.lastpassmacdesktop",
    "com.lastpass.lastpass",
    "com.dashlane.dashlane",
    "com.dashlane.dashlanephonefinal",
    "org.keepassxc.keepassxc",
    "1password.exe",
    "bitwarden.exe",
    "lastpass.exe",
    "dashlane.exe",
    "keepassxc.exe",
    "keepass.exe",
];

/// The same apps by the words in their name, for a build whose id differs.
const EXCLUDED_NAMES: &[&str] = &["1password", "bitwarden", "lastpass", "dashlane", "keepass", "keychain access", "связка ключей"];

/// The page of System Settings that shows the passwords (its window is
/// titled after the page).
const SETTINGS_IDS: &[&str] = &["com.apple.systempreferences", "com.apple.systemsettings"];
const PASSWORD_PAGES: &[&str] = &["passwords", "пароли"];

/// Words in a browser window's title that mark a private window.
const PRIVATE_MARKERS: &[&str] = &["private browsing", "incognito", "inprivate", "инкогнито", "приватн", "частный доступ"];

/// Is this a private browser window (by the markers in its title)? Only
/// browsers: a document called "Incognito" is not a private window.
pub fn private_window(f: &Front) -> bool {
    if category_of(&f.id) != Category::Browser {
        return false;
    }
    let Some(title) = &f.title else { return false };
    let title = title.to_lowercase();
    PRIVATE_MARKERS.iter().any(|m| title.contains(m))
}

/// Is the app in front one the synth must not see: a password manager,
/// the passwords page of System Settings, a private browser window, or an
/// app the owner excluded (`own`: by id, or by name for a mark without
/// one)?
pub fn excluded(f: &Front, own: &[AppMark]) -> bool {
    let id = f.id.trim().to_lowercase();
    let name = f.name.trim().to_lowercase();
    if !id.is_empty() && EXCLUDED_IDS.iter().any(|k| under(&id, k)) {
        return true;
    }
    if EXCLUDED_NAMES.iter().any(|n| name.contains(n)) || name == "passwords" || name == "пароли" {
        return true;
    }
    if SETTINGS_IDS.contains(&id.as_str()) {
        let title = f.title.as_deref().unwrap_or("").trim().to_lowercase();
        if PASSWORD_PAGES.iter().any(|p| title == *p) {
            return true;
        }
    }
    if private_window(f) {
        return true;
    }
    own.iter().any(|m| {
        let mid = m.id.trim().to_lowercase();
        if !mid.is_empty() {
            mid == id
        } else {
            let mname = m.name.trim().to_lowercase();
            !mname.is_empty() && mname == name
        }
    })
}

/// Apps that are not an activity: the screen is locked or asleep.
const NOT_AN_ACTIVITY: &[&str] = &["com.apple.loginwindow", "com.apple.screensaver.engine", "lockapp.exe", "logonui.exe"];

/// Turns the app in front into `device.activity` values, with `since` =
/// when this app with this title came to the front.
#[derive(Debug, Default)]
pub struct ActivityTracker {
    key: Option<String>,
    since: i64,
}

impl ActivityTracker {
    /// The value for this reading at `now` (unix seconds). None while
    /// nothing is in front.
    pub fn update(&mut self, front: Option<&Front>, own: &[AppMark], now: i64) -> Option<Value> {
        let Some(f) = front.filter(|f| !NOT_AN_ACTIVITY.contains(&f.id.trim().to_lowercase().as_str())) else {
            self.key = None;
            return None;
        };
        let (app, title, category) = if excluded(f, own) {
            (PRIVATE.to_string(), None, Category::Private)
        } else {
            let mut app = clean(&f.name, MAX_APP_BYTES);
            if app.is_empty() {
                app = clean(&f.id, MAX_APP_BYTES);
            }
            if app.is_empty() {
                app = "unknown".into();
            }
            let title = f.title.as_deref().map(|t| clean(t, MAX_TITLE_BYTES)).filter(|t| !t.is_empty());
            (app, title, category_of(&f.id))
        };
        let key = format!("{app}\n{}", title.as_deref().unwrap_or(""));
        if self.key.as_deref() != Some(key.as_str()) {
            self.key = Some(key);
            self.since = now;
        }
        Some(activity_value(&app, title.as_deref(), category, Some(self.since)))
    }
}

/// One `device.activity` item.
pub fn activity_value(app: &str, title: Option<&str>, category: Category, since: Option<i64>) -> Value {
    let mut v = json!({"app": clean(app, MAX_APP_BYTES), "category": category.as_str()});
    if category != Category::Private {
        if let Some(t) = title.map(|t| clean(t, MAX_TITLE_BYTES)).filter(|t| !t.is_empty()) {
            v["title"] = json!(t);
        }
    }
    if let Some(s) = since {
        v["since"] = json!(rfc3339(s));
    }
    v
}

// --- calls ---

/// `device.call` `app`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallApp {
    Zoom,
    Meet,
    Facetime,
    Telegram,
    Whatsapp,
    Slack,
    Teams,
    Discord,
    Other,
}

impl CallApp {
    pub fn as_str(self) -> &'static str {
        match self {
            CallApp::Zoom => "zoom",
            CallApp::Meet => "meet",
            CallApp::Facetime => "facetime",
            CallApp::Telegram => "telegram",
            CallApp::Whatsapp => "whatsapp",
            CallApp::Slack => "slack",
            CallApp::Teams => "teams",
            CallApp::Discord => "discord",
            CallApp::Other => "other",
        }
    }
}

/// The call apps by id: macOS bundle ids (the app and its helpers), the
/// Windows program file names, and the Windows Store package names the
/// microphone's consent store lists. Lower case.
const CALL_APPS: &[(&str, CallApp)] = &[
    ("us.zoom", CallApp::Zoom),
    ("zoom.exe", CallApp::Zoom),
    ("com.apple.facetime", CallApp::Facetime),
    // FaceTime's (and an iPhone call's) audio runs in this daemon.
    ("com.apple.avconferenced", CallApp::Facetime),
    ("avconferenced", CallApp::Facetime),
    ("ru.keepcoder.telegram", CallApp::Telegram),
    ("org.telegram.desktop", CallApp::Telegram),
    ("telegram.exe", CallApp::Telegram),
    ("telegrammessengerllp.telegramdesktop", CallApp::Telegram),
    ("net.whatsapp.whatsapp", CallApp::Whatsapp),
    ("desktop.whatsapp", CallApp::Whatsapp),
    ("whatsapp.exe", CallApp::Whatsapp),
    ("whatsapp.root.exe", CallApp::Whatsapp),
    ("5319275a.whatsappdesktop", CallApp::Whatsapp),
    ("com.tinyspeck.slackmacgap", CallApp::Slack),
    ("slack.exe", CallApp::Slack),
    ("91750d7e.slack", CallApp::Slack),
    ("com.microsoft.teams", CallApp::Teams),
    ("com.microsoft.teams2", CallApp::Teams),
    ("ms-teams.exe", CallApp::Teams),
    ("teams.exe", CallApp::Teams),
    ("msteams", CallApp::Teams),
    ("com.hnc.discord", CallApp::Discord),
    ("discord.exe", CallApp::Discord),
];

/// Browsers by id, for Google Meet: their bundle ids (with the helpers
/// that hold the microphone: Chrome's `com.google.Chrome.helper`,
/// Safari's `com.apple.WebKit.GPU`) and their Windows file names.
const BROWSERS: &[&str] = &[
    "com.apple.safari",
    "com.apple.safaritechnologypreview",
    "com.apple.webkit",
    "com.google.chrome",
    "company.thebrowser.browser",
    "org.mozilla.firefox",
    "com.microsoft.edgemac",
    "com.brave.browser",
    "com.operasoftware.opera",
    "com.vivaldi.vivaldi",
    "org.chromium.chromium",
    "app.zen-browser.zen",
    "com.kagi.kagimacos",
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "brave.exe",
    "opera.exe",
    "vivaldi.exe",
    "arc.exe",
    "zen.exe",
];

/// Which call app this id is.
pub fn call_app_of(id: &str) -> Option<CallApp> {
    lookup(CALL_APPS, id)
}

/// Is this id a browser (or one of its helpers)?
pub fn is_browser(id: &str) -> bool {
    let id = id.trim().to_lowercase();
    BROWSERS.iter().any(|k| under(&id, k))
}

/// A Google Meet window title: "Meet - abc-defg-hij", "Meet: ...", "Google
/// Meet", or the address.
pub fn meet_title(title: &str) -> bool {
    let t = title.trim();
    let lower = t.to_lowercase();
    if lower.contains("meet.google.com") || lower.contains("google meet") {
        return true;
    }
    match t.strip_prefix("Meet") {
        Some("") => true,
        Some(rest) => rest.starts_with([' ', ':', '-', '\u{2013}']),
        None => false,
    }
}

/// One reading of the microphone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MicSample {
    /// The processes OTHER than this app that record now (ids: bundle ids,
    /// program file names, package names), where the system lists them
    /// (macOS 14 and later, Windows). None where it does not.
    pub recorders: Option<Vec<String>>,
    /// The default input device runs for some process, this app included
    /// (CoreAudio `kAudioDevicePropertyDeviceIsRunningSomewhere`). Used
    /// only without `recorders`.
    pub device_running: bool,
    /// This app has the microphone open itself (a hold of the island,
    /// wake by name listening).
    pub own_capture: bool,
    /// The call apps that run now, the one in front first.
    pub running: Vec<String>,
}

/// Reads the microphone. Never opens it, never records: only asks the
/// system whether it is in use and by whom.
pub trait MicProbe: Send {
    fn sample(&mut self) -> MicSample;
}

/// Is a call running, and in which app? `meet`: a Google Meet window was
/// in front in a browser lately.
///
///  - The microphone is in use: another process records (where the system
///    lists them), else the device runs while this app does not hold it
///    (without the list the app's own microphone cannot be told from a
///    call's, so it never counts).
///  - The app: a recording call app; else Meet, when a browser records
///    and a Meet window was seen; else, with the recorders unknown, the
///    call app that runs (the front one first).
///  - A call needs a call app running or Meet (the microphone of a
///    dictation, a voice memo or a recorder is not a call): with the
///    recorders known and none of them a call app, a running call app
///    makes it `other`.
pub fn decide_call(s: &MicSample, meet: bool) -> Option<CallApp> {
    let mic = match &s.recorders {
        Some(list) => !list.is_empty(),
        None => s.device_running && !s.own_capture,
    };
    if !mic {
        return None;
    }
    let running = s.running.iter().find_map(|id| call_app_of(id));
    match &s.recorders {
        Some(list) => {
            if let Some(app) = list.iter().find_map(|id| call_app_of(id)) {
                return Some(app);
            }
            if meet && list.iter().any(|id| is_browser(id)) {
                return Some(CallApp::Meet);
            }
            running.map(|_| CallApp::Other)
        }
        None => {
            if meet {
                return Some(CallApp::Meet);
            }
            running
        }
    }
}

/// Turns microphone readings into `device.call` values, with `since` =
/// when the call began.
#[derive(Debug, Default)]
pub struct CallTracker {
    call: Option<(CallApp, i64)>,
    meet_seen: Option<i64>,
}

impl CallTracker {
    pub fn update(&mut self, s: &MicSample, front: Option<&Front>, now: i64) -> Value {
        if let Some(f) = front {
            if is_browser(&f.id) && f.title.as_deref().is_some_and(meet_title) {
                self.meet_seen = Some(now);
            }
        }
        // A Meet call stays one for as long as it lasts, whatever is in
        // front now.
        let meet = self.meet_seen.is_some_and(|t| now - t <= MEET_MEMORY_SECS) || matches!(self.call, Some((CallApp::Meet, _)));
        self.call = match (decide_call(s, meet), self.call) {
            (None, _) => None,
            (Some(app), Some((_, since))) => Some((app, since)),
            (Some(app), None) => Some((app, now)),
        };
        call_value(self.call)
    }

    /// The call now: its app and when it began.
    pub fn current(&self) -> Option<(CallApp, i64)> {
        self.call
    }
}

/// One `device.call` item.
pub fn call_value(call: Option<(CallApp, i64)>) -> Value {
    match call {
        None => json!({"in_call": false}),
        Some((app, since)) => json!({"in_call": true, "since": rfc3339(since), "app": app.as_str()}),
    }
}

// --- the place ---

/// The Wi-Fi network as a probe reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "ssid", rename_all = "snake_case")]
pub enum Wifi {
    /// Connected to this network (its SSID). Stays on this computer.
    Network(String),
    /// No Wi-Fi network (off, not connected, a cable).
    NotConnected,
    /// The system has not asked the owner yet whether the app may know
    /// the network (macOS: Location Services).
    NotAsked,
    /// The owner (or the system) did not allow it.
    Denied,
    /// This system has no way to read it.
    Unsupported,
    /// Not read yet (the source is off).
    Unknown,
}

pub trait WifiProbe: Send {
    fn read(&mut self) -> Wifi;
}

/// The owner's name for this network: `{name, state: "present"}`, or None
/// for a network without a name, or no network at all.
pub fn place_of(wifi: &Wifi, places: &[NamedNetwork]) -> Option<Value> {
    let Wifi::Network(ssid) = wifi else { return None };
    let named = places.iter().find(|p| p.ssid == *ssid)?;
    let name = clean(&named.name, MAX_PLACE_BYTES);
    (!name.is_empty()).then(|| json!({"name": name, "state": "present"}))
}

// --- one round ---

/// Reads the probes for the sources that are on and keeps what each
/// source needs between rounds.
pub struct Sampler {
    mic: Box<dyn MicProbe>,
    front: Box<dyn FrontProbe>,
    wifi: Box<dyn WifiProbe>,
    call: CallTracker,
    activity: ActivityTracker,
    wifi_now: Wifi,
    wifi_at: Option<Instant>,
    front_now: Option<Front>,
    recent: Vec<AppMark>,
    was: BTreeSet<&'static str>,
}

impl Sampler {
    pub fn new(mic: Box<dyn MicProbe>, front: Box<dyn FrontProbe>, wifi: Box<dyn WifiProbe>) -> Sampler {
        Sampler {
            mic,
            front,
            wifi,
            call: CallTracker::default(),
            activity: ActivityTracker::default(),
            wifi_now: Wifi::Unknown,
            wifi_at: None,
            front_now: None,
            recent: Vec::new(),
            was: BTreeSet::new(),
        }
    }

    /// One round at `now` (`unix`: the same moment in unix seconds). The
    /// sources in `wanted` are read; each gives its current value (None:
    /// nothing to report). A source that was wanted in the last round and
    /// is not now gives None once and forgets its state.
    pub fn tick(&mut self, now: Instant, unix: i64, wanted: &BTreeSet<String>, settings: &ObserveSettings) -> Vec<(&'static str, Option<Value>)> {
        let mut out = Vec::new();
        let on = |r: &str| wanted.contains(r);
        let front = if on(CALL) || on(ACTIVITY) { self.front.front() } else { None };
        if let Some(f) = &front {
            self.remember(f, settings);
        }
        self.front_now = front.clone();

        if on(CALL) {
            let s = self.mic.sample();
            out.push((CALL, Some(self.call.update(&s, front.as_ref(), unix))));
        } else if self.was.contains(CALL) {
            self.call = CallTracker::default();
            out.push((CALL, None));
        }

        if on(ACTIVITY) {
            out.push((ACTIVITY, self.activity.update(front.as_ref(), &settings.excluded, unix)));
        } else if self.was.contains(ACTIVITY) {
            self.activity = ActivityTracker::default();
            out.push((ACTIVITY, None));
        }

        if on(PLACE) {
            if self.wifi_at.is_none_or(|at| now.saturating_duration_since(at) >= WIFI_EVERY) {
                self.wifi_now = self.wifi.read();
                self.wifi_at = Some(now);
            }
            out.push((PLACE, place_of(&self.wifi_now, &settings.places)));
        } else if self.was.contains(PLACE) {
            self.wifi_now = Wifi::Unknown;
            self.wifi_at = None;
            out.push((PLACE, None));
        }

        self.was = [CALL, ACTIVITY, PLACE].into_iter().filter(|r| on(r)).collect();
        out
    }

    /// Read the network at the next round (the owner just allowed it, or
    /// switched the source on).
    pub fn wifi_again(&mut self) {
        self.wifi_at = None;
    }

    /// The network as last read (`Unknown` while the place is off).
    pub fn wifi(&self) -> &Wifi {
        &self.wifi_now
    }

    /// The app in front as last read (None while the call and the
    /// activity are off).
    pub fn front(&self) -> Option<&Front> {
        self.front_now.as_ref()
    }

    /// The call as the last round saw it.
    pub fn call(&self) -> Option<(CallApp, i64)> {
        self.call.current()
    }

    /// The apps seen in front lately, newest first, without the ones
    /// already excluded (the exclusion picker).
    pub fn recent(&self) -> &[AppMark] {
        &self.recent
    }

    fn remember(&mut self, f: &Front, settings: &ObserveSettings) {
        if f.id.trim().is_empty() || excluded(f, &settings.excluded) || NOT_AN_ACTIVITY.contains(&f.id.trim().to_lowercase().as_str()) {
            return;
        }
        let mark = AppMark { id: f.id.trim().to_string(), name: clean(&f.name, MAX_APP_BYTES) };
        self.recent.retain(|m| !m.id.eq_ignore_ascii_case(&mark.id));
        self.recent.insert(0, mark);
        self.recent.truncate(RECENT_APPS);
    }
}
