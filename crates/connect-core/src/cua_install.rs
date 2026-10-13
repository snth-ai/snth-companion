//! The app's own copy of Cua Driver: fetch the pinned release, check it,
//! unpack it into the app's data dir, prove it runs, and switch to it.
//!
//! Why not the official installer (`https://cua.ai/driver/install.sh`,
//! which hands over to `_install-rust.sh`; `install.ps1` on Windows)? Read
//! at the pinned release, it cannot give the app a private copy:
//!
//!  - macOS: the app bundle always goes to `/Applications/CuaDriver.app`
//!    (`APP_DEST` is a constant; `CUA_DRIVER_RS_HOME` only moves the lock
//!    and state files). An existing bundle there is replaced.
//!  - Every OS: it stops every running driver of the user (`pkill -x
//!    cua-driver`, `launchctl unload` / `systemctl --user stop`, on
//!    Windows `taskkill`), deletes `~/.cua-driver-rs`, and may reset the
//!    driver's macOS permissions (`tccutil reset`).
//!
//! The owner's own driver must not be touched, so the app fetches the
//! release archive itself. It takes the bare-binary archive of the pinned
//! version from the same GitHub release the installer uses, and accepts it
//! only if its SHA-256 equals the value compiled into this file (the
//! installer trusts the release's `SHA256SUMS`; a hash pinned in the
//! signed app is stricter). The bare binary is what an embedding host
//! needs: the driver's embedding guide has the host spawn
//! `cua-driver serve --embedded` as a child, so macOS attributes the
//! Accessibility and Screen Recording checks to the host app, not to a
//! `CuaDriver.app` bundle (see `tcc.rs`).
//!
//! Layout under the root (`<app data>/cua`):
//!
//! ```text
//! releases/<version>/cua-driver    the copy the app runs
//! home/                            the driver's own state (CUA_DRIVER_RS_HOME)
//! tmp/                             downloads and staging, removed after
//! ```
//!
//! A new app version with a different pin looks for `releases/<new>`,
//! does not find it, installs it, and removes the older release dirs.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use crate::cua::{DriverEnv, PINNED_VERSION};
use crate::journal::{Entry, Journal};

/// Bare-binary release archives of `PINNED_VERSION` and their SHA-256,
/// from that release's SHA256SUMS: (os, arch, archive suffix, sha256).
/// `*` matches every arch (the macOS archive is a universal binary).
///
/// CHANGE TOGETHER WITH `cua::PINNED_VERSION`. A test fails when a row's
/// hash is not 64 hex characters or a supported platform has no row.
const PINNED_ARCHIVES: &[(&str, &str, &str, &str)] = &[
    (
        "macos",
        "*",
        "darwin-universal-binary.tar.gz",
        "52de49b5046df19245ad02ff9aa89ce9caf69e84988e1d0fac677611dd83e48c",
    ),
    (
        "linux",
        "x86_64",
        "linux-x86_64-binary.tar.gz",
        "a41fe2bdcbeea4501dc594c074f16b0975728e0cc7b08b90acea8564d48fafee",
    ),
    (
        "linux",
        "aarch64",
        "linux-arm64-binary.tar.gz",
        "1c839d35401bbdea20f3e1cf50ace926ef17be6f32cea7e821b02b263c4933dc",
    ),
    (
        "windows",
        "x86_64",
        "windows-x86_64-binary.zip",
        "021bc2e5330c80a6c5e2c347c305fafbd4defe195d6282e0d20436f2f13c19bb",
    ),
    (
        "windows",
        "aarch64",
        "windows-arm64-binary.zip",
        "21820a2ae20cb4bb998a340ff87c11007b9eb5cec8d1b8338c7b0d6dc88b01a6",
    ),
];

/// Files in the archive the app has no use for (the SDK library and its
/// bindings); they are most of its size.
const UNUSED_PREFIXES: &[&str] = &[
    "libcua_driver_sdk.",
    "cua_driver_sdk.",
    "cua_driver_node_runtime.",
    "cua_driver_abi.",
];

/// The whole install may take this long (a slow link moving ~50 MB).
const INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// One `--version` run, and the unpack.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
const UNPACK_TIMEOUT: Duration = Duration::from_secs(120);

/// One downloadable release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseSpec {
    pub version: String,
    pub url: String,
    pub archive_name: String,
    /// Lowercase hex.
    pub sha256: String,
}

/// The pinned release for an OS and CPU, as `std::env::consts` name them.
/// None when the driver does not ship for that platform.
pub fn release_for(os: &str, arch: &str) -> Option<ReleaseSpec> {
    let (_, _, suffix, sha256) = PINNED_ARCHIVES
        .iter()
        .find(|(o, a, _, _)| *o == os && (*a == "*" || *a == arch))?;
    let archive_name = format!("cua-driver-rs-{PINNED_VERSION}-{suffix}");
    Some(ReleaseSpec {
        version: PINNED_VERSION.to_string(),
        url: format!(
            "https://github.com/trycua/cua/releases/download/cua-driver-rs-v{PINNED_VERSION}/{archive_name}"
        ),
        archive_name,
        sha256: sha256.to_string(),
    })
}

/// The pinned release for the computer this runs on.
pub fn pinned_release() -> Option<ReleaseSpec> {
    release_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// The tar to unpack with. On Windows it is named by its full path: the
/// system one (bsdtar, which also reads .zip) must be used, and a GNU tar
/// that happens to be earlier on PATH (Git ships one) takes `C:\...` for a
/// remote host and cannot read .zip at all.
fn system_tar() -> PathBuf {
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let tar = PathBuf::from(root).join("System32").join("tar.exe");
        if tar.is_file() {
            return tar;
        }
    }
    PathBuf::from("tar")
}

fn bin_name() -> &'static str {
    if cfg!(windows) {
        "cua-driver.exe"
    } else {
        "cua-driver"
    }
}

/// Why an install did not finish, for the UI to put into plain words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailCode {
    /// No connection, or the server did not answer.
    Offline,
    /// The server answered with an error.
    Download,
    /// The file does not match the pinned hash; it was thrown away.
    Checksum,
    /// Could not write or unpack on this computer.
    Disk,
    /// The unpacked driver did not report the pinned version.
    Verify,
    /// The driver does not ship for this OS / CPU.
    Unsupported,
    /// Took longer than the time limit.
    Timeout,
    /// The app is quitting.
    Cancelled,
}

/// Where the helper stands. One of these is always current.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum InstallState {
    /// Nothing has been checked yet in this run of the app.
    Idle,
    Checking,
    Downloading {
        received: u64,
        total: Option<u64>,
    },
    Installing,
    Verifying,
    Ready {
        version: String,
    },
    /// `detail` is for diagnostics, not for the main UI.
    Failed {
        code: FailCode,
        detail: String,
    },
}

#[derive(Debug)]
pub enum FetchError {
    /// DNS, connect, TLS, a stalled read.
    Network(String),
    /// The server answered, but not with the file.
    Status(u16),
    /// Could not write the file.
    Io(String),
    Cancelled,
}

/// Downloads a URL into a file. Behind a trait so tests run offline.
pub trait Fetcher: Send + Sync {
    /// Blocking. Calls `progress(received, total)` as bytes arrive and
    /// stops with `Cancelled` soon after `cancel` turns true.
    fn fetch(
        &self,
        url: &str,
        dest: &Path,
        progress: &mut dyn FnMut(u64, Option<u64>),
        cancel: &AtomicBool,
    ) -> Result<(), FetchError>;
}

/// The real one: HTTPS GET (rustls), redirects followed.
pub struct UreqFetcher;

impl Fetcher for UreqFetcher {
    fn fetch(
        &self,
        url: &str,
        dest: &Path,
        progress: &mut dyn FnMut(u64, Option<u64>),
        cancel: &AtomicBool,
    ) -> Result<(), FetchError> {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(20))
            // A read that stalls this long counts as a lost connection.
            .timeout_read(Duration::from_secs(45))
            .user_agent("snth-connect")
            .build();
        let resp = match agent.get(url).call() {
            Ok(r) => r,
            Err(ureq::Error::Status(code, _)) => return Err(FetchError::Status(code)),
            Err(ureq::Error::Transport(t)) => return Err(FetchError::Network(t.to_string())),
        };
        let total = resp
            .header("Content-Length")
            .and_then(|v| v.parse::<u64>().ok());
        let mut reader = resp.into_reader();
        let mut file = std::fs::File::create(dest).map_err(|e| FetchError::Io(e.to_string()))?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut received = 0u64;
        progress(0, total);
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err(FetchError::Cancelled);
            }
            let n = reader
                .read(&mut buf)
                .map_err(|e| FetchError::Network(e.to_string()))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])
                .map_err(|e| FetchError::Io(e.to_string()))?;
            received += n as u64;
            progress(received, total);
        }
        file.sync_all().map_err(|e| FetchError::Io(e.to_string()))?;
        if let Some(t) = total {
            if received != t {
                return Err(FetchError::Network(format!(
                    "the download ended early ({received} of {t} bytes)"
                )));
            }
        }
        Ok(())
    }
}

type Failure = (FailCode, String);

/// Installs and updates the app's private copy. One run at a time; a
/// second caller waits for the first and takes its result.
pub struct CuaInstaller {
    root: PathBuf,
    spec: Option<ReleaseSpec>,
    /// The version whose release dir the app runs (the spec's, or the pin
    /// on a platform with no release).
    version: String,
    fetcher: Arc<dyn Fetcher>,
    journal: Arc<Journal>,
    state: watch::Sender<InstallState>,
    gate: tokio::sync::Mutex<()>,
    /// The stop flag of the run in progress.
    current: Mutex<Arc<AtomicBool>>,
    /// The app is quitting: no new run starts.
    stopped: AtomicBool,
    timeout: Duration,
}

impl CuaInstaller {
    pub fn new(
        root: PathBuf,
        spec: Option<ReleaseSpec>,
        fetcher: Arc<dyn Fetcher>,
        journal: Arc<Journal>,
    ) -> CuaInstaller {
        let version = spec
            .as_ref()
            .map(|s| s.version.clone())
            .unwrap_or_else(|| PINNED_VERSION.to_string());
        let (state, _) = watch::channel(InstallState::Idle);
        CuaInstaller {
            root,
            spec,
            version,
            fetcher,
            journal,
            state,
            gate: tokio::sync::Mutex::new(()),
            current: Mutex::new(Arc::new(AtomicBool::new(false))),
            stopped: AtomicBool::new(false),
            timeout: INSTALL_TIMEOUT,
        }
    }

    /// For tests: a shorter overall time limit.
    pub fn with_timeout(mut self, timeout: Duration) -> CuaInstaller {
        self.timeout = timeout;
        self
    }

    /// The binary the app runs. The path is fixed for an app version, so
    /// it is known before anything is installed.
    pub fn bin_path(&self) -> PathBuf {
        self.release_dir().join(bin_name())
    }

    fn releases_dir(&self) -> PathBuf {
        self.root.join("releases")
    }

    fn release_dir(&self) -> PathBuf {
        self.releases_dir().join(&self.version)
    }

    /// The environment for every process started from the private copy.
    pub fn env(&self) -> DriverEnv {
        DriverEnv {
            home: Some(self.root.join("home")),
        }
    }

    pub fn state(&self) -> InstallState {
        self.state.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<InstallState> {
        self.state.subscribe()
    }

    fn set(&self, s: InstallState) -> InstallState {
        // send_replace: the value is kept even when nobody listens yet.
        self.state.send_replace(s.clone());
        s
    }

    /// The app is quitting: stop a running install and refuse new ones.
    pub fn shutdown(&self) {
        self.stopped.store(true, Ordering::Release);
        self.current.lock().unwrap().store(true, Ordering::Release);
    }

    /// Make sure the pinned copy is in place: check it, and install it if
    /// it is missing, broken or of another version. Safe to call again
    /// after a failure. Returns the final state, and whether this call
    /// put a new copy in place.
    pub async fn ensure(&self) -> (InstallState, bool) {
        let _one_at_a_time = self.gate.lock().await;
        if self.stopped.load(Ordering::Acquire) {
            return (
                self.failed((FailCode::Cancelled, "the app is quitting".into())),
                false,
            );
        }
        self.set(InstallState::Checking);
        let bin = self.bin_path();
        if installed_version(&bin, &self.env()).await.as_deref() == Some(self.version.as_str()) {
            return (
                self.set(InstallState::Ready {
                    version: self.version.clone(),
                }),
                false,
            );
        }
        let Some(spec) = self.spec.clone() else {
            let why = format!(
                "no Cua Driver release for {} {}",
                std::env::consts::OS,
                std::env::consts::ARCH
            );
            return (self.failed((FailCode::Unsupported, why)), false);
        };

        let stop = Arc::new(AtomicBool::new(false));
        *self.current.lock().unwrap() = stop.clone();
        // Leftovers of a run that was cut short.
        let _ = std::fs::remove_dir_all(self.root.join("tmp"));
        let staging = self
            .root
            .join("tmp")
            .join(format!("{}", std::process::id()));

        let outcome =
            tokio::time::timeout(self.timeout, self.install(&spec, &staging, &stop)).await;
        // Whatever happened, stop the download thread and drop the staging.
        let timed_out = outcome.is_err();
        if timed_out {
            stop.store(true, Ordering::Release);
        }
        let _ = std::fs::remove_dir_all(self.root.join("tmp"));
        match outcome {
            Ok(Ok(())) => (
                self.set(InstallState::Ready {
                    version: spec.version,
                }),
                true,
            ),
            Ok(Err(failure)) => (self.failed(failure), false),
            Err(_) => {
                let why = format!("not finished after {} s", self.timeout.as_secs());
                (self.failed((FailCode::Timeout, why)), false)
            }
        }
    }

    fn failed(&self, (code, detail): Failure) -> InstallState {
        crate::logln!("cua helper: {code:?}: {detail}");
        self.set(InstallState::Failed { code, detail })
    }

    async fn install(
        &self,
        spec: &ReleaseSpec,
        staging: &Path,
        stop: &Arc<AtomicBool>,
    ) -> Result<(), Failure> {
        let disk = |what: &str, e: std::io::Error| (FailCode::Disk, format!("{what}: {e}"));
        let unpacked = staging.join("unpacked");
        std::fs::create_dir_all(&unpacked).map_err(|e| disk("create the staging folder", e))?;
        let archive = staging.join(&spec.archive_name);

        // 1. Download.
        self.set(InstallState::Downloading {
            received: 0,
            total: None,
        });
        let fetcher = self.fetcher.clone();
        let (url, dest, flag, state) = (
            spec.url.clone(),
            archive.clone(),
            stop.clone(),
            self.state.clone(),
        );
        let fetched = tokio::task::spawn_blocking(move || {
            let mut last = (Instant::now(), 0u64);
            let mut progress = |received: u64, total: Option<u64>| {
                // A few updates a second are plenty for a progress line.
                let done = total == Some(received);
                if !done
                    && received != 0
                    && last.0.elapsed() < Duration::from_millis(250)
                    && received - last.1 < 4 * 1024 * 1024
                {
                    return;
                }
                last = (Instant::now(), received);
                state.send_replace(InstallState::Downloading { received, total });
            };
            fetcher.fetch(&url, &dest, &mut progress, &flag)
        })
        .await
        .map_err(|e| (FailCode::Disk, format!("download task: {e}")))?;
        match fetched {
            Ok(()) => {}
            Err(FetchError::Cancelled) => return Err((FailCode::Cancelled, "stopped".into())),
            Err(FetchError::Network(e)) => return Err((FailCode::Offline, e)),
            Err(FetchError::Status(code)) => {
                return Err((
                    FailCode::Download,
                    format!("the server answered HTTP {code}"),
                ))
            }
            Err(FetchError::Io(e)) => {
                return Err((FailCode::Disk, format!("write the download: {e}")))
            }
        }

        // 2. The file must be exactly the one this app version was built for.
        let path = archive.clone();
        let actual = tokio::task::spawn_blocking(move || sha256_file(&path))
            .await
            .map_err(|e| (FailCode::Disk, format!("hash task: {e}")))?
            .map_err(|e| disk("read the download", e))?;
        if !actual.eq_ignore_ascii_case(&spec.sha256) {
            return Err((
                FailCode::Checksum,
                format!(
                    "{} has sha256 {actual}, expected {}",
                    spec.archive_name, spec.sha256
                ),
            ));
        }
        self.check_stop(stop)?;

        // 3. Unpack, with the system's tar (bsdtar on Windows 10+ reads
        //    the .zip too).
        self.set(InstallState::Installing);
        let mut tar = crate::proc::command(system_tar());
        tar.arg("-xf")
            .arg(&archive)
            .arg("-C")
            .arg(&unpacked)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let out = tokio::time::timeout(UNPACK_TIMEOUT, tar.output())
            .await
            .map_err(|_| (FailCode::Disk, "unpacking took too long".to_string()))?
            .map_err(|e| disk("run tar", e))?;
        if !out.status.success() {
            let why = String::from_utf8_lossy(&out.stderr);
            return Err((FailCode::Disk, format!("tar failed: {}", why.trim())));
        }
        let staged_bin = unpacked.join(bin_name());
        if !staged_bin.is_file() {
            return Err((FailCode::Disk, format!("the archive has no {}", bin_name())));
        }
        if let Ok(rd) = std::fs::read_dir(&unpacked) {
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if UNUSED_PREFIXES.iter().any(|p| name.starts_with(p)) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        make_executable(&staged_bin);
        self.check_stop(stop)?;

        // 4. It must run and be the pinned version before it replaces
        //    anything.
        self.set(InstallState::Verifying);
        let reported = installed_version(&staged_bin, &self.env()).await;
        if reported.as_deref() != Some(spec.version.as_str()) {
            return Err((
                FailCode::Verify,
                format!(
                    "the unpacked driver reports version {:?}, expected {}",
                    reported.unwrap_or_default(),
                    spec.version
                ),
            ));
        }
        self.check_stop(stop)?;

        // 5. Switch: one rename puts the new release dir in place.
        let releases = self.releases_dir();
        std::fs::create_dir_all(&releases).map_err(|e| disk("create the releases folder", e))?;
        let _ = std::fs::create_dir_all(self.root.join("home"));
        let target = self.release_dir();
        let others: Vec<PathBuf> = std::fs::read_dir(&releases)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p != &target)
                    .collect()
            })
            .unwrap_or_default();
        if target.exists() {
            // A broken copy of this same version (it failed the check above).
            std::fs::remove_dir_all(&target).map_err(|e| disk("remove the broken copy", e))?;
        }
        std::fs::rename(&unpacked, &target).map_err(|e| disk("move the new copy into place", e))?;
        // Older versions are of no use to this app version. Best effort:
        // on Windows a still-running old copy cannot be deleted; the next
        // install tries again.
        let updated = !others.is_empty();
        for old in others {
            let _ = std::fs::remove_dir_all(old);
        }

        // One line in the activity journal: what, which version. Nothing else.
        self.journal.record(Entry::new(
            "computer",
            if updated {
                "helper.update"
            } else {
                "helper.install"
            },
            spec.version.clone(),
            "ok",
        ));
        Ok(())
    }

    fn check_stop(&self, stop: &AtomicBool) -> Result<(), Failure> {
        if stop.load(Ordering::Acquire) {
            Err((FailCode::Cancelled, "stopped".into()))
        } else {
            Ok(())
        }
    }
}

/// `<bin> --version` -> the version it prints ("cua-driver 0.33.1"), or
/// None when it is missing, does not run, or hangs.
pub async fn installed_version(bin: &Path, env: &DriverEnv) -> Option<String> {
    if !bin.is_file() {
        return None;
    }
    let mut cmd = crate::proc::command(bin);
    env.apply(&mut cmd);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(PROBE_TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .nth(1)
        .map(|s| s.to_string())
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    use std::fmt::Write as _;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let mut hex = String::with_capacity(64);
    for byte in hasher.finalize() {
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn every_supported_platform_has_a_pinned_archive() {
        for (os, arch) in [
            ("macos", "aarch64"),
            ("macos", "x86_64"),
            ("linux", "x86_64"),
            ("linux", "aarch64"),
            ("windows", "x86_64"),
            ("windows", "aarch64"),
        ] {
            let spec =
                release_for(os, arch).unwrap_or_else(|| panic!("no archive for {os} {arch}"));
            assert_eq!(spec.version, PINNED_VERSION);
            assert_eq!(spec.sha256.len(), 64, "{os} {arch}");
            assert!(spec
                .sha256
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
            assert!(spec.url.starts_with(&format!(
                "https://github.com/trycua/cua/releases/download/cua-driver-rs-v{PINNED_VERSION}/cua-driver-rs-{PINNED_VERSION}-"
            )));
            assert!(spec.url.ends_with(&spec.archive_name));
            assert!(
                spec.archive_name.contains("-binary."),
                "the bare-binary archive, not the app bundle"
            );
        }
        assert!(release_for("freebsd", "x86_64").is_none());
        assert!(pinned_release().is_some());
    }

    #[test]
    fn sha256_of_a_known_file() {
        let dir = temp_root("sha");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("abc");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(
            sha256_file(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn temp_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("connect-cua-install-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// The real download path against a local server: progress, the
    /// length check, an error status, a dead port.
    #[test]
    fn ureq_fetcher_against_a_local_server() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let served = body.clone();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut sock, _) = listener.accept().unwrap();
                let mut line = String::new();
                let mut reader = BufReader::new(sock.try_clone().unwrap());
                reader.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap() == 0 || header == "\r\n" {
                        break;
                    }
                }
                if path == "/file" {
                    write!(
                        sock,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        served.len()
                    )
                    .unwrap();
                    sock.write_all(&served).unwrap();
                } else {
                    write!(
                        sock,
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .unwrap();
                }
            }
        });

        let dir = temp_root("fetch");
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("file");
        let never = AtomicBool::new(false);
        let mut seen = Vec::new();
        UreqFetcher
            .fetch(
                &format!("http://127.0.0.1:{port}/file"),
                &dest,
                &mut |r, t| seen.push((r, t)),
                &never,
            )
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(seen.first(), Some(&(0, Some(300_000))));
        assert_eq!(seen.last(), Some(&(300_000, Some(300_000))));

        let missing = UreqFetcher.fetch(
            &format!("http://127.0.0.1:{port}/nope"),
            &dest,
            &mut |_, _| {},
            &never,
        );
        assert!(matches!(missing, Err(FetchError::Status(404))));
        server.join().unwrap();

        // Nothing listens there any more.
        let dead = UreqFetcher.fetch(
            &format!("http://127.0.0.1:{port}/file"),
            &dest,
            &mut |_, _| {},
            &never,
        );
        assert!(matches!(dead, Err(FetchError::Network(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // The rest drives the whole pipeline with a stand-in driver (a shell
    // script that answers --version) packed with the real tar, so only
    // the network is faked.
    #[cfg(unix)]
    mod pipeline {
        use super::*;

        /// Build `<dir>/<name>` holding a fake `cua-driver` that reports
        /// `version`, plus one file the installer should drop.
        fn make_archive(dir: &Path, name: &str, version: &str) -> (PathBuf, String) {
            let src = dir.join(format!("src-{version}"));
            std::fs::create_dir_all(&src).unwrap();
            let script = format!("#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"cua-driver {version}\"; fi\n");
            std::fs::write(src.join("cua-driver"), script).unwrap();
            make_executable(&src.join("cua-driver"));
            std::fs::write(src.join("cua-cursor-theme"), b"theme").unwrap();
            std::fs::write(src.join("libcua_driver_sdk.dylib"), vec![0u8; 4096]).unwrap();
            let archive = dir.join(name);
            let ok = crate::proc::std_command("tar")
                .arg("-czf")
                .arg(&archive)
                .arg("-C")
                .arg(&src)
                .args(["cua-driver", "cua-cursor-theme", "libcua_driver_sdk.dylib"])
                .status()
                .unwrap()
                .success();
            assert!(ok);
            let sha = sha256_file(&archive).unwrap();
            (archive, sha)
        }

        /// Serves a local file as "the download", counting the calls.
        struct FileFetcher {
            source: PathBuf,
            calls: AtomicUsize,
            /// Fail this many calls first, as if offline.
            offline_first: AtomicUsize,
        }

        impl FileFetcher {
            fn new(source: PathBuf) -> Arc<FileFetcher> {
                Arc::new(FileFetcher {
                    source,
                    calls: AtomicUsize::new(0),
                    offline_first: AtomicUsize::new(0),
                })
            }
        }

        impl Fetcher for FileFetcher {
            fn fetch(
                &self,
                _url: &str,
                dest: &Path,
                progress: &mut dyn FnMut(u64, Option<u64>),
                _cancel: &AtomicBool,
            ) -> Result<(), FetchError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                if self.offline_first.load(Ordering::SeqCst) > 0 {
                    self.offline_first.fetch_sub(1, Ordering::SeqCst);
                    return Err(FetchError::Network("no route to host".into()));
                }
                let bytes =
                    std::fs::read(&self.source).map_err(|e| FetchError::Io(e.to_string()))?;
                let total = bytes.len() as u64;
                progress(0, Some(total));
                std::fs::write(dest, &bytes).map_err(|e| FetchError::Io(e.to_string()))?;
                progress(total, Some(total));
                Ok(())
            }
        }

        /// Never finishes on its own: waits for the stop flag.
        struct StuckFetcher;
        impl Fetcher for StuckFetcher {
            fn fetch(
                &self,
                _url: &str,
                _dest: &Path,
                progress: &mut dyn FnMut(u64, Option<u64>),
                cancel: &AtomicBool,
            ) -> Result<(), FetchError> {
                progress(0, Some(1000));
                while !cancel.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(FetchError::Cancelled)
            }
        }

        fn spec(version: &str, name: &str, sha: &str) -> ReleaseSpec {
            ReleaseSpec {
                version: version.into(),
                url: format!("https://example.invalid/{name}"),
                archive_name: name.into(),
                sha256: sha.into(),
            }
        }

        struct Rig {
            dir: PathBuf,
            root: PathBuf,
            journal: Arc<Journal>,
        }

        fn rig(name: &str) -> Rig {
            let dir = temp_root(name);
            std::fs::create_dir_all(&dir).unwrap();
            Rig {
                root: dir.join("cua"),
                dir,
                journal: Arc::new(Journal::new(None)),
            }
        }

        impl Drop for Rig {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }

        #[tokio::test]
        async fn installs_then_is_a_no_op() {
            let rig = rig("fresh");
            let (archive, sha) = make_archive(&rig.dir, "d.tar.gz", "9.9.9");
            let fetcher = FileFetcher::new(archive);
            let inst = CuaInstaller::new(
                rig.root.clone(),
                Some(spec("9.9.9", "d.tar.gz", &sha)),
                fetcher.clone(),
                rig.journal.clone(),
            );
            assert_eq!(inst.state(), InstallState::Idle);
            assert_eq!(inst.bin_path(), rig.root.join("releases/9.9.9/cua-driver"));

            // Record every state the UI would see.
            let mut rx = inst.subscribe();
            let seen = Arc::new(Mutex::new(Vec::new()));
            let log = seen.clone();
            let watcher = tokio::spawn(async move {
                while rx.changed().await.is_ok() {
                    log.lock().unwrap().push(rx.borrow().clone());
                }
            });

            let (state, installed) = inst.ensure().await;
            assert_eq!(
                state,
                InstallState::Ready {
                    version: "9.9.9".into()
                }
            );
            assert!(installed);
            assert!(inst.bin_path().is_file());
            assert!(rig.root.join("releases/9.9.9/cua-cursor-theme").is_file());
            assert!(
                !rig.root
                    .join("releases/9.9.9/libcua_driver_sdk.dylib")
                    .exists(),
                "the SDK library is dropped"
            );
            assert!(!rig.root.join("tmp").exists(), "staging is cleaned up");
            assert_eq!(
                installed_version(&inst.bin_path(), &inst.env())
                    .await
                    .as_deref(),
                Some("9.9.9")
            );

            // One journal line, naming only the version.
            let entries = rig.journal.recent();
            assert_eq!(entries.len(), 1);
            assert_eq!(
                (entries[0].capability.as_str(), entries[0].action.as_str()),
                ("computer", "helper.install")
            );
            assert_eq!(
                (entries[0].target.as_str(), entries[0].outcome.as_str()),
                ("9.9.9", "ok")
            );

            // Again: nothing to do, nothing downloaded, nothing journaled.
            let (state, installed) = inst.ensure().await;
            assert_eq!(
                state,
                InstallState::Ready {
                    version: "9.9.9".into()
                }
            );
            assert!(!installed);
            assert_eq!(fetcher.calls.load(Ordering::SeqCst), 1);
            assert_eq!(rig.journal.recent().len(), 1);

            drop(inst);
            watcher.await.unwrap();
            let seen = seen.lock().unwrap();
            let pos = |want: &dyn Fn(&InstallState) -> bool| seen.iter().position(want);
            let downloading =
                pos(&|s| matches!(s, InstallState::Downloading { .. })).expect("downloading");
            let ready = pos(&|s| matches!(s, InstallState::Ready { .. })).expect("ready");
            assert!(downloading < ready);
        }

        #[tokio::test]
        async fn a_wrong_file_is_refused_and_a_retry_works() {
            let rig = rig("checksum");
            let (archive, sha) = make_archive(&rig.dir, "d.tar.gz", "9.9.9");
            let (other, _) = make_archive(&rig.dir, "other.tar.gz", "6.6.6");
            let good = spec("9.9.9", "d.tar.gz", &sha);

            // The server hands out something else than the pinned file.
            let inst = CuaInstaller::new(
                rig.root.clone(),
                Some(good.clone()),
                FileFetcher::new(other),
                rig.journal.clone(),
            );
            let (state, installed) = inst.ensure().await;
            assert!(
                matches!(
                    state,
                    InstallState::Failed {
                        code: FailCode::Checksum,
                        ..
                    }
                ),
                "{state:?}"
            );
            assert!(!installed);
            assert!(!inst.bin_path().exists(), "nothing was put in place");
            assert!(!rig.root.join("tmp").exists());
            assert!(rig.journal.recent().is_empty());

            // Offline once, then fine: the same installer recovers.
            let fetcher = FileFetcher::new(archive);
            fetcher.offline_first.store(1, Ordering::SeqCst);
            let inst =
                CuaInstaller::new(rig.root.clone(), Some(good), fetcher, rig.journal.clone());
            let (state, _) = inst.ensure().await;
            assert!(
                matches!(
                    state,
                    InstallState::Failed {
                        code: FailCode::Offline,
                        ..
                    }
                ),
                "{state:?}"
            );
            let (state, installed) = inst.ensure().await;
            assert_eq!(
                state,
                InstallState::Ready {
                    version: "9.9.9".into()
                }
            );
            assert!(installed);
        }

        #[tokio::test]
        async fn a_driver_of_another_version_is_not_accepted() {
            let rig = rig("verify");
            // The archive is the pinned file, but the binary in it says 6.6.6.
            let (archive, sha) = make_archive(&rig.dir, "d.tar.gz", "6.6.6");
            let inst = CuaInstaller::new(
                rig.root.clone(),
                Some(spec("9.9.9", "d.tar.gz", &sha)),
                FileFetcher::new(archive),
                rig.journal.clone(),
            );
            let (state, _) = inst.ensure().await;
            assert!(
                matches!(
                    state,
                    InstallState::Failed {
                        code: FailCode::Verify,
                        ..
                    }
                ),
                "{state:?}"
            );
            assert!(!inst.bin_path().exists());
        }

        #[tokio::test]
        async fn a_new_pin_updates_and_drops_the_old_copy() {
            let rig = rig("update");
            let (old, old_sha) = make_archive(&rig.dir, "old.tar.gz", "1.0.0");
            let inst = CuaInstaller::new(
                rig.root.clone(),
                Some(spec("1.0.0", "old.tar.gz", &old_sha)),
                FileFetcher::new(old),
                rig.journal.clone(),
            );
            assert!(inst.ensure().await.1);
            assert!(rig.root.join("releases/1.0.0/cua-driver").is_file());

            // The next app version pins 2.0.0.
            let (new, new_sha) = make_archive(&rig.dir, "new.tar.gz", "2.0.0");
            let inst = CuaInstaller::new(
                rig.root.clone(),
                Some(spec("2.0.0", "new.tar.gz", &new_sha)),
                FileFetcher::new(new),
                rig.journal.clone(),
            );
            let (state, installed) = inst.ensure().await;
            assert_eq!(
                state,
                InstallState::Ready {
                    version: "2.0.0".into()
                }
            );
            assert!(installed);
            assert!(rig.root.join("releases/2.0.0/cua-driver").is_file());
            assert!(!rig.root.join("releases/1.0.0").exists());
            let actions: Vec<String> = rig
                .journal
                .recent()
                .iter()
                .map(|e| e.action.clone())
                .collect();
            assert_eq!(actions, ["helper.install", "helper.update"]);
        }

        #[tokio::test]
        async fn a_broken_copy_of_the_pinned_version_is_replaced() {
            let rig = rig("repair");
            let (archive, sha) = make_archive(&rig.dir, "d.tar.gz", "9.9.9");
            let broken = rig.root.join("releases/9.9.9");
            std::fs::create_dir_all(&broken).unwrap();
            std::fs::write(broken.join("cua-driver"), b"not a program").unwrap();
            let inst = CuaInstaller::new(
                rig.root.clone(),
                Some(spec("9.9.9", "d.tar.gz", &sha)),
                FileFetcher::new(archive),
                rig.journal.clone(),
            );
            let (state, installed) = inst.ensure().await;
            assert_eq!(
                state,
                InstallState::Ready {
                    version: "9.9.9".into()
                }
            );
            assert!(installed);
            assert_eq!(rig.journal.recent()[0].action, "helper.install");
        }

        #[tokio::test]
        async fn two_callers_share_one_install() {
            let rig = rig("shared");
            let (archive, sha) = make_archive(&rig.dir, "d.tar.gz", "9.9.9");
            let fetcher = FileFetcher::new(archive);
            let inst = Arc::new(CuaInstaller::new(
                rig.root.clone(),
                Some(spec("9.9.9", "d.tar.gz", &sha)),
                fetcher.clone(),
                rig.journal.clone(),
            ));
            let (a, b) = tokio::join!(inst.ensure(), inst.ensure());
            assert!(
                matches!(a.0, InstallState::Ready { .. })
                    && matches!(b.0, InstallState::Ready { .. })
            );
            assert_eq!(
                fetcher.calls.load(Ordering::SeqCst),
                1,
                "the second caller found it installed"
            );
            assert_eq!([a.1, b.1].iter().filter(|x| **x).count(), 1);
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn quitting_stops_a_running_install_and_blocks_new_ones() {
            let rig = rig("quit");
            let inst = Arc::new(CuaInstaller::new(
                rig.root.clone(),
                Some(spec("9.9.9", "d.tar.gz", &"0".repeat(64))),
                Arc::new(StuckFetcher),
                rig.journal.clone(),
            ));
            let running = inst.clone();
            let task = tokio::spawn(async move { running.ensure().await });
            // Wait until the download is under way, then quit.
            let mut rx = inst.subscribe();
            while !matches!(*rx.borrow(), InstallState::Downloading { .. }) {
                rx.changed().await.unwrap();
            }
            inst.shutdown();
            let (state, installed) = task.await.unwrap();
            assert!(
                matches!(
                    state,
                    InstallState::Failed {
                        code: FailCode::Cancelled,
                        ..
                    }
                ),
                "{state:?}"
            );
            assert!(!installed);
            assert!(!rig.root.join("tmp").exists());
            let (state, _) = inst.ensure().await;
            assert!(matches!(
                state,
                InstallState::Failed {
                    code: FailCode::Cancelled,
                    ..
                }
            ));
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn an_install_that_hangs_times_out() {
            let rig = rig("timeout");
            let inst = CuaInstaller::new(
                rig.root.clone(),
                Some(spec("9.9.9", "d.tar.gz", &"0".repeat(64))),
                Arc::new(StuckFetcher),
                rig.journal.clone(),
            )
            .with_timeout(Duration::from_millis(300));
            let (state, installed) = inst.ensure().await;
            assert!(
                matches!(
                    state,
                    InstallState::Failed {
                        code: FailCode::Timeout,
                        ..
                    }
                ),
                "{state:?}"
            );
            assert!(!installed);
            // The time limit is per run: the next call starts fresh.
            let (state, _) = inst.ensure().await;
            assert!(matches!(
                state,
                InstallState::Failed {
                    code: FailCode::Timeout,
                    ..
                }
            ));
        }

        #[tokio::test]
        async fn a_platform_without_a_release_says_so() {
            let rig = rig("unsupported");
            let inst = CuaInstaller::new(
                rig.root.clone(),
                None,
                Arc::new(StuckFetcher),
                rig.journal.clone(),
            );
            let (state, _) = inst.ensure().await;
            assert!(matches!(
                state,
                InstallState::Failed {
                    code: FailCode::Unsupported,
                    ..
                }
            ));
        }

        /// The real thing, off by default: downloads the pinned release
        /// from GitHub into a temp dir and runs its `--version` there.
        /// Run on purpose with:
        ///   cargo test -p connect-core real_pinned_release -- --ignored --nocapture
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[ignore = "needs the network; downloads about 47 MB"]
        async fn real_pinned_release_installs_into_a_temp_dir() {
            let rig = rig("real");
            let inst = CuaInstaller::new(
                rig.root.clone(),
                pinned_release(),
                Arc::new(UreqFetcher),
                rig.journal.clone(),
            );
            let started = Instant::now();
            let (state, installed) = inst.ensure().await;
            eprintln!(
                "state after {:.1} s: {state:?}",
                started.elapsed().as_secs_f64()
            );
            assert_eq!(
                state,
                InstallState::Ready {
                    version: PINNED_VERSION.into()
                }
            );
            assert!(installed);
            let mut names: Vec<String> =
                std::fs::read_dir(rig.root.join("releases").join(PINNED_VERSION))
                    .unwrap()
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect();
            names.sort();
            eprintln!("installed files: {names:?}");
            assert!(names.contains(&bin_name().to_string()));
            let (state, installed) = inst.ensure().await;
            assert_eq!(
                state,
                InstallState::Ready {
                    version: PINNED_VERSION.into()
                }
            );
            assert!(!installed);
        }
    }
}
