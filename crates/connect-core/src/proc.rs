//! The one place a child process is created.
//!
//! On Windows a GUI app that starts a console program (`cua-driver`,
//! `tar`, `hostname`, anything) gets a console window flashed on the
//! screen for every start, unless the child is created with
//! `CREATE_NO_WINDOW`. Every spawn in this crate and in the app's shell
//! goes through `command` / `std_command`, which set that flag, so no call
//! site can forget it. A test below fails if a raw `Command::new` shows up
//! anywhere else.
//!
//! On other systems these are plain `Command::new`.

use std::ffi::OsStr;

/// `CREATE_NO_WINDOW`: the child gets no console at all.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A child process for async code (tokio).
pub fn command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    #[allow(unused_mut)]
    let mut cmd = tokio::process::Command::new(program);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// A child process for blocking code.
pub fn std_command(program: impl AsRef<OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Does this process have a console window? (Windows; false elsewhere.)
/// A child made by `command` must answer false: that is how the self-test
/// proves no window can flash.
pub fn has_console_window() -> bool {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetConsoleWindow() -> *mut std::ffi::c_void;
        }
        unsafe { !GetConsoleWindow().is_null() }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }

    /// No raw `Command::new` outside this file, in the core, the CLI and
    /// the app's shell: every child must come from `command` or
    /// `std_command` (tests included, so the rule has no exceptions to
    /// remember).
    #[test]
    fn every_spawn_goes_through_this_module() {
        let core = Path::new(env!("CARGO_MANIFEST_DIR"));
        let repo = core.parent().and_then(Path::parent).expect("repo root");
        let mut files = Vec::new();
        for dir in ["crates/connect-core/src", "crates/connect-host/src", "crates/connectd/src", "app/src-tauri/src"] {
            let dir = repo.join(dir);
            assert!(dir.is_dir(), "{} is missing", dir.display());
            rust_files(&dir, &mut files);
        }
        assert!(files.len() > 20, "found only {} source files", files.len());
        // Spelled in two parts so this file does not trip its own check.
        let needle = ["Command", "::new("].concat();
        let mut offenders = Vec::new();
        for file in files {
            if file.ends_with("connect-core/src/proc.rs") || file.ends_with("connect-core\\src\\proc.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&file).unwrap();
            for (n, line) in text.lines().enumerate() {
                if line.contains(&needle) {
                    offenders.push(format!("{}:{}", file.display(), n + 1));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "use connect_core::proc::command / std_command instead of a raw Command::new here:\n{}",
            offenders.join("\n")
        );
    }

    #[tokio::test]
    async fn a_child_runs_and_reports() {
        // Any program that exists everywhere and exits by itself.
        let (program, args): (&str, &[&str]) = if cfg!(windows) {
            ("cmd", &["/C", "exit 0"])
        } else {
            ("/bin/sh", &["-c", "exit 0"])
        };
        assert!(super::command(program).args(args).status().await.unwrap().success());
        assert!(super::std_command(program).args(args).status().unwrap().success());
    }
}
