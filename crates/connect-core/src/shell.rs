//! `shell`: the synth runs a command on this computer and gets its output
//! (rpc `shell.exec`, connect.md "Command line").
//!
//! Full access by definition: a command runs as the owner, with
//! everything the owner can reach (every file, the network, the installed
//! programs). The shared folders of `files` do not limit it. That is why it
//! is off in every preset and its switch sits in the developer section.
//!
//! The one gate is `exec`: pause, the switch, the request checked and
//! bounded, the ask (mode `ask`: every command, with the whole command and
//! its folder on the screen; only "always for this exact command" can be
//! remembered), the run, one journal line (the command, the folder, the
//! exit code, the time, the bytes of output; never the output).
//!
//! The run:
//! - Windows: PowerShell (`pwsh.exe` when installed, else Windows
//!   PowerShell), `-NoProfile -NonInteractive -EncodedCommand`, so no
//!   quoting of the command can go wrong; output forced to UTF-8; no
//!   console window (`proc::command`); the whole process tree in a Job
//!   Object, terminated on a timeout, a cancel or a pause.
//! - macOS / Linux: the owner's login shell (`$SHELL`, else `/bin/zsh` on
//!   a Mac and `/bin/sh` elsewhere) with `-l -c`, so the PATH of the
//!   owner's profile (Homebrew's ffmpeg) is there; its own process group,
//!   which gets SIGTERM and then SIGKILL on a timeout, a cancel or a pause.
//! - No terminal: stdin is the bytes the synth sent (then closed) or
//!   nothing. Each of stdout and stderr keeps its last `OUTPUT_CAP` bytes:
//!   the end of an output is where the error is.
//! - A process the command left running in the background is not killed
//!   when the command ends by itself; its output after that is not read.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;

use crate::core::Shared;
use crate::grant::{ApprovalRequest, Remember};
use crate::journal::Entry;
use crate::settings::{ShellMode, ShellSettings};

/// The capability's name (settings, grants, the journal, the wire).
pub const CAP: &str = "shell";

/// A command runs this long unless the synth asks for another limit.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// The longest a command may run.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// The shortest limit the synth may ask for.
const MIN_TIMEOUT: Duration = Duration::from_secs(1);
/// What is kept of each of stdout and stderr: the last this many bytes.
pub const OUTPUT_CAP: usize = 1024 * 1024;
/// The longest command text. Windows takes at most 32767 characters on a
/// command line, and the command travels there UTF-16 and base64 encoded.
/// A longer program belongs in a script file.
pub const MAX_COMMAND: usize = 8 * 1024;
/// The most stdin the synth may hand a command.
pub const MAX_STDIN: usize = 4 * 1024 * 1024;
/// The longest command the owner may say "always" to (it is kept with the
/// grants and listed on the screen).
const MAX_REMEMBERED: usize = 2000;
/// After the command ended, how long its output is still read (a process
/// it left in the background may hold the pipe open).
const DRAIN_AFTER_EXIT: Duration = Duration::from_secs(2);
/// Between SIGTERM and SIGKILL.
#[cfg(unix)]
const TERM_GRACE: Duration = Duration::from_millis(1500);
/// The longest a command shows in the journal.
const JOURNAL_COMMAND: usize = 2000;

/// What stops every running command: a pause, the stop shortcut, the
/// owner turning the command line off. A generation counter, so a
/// command that starts after a stop is not stopped by it.
pub struct State {
    stop: watch::Sender<u64>,
}

impl Default for State {
    fn default() -> Self {
        State { stop: watch::channel(0).0 }
    }
}

impl State {
    /// Stop every command running now.
    pub fn stop_all(&self) {
        self.stop.send_modify(|g| *g = g.wrapping_add(1));
    }

    fn watch(&self) -> watch::Receiver<u64> {
        self.stop.subscribe()
    }
}

/// The shell commands run in on this computer.
#[derive(Debug, Clone)]
pub struct OsShell {
    pub program: PathBuf,
    /// What the synth is told: `pwsh`, `powershell`, `zsh`, `bash`, `sh`...
    pub label: String,
    pub powershell: bool,
}

/// The shell of this computer, found once.
pub fn os_shell() -> &'static OsShell {
    static SHELL: OnceLock<OsShell> = OnceLock::new();
    SHELL.get_or_init(detect)
}

#[cfg(windows)]
fn detect() -> OsShell {
    // PowerShell 7 when installed (it is on the PATH as pwsh.exe), else
    // the Windows PowerShell every Windows has.
    let on_path = |name: &str| {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths).map(|dir| dir.join(name)).find(|p| p.is_file())
        })
    };
    match on_path("pwsh.exe") {
        Some(program) => OsShell { program, label: "pwsh".into(), powershell: true },
        None => OsShell { program: PathBuf::from("powershell.exe"), label: "powershell".into(), powershell: true },
    }
}

#[cfg(not(windows))]
fn detect() -> OsShell {
    let fallback = if cfg!(target_os = "macos") { "/bin/zsh" } else { "/bin/sh" };
    let program = std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_file())
        .unwrap_or_else(|| PathBuf::from(fallback));
    let label = program
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .take(32)
        .collect::<String>();
    let label = if label.is_empty() { "sh".to_string() } else { label };
    OsShell { program, label, powershell: false }
}

/// `capabilities.shell` in the hello: the switch, the mode and which
/// shell runs the commands. Added after the first release of protocol v1:
/// a synth that does not know it ignores it.
pub fn wire_value(s: &ShellSettings) -> Value {
    let mode = match s.mode {
        ShellMode::Ask => "ask",
        ShellMode::Auto => "auto",
    };
    json!({"enabled": s.enabled, "mode": mode, "os_shell": os_shell().label})
}

/// One `shell.exec` request, checked and bounded.
#[derive(Debug, Clone)]
pub struct Request {
    pub command: String,
    pub cwd: PathBuf,
    pub timeout: Duration,
    pub stdin: Option<Vec<u8>>,
    /// The synth's own words. Untrusted.
    pub reason: String,
}

fn home() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf())
}

impl Request {
    pub async fn parse(params: &Value) -> Result<Request, String> {
        let command = params
            .get("command")
            .and_then(Value::as_str)
            .ok_or("bad request: params.command is required")?;
        if command.trim().is_empty() {
            return Err("bad request: params.command is empty".into());
        }
        if command.len() > MAX_COMMAND {
            return Err(format!(
                "bad request: the command is {} bytes, at most {MAX_COMMAND}; put a longer program in a script file and run that",
                command.len()
            ));
        }
        if command.contains('\0') {
            return Err("bad request: the command contains a NUL character".into());
        }
        let cwd = match params.get("cwd").and_then(Value::as_str).map(str::trim).filter(|c| !c.is_empty()) {
            None => home().ok_or("unavailable: this computer has no home folder to run in; pass cwd")?,
            Some(c) => {
                let p = PathBuf::from(c);
                if !p.is_absolute() {
                    return Err(format!("bad request: cwd must be an absolute path, not {c:?}"));
                }
                p
            }
        };
        match tokio::fs::metadata(&cwd).await {
            Ok(md) if md.is_dir() => {}
            Ok(_) => return Err(format!("bad request: cwd {} is not a folder", cwd.display())),
            Err(_) => return Err(format!("not found: {}", cwd.display())),
        }
        let timeout = match params.get("timeout_ms").and_then(Value::as_u64) {
            None | Some(0) => DEFAULT_TIMEOUT,
            Some(ms) => Duration::from_millis(ms).clamp(MIN_TIMEOUT, MAX_TIMEOUT),
        };
        let stdin = match params.get("stdin_b64").and_then(Value::as_str) {
            None | Some("") => None,
            Some(b64) => {
                let data = base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .map_err(|e| format!("bad request: stdin_b64: {e}"))?;
                if data.len() > MAX_STDIN {
                    return Err(format!("bad request: stdin is {} bytes, at most {MAX_STDIN}", data.len()));
                }
                Some(data)
            }
        };
        let reason = params.get("reason").and_then(Value::as_str).unwrap_or("").to_string();
        Ok(Request { command: command.to_string(), cwd, timeout, stdin, reason })
    }
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// It exited by itself (code None: killed by a signal from elsewhere).
    Exited(Option<i32>),
    TimedOut,
    /// The synth went away (it closed the stream).
    Cancelled,
    /// The owner paused, pressed the stop shortcut or turned the command
    /// line off.
    Stopped,
}

/// What a run produced.
#[derive(Debug, Clone)]
pub struct Run {
    pub end: End,
    pub stdout: Tail,
    pub stderr: Tail,
    pub duration: Duration,
}

/// The last `cap` bytes of a stream, and how many there were in all.
#[derive(Debug, Clone, Default)]
pub struct Tail {
    pub data: Vec<u8>,
    pub total: u64,
}

impl Tail {
    pub fn truncated(&self) -> bool {
        self.total > self.data.len() as u64
    }
}

async fn read_tail<R: AsyncRead + Unpin>(mut r: R, cap: usize) -> Tail {
    let mut tail = Tail::default();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                tail.total += n as u64;
                tail.data.extend_from_slice(&buf[..n]);
                // Trim in batches, not on every read.
                if tail.data.len() > cap * 2 {
                    let cut = tail.data.len() - cap;
                    tail.data.drain(..cut);
                }
            }
        }
    }
    if tail.data.len() > cap {
        let cut = tail.data.len() - cap;
        tail.data.drain(..cut);
    }
    tail
}

/// The program PowerShell runs: output in UTF-8 without a byte order
/// mark, no progress bars, plain text, then the command, then an exit
/// code that says how it ended (the last native program's code, else 1
/// when the last command failed).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn powershell_script(command: &str) -> String {
    format!(
        "$ProgressPreference = 'SilentlyContinue'\n\
         try {{ [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false }} catch {{}}\n\
         $OutputEncoding = New-Object System.Text.UTF8Encoding $false\n\
         if ($PSStyle) {{ $PSStyle.OutputRendering = 'PlainText' }}\n\
         {command}\n\
         $snthOk = $?\n\
         if ($LASTEXITCODE) {{ exit $LASTEXITCODE }}\n\
         if (-not $snthOk) {{ exit 1 }}\n\
         exit 0\n"
    )
}

/// `-EncodedCommand` takes the program as base64 of UTF-16LE.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn encode_powershell(script: &str) -> String {
    let utf16: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    base64::engine::general_purpose::STANDARD.encode(utf16)
}

fn build(shell: &OsShell, req: &Request) -> tokio::process::Command {
    let mut cmd = crate::proc::command(&shell.program);
    if shell.powershell {
        cmd.args(["-NoProfile", "-NonInteractive", "-EncodedCommand"])
            .arg(encode_powershell(&powershell_script(&req.command)));
    } else {
        cmd.arg("-l").arg("-c").arg(&req.command);
        #[cfg(unix)]
        cmd.process_group(0);
    }
    cmd.current_dir(&req.cwd)
        .stdin(if req.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd
}

/// The whole tree of a command, to be stopped at once.
struct Tree {
    #[cfg(unix)]
    pgid: Option<i32>,
    #[cfg(windows)]
    job: Option<win::Job>,
}

impl Tree {
    fn of(child: &tokio::process::Child) -> Tree {
        #[cfg(unix)]
        {
            Tree { pgid: child.id().map(|p| p as i32) }
        }
        #[cfg(windows)]
        {
            Tree { job: child.raw_handle().and_then(win::Job::with_process) }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = child;
            Tree {}
        }
    }

    async fn kill(&self, child: &mut tokio::process::Child) {
        #[cfg(unix)]
        if let Some(pgid) = self.pgid {
            // A moment to finish cleanly (ffmpeg closes its file), then
            // the whole group, also when the leader is already gone.
            unix::signal_group(pgid, unix::SIGTERM);
            let _ = tokio::time::timeout(TERM_GRACE, child.wait()).await;
            unix::signal_group(pgid, unix::SIGKILL);
        }
        #[cfg(windows)]
        if let Some(job) = &self.job {
            job.terminate();
        }
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    }
}

#[cfg(unix)]
mod unix {
    pub const SIGTERM: i32 = 15;
    pub const SIGKILL: i32 = 9;
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    /// Signal every process of the group (a negative pid).
    pub fn signal_group(pgid: i32, sig: i32) {
        if pgid > 1 {
            unsafe {
                kill(-pgid, sig);
            }
        }
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn TerminateJobObject(job: *mut c_void, exit_code: u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    /// A Job Object holding a command and every process it starts. The
    /// command is put in right after it started; PowerShell takes far
    /// longer than that to start anything of its own.
    pub struct Job(*mut c_void);

    // The handle is only used through the kernel's own synchronization.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn with_process(process: std::os::windows::io::RawHandle) -> Option<Job> {
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return None;
                }
                if AssignProcessToJobObject(job, process as *mut c_void) == 0 {
                    CloseHandle(job);
                    return None;
                }
                Some(Job(job))
            }
        }

        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // Closing the handle does not end the processes: a command
            // that ended by itself keeps what it started in the background.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

/// Run one command. `cancel` resolves when the synth went away; `stop`
/// changes when the owner stops everything.
pub async fn run(
    shell: &OsShell,
    req: &Request,
    cancel: impl std::future::Future<Output = ()>,
    mut stop: watch::Receiver<u64>,
) -> Result<Run, String> {
    let started = Instant::now();
    let mut child = build(shell, req).spawn().map_err(|e| format!("failed: cannot start {}: {e}", shell.label))?;
    let tree = Tree::of(&child);
    if let (Some(data), Some(mut stdin)) = (req.stdin.clone(), child.stdin.take()) {
        tokio::spawn(async move {
            let _ = stdin.write_all(&data).await;
            let _ = stdin.shutdown().await;
        });
    }
    let out = tokio::spawn(read_tail(child.stdout.take().expect("piped"), OUTPUT_CAP));
    let err = tokio::spawn(read_tail(child.stderr.take().expect("piped"), OUTPUT_CAP));

    stop.borrow_and_update();
    tokio::pin!(cancel);
    let end = tokio::select! {
        status = child.wait() => End::Exited(status.ok().and_then(|s| s.code())),
        _ = tokio::time::sleep(req.timeout) => End::TimedOut,
        _ = &mut cancel => End::Cancelled,
        _ = stop.changed() => End::Stopped,
    };
    if !matches!(end, End::Exited(_)) {
        tree.kill(&mut child).await;
    }
    let duration = started.elapsed();

    // The output: what is in the pipes now, and a moment more for what is
    // on its way. A background process may keep a pipe open for good.
    let collect = |h: tokio::task::JoinHandle<Tail>| async move {
        let abort = h.abort_handle();
        match tokio::time::timeout(DRAIN_AFTER_EXIT, h).await {
            Ok(Ok(t)) => t,
            _ => {
                abort.abort();
                Tail::default()
            }
        }
    };
    let (stdout, stderr) = tokio::join!(collect(out), collect(err));
    drop(tree);
    Ok(Run { end, stdout, stderr, duration })
}

/// The command as the journal and the screen show it: one line, bounded.
fn one_line(command: &str, max: usize) -> String {
    let mut out: String = command
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect();
    if command.chars().count() > max {
        out.push_str("...");
    }
    out
}

/// The gate: every `shell.exec` goes through here and nothing else runs a
/// command for the synth.
pub async fn exec(shared: &Arc<Shared>, params: &Value, cancel: impl std::future::Future<Output = ()>) -> Result<Value, String> {
    let paused = || "denied: the command line is paused by the owner".to_string();
    let off = || "denied: the command line is turned off on this computer; only the owner can turn it on (SNTH Connect, General, developer mode)".to_string();
    if shared.paused() {
        return Err(paused());
    }
    let settings = shared.settings().shell;
    if !settings.enabled {
        return Err(off());
    }
    let req = Request::parse(params).await?;
    let shell = os_shell();
    let target = one_line(&req.command, JOURNAL_COMMAND);
    let cwd_text = crate::files::plain(&req.cwd).to_string_lossy().to_string();
    let journal = |outcome: String| {
        let mut e = Entry::new(CAP, "shell.exec", target.clone(), outcome);
        e.cwd = Some(cwd_text.clone());
        e
    };

    if settings.mode == ShellMode::Ask && !shared.grant_allows(CAP, &req.command) {
        let remember = if req.command.len() <= MAX_REMEMBERED && !req.command.contains('\n') {
            Remember::AlwaysOnly
        } else {
            Remember::Never
        };
        let ask = ApprovalRequest::new(CAP, req.command.clone(), &req.reason, format!("run a command in {cwd_text}"))
            .with_details(vec![
                ("command".to_string(), req.command.clone()),
                ("cwd".to_string(), cwd_text.clone()),
                ("shell".to_string(), shell.label.clone()),
            ])
            .with_remember(remember);
        let answer = shared.ask_scoped(CAP, &req.command, ask).await;
        if !answer.allow {
            shared.journal().record(journal("denied: owner".into()));
            return Err("denied: the owner did not allow this command".into());
        }
        // The owner may have paused or switched it off while deciding.
        if shared.paused() {
            return Err(paused());
        }
        if !shared.settings().shell.enabled {
            return Err(off());
        }
    }

    let run = match run(shell, &req, cancel, shared.shell.watch()).await {
        Ok(r) => r,
        Err(why) => {
            shared.journal().record(journal(why.clone()));
            return Err(why);
        }
    };
    let outcome = match run.end {
        End::Exited(Some(0)) => "ok".to_string(),
        End::Exited(Some(code)) => format!("exit {code}"),
        End::Exited(None) => "ended by a signal".to_string(),
        End::TimedOut => "timed out".to_string(),
        End::Cancelled => "cancelled: the synth went away".to_string(),
        End::Stopped => "stopped: paused by the owner".to_string(),
    };
    let mut entry = journal(outcome).with_bytes(run.stdout.total + run.stderr.total);
    entry.duration_ms = Some(run.duration.as_millis() as u64);
    if let End::Exited(code) = run.end {
        entry.exit_code = code;
    }
    shared.journal().record(entry);

    if run.end == End::Stopped {
        return Err("denied: the owner stopped the command (SNTH Connect was paused or the command line turned off)".into());
    }
    Ok(result_value(&run, &shell.label))
}

/// The answer of `shell.exec`. Output travels as base64 of the raw bytes:
/// it is what the program wrote, in whatever encoding (on Windows forced
/// to UTF-8 where PowerShell can), and JSON strings cannot carry bytes
/// that are not UTF-8.
pub fn result_value(run: &Run, shell: &str) -> Value {
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    json!({
        "exit_code": match run.end { End::Exited(code) => json!(code), _ => Value::Null },
        "stdout_b64": b64(&run.stdout.data),
        "stderr_b64": b64(&run.stderr.data),
        "stdout_bytes": run.stdout.total,
        "stderr_bytes": run.stderr.total,
        "truncated_stdout": run.stdout.truncated(),
        "truncated_stderr": run.stderr.truncated(),
        "duration_ms": run.duration.as_millis() as u64,
        "timed_out": run.end == End::TimedOut,
        "shell": shell,
    })
}

#[cfg(test)]
mod tests;
