//! What the shell remembers on its own (the core keeps the settings, the
//! pairing and the grants): onboarding progress, language, autostart,
//! the stop shortcut, and whether the owner left the app paused.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// "Stop everything". Cmd+Alt+. on macOS, Ctrl+Alt+. elsewhere.
pub const DEFAULT_STOP_SHORTCUT: &str = "CommandOrControl+Alt+Period";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prefs {
    /// Onboarding finished for the current pairing.
    #[serde(default)]
    pub onboarded: bool,
    /// "en" / "ru" when the owner picked one; None = follow the OS.
    #[serde(default)]
    pub language: Option<String>,
    /// The language the UI last resolved to, so the tray is right before
    /// the webview has loaded.
    #[serde(default)]
    pub last_language: Option<String>,
    /// Launch at login (what the owner asked for; the OS entry is synced
    /// to it).
    #[serde(default)]
    pub autostart: bool,
    /// Accelerator string, empty = no shortcut.
    #[serde(default = "default_shortcut")]
    pub stop_shortcut: String,
    /// The owner paused and has not resumed: a relaunch stays paused.
    #[serde(default)]
    pub paused: bool,
    /// "Update automatically": download new versions and switch to them
    /// at a quiet moment. Off = only say that one is available.
    #[serde(default = "default_true")]
    pub auto_update: bool,
    /// The version that ran last, to notice the first run after an update.
    #[serde(default)]
    pub last_version: Option<String>,
    /// Set right before an update restart: come back hidden (tray only)
    /// or with the window, as it was. Used once at the next launch.
    #[serde(default)]
    pub resume_hidden: Option<bool>,
    /// Developer mode: shows the developer section on the General screen
    /// (the command line). Off until the owner turns it on; turning it off
    /// also turns the command line off.
    #[serde(default)]
    pub developer: bool,
}

fn default_true() -> bool {
    true
}

fn default_shortcut() -> String {
    DEFAULT_STOP_SHORTCUT.to_string()
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            onboarded: false,
            language: None,
            last_language: None,
            autostart: false,
            stop_shortcut: default_shortcut(),
            paused: false,
            auto_update: true,
            last_version: None,
            resume_hidden: None,
            developer: false,
        }
    }
}

pub struct PrefsFile {
    path: PathBuf,
}

impl PrefsFile {
    pub fn new(path: PathBuf) -> PrefsFile {
        PrefsFile { path }
    }

    /// A missing or unreadable file is a fresh install.
    pub fn load(&self) -> Prefs {
        std::fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Atomic write (temp file + rename); errors are ignored, the next
    /// change writes again.
    pub fn save(&self, prefs: &Prefs) {
        let _ = write_atomic(&self.path, prefs);
    }
}

fn write_atomic(path: &Path, prefs: &Prefs) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(prefs).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, path)
}
