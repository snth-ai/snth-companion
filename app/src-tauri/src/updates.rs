//! Automatic updates.
//!
//! The app checks the release feed shortly after launch and then about
//! every six hours. With "Update automatically" on (the default) a new
//! version is downloaded and verified in the background, and put in place
//! at a moment that disturbs nobody:
//!
//!  - nothing is in flight (the synth has no stream open, no request is
//!    waiting for an answer, the helper is not being installed), AND
//!  - the owner has been away from the computer for a few minutes, or the
//!    app was started hidden (at login) and its window was never opened.
//!
//! Until such a moment comes the downloaded update waits. It is also
//! applied when the owner quits the app. If neither happens (a crash, a
//! shutdown), the next launch simply finds the update again.
//!
//! After the switch the app comes back the way it was: in the tray only
//! if the window was hidden, paused if the owner had paused it, and the
//! link reconnects by itself.
//!
//! With the setting off the app only says that a version is available;
//! the button installs it.
//!
//! What can go wrong, and what happens then:
//!
//!  - Offline, the feed is unreachable, a download breaks: nothing is
//!    shown, the next tick tries again. (A check the owner started with
//!    the button does report the failure.)
//!  - The app cannot replace itself where it runs (see `Block`): no
//!    download is attempted; the UI says what to do.
//!  - The install fails: on macOS the app first makes a safety copy of
//!    itself; if the new bundle is missing or does not pass `codesign
//!    --verify` afterwards, the copy is put back, so the old version keeps
//!    working. The update is tried again at the next scheduled check.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use connect_core::{Entry, InstallState};
use serde::Serialize;
use tauri::Manager;
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::watch;

use crate::state::Ctx;
use crate::ui::{self, UiEvent};
use crate::windows;

/// First check this long after launch (plus up to 20 s), so startup and
/// the first link are not competing with it.
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(25);
/// Then every six hours, give or take twenty minutes.
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
const CHECK_JITTER_SECS: u64 = 20 * 60;
/// After a failed download or install: try again this much later.
const RETRY_AFTER: Duration = Duration::from_secs(20 * 60);
/// How often the loop looks for a quiet moment to apply a ready update.
const BEAT: Duration = Duration::from_secs(30);
/// "Away": no mouse or keyboard for this long.
const AWAY_SECONDS: u64 = 180;

/// Why this copy of the app cannot replace itself. (Each OS can only run
/// into some of these, hence the allow.)
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Block {
    /// macOS runs the app from a temporary read-only copy ("App
    /// Translocation"): it was opened straight from the download without
    /// being moved first.
    Translocated,
    /// The folder the app is in cannot be written by this user (a disk
    /// image, a read-only volume, an Applications copy owned by someone
    /// else).
    ReadOnly,
    /// Linux: installed as a system package (.deb / .rpm). The package
    /// manager owns the files; only the AppImage updates itself.
    Package,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Phase {
    /// Nothing to do (up to date, or not checked yet).
    Idle,
    Checking,
    Downloading { version: String, received: u64, total: Option<u64> },
    /// Downloaded and verified; waiting for a quiet moment (or the button).
    Ready { version: String, notes: Option<String> },
    /// A newer version exists but is not downloaded: the setting is off,
    /// or this copy cannot replace itself.
    Available { version: String, notes: Option<String> },
    Applying { version: String },
    /// `step`: "check" | "download" | "install". Only a check the owner
    /// asked for and a failed install are ever shown this way.
    Failed { version: Option<String>, step: &'static str, detail: String },
}

/// What the UI shows about updates.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateStatus {
    #[serde(flatten)]
    pub phase: Phase,
    /// The "Update automatically" setting.
    pub auto: bool,
    /// False in a build without the updater key (and in debug builds).
    pub configured: bool,
    pub blocked: Option<Block>,
    /// Unix seconds of the last successful check.
    pub last_checked: Option<i64>,
    /// This run is the first after an update from that version.
    pub updated_from: Option<String>,
}

struct Downloaded {
    update: Update,
    bytes: Vec<u8>,
}

pub struct Updates {
    phase: Mutex<Phase>,
    /// The update the feed announced (kept to download or install later).
    announced: Mutex<Option<Update>>,
    downloaded: Mutex<Option<Downloaded>>,
    /// One check / download / install at a time.
    gate: tokio::sync::Mutex<()>,
    next_check: Mutex<Instant>,
    last_checked: Mutex<Option<i64>>,
    pub configured: bool,
    pub blocked: Option<Block>,
    pub updated_from: Option<String>,
    /// The main window was shown at least once in this run.
    pub window_was_shown: AtomicBool,
    /// The app was started hidden (at login, or back from an update).
    pub started_hidden: AtomicBool,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A few seconds of spread from the clock, so a fleet of installs does not
/// hit the feed in the same second. Not a security value.
fn jitter(max_secs: u64) -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    Duration::from_secs(nanos % (max_secs + 1))
}

impl Updates {
    pub fn new(configured: bool, updated_from: Option<String>) -> Updates {
        // A debug build is never a release: it does not update itself.
        let configured = configured && !cfg!(debug_assertions);
        Updates {
            phase: Mutex::new(Phase::Idle),
            announced: Mutex::new(None),
            downloaded: Mutex::new(None),
            gate: tokio::sync::Mutex::new(()),
            next_check: Mutex::new(Instant::now() + FIRST_CHECK_AFTER + jitter(20)),
            last_checked: Mutex::new(None),
            configured,
            blocked: if configured { detect_block() } else { None },
            updated_from,
            window_was_shown: AtomicBool::new(false),
            started_hidden: AtomicBool::new(false),
        }
    }

    pub fn status(&self, auto: bool) -> UpdateStatus {
        UpdateStatus {
            phase: self.phase.lock().unwrap().clone(),
            auto,
            configured: self.configured,
            blocked: self.blocked,
            last_checked: *self.last_checked.lock().unwrap(),
            updated_from: self.updated_from.clone(),
        }
    }

    fn has_download(&self) -> bool {
        self.downloaded.lock().unwrap().is_some()
    }
}

fn auto(ctx: &Ctx) -> bool {
    ctx.prefs.lock().unwrap().auto_update
}

fn set_phase(ctx: &Ctx, phase: Phase) {
    *ctx.updates.phase.lock().unwrap() = phase;
    ui::emit(&ctx.app, UiEvent::Update { status: ctx.updates.status(auto(ctx)) });
}

// --- where the app runs ---

/// The thing the updater replaces: the `.app` bundle on macOS, the
/// AppImage file on Linux. None when the app is not running from one (a
/// development run).
fn install_target() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let exe = tauri::utils::platform::current_exe().ok()?;
        let bundle = exe.parent()?.parent()?.parent()?.to_path_buf();
        (bundle.extension().and_then(|e| e.to_str()) == Some("app")).then_some(bundle)
    }
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("APPIMAGE").map(PathBuf::from)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        tauri::utils::platform::current_exe().ok()
    }
}

/// May this user write there? `access(2)` answers for the real user and
/// knows about read-only volumes and ACLs, without touching anything.
#[cfg(unix)]
fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// Is a path inside macOS's App Translocation area?
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn is_translocated(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "AppTranslocation")
}

fn detect_block() -> Option<Block> {
    #[cfg(target_os = "macos")]
    {
        let bundle = install_target()?;
        if is_translocated(&bundle) {
            return Some(Block::Translocated);
        }
        // Replacing the bundle means renaming it out of its folder and a
        // new one in: both the folder and the bundle must be ours to change.
        let parent_ok = bundle.parent().is_some_and(writable);
        if !parent_ok || !writable(&bundle) {
            return Some(Block::ReadOnly);
        }
        None
    }
    #[cfg(target_os = "linux")]
    {
        use tauri::utils::config::BundleType;
        match tauri::utils::platform::bundle_type() {
            Some(BundleType::Deb) | Some(BundleType::Rpm) => return Some(Block::Package),
            _ => {}
        }
        match install_target() {
            Some(appimage) if appimage.parent().is_some_and(writable) && writable(&appimage) => None,
            Some(_) => Some(Block::ReadOnly),
            // Not an AppImage either (run from a build folder).
            None => Some(Block::Package),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

// --- checking and downloading ---

/// Ask the feed. `by_owner`: the "Check now" button, whose failures are
/// shown; the scheduled checks fail silently.
pub async fn check(ctx: &Arc<Ctx>, by_owner: bool) {
    let up = &ctx.updates;
    if !up.configured {
        return;
    }
    let _one = up.gate.lock().await;
    *up.next_check.lock().unwrap() = Instant::now() + CHECK_EVERY - Duration::from_secs(CHECK_JITTER_SECS) + jitter(2 * CHECK_JITTER_SECS);

    let before = up.phase.lock().unwrap().clone();
    // A downloaded update stays "ready" while the feed is asked again.
    let show_checking = !matches!(before, Phase::Ready { .. } | Phase::Downloading { .. } | Phase::Applying { .. });
    if show_checking {
        set_phase(ctx, Phase::Checking);
    }
    let found = match ctx.app.updater() {
        Ok(updater) => updater.check().await.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    match found {
        Err(detail) => {
            if by_owner {
                set_phase(ctx, Phase::Failed { version: None, step: "check", detail });
            } else if show_checking {
                // Offline or a feed hiccup: say nothing, try at the next tick.
                set_phase(ctx, if matches!(before, Phase::Checking) { Phase::Idle } else { before });
            }
        }
        Ok(None) => {
            *up.last_checked.lock().unwrap() = Some(now_unix());
            *up.announced.lock().unwrap() = None;
            *up.downloaded.lock().unwrap() = None;
            set_phase(ctx, Phase::Idle);
        }
        Ok(Some(update)) => {
            *up.last_checked.lock().unwrap() = Some(now_unix());
            let same_as_downloaded = up
                .downloaded
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|d| d.update.version == update.version);
            if same_as_downloaded {
                return;
            }
            // Something newer than what was downloaded, or nothing yet.
            *up.downloaded.lock().unwrap() = None;
            let (version, notes) = (update.version.clone(), update.body.clone());
            *up.announced.lock().unwrap() = Some(update);
            set_phase(ctx, Phase::Available { version, notes });
        }
    }
}

/// Download and verify the announced update (the plugin checks the
/// signature against the public key in tauri.conf.json). Ok(true) = ready.
async fn download(ctx: &Arc<Ctx>, by_owner: bool) -> bool {
    let up = &ctx.updates;
    let _one = up.gate.lock().await;
    if up.has_download() {
        return true;
    }
    let Some(update) = up.announced.lock().unwrap().clone() else {
        return false;
    };
    let version = update.version.clone();
    let notes = update.body.clone();
    set_phase(ctx, Phase::Downloading { version: version.clone(), received: 0, total: None });

    let progress_ctx = ctx.clone();
    let progress_version = version.clone();
    let mut received = 0u64;
    let mut shown = 0u64;
    let result = update
        .download(
            move |chunk, total| {
                received += chunk as u64;
                // A progress line needs a few updates a second, not one per chunk.
                if received - shown >= 512 * 1024 || Some(received) == total {
                    shown = received;
                    set_phase(
                        &progress_ctx,
                        Phase::Downloading { version: progress_version.clone(), received, total },
                    );
                }
            },
            || {},
        )
        .await;
    match result {
        Ok(bytes) => {
            *up.downloaded.lock().unwrap() = Some(Downloaded { update, bytes });
            set_phase(ctx, Phase::Ready { version, notes });
            true
        }
        Err(e) => {
            *up.next_check.lock().unwrap() = Instant::now() + RETRY_AFTER + jitter(120);
            if by_owner {
                set_phase(ctx, Phase::Failed { version: Some(version), step: "download", detail: e.to_string() });
            } else {
                // Silent: it is still "available", and tried again later.
                set_phase(ctx, Phase::Available { version, notes });
            }
            false
        }
    }
}

// --- applying ---

/// Is this a moment at which a restart disturbs nobody?
fn quiet_moment(ctx: &Ctx) -> bool {
    if !nothing_in_flight(ctx) {
        return false;
    }
    let away = connect_core::presence::read().idle_seconds >= AWAY_SECONDS;
    let never_opened = ctx.updates.started_hidden.load(Ordering::Acquire)
        && !ctx.updates.window_was_shown.load(Ordering::Acquire);
    away || never_opened
}

fn nothing_in_flight(ctx: &Ctx) -> bool {
    let helper_busy = matches!(
        ctx.core.cua_helper_state(),
        InstallState::Checking | InstallState::Downloading { .. } | InstallState::Installing | InstallState::Verifying
    );
    ctx.core.active_streams() == 0 && ctx.approvals.is_empty() && !helper_busy
}

/// How an install was asked for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Apply {
    /// By itself at a quiet moment: restart into the new version. Given up
    /// if something came into flight in the meantime.
    Quiet,
    /// The owner clicked the button: restart now.
    ByOwner,
    /// The owner is quitting: install, do not come back.
    AtQuit,
}

/// Put the downloaded update in place. On success with a restart this
/// does not return (the process exits). Err = nothing changed, the
/// current version keeps running.
pub async fn apply(ctx: &Arc<Ctx>, how: Apply) -> Result<(), String> {
    let up = &ctx.updates;
    if let Some(block) = up.blocked {
        return Err(format!("blocked: {block:?}"));
    }
    let _one = up.gate.lock().await;
    let Some(Downloaded { update, bytes }) = up.downloaded.lock().unwrap().take() else {
        return Err("no downloaded update".into());
    };
    let version = update.version.clone();
    let notes = update.body.clone();
    let put_back = |ctx: &Arc<Ctx>, update: Update, bytes: Vec<u8>| {
        *ctx.updates.downloaded.lock().unwrap() = Some(Downloaded { update, bytes });
        set_phase(ctx, Phase::Ready { version: version.clone(), notes: notes.clone() });
    };

    // On its own the app never cuts work short: look once more, right
    // before the point of no return.
    if how == Apply::Quiet && !nothing_in_flight(ctx) {
        put_back(ctx, update, bytes);
        return Err("busy".into());
    }
    // From here on the synth's side is closed: new streams are refused
    // and our helper daemon is stopped. (At quit that is already done.)
    let owner_paused = ctx.prefs.lock().unwrap().paused;
    if how != Apply::AtQuit {
        ctx.core.pause().await;
    }
    set_phase(ctx, Phase::Applying { version: version.clone() });

    let relaunch = how != Apply::AtQuit;
    if relaunch {
        // Come back the way it was: tray only if the window is hidden now.
        let hidden = !ctx
            .app
            .get_webview_window(windows::MAIN)
            .and_then(|w| w.is_visible().ok())
            .unwrap_or(false);
        ctx.update_prefs(|p| p.resume_hidden = Some(hidden));
    }
    ctx.core.flush();

    let target = install_target();
    let backup_root = ctx.app.path().app_data_dir().ok().map(|d| d.join("update"));
    // Windows: the installer restarts the app unless told not to, and
    // `install` itself ends this process.
    let update = update.restart_after_install(relaunch);
    let installing = update.clone();
    let payload = bytes.clone();
    let installed = tauri::async_runtime::spawn_blocking(move || {
        install_guarded(&installing, &payload, target.as_deref(), backup_root.as_deref())
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);

    if let Err(detail) = installed {
        // The old version is intact (or was put back): carry on with it.
        ctx.update_prefs(|p| p.resume_hidden = None);
        if how != Apply::AtQuit && !owner_paused {
            ctx.core.resume();
        }
        *up.next_check.lock().unwrap() = Instant::now() + RETRY_AFTER + jitter(120);
        set_phase(ctx, Phase::Failed { version: Some(version), step: "install", detail: detail.clone() });
        return Err(detail);
    }

    if relaunch {
        schedule_relaunch();
        crate::actions::exit(ctx).await;
    }
    Ok(())
}

/// Run the plugin's install with a way back.
///
/// The plugin moves the running bundle into a temp dir and the new one
/// into its place; if that second move fails the temp dir (and the app
/// with it) is deleted. So on macOS a copy of the current bundle is made
/// first (a clone on APFS: instant, no space), the result is checked, and
/// the copy is put back if the app is not whole afterwards.
fn install_guarded(update: &Update, bytes: &[u8], target: Option<&Path>, backup_root: Option<&Path>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let (Some(bundle), Some(root)) = (target, backup_root) else {
            return Err("the app is not running from an app bundle".into());
        };
        // Not named *.app, so the system never treats the copy as an app.
        let backup = root.join("previous-version");
        let _ = std::fs::remove_dir_all(&backup);
        std::fs::create_dir_all(root).map_err(|e| format!("safety copy: {e}"))?;
        let copied = ["-Rcp", "-Rp"].iter().any(|flags| {
            let _ = std::fs::remove_dir_all(&backup);
            std::process::Command::new("/bin/cp")
                .arg(flags)
                .arg(bundle)
                .arg(&backup)
                .status()
                .is_ok_and(|s| s.success())
        });
        if !copied {
            let _ = std::fs::remove_dir_all(&backup);
            return Err("could not make a safety copy of the app".into());
        }

        let result = update.install(bytes).map_err(|e| e.to_string()).and_then(|()| bundle_is_whole(bundle));
        if result.is_err() && bundle_is_whole(bundle).is_err() {
            // Put the previous version back where it was.
            let _ = std::fs::remove_dir_all(bundle);
            if std::fs::rename(&backup, bundle).is_err() {
                let _ = std::process::Command::new("/bin/cp").arg("-Rp").arg(&backup).arg(bundle).status();
            }
        }
        let _ = std::fs::remove_dir_all(&backup);
        result
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Linux: the plugin keeps its own backup of the AppImage and
        // restores it on failure. Windows: the installer does the work.
        let _ = (target, backup_root);
        update.install(bytes).map_err(|e| e.to_string())
    }
}

/// The bundle has its program and its signature holds.
#[cfg(target_os = "macos")]
fn bundle_is_whole(bundle: &Path) -> Result<(), String> {
    let macos_dir = bundle.join("Contents").join("MacOS");
    let has_program = std::fs::read_dir(&macos_dir).is_ok_and(|mut rd| rd.any(|e| e.is_ok_and(|e| e.path().is_file())));
    if !has_program {
        return Err("the installed app has no program in it".into());
    }
    let verified = std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(bundle)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if verified {
        Ok(())
    } else {
        Err("the installed app did not pass the signature check".into())
    }
}

/// Start the app again once this process is gone.
///
/// A small shell waits for our pid to exit and then opens the app the way
/// the system does (`open` on macOS), so the new process is launched by
/// the system and not as a child of the old one: macOS ties the app's
/// permissions to who launched it, and the single-instance check must not
/// find the old process still there.
fn schedule_relaunch() {
    #[cfg(unix)]
    {
        let Some(target) = install_target() else {
            return;
        };
        let launch = if cfg!(target_os = "macos") { "exec /usr/bin/open \"$2\"" } else { "exec \"$2\"" };
        let script = format!(
            "i=0; while kill -0 \"$1\" 2>/dev/null && [ \"$i\" -lt 150 ]; do sleep 0.2; i=$((i+1)); done; {launch}"
        );
        let _ = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .arg("snth-connect-relaunch")
            .arg(std::process::id().to_string())
            .arg(target)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    // Windows: the installer relaunches the app (restart_after_install).
}

// --- what the owner can ask for ---

/// "Check now".
pub async fn check_by_owner(ctx: &Arc<Ctx>) {
    check(ctx, true).await;
    // With the setting on, a found update is downloaded right away.
    if auto(ctx) && ctx.updates.blocked.is_none() && matches!(*ctx.updates.phase.lock().unwrap(), Phase::Available { .. }) {
        download(ctx, true).await;
    }
}

/// The setting was just switched on: look for an update without fuss
/// and fetch it if there is one.
pub async fn fetch_if_wanted(ctx: &Arc<Ctx>) {
    if !ctx.updates.configured || ctx.updates.blocked.is_some() {
        return;
    }
    if !matches!(*ctx.updates.phase.lock().unwrap(), Phase::Available { .. } | Phase::Ready { .. }) {
        check(ctx, false).await;
    }
    if auto(ctx) && matches!(*ctx.updates.phase.lock().unwrap(), Phase::Available { .. }) {
        download(ctx, false).await;
    }
}

/// "Install and restart" / "Restart now".
pub async fn install_by_owner(ctx: &Arc<Ctx>) -> Result<(), String> {
    if let Some(block) = ctx.updates.blocked {
        return Err(format!("blocked: {block:?}"));
    }
    if !ctx.updates.has_download() && !download(ctx, true).await {
        return Err("download failed".into());
    }
    apply(ctx, Apply::ByOwner).await
}

/// The owner is quitting: use the moment. Never restarts the app.
pub async fn apply_at_quit(ctx: &Arc<Ctx>) {
    if ctx.updates.configured && ctx.updates.blocked.is_none() && auto(ctx) && ctx.updates.has_download() {
        let _ = apply(ctx, Apply::AtQuit).await;
    }
}

/// The background loop: scheduled checks, downloads, and the wait for a
/// quiet moment.
pub fn spawn(ctx: Arc<Ctx>, mut shutdown: watch::Receiver<bool>) {
    if !ctx.updates.configured {
        return;
    }
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(BEAT) => {}
                _ = shutdown.changed() => break,
            }
            if *shutdown.borrow() {
                break;
            }
            let up = &ctx.updates;
            if Instant::now() >= *up.next_check.lock().unwrap() {
                check(&ctx, false).await;
            }
            if !auto(&ctx) || up.blocked.is_some() {
                continue;
            }
            let phase = up.phase.lock().unwrap().clone();
            match phase {
                // Found (or the setting was just switched on): fetch it.
                // After a failed download this waits for the next check.
                Phase::Available { .. } if Instant::now() + CHECK_EVERY / 2 < *up.next_check.lock().unwrap() => {
                    download(&ctx, false).await;
                }
                Phase::Ready { .. } if quiet_moment(&ctx) => {
                    let _ = apply(&ctx, Apply::Quiet).await;
                }
                _ => {}
            }
        }
    });
}

/// First run after an update: one line in the activity journal, and the
/// version is remembered for the next comparison. Returns the version the
/// app was updated from, if this is such a run.
pub fn note_version(ctx_prefs: &mut crate::prefs::Prefs, current: &str, journal: &connect_core::Journal) -> Option<String> {
    let previous = ctx_prefs.last_version.replace(current.to_string());
    match previous {
        Some(prev) if prev != current => {
            journal.record(Entry::new("app", "update", current.to_string(), "ok"));
            Some(prev)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translocated_paths_are_recognized() {
        assert!(is_translocated(Path::new(
            "/private/var/folders/ab/xyz/T/AppTranslocation/6F2A1C0E-1111-2222-3333-444455556666/d/SNTH Connect.app"
        )));
        assert!(!is_translocated(Path::new("/Applications/SNTH Connect.app")));
        assert!(!is_translocated(Path::new("/Users/someone/Downloads/SNTH Connect.app")));
    }

    #[cfg(unix)]
    #[test]
    fn writability_is_read_without_touching_anything() {
        let dir = std::env::temp_dir().join(format!("snth-connect-writable-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(writable(&dir));
        assert!(!writable(Path::new("/System")));
        assert!(!writable(&dir.join("does-not-exist")));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "the check leaves nothing behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_first_run_of_a_new_version_is_journaled_once() {
        let journal = connect_core::Journal::new(None);
        let mut prefs = crate::prefs::Prefs::default();
        // A fresh install: nothing to report.
        assert_eq!(note_version(&mut prefs, "0.1.0", &journal), None);
        assert!(journal.recent().is_empty());
        // Same version again.
        assert_eq!(note_version(&mut prefs, "0.1.0", &journal), None);
        // The next launch runs 0.2.0.
        assert_eq!(note_version(&mut prefs, "0.2.0", &journal).as_deref(), Some("0.1.0"));
        let entries = journal.recent();
        assert_eq!(entries.len(), 1);
        assert_eq!((entries[0].capability.as_str(), entries[0].action.as_str(), entries[0].target.as_str()), ("app", "update", "0.2.0"));
        assert_eq!(note_version(&mut prefs, "0.2.0", &journal), None);
        assert_eq!(journal.recent().len(), 1);
    }

    #[test]
    fn a_debug_build_never_updates_itself() {
        let up = Updates::new(true, None);
        assert_eq!(up.configured, !cfg!(debug_assertions));
        let off = Updates::new(false, None);
        assert!(!off.configured && off.blocked.is_none());
    }

    #[test]
    fn the_status_carries_the_phase_inline() {
        let up = Updates::new(false, Some("0.1.0".into()));
        *up.phase.lock().unwrap() = Phase::Downloading { version: "0.2.0".into(), received: 5, total: Some(10) };
        let json = serde_json::to_value(up.status(true)).unwrap();
        assert_eq!(json["phase"], "downloading");
        assert_eq!(json["version"], "0.2.0");
        assert_eq!(json["received"], 5);
        assert_eq!(json["auto"], true);
        assert_eq!(json["updated_from"], "0.1.0");
        assert!(json["blocked"].is_null());
    }
}
