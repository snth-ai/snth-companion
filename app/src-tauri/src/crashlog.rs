//! Crash notes: what made the app stop, in files the owner can send.
//!
//! The folder is the one Tauri's `app_log_dir` names for this app:
//!
//!   macOS:   ~/Library/Logs/ai.snth.connect/
//!   Windows: %LOCALAPPDATA%\ai.snth.connect\logs\
//!   Linux:   $XDG_DATA_HOME/ai.snth.connect/logs/ (~/.local/share/...)
//!
//! Two files:
//!
//!  - `panic.log`: every Rust panic, with the message, where it happened,
//!    the executable's load address and a backtrace. A release build is
//!    stripped, so its frames are bare addresses; with the load address
//!    they can be symbolicated afterwards against the debug symbols of
//!    that version (`atos -o <dSYM> -l <load address> <addresses>` on
//!    macOS). See app/README.md, "Crash logs".
//!  - `stderr.log`: what the app writes to stderr, when stderr goes
//!    nowhere (an app started from Finder, the Start menu or a desktop
//!    file). That is connect-core's log lines, the panic message of the
//!    default hook (with the thread's name), and the runtime's last words
//!    before an abort that no panic hook sees, such as "fatal runtime
//!    error: Rust cannot catch foreign exceptions" (the 0.1.7 Logins
//!    crash). Started from a terminal, stderr stays where it is.
//!
//! Both are kept small: a file over `KEEP_BYTES` is moved to `<name>.1`
//! (replacing the one before) when the app starts.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const IDENTIFIER: &str = "ai.snth.connect";
pub const PANIC_FILE: &str = "panic.log";
pub const STDERR_FILE: &str = "stderr.log";
const KEEP_BYTES: u64 = 2 * 1024 * 1024;

/// The app's log folder (not created here).
pub fn log_dir() -> Option<PathBuf> {
    let base = directories::BaseDirs::new()?;
    if cfg!(target_os = "macos") {
        Some(base.home_dir().join("Library/Logs").join(IDENTIFIER))
    } else if cfg!(windows) {
        Some(base.data_local_dir().join(IDENTIFIER).join("logs"))
    } else {
        Some(base.data_dir().join(IDENTIFIER).join("logs"))
    }
}

/// Start writing crash notes. Call first thing in `main`: a panic before
/// this is lost. `version` is the app's version (package.json's, not the
/// crate's).
pub fn install(version: &str) {
    let Some(dir) = log_dir() else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    rotate(&dir.join(PANIC_FILE));
    redirect_stderr(&dir);
    let file = dir.join(PANIC_FILE);
    let version = version.to_string();
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Nothing in here may panic: a panic inside the hook aborts at once.
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("(a panic without a text message)");
        let place = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_else(|| "(unknown place)".into());
        let backtrace = std::backtrace::Backtrace::force_capture();
        let note = record(&utc_now(), &version, message, &place, image_base(), &format!("{backtrace:#}"));
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&file) {
            let _ = f.write_all(note.as_bytes());
        }
        default(info);
    }));
}

/// One entry of `panic.log`.
fn record(when: &str, version: &str, message: &str, place: &str, base: Option<usize>, backtrace: &str) -> String {
    let base = base.map_or_else(|| "unknown".to_string(), |b| format!("{b:#x}"));
    format!(
        "=== {when} SNTH Connect {version} ({} {}), pid {}\npanicked at {place}:\n{message}\nload address: {base}\nbacktrace:\n{backtrace}\n\n",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::process::id(),
    )
}

/// What `panic.log` holds, for the diagnostics: how many panics, and the
/// time of the last one. None when there is no file.
pub fn summary() -> Option<(usize, String)> {
    let text = std::fs::read_to_string(log_dir()?.join(PANIC_FILE)).ok()?;
    summarize(&text)
}

fn summarize(text: &str) -> Option<(usize, String)> {
    let heads: Vec<&str> = text.lines().filter(|l| l.starts_with("=== ")).collect();
    let last = heads.last()?;
    let when = last.trim_start_matches("=== ").split(' ').next().unwrap_or("").to_string();
    Some((heads.len(), when))
}

fn rotate(file: &Path) {
    if std::fs::metadata(file).is_ok_and(|m| m.len() > KEEP_BYTES) {
        let mut old = file.as_os_str().to_owned();
        old.push(".1");
        let _ = std::fs::rename(file, PathBuf::from(old));
    }
}

/// "2026-10-15T08:01:35Z"
fn utc_now() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    utc(secs)
}

fn utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rest = secs % 86_400;
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, (rest / 60) % 60, rest % 60)
}

/// Where the executable is loaded (ASLR): what turns a stripped frame's
/// address into an offset in the binary.
#[cfg(target_os = "macos")]
fn image_base() -> Option<usize> {
    extern "C" {
        fn _dyld_get_image_header(index: u32) -> *const std::ffi::c_void;
    }
    // Image 0 is the main executable.
    let header = unsafe { _dyld_get_image_header(0) };
    (!header.is_null()).then_some(header as usize)
}

#[cfg(windows)]
fn image_base() -> Option<usize> {
    let module = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) };
    (!module.is_null()).then_some(module as usize)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn image_base() -> Option<usize> {
    let exe = std::fs::read_link("/proc/self/exe").ok()?;
    let exe = exe.to_str()?;
    let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
    let line = maps.lines().find(|l| l.ends_with(exe))?;
    usize::from_str_radix(line.split('-').next()?, 16).ok()
}

/// Send stderr to `stderr.log` when it goes nowhere.
#[cfg(unix)]
fn redirect_stderr(dir: &Path) {
    use std::os::fd::IntoRawFd;
    if unsafe { libc::isatty(libc::STDERR_FILENO) } == 1 {
        return;
    }
    let path = dir.join(STDERR_FILE);
    rotate(&path);
    let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let fd = file.into_raw_fd();
    unsafe {
        libc::dup2(fd, libc::STDERR_FILENO);
        libc::close(fd);
    }
}

/// Windows: a release build has no console (windows_subsystem), so stderr
/// has no handle at all. Give it the file's. Rust's stderr asks for the
/// handle at every write, so everything after this lands in the file.
#[cfg(windows)]
fn redirect_stderr(dir: &Path) {
    use std::os::windows::io::IntoRawHandle;
    use windows_sys::Win32::System::Console::{GetStdHandle, SetStdHandle, STD_ERROR_HANDLE};
    let current = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    if !current.is_null() && current != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return; // a console or a pipe of whoever started us
    }
    let path = dir.join(STDERR_FILE);
    rotate(&path);
    let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    // The handle stays open for the life of the process.
    let handle = file.into_raw_handle();
    unsafe {
        SetStdHandle(STD_ERROR_HANDLE, handle as _);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_folder_is_where_tauri_puts_logs() {
        let dir = log_dir().unwrap();
        let text = dir.to_string_lossy().replace('\\', "/");
        if cfg!(target_os = "macos") {
            assert!(text.ends_with("/Library/Logs/ai.snth.connect"), "{text}");
        } else if cfg!(windows) {
            assert!(text.ends_with("/ai.snth.connect/logs"), "{text}");
        } else {
            assert!(text.ends_with("/ai.snth.connect/logs"), "{text}");
        }
    }

    #[test]
    fn a_record_has_what_symbolication_needs() {
        let note = record("2026-10-15T08:01:35Z", "0.1.8", "boom", "src/x.rs:1:2", Some(0x1047e0000), "   0:        0x1047e1234 - <unknown>");
        assert!(note.starts_with("=== 2026-10-15T08:01:35Z SNTH Connect 0.1.8 ("), "{note}");
        assert!(note.contains("SNTH Connect 0.1.8 ("), "{note}");
        assert!(note.contains("panicked at src/x.rs:1:2:\nboom\n"), "{note}");
        assert!(note.contains("load address: 0x1047e0000\n"), "{note}");
        assert!(note.contains("0x1047e1234"), "{note}");
        assert!(record("t", "0.1.8", "boom", "x", None, "").contains("load address: unknown"));
        // The executable's own load address is known on every system the app ships for.
        assert!(image_base().is_some());
    }

    #[test]
    fn the_summary_counts_panics_and_names_the_last() {
        let a = record("2026-10-15T08:01:35Z", "0.1.8", "one", "a.rs:1:1", None, "");
        let b = record("2026-10-16T01:02:03Z", "0.1.8", "two", "b.rs:1:1", None, "");
        let (count, last) = summarize(&format!("{a}{b}")).unwrap();
        assert_eq!(count, 2);
        assert_eq!(last, "2026-10-16T01:02:03Z");
        assert_eq!(summarize(""), None);
    }

    #[test]
    fn dates_are_plain_utc() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(1_760_515_295), "2025-10-15T08:01:35Z");
        assert_eq!(utc(951_782_400), "2000-02-29T00:00:00Z");
    }

    /// The hook as the app installs it, in a child process with its own
    /// HOME / LOCALAPPDATA, so the test never writes into the real log
    /// folder: a panic lands in panic.log with a backtrace.
    #[test]
    fn a_panic_is_written_down() {
        const CHILD: &str = "SNTH_CONNECT_CRASHLOG_CHILD";
        if std::env::var_os(CHILD).is_some() {
            install("9.9.9");
            let _ = std::thread::Builder::new()
                .name("doomed".into())
                .spawn(|| panic!("written down {}", 42))
                .unwrap()
                .join();
            return;
        }
        let root = std::env::temp_dir().join(format!("snth-connect-crashlog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let out = connect_core::proc::std_command(std::env::current_exe().unwrap())
            .args(["--exact", "crashlog::tests::a_panic_is_written_down", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1")
            .env("HOME", &root)
            .env("XDG_DATA_HOME", root.join(".local/share"))
            .env("LOCALAPPDATA", root.join("AppData/Local"))
            .env("USERPROFILE", &root)
            .output()
            .unwrap();
        assert!(out.status.success(), "{:?}\n{}", out.status, String::from_utf8_lossy(&out.stderr));
        let found = walk(&root).into_iter().find(|p| p.file_name().is_some_and(|n| n == PANIC_FILE));
        let Some(file) = found else {
            // BaseDirs on Windows asks the shell for the folder, not the
            // environment: the hook wrote to the real folder. Nothing to
            // read back here in that case.
            assert!(cfg!(windows), "no panic.log under {}", root.display());
            let _ = std::fs::remove_dir_all(&root);
            return;
        };
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("SNTH Connect 9.9.9"), "{text}");
        assert!(text.contains("written down 42"), "{text}");
        assert!(text.contains("crashlog.rs"), "{text}");
        assert!(text.contains("load address: 0x"), "{text}");
        assert!(text.contains("backtrace:\n   0:"), "{text}");
        assert_eq!(summarize(&text).unwrap().0, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk(&path));
            } else {
                out.push(path);
            }
        }
        out
    }
}
