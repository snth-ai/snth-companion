//! Telling the browsers where the helper is: the native messaging host
//! manifest, per user, for the browsers that are installed.
//!
//! macOS and Linux: one small JSON file in each browser's own
//! `NativeMessagingHosts` folder under the user's home.
//! Windows: one JSON file in the app's data folder, and for each browser a
//! key under HKEY_CURRENT_USER whose default value is the path of that
//! file.
//!
//! Nothing here needs administrator rights and nothing is written for a
//! browser that is not on the computer. The app calls `install` at every
//! start, so the manifests follow the app when it is updated or moved,
//! and `remove` when the computer is unlinked.
//!
//! The manifest names the extensions that may start the helper
//! (`allowed_origins`); the browser enforces that list, and the helper
//! checks the same list again for itself.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::json;

/// A Chromium-family browser we register the helper with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Browser {
    /// Stable short id: "chrome", "edge", "brave", "chromium".
    pub id: &'static str,
    /// The name people know it by.
    pub name: &'static str,
    /// Its data folder, relative to the per-system base.
    data_dir: &'static str,
    /// Windows: its key under HKCU\Software.
    #[cfg_attr(not(windows), allow(dead_code))]
    registry: &'static str,
}

#[cfg(target_os = "macos")]
pub const BROWSERS: &[Browser] = &[
    Browser { id: "chrome", name: "Chrome", data_dir: "Google/Chrome", registry: "" },
    Browser { id: "edge", name: "Microsoft Edge", data_dir: "Microsoft Edge", registry: "" },
    Browser { id: "brave", name: "Brave", data_dir: "BraveSoftware/Brave-Browser", registry: "" },
    Browser { id: "chromium", name: "Chromium", data_dir: "Chromium", registry: "" },
];

#[cfg(all(unix, not(target_os = "macos")))]
pub const BROWSERS: &[Browser] = &[
    Browser { id: "chrome", name: "Chrome", data_dir: "google-chrome", registry: "" },
    Browser { id: "edge", name: "Microsoft Edge", data_dir: "microsoft-edge", registry: "" },
    Browser { id: "brave", name: "Brave", data_dir: "BraveSoftware/Brave-Browser", registry: "" },
    Browser { id: "chromium", name: "Chromium", data_dir: "chromium", registry: "" },
];

#[cfg(windows)]
pub const BROWSERS: &[Browser] = &[
    Browser { id: "chrome", name: "Chrome", data_dir: r"Google\Chrome\User Data", registry: r"Software\Google\Chrome" },
    Browser { id: "edge", name: "Microsoft Edge", data_dir: r"Microsoft\Edge\User Data", registry: r"Software\Microsoft\Edge" },
    Browser {
        id: "brave",
        name: "Brave",
        data_dir: r"BraveSoftware\Brave-Browser\User Data",
        registry: r"Software\BraveSoftware\Brave-Browser",
    },
    Browser { id: "chromium", name: "Chromium", data_dir: r"Chromium\User Data", registry: r"Software\Chromium" },
];

/// The browser with this id, if we know it.
pub fn browser(id: &str) -> Option<Browser> {
    BROWSERS.iter().copied().find(|b| b.id == id)
}

/// The manifest as text. `helper` must be absolute: the browsers refuse a
/// relative path on macOS and Linux.
pub fn manifest_json(host_name: &str, helper: &Path, extension_ids: &[&str]) -> String {
    let origins: Vec<String> = extension_ids.iter().map(|id| crate::origin_of(id)).collect();
    let mut text = serde_json::to_string_pretty(&json!({
        "name": host_name,
        "description": "SNTH Connect: the link to the SNTH Vault browser extension",
        "path": helper.to_string_lossy(),
        "type": "stdio",
        "allowed_origins": origins,
    }))
    .expect("a manifest is always serializable");
    text.push('\n');
    text
}

/// Write `<dir>/<host name>.json` when it is missing or says something
/// else. Returns whether the file changed.
fn write_if_changed(dir: &Path, host_name: &str, text: &str) -> io::Result<bool> {
    let file = dir.join(format!("{host_name}.json"));
    if std::fs::read_to_string(&file).is_ok_and(|have| have == text) {
        return Ok(false);
    }
    std::fs::create_dir_all(dir)?;
    // Whole file or nothing: a browser may read it at any moment.
    let tmp = dir.join(format!(".{host_name}.json.tmp"));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &file)?;
    Ok(true)
}

/// Put a manifest into one folder, whatever browser it belongs to. For a
/// throwaway browser profile in a test: Chromium on macOS and Linux reads
/// `<user data dir>/NativeMessagingHosts` when it is started with its own
/// user data dir.
pub fn write_manifest(dir: &Path, host_name: &str, helper: &Path, extension_ids: &[&str]) -> io::Result<PathBuf> {
    write_if_changed(dir, host_name, &manifest_json(host_name, helper, extension_ids))?;
    Ok(dir.join(format!("{host_name}.json")))
}

/// What `install` did for one browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Written,
    Unchanged,
}

/// Where one browser stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub browser: Browser,
    /// The browser is on this computer (it has a data folder).
    pub present: bool,
    /// The helper our manifest for it points to, when one is registered.
    pub helper: Option<PathBuf>,
}

/// Where the browsers keep their data and where the manifests go.
#[derive(Debug, Clone)]
pub struct Locations {
    /// macOS: ~/Library/Application Support. Linux: ~/.config.
    /// Windows: %LOCALAPPDATA%.
    base: PathBuf,
    /// Windows: the folder the one manifest file is kept in.
    #[cfg_attr(not(windows), allow(dead_code))]
    manifest_dir: PathBuf,
    host_name: String,
}

impl Locations {
    /// The real places of the current user. `app_data` is the app's own
    /// data folder (used on Windows for the manifest file).
    pub fn system(app_data: &Path) -> io::Result<Locations> {
        Ok(Locations {
            base: system_base()?,
            manifest_dir: app_data.join("browser"),
            host_name: crate::HOST_NAME.to_string(),
        })
    }

    /// Other places, for tests: browser data under `base`, and (Windows)
    /// the registry keys of the real browsers but under another host name,
    /// so a test never replaces what an installed app registered.
    pub fn custom(base: impl Into<PathBuf>, manifest_dir: impl Into<PathBuf>, host_name: impl Into<String>) -> Locations {
        Locations { base: base.into(), manifest_dir: manifest_dir.into(), host_name: host_name.into() }
    }

    pub fn host_name(&self) -> &str {
        &self.host_name
    }

    /// The folder that says a browser is installed for this user.
    pub fn data_dir(&self, b: &Browser) -> PathBuf {
        self.base.join(b.data_dir)
    }

    fn present(&self, b: &Browser) -> bool {
        self.data_dir(b).is_dir()
    }

    #[cfg(unix)]
    fn manifest_file(&self, b: &Browser) -> PathBuf {
        self.data_dir(b).join("NativeMessagingHosts").join(format!("{}.json", self.host_name))
    }

    #[cfg(windows)]
    fn manifest_file(&self, _b: &Browser) -> PathBuf {
        self.manifest_dir.join(format!("{}.json", self.host_name))
    }

    #[cfg(windows)]
    fn registry_key(&self, b: &Browser) -> String {
        format!(r"{}\NativeMessagingHosts\{}", b.registry, self.host_name)
    }

    /// The manifest file a browser would read for us right now.
    fn registered_file(&self, b: &Browser) -> Option<PathBuf> {
        #[cfg(unix)]
        {
            let file = self.manifest_file(b);
            file.is_file().then_some(file)
        }
        #[cfg(windows)]
        {
            crate::win::reg_get_default(&self.registry_key(b))
                .ok()
                .flatten()
                .map(PathBuf::from)
                .filter(|f| f.is_file())
        }
    }

    /// Every known browser: is it here, and where does our manifest for
    /// it point.
    pub fn status(&self) -> Vec<Status> {
        BROWSERS
            .iter()
            .map(|b| {
                let helper = self.registered_file(b).and_then(|file| {
                    let text = std::fs::read_to_string(file).ok()?;
                    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
                    v.get("path").and_then(|p| p.as_str()).map(PathBuf::from)
                });
                Status { browser: *b, present: self.present(b), helper }
            })
            .collect()
    }

    /// Register the helper with every browser that is on this computer.
    /// Safe to call at every start: it writes only what differs.
    pub fn install(&self, helper: &Path, extension_ids: &[&str]) -> Vec<(Browser, io::Result<Change>)> {
        let text = manifest_json(&self.host_name, helper, extension_ids);
        let present: Vec<&Browser> = BROWSERS.iter().filter(|b| self.present(b)).collect();
        #[cfg(unix)]
        {
            present.into_iter().map(|b| (*b, self.install_one(b, &text))).collect()
        }
        #[cfg(windows)]
        {
            if present.is_empty() {
                return Vec::new();
            }
            // One manifest file for every browser: written once, and a
            // change to it is a change for each of them.
            let file = write_if_changed(&self.manifest_dir, &self.host_name, &text);
            present
                .into_iter()
                .map(|b| {
                    let result = match &file {
                        Ok(changed) => self.register(b).map(|registered| {
                            if *changed || registered {
                                Change::Written
                            } else {
                                Change::Unchanged
                            }
                        }),
                        Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
                    };
                    (*b, result)
                })
                .collect()
        }
    }

    #[cfg(unix)]
    fn install_one(&self, b: &Browser, text: &str) -> io::Result<Change> {
        let dir = self.data_dir(b).join("NativeMessagingHosts");
        Ok(if write_if_changed(&dir, &self.host_name, text)? { Change::Written } else { Change::Unchanged })
    }

    /// Point one browser's registry key at the manifest file. Returns
    /// whether the key had to be written.
    #[cfg(windows)]
    fn register(&self, b: &Browser) -> io::Result<bool> {
        let want = self.manifest_file(b).to_string_lossy().into_owned();
        let key = self.registry_key(b);
        if crate::win::reg_get_default(&key)?.as_deref() == Some(want.as_str()) {
            return Ok(false);
        }
        crate::win::reg_set_default(&key, &want)?;
        Ok(true)
    }

    /// Take our manifest away from every browser, present or not. Returns
    /// the browsers something was removed for.
    pub fn remove(&self) -> Vec<(Browser, io::Result<()>)> {
        let mut out = Vec::new();
        for b in BROWSERS {
            #[cfg(unix)]
            {
                let file = self.manifest_file(b);
                if file.is_file() {
                    out.push((*b, std::fs::remove_file(&file)));
                }
            }
            #[cfg(windows)]
            {
                match crate::win::reg_delete(&self.registry_key(b)) {
                    Ok(true) => out.push((*b, Ok(()))),
                    Ok(false) => {}
                    Err(e) => out.push((*b, Err(e))),
                }
            }
        }
        #[cfg(windows)]
        {
            let _ = std::fs::remove_file(self.manifest_dir.join(format!("{}.json", self.host_name)));
        }
        out
    }
}

#[cfg(target_os = "macos")]
fn system_base() -> io::Result<PathBuf> {
    home().map(|h| h.join("Library/Application Support"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn system_base() -> io::Result<PathBuf> {
    match std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()).map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => Ok(dir),
        _ => home().map(|h| h.join(".config")),
    }
}

#[cfg(windows)]
fn system_base() -> io::Result<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA is not set"))
}

#[cfg(unix)]
fn home() -> io::Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .filter(|h| h.is_absolute())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no home folder (HOME is not set)"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    const ID: &str = "fmccidehlojeieenopnkfeggeghkhgpa";

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("connect-host-install-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn chrome() -> Browser {
        browser("chrome").unwrap()
    }

    #[test]
    fn the_manifest_names_the_helper_and_only_our_extension() {
        let text = manifest_json("ai.snth.connect", Path::new("/Applications/SNTH Connect.app/Contents/MacOS/snth-connect-host"), &[ID]);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["name"], "ai.snth.connect");
        assert_eq!(v["type"], "stdio");
        assert_eq!(v["path"], "/Applications/SNTH Connect.app/Contents/MacOS/snth-connect-host");
        assert_eq!(v["allowed_origins"], json!(["chrome-extension://fmccidehlojeieenopnkfeggeghkhgpa/"]));
        // No wildcard can be written here: Chromium refuses one anyway.
        assert!(!text.contains('*'));
    }

    #[test]
    fn only_browsers_that_are_here_get_a_manifest() {
        let base = temp("present");
        let loc = Locations::custom(&base, base.join("unused"), "ai.snth.connect");
        // Nothing installed: nothing written, nothing created.
        assert!(loc.install(Path::new("/opt/helper"), &[ID]).is_empty());
        assert_eq!(std::fs::read_dir(&base).unwrap().count(), 0);

        std::fs::create_dir_all(base.join(chrome().data_dir)).unwrap();
        let done = loc.install(Path::new("/opt/helper"), &[ID]);
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].0.id, "chrome");
        assert_eq!(*done[0].1.as_ref().unwrap(), Change::Written);

        let file = base.join(chrome().data_dir).join("NativeMessagingHosts/ai.snth.connect.json");
        assert!(file.is_file());
        for st in loc.status() {
            if st.browser.id == "chrome" {
                assert!(st.present);
                assert_eq!(st.helper.as_deref(), Some(Path::new("/opt/helper")));
            } else {
                assert!(!st.present && st.helper.is_none(), "{:?}", st.browser);
            }
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_second_install_writes_nothing_and_a_moved_app_is_followed() {
        let base = temp("move");
        std::fs::create_dir_all(base.join(chrome().data_dir)).unwrap();
        let loc = Locations::custom(&base, base.join("unused"), "ai.snth.connect");
        assert_eq!(*loc.install(Path::new("/old/helper"), &[ID])[0].1.as_ref().unwrap(), Change::Written);
        assert_eq!(*loc.install(Path::new("/old/helper"), &[ID])[0].1.as_ref().unwrap(), Change::Unchanged);
        // The app was updated into another place.
        assert_eq!(*loc.install(Path::new("/new/helper"), &[ID])[0].1.as_ref().unwrap(), Change::Written);
        let chrome_status = loc.status().into_iter().find(|s| s.browser.id == "chrome").unwrap();
        assert_eq!(chrome_status.helper.as_deref(), Some(Path::new("/new/helper")));
        // No temp file is left next to it.
        let dir = base.join(chrome().data_dir).join("NativeMessagingHosts");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn remove_takes_ours_away_and_leaves_other_hosts_alone() {
        let base = temp("remove");
        let dir = base.join(chrome().data_dir).join("NativeMessagingHosts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("com.other.host.json"), "{}").unwrap();
        let loc = Locations::custom(&base, base.join("unused"), "ai.snth.connect");
        loc.install(Path::new("/opt/helper"), &[ID]);
        assert!(dir.join("ai.snth.connect.json").is_file());

        let removed = loc.remove();
        assert_eq!(removed.len(), 1);
        assert!(removed[0].1.is_ok());
        assert!(!dir.join("ai.snth.connect.json").exists());
        assert!(dir.join("com.other.host.json").is_file());
        // Again: nothing left to remove, no error.
        assert!(loc.remove().is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_manifest_for_a_throwaway_profile() {
        let base = temp("profile");
        let dir = base.join("profile/NativeMessagingHosts");
        let file = write_manifest(&dir, "ai.snth.connect", Path::new("/opt/helper"), &[ID]).unwrap();
        assert_eq!(file, dir.join("ai.snth.connect.json"));
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["path"], "/opt/helper");
        let _ = std::fs::remove_dir_all(&base);
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    const ID: &str = "fmccidehlojeieenopnkfeggeghkhgpa";
    // A host name of the test's own: the keys of an installed app are
    // never touched.
    const HOST: &str = "ai.snth.connect.unittest";

    #[test]
    fn the_registry_points_the_browser_at_one_manifest_file() {
        let root = std::env::temp_dir().join(format!("connect-host-reg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let base = root.join("local");
        let loc = Locations::custom(&base, root.join("manifest"), HOST);
        let chrome = browser("chrome").unwrap();
        std::fs::create_dir_all(loc.data_dir(&chrome)).unwrap();

        let helper = root.join("app").join("snth-connect-host.exe");
        let done = loc.install(&helper, &[ID]);
        assert_eq!(done.len(), 1);
        assert_eq!(*done[0].1.as_ref().unwrap(), Change::Written);
        let key = format!(r"Software\Google\Chrome\NativeMessagingHosts\{HOST}");
        let file = crate::win::reg_get_default(&key).unwrap().expect("the key was written");
        assert_eq!(std::path::PathBuf::from(&file), root.join("manifest").join(format!("{HOST}.json")));
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["path"], helper.to_string_lossy().as_ref());
        assert_eq!(v["allowed_origins"], json!([format!("chrome-extension://{ID}/")]));

        assert_eq!(*loc.install(&helper, &[ID])[0].1.as_ref().unwrap(), Change::Unchanged);
        let st = loc.status().into_iter().find(|s| s.browser.id == "chrome").unwrap();
        assert_eq!(st.helper.as_deref(), Some(helper.as_path()));

        let removed = loc.remove();
        assert_eq!(removed.len(), 1);
        assert!(removed[0].1.is_ok());
        assert_eq!(crate::win::reg_get_default(&key).unwrap(), None);
        assert!(loc.remove().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
