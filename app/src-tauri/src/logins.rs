//! The shell's side of "Logins": telling the browsers where the helper
//! of the SNTH Vault extension is, serving the helper, and putting
//! together what the Logins page shows.
//!
//! THE RULE the page states: a password the owner shares goes to the
//! synth's vault and is used only in its cloud browser; a sign-in session
//! (cookies) goes only to this app and is used only for requests sent from
//! this computer. Nothing here holds either: the vault list is names and
//! dates, a session is a remembered permission for a site.
//!
//! The helper is registered with the browsers only while this computer is
//! linked to a synth: `sync` writes the host manifests then, follows the
//! app when it was updated or moved (it runs at every start), and takes
//! them away when the computer is unlinked. Per-user places only, and only
//! for browsers that are installed.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use connect_core::browser::{browser_name, BrowserView, VaultList, VaultSite};
use connect_host::endpoint::Endpoint;
use connect_host::install::{Locations, BROWSERS};
use serde::Serialize;
use tokio::sync::watch;

use crate::state::Ctx;
use crate::ui::{self, UiEvent};

/// The page the extension is installed from.
pub fn extension_url(hub: &str) -> String {
    format!("{}/vault/extension/", hub.trim_end_matches('/'))
}

fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

// --- the helper ---

const HELPER_NAME: &str = if cfg!(windows) { "snth-connect-host.exe" } else { "snth-connect-host" };

/// The helper that ships next to this program.
fn bundled_helper() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let helper = exe.parent()?.join(HELPER_NAME);
    helper.is_file().then_some(helper)
}

/// Will the app still be at this path after it quits? Not when it runs
/// from an AppImage (mounted for the run) or from a copy macOS made
/// because the app was opened outside Applications.
fn stays_put(path: &Path, appimage: bool) -> bool {
    !appimage && !path.to_string_lossy().contains("/AppTranslocation/")
}

/// The helper the browsers are pointed at: the one next to the app, or,
/// when the app's own place is temporary, a copy in the app's data folder
/// (kept in step with the bundled one).
fn helper_for_browsers(data_dir: &Path) -> Option<PathBuf> {
    let bundled = bundled_helper()?;
    if stays_put(&bundled, std::env::var_os("APPIMAGE").is_some()) {
        return Some(bundled);
    }
    copy_helper(&bundled, &data_dir.join("browser"))
}

fn copy_helper(bundled: &Path, dir: &Path) -> Option<PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    let dest = dir.join(HELPER_NAME);
    let same = match (std::fs::metadata(bundled), std::fs::metadata(&dest)) {
        (Ok(a), Ok(b)) => a.len() == b.len() && a.modified().ok() <= b.modified().ok(),
        _ => false,
    };
    if !same {
        let tmp = dir.join(format!(".{HELPER_NAME}.tmp"));
        std::fs::copy(bundled, &tmp).ok()?;
        std::fs::rename(&tmp, &dest).ok()?;
    }
    Some(dest)
}

/// Register the helper with the installed browsers while this computer is
/// linked to a synth, and take the registration away while it is not.
/// Cheap and safe to call again: it writes only what differs.
pub fn sync(ctx: &Ctx) {
    let Ok(locations) = Locations::system(&ctx.data_dir) else {
        return;
    };
    if !ctx.core.is_paired() {
        for (browser, result) in locations.remove() {
            if let Err(e) = result {
                connect_core::logln!("browser link: could not unregister from {}: {e}", browser.name);
            }
        }
        return;
    }
    let Some(helper) = helper_for_browsers(&ctx.data_dir) else {
        connect_core::logln!("browser link: the helper is not next to the app; nothing registered");
        return;
    };
    for (browser, result) in locations.install(&helper, connect_host::EXTENSION_IDS) {
        if let Err(e) = result {
            connect_core::logln!("browser link: could not register with {}: {e}", browser.name);
        }
    }
}

/// Serve the extension's helper for as long as the app runs. One endpoint
/// per user: a second copy of the app cannot take it.
pub fn spawn_link(ctx: std::sync::Arc<Ctx>, shutdown: watch::Receiver<bool>) {
    tauri::async_runtime::spawn(async move {
        let endpoint = match Endpoint::system() {
            Ok(e) => e,
            Err(e) => {
                *ctx.browser_link.lock().unwrap() = Some(e.to_string());
                return;
            }
        };
        if let Err(e) = ctx.core.run_browser_link(&endpoint, shutdown).await {
            connect_core::logln!("browser link: {e}");
            *ctx.browser_link.lock().unwrap() = Some(e.to_string());
            ui::emit(&ctx.app, UiEvent::State);
        }
    });
}

// --- what the page shows ---

/// One browser on the Logins page.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BrowserRow {
    /// "chrome", "edge", "brave", "chromium", or "browser" (one this app
    /// does not know by name).
    pub browser: String,
    pub name: String,
    /// The extension's id for its browser profile. None = no extension
    /// has talked to the app from this browser.
    pub install: Option<String>,
    /// "missing" (no extension heard from), "installed" (heard from, not
    /// linked) or "linked".
    pub state: &'static str,
    /// The extension is talking to the app right now.
    pub connected: bool,
    pub ext_version: String,
    /// The sites it may hand a sign-in for.
    pub sites: Vec<String>,
    pub last_seen: i64,
}

/// The rows: every extension that has talked to the app, and one "missing"
/// row for each installed browser none has talked from.
pub fn browser_rows(views: Vec<BrowserView>, installed: &[&str]) -> Vec<BrowserRow> {
    let mut rows: Vec<BrowserRow> = views
        .into_iter()
        .map(|v| BrowserRow {
            name: browser_name(&v.browser).to_string(),
            state: if v.linked { "linked" } else { "installed" },
            install: Some(v.install),
            connected: v.connected,
            ext_version: v.ext_version,
            sites: v.sites,
            last_seen: v.last_seen,
            browser: v.browser,
        })
        .collect();
    for id in installed {
        if !rows.iter().any(|r| r.browser == *id) {
            rows.push(BrowserRow {
                browser: id.to_string(),
                name: browser_name(id).to_string(),
                install: None,
                state: "missing",
                connected: false,
                ext_version: String::new(),
                sites: Vec::new(),
                last_seen: 0,
            });
        }
    }
    // The order of the known browsers, then the rest; linked first inside
    // one browser.
    let rank = |r: &BrowserRow| BROWSERS.iter().position(|b| b.id == r.browser).unwrap_or(BROWSERS.len());
    rows.sort_by(|a, b| (rank(a), a.state != "linked", &a.install).cmp(&(rank(b), b.state != "linked", &b.install)));
    rows
}

/// The browsers this computer has (by their data folders).
fn installed_browsers(data_dir: &Path) -> Vec<&'static str> {
    Locations::system(data_dir)
        .map(|l| l.status().into_iter().filter(|s| s.present).map(|s| s.browser.id).collect())
        .unwrap_or_default()
}

/// What the synth's vault holds, as far as this app knows.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum VaultView {
    /// Not asked in this run yet.
    #[default]
    Unknown,
    Ready {
        sites: Vec<VaultSite>,
        /// Browsers paired with the vault (here or anywhere else).
        devices: usize,
        fetched_at: i64,
    },
    /// The synth cannot be asked right now (no link).
    Offline,
    /// The synth is older than this feature.
    Outdated,
    /// The synth has no vault.
    Unavailable,
    Failed,
}

/// Which of the states a failed request is.
pub fn vault_failure(error: &str) -> VaultView {
    if error.contains("needs an update") || error.contains("unknown method") {
        VaultView::Outdated
    } else if error.contains("not linked") || error.contains("dropped") || error.contains("did not answer") {
        VaultView::Offline
    } else if error.contains("not set up") || error.contains("no owner configured") {
        VaultView::Unavailable
    } else {
        VaultView::Failed
    }
}

#[derive(Default)]
pub struct VaultCache(Mutex<VaultView>);

impl VaultCache {
    pub fn get(&self) -> VaultView {
        self.0.lock().unwrap().clone()
    }
    pub fn set(&self, v: VaultView) {
        *self.0.lock().unwrap() = v;
    }
    pub fn ready(&self, list: VaultList) {
        self.set(VaultView::Ready { sites: list.sites, devices: list.devices.len(), fetched_at: now_unix() });
    }
    /// One entry was removed: reflect it without another round trip.
    pub fn drop_site(&self, site: &str) {
        if let VaultView::Ready { sites, .. } = &mut *self.0.lock().unwrap() {
            sites.retain(|s| s.site != site);
        }
    }
}

/// Everything the Logins page renders from.
#[derive(Debug, Clone, Serialize)]
pub struct LoginsState {
    pub browsers: Vec<BrowserRow>,
    /// The browser link is being served. False: another copy of the app
    /// holds it, or it could not start.
    pub link_ready: bool,
    pub vault: VaultView,
}

pub fn state(ctx: &Ctx) -> LoginsState {
    LoginsState {
        browsers: browser_rows(ctx.core.browsers(), &installed_browsers(&ctx.data_dir)),
        link_ready: ctx.browser_link.lock().unwrap().is_none(),
        vault: ctx.vault.get(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(install: &str, browser: &str, linked: bool, connected: bool) -> BrowserView {
        BrowserView {
            install: install.into(),
            browser: browser.into(),
            connected,
            linked,
            ext_version: "0.2.0".into(),
            sites: vec![],
            last_seen: 5,
        }
    }

    #[test]
    fn every_installed_browser_has_a_row_and_every_extension_too() {
        let rows = browser_rows(
            vec![view("bb", "chrome", false, true), view("aa", "chrome", true, false), view("cc", "browser", false, true)],
            &["chrome", "edge"],
        );
        let got: Vec<(&str, &str, Option<&str>)> = rows.iter().map(|r| (r.browser.as_str(), r.state, r.install.as_deref())).collect();
        assert_eq!(
            got,
            [
                // Two profiles of Chrome, the linked one first.
                ("chrome", "linked", Some("aa")),
                ("chrome", "installed", Some("bb")),
                // Edge is installed, no extension has talked from it.
                ("edge", "missing", None),
                // An extension from a browser the app does not know by name.
                ("browser", "installed", Some("cc")),
            ]
        );
        assert_eq!(rows[2].name, "Microsoft Edge");
        assert_eq!(rows[3].name, "Browser");
        // No browser installed and nothing heard: nothing to show.
        assert!(browser_rows(vec![], &[]).is_empty());
    }

    #[test]
    fn a_failed_vault_request_is_told_apart() {
        assert_eq!(vault_failure("unavailable: not linked to the synth right now"), VaultView::Offline);
        assert_eq!(vault_failure("unavailable: the link to the synth dropped"), VaultView::Offline);
        assert_eq!(vault_failure("unavailable: the synth did not answer in time"), VaultView::Offline);
        assert_eq!(vault_failure("unavailable: the synth does not answer this yet (it needs an update)"), VaultView::Outdated);
        assert_eq!(vault_failure("unknown method \"vault.list\""), VaultView::Outdated);
        assert_eq!(vault_failure("unavailable: the login vault is not set up on this synth"), VaultView::Unavailable);
        assert_eq!(vault_failure("unavailable: vault list -> 502: bad gateway"), VaultView::Failed);
    }

    #[test]
    fn the_vault_cache_drops_a_removed_site() {
        let cache = VaultCache::default();
        assert_eq!(cache.get(), VaultView::Unknown);
        cache.ready(VaultList {
            sites: vec![
                VaultSite { site: "a.com".into(), has_totp: false, updated: 1, last_used: 0, use_count: 0 },
                VaultSite { site: "b.com".into(), has_totp: true, updated: 2, last_used: 3, use_count: 4 },
            ],
            devices: vec![],
        });
        cache.drop_site("a.com");
        match cache.get() {
            VaultView::Ready { sites, devices, .. } => {
                assert_eq!(sites.len(), 1);
                assert_eq!(sites[0].site, "b.com");
                assert_eq!(devices, 0);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_temporary_place_is_not_registered_with_the_browsers() {
        assert!(stays_put(Path::new("/Applications/SNTH Connect.app/Contents/MacOS/snth-connect-host"), false));
        assert!(stays_put(Path::new(r"C:\Users\x\AppData\Local\SNTH Connect\snth-connect-host.exe"), false));
        // Opened from the disk image or from Downloads: macOS runs a copy.
        assert!(!stays_put(
            Path::new("/private/var/folders/ab/T/AppTranslocation/1234/d/SNTH Connect.app/Contents/MacOS/snth-connect-host"),
            false
        ));
        // An AppImage is mounted somewhere new at every start.
        assert!(!stays_put(Path::new("/tmp/.mount_SNTHxyz/usr/bin/snth-connect-host"), true));
    }

    #[test]
    fn the_helper_copy_follows_the_bundled_one() {
        let dir = std::env::temp_dir().join(format!("snth-connect-helper-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bundled = dir.join("bundled");
        std::fs::write(&bundled, b"version one").unwrap();
        let dest = copy_helper(&bundled, &dir.join("browser")).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"version one");
        // The app was updated: the copy is replaced.
        std::fs::write(&bundled, b"version two, longer").unwrap();
        assert_eq!(copy_helper(&bundled, &dir.join("browser")).unwrap(), dest);
        assert_eq!(std::fs::read(&dest).unwrap(), b"version two, longer");
        assert_eq!(std::fs::read_dir(dir.join("browser")).unwrap().count(), 1, "no temp file is left");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_install_page_is_on_the_hub() {
        assert_eq!(extension_url("https://hub.snth.ai"), "https://hub.snth.ai/vault/extension/");
        assert_eq!(extension_url("https://hub.snth.ai/"), "https://hub.snth.ai/vault/extension/");
    }
}
