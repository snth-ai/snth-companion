//! Where the owner's browsers keep their cookies, and where the key that
//! unlocks Chromium cookies comes from. This only builds the list the
//! core's `MultiReader` works from; nothing is opened or decrypted here.
//! A cookie database is read (from a copy) only when the synth asks for a
//! site's cookies and the owner's settings or answer allow it.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use connect_core::{KeySource, MultiReader};

/// Chromium-family browsers: display name (also the Keychain account on
/// macOS) and the user-data directory relative to the per-OS base.
#[cfg(target_os = "macos")]
const CHROMIUM: &[(&str, &str)] = &[
    ("Chrome", "Google/Chrome"),
    ("Arc", "Arc/User Data"),
    ("Brave", "BraveSoftware/Brave-Browser"),
    ("Microsoft Edge", "Microsoft Edge"),
    ("Vivaldi", "Vivaldi"),
    ("Chromium", "Chromium"),
];

#[cfg(all(unix, not(target_os = "macos")))]
const CHROMIUM: &[(&str, &str)] = &[
    ("Chrome", "google-chrome"),
    ("Brave", "BraveSoftware/Brave-Browser"),
    ("Microsoft Edge", "microsoft-edge"),
    ("Vivaldi", "vivaldi"),
    ("Chromium", "chromium"),
];

#[cfg(windows)]
const CHROMIUM: &[(&str, &str)] = &[
    ("Chrome", "Google/Chrome/User Data"),
    ("Brave", "BraveSoftware/Brave-Browser/User Data"),
    ("Microsoft Edge", "Microsoft/Edge/User Data"),
    ("Vivaldi", "Vivaldi/User Data"),
    ("Chromium", "Chromium/User Data"),
];

/// The base directory Chromium browsers put their user data under.
fn chromium_base() -> Option<PathBuf> {
    let dirs = directories::BaseDirs::new()?;
    #[cfg(target_os = "macos")]
    {
        Some(dirs.home_dir().join("Library/Application Support"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Some(dirs.config_dir().to_path_buf())
    }
    #[cfg(windows)]
    {
        Some(dirs.data_local_dir().to_path_buf())
    }
}

fn firefox_profiles_dir() -> Option<PathBuf> {
    let dirs = directories::BaseDirs::new()?;
    #[cfg(target_os = "macos")]
    {
        Some(dirs.home_dir().join("Library/Application Support/Firefox/Profiles"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Some(dirs.home_dir().join(".mozilla/firefox"))
    }
    #[cfg(windows)]
    {
        Some(dirs.data_dir().join("Mozilla/Firefox/Profiles"))
    }
}

/// The profile folders of one Chromium user-data dir: "Default" first,
/// then "Profile N". Lists directory names only.
fn chromium_profiles(user_data: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let default = user_data.join("Default");
    if default.is_dir() {
        out.push(default);
    }
    if let Ok(rd) = std::fs::read_dir(user_data) {
        let mut extra: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_dir()
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("Profile "))
            })
            .collect();
        extra.sort();
        out.extend(extra);
    }
    out
}

/// Cookie databases of every installed browser we know how to read.
pub fn system_reader() -> MultiReader {
    let mut chromium = Vec::new();
    if let Some(base) = chromium_base() {
        for (name, rel) in CHROMIUM {
            for profile in chromium_profiles(&base.join(rel)) {
                // Newer builds keep the file under Network/; older ones
                // next to the profile. The reader skips a missing file.
                chromium.push((name.to_string(), profile.join("Network").join("Cookies")));
                chromium.push((name.to_string(), profile.join("Cookies")));
            }
        }
    }
    let mut firefox = Vec::new();
    if let Some(dir) = firefox_profiles_dir() {
        if let Ok(rd) = std::fs::read_dir(dir) {
            let mut profiles: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            profiles.sort();
            for p in profiles {
                firefox.push(p.join("cookies.sqlite"));
            }
        }
    }
    MultiReader {
        chromium,
        firefox,
        keys: Arc::new(SystemKeys::default()),
    }
}

/// The Chromium "Safe Storage" password from the OS.
///
/// macOS: the browser's own Keychain item ("Chrome Safe Storage"). The
/// first read makes macOS ask the owner to allow it; the answer is kept in
/// memory for the life of the process so the prompt does not repeat.
/// Linux: the fixed password Chromium uses when no keyring is involved
/// (v10 values). Values locked with the desktop keyring (v11) are skipped
/// by the reader; that is P2 work.
/// Windows: no key (Chrome encrypts per application); the reader reports
/// `unavailable`.
#[derive(Default)]
pub struct SystemKeys {
    cache: Mutex<HashMap<String, Option<Vec<u8>>>>,
}

impl KeySource for SystemKeys {
    fn safe_storage_password(&self, browser: &str) -> io::Result<Option<Vec<u8>>> {
        if let Some(hit) = self.cache.lock().unwrap().get(browser) {
            return Ok(hit.clone());
        }
        let key = read_safe_storage(browser)?;
        self.cache
            .lock()
            .unwrap()
            .insert(browser.to_string(), key.clone());
        Ok(key)
    }
}

#[cfg(target_os = "macos")]
fn read_safe_storage(browser: &str) -> io::Result<Option<Vec<u8>>> {
    let entry = keyring::Entry::new(&format!("{browser} Safe Storage"), browser)
        .map_err(io::Error::other)?;
    match entry.get_password() {
        Ok(p) => Ok(Some(p.into_bytes())),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(io::Error::other(e)),
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn read_safe_storage(_browser: &str) -> io::Result<Option<Vec<u8>>> {
    Ok(Some(b"peanuts".to_vec()))
}

#[cfg(windows)]
fn read_safe_storage(_browser: &str) -> io::Result<Option<Vec<u8>>> {
    Ok(None)
}
