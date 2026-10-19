//! The command line: the run on this system's own shell (these tests run
//! on macOS, Linux and, in CI, Windows with PowerShell), the gate, the
//! journal, and the cancel when the synth closes the stream.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::*;
use crate::grant::{Approval, Duration_};
use crate::settings::ShellMode;
use crate::testutil;

/// The same thing written for this computer's shell.
fn pick(posix: &str, powershell: &str) -> String {
    if os_shell().powershell { powershell.to_string() } else { posix.to_string() }
}

fn tmp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("connect-shell-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Plain, the way the owner and the synth write a path (Windows
    // canonicalizes to the \\?\ form).
    crate::files::plain(&std::fs::canonicalize(&dir).unwrap())
}

fn req(command: &str, timeout: Duration) -> Request {
    Request {
        command: command.to_string(),
        cwd: std::env::temp_dir(),
        timeout,
        stdin: None,
        reason: String::new(),
    }
}

async fn run_now(r: &Request) -> Run {
    let state = State::default();
    run(os_shell(), r, std::future::pending::<()>(), state.watch()).await.unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).replace("\r\n", "\n")
}

#[tokio::test]
async fn output_both_streams_in_utf8_and_the_exit_code() {
    let r = req(
        &pick("printf 'h\\303\\251llo\\n'; printf 'oops\\n' >&2", "Write-Output 'h\u{e9}llo'; [Console]::Error.WriteLine('oops')"),
        Duration::from_secs(60),
    );
    let run = run_now(&r).await;
    assert_eq!(run.end, End::Exited(Some(0)), "stderr: {}", text(&run.stderr.data));
    assert_eq!(text(&run.stdout.data), "h\u{e9}llo\n");
    assert!(text(&run.stderr.data).contains("oops"));
    assert!(!run.stdout.truncated());

    let run = run_now(&req("exit 3", Duration::from_secs(60))).await;
    assert_eq!(run.end, End::Exited(Some(3)));
    // A program's own exit code is the command's.
    let run = run_now(&req(&pick("sh -c 'exit 4'", "cmd /c exit 4"), Duration::from_secs(60))).await;
    assert_eq!(run.end, End::Exited(Some(4)));
}

#[tokio::test]
async fn stdin_reaches_the_command_and_is_closed() {
    let mut r = req(&pick("cat", "[Console]::In.ReadToEnd()"), Duration::from_secs(60));
    r.stdin = Some(b"line one\nline two\n".to_vec());
    let run = run_now(&r).await;
    assert_eq!(run.end, End::Exited(Some(0)), "stderr: {}", text(&run.stderr.data));
    assert!(text(&run.stdout.data).contains("line one\nline two"));
    // No stdin = nothing to read, not a wait.
    let started = Instant::now();
    let run = run_now(&req(&pick("cat", "[Console]::In.ReadToEnd()"), Duration::from_secs(30))).await;
    assert_eq!(run.end, End::Exited(Some(0)));
    assert!(started.elapsed() < Duration::from_secs(20));
}

#[tokio::test]
async fn the_working_folder_is_the_one_asked_for() {
    let dir = tmp("cwd");
    let mut r = req(&pick("pwd -P", "(Get-Location).Path"), Duration::from_secs(60));
    r.cwd = dir.clone();
    let run = run_now(&r).await;
    let got = PathBuf::from(text(&run.stdout.data).trim());
    assert_eq!(crate::files::plain(&std::fs::canonicalize(got).unwrap()), dir);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_long_output_keeps_its_tail() {
    let r = req(
        &pick(
            "head -c 3000000 /dev/zero | tr '\\0' 'a'; printf END",
            "[Console]::Out.Write('a' * 3000000); [Console]::Out.Write('END')",
        ),
        Duration::from_secs(120),
    );
    let run = run_now(&r).await;
    assert_eq!(run.stdout.total, 3_000_003);
    assert_eq!(run.stdout.data.len(), OUTPUT_CAP);
    assert!(run.stdout.truncated());
    assert!(run.stdout.data.ends_with(b"aaaEND"));
    let v = result_value(&run, "x");
    assert_eq!(v["truncated_stdout"], true);
    assert_eq!(v["stdout_bytes"], 3_000_003);
}

#[tokio::test]
async fn a_timeout_ends_the_command() {
    let started = Instant::now();
    let run = run_now(&req(&pick("sleep 30", "Start-Sleep -Seconds 30"), Duration::from_secs(2))).await;
    assert_eq!(run.end, End::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(15), "took {:?}", started.elapsed());
    let v = result_value(&run, "x");
    assert_eq!(v["timed_out"], true);
    assert_eq!(v["exit_code"], Value::Null);
}

/// What the command started in the background dies with it on a timeout:
/// the whole process group.
#[cfg(unix)]
#[tokio::test]
async fn a_timeout_ends_what_the_command_started_too() {
    let run = run_now(&req("sleep 60 & echo $!; wait", Duration::from_secs(2))).await;
    assert_eq!(run.end, End::TimedOut);
    let pid: i32 = text(&run.stdout.data).trim().parse().expect("the background pid");
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    let alive = || unsafe { kill(pid, 0) } == 0;
    let until = Instant::now() + Duration::from_secs(5);
    while alive() && Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!alive(), "the background process {pid} survived");
}

#[tokio::test]
async fn the_synth_going_away_and_the_owners_stop_end_it() {
    let r = req(&pick("sleep 30", "Start-Sleep -Seconds 30"), Duration::from_secs(60));
    let state = State::default();
    let started = Instant::now();
    let cancel = tokio::time::sleep(Duration::from_millis(500));
    let run1 = run(os_shell(), &r, cancel, state.watch()).await.unwrap();
    assert_eq!(run1.end, End::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(15));

    let state = Arc::new(State::default());
    let stopper = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        stopper.stop_all();
    });
    let run2 = run(os_shell(), &r, std::future::pending::<()>(), state.watch()).await.unwrap();
    assert_eq!(run2.end, End::Stopped);
}

#[test]
fn powershell_gets_the_command_whole_and_in_utf16() {
    let command = "Write-Output \"it's \u{e9}\"; & 'C:\\Program Files\\x.exe' -a \"b c\"";
    let script = powershell_script(command);
    assert!(script.contains(command));
    assert!(script.contains("UTF8Encoding $false"));
    let raw = base64::engine::general_purpose::STANDARD.decode(encode_powershell(&script)).unwrap();
    let units: Vec<u16> = raw.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    assert_eq!(String::from_utf16(&units).unwrap(), script);
    // The longest command still fits a Windows command line.
    let longest = "x".repeat(MAX_COMMAND);
    assert!(encode_powershell(&powershell_script(&longest)).len() + 100 < 32767);
}

// --- the gate ---

async fn exec_now(shared: &Arc<crate::core::Shared>, params: Value) -> Result<Value, String> {
    exec(shared, &params, std::future::pending::<()>()).await
}

fn on(core: &crate::core::Core, mode: ShellMode) {
    core.mutate_settings(|s| {
        s.shell.enabled = true;
        s.shell.mode = mode;
    });
}

fn stdout_of(v: &Value) -> String {
    let b = base64::engine::general_purpose::STANDARD.decode(v["stdout_b64"].as_str().unwrap()).unwrap();
    text(&b)
}

#[tokio::test]
async fn off_until_the_owner_turns_it_on_and_paused_means_no() {
    let owner = testutil::Owner::new(Approval::once());
    let core = testutil::core(owner.clone());
    let shared = core.shared();
    let e = exec_now(&shared, json!({"command": "echo hi"})).await.unwrap_err();
    assert!(e.starts_with("denied: the command line is turned off"), "{e}");
    let caps = crate::wire::capabilities_value(&core.settings(), "", "ok", shared.apps.backend().as_ref());
    assert_eq!(caps["shell"]["enabled"], false);
    assert_eq!(caps["shell"]["mode"], "ask");
    assert_eq!(caps["shell"]["os_shell"], os_shell().label.as_str());

    on(&core, ShellMode::Auto);
    core.pause().await;
    let e = exec_now(&shared, json!({"command": "echo hi"})).await.unwrap_err();
    assert_eq!(e, "denied: the command line is paused by the owner");
    assert_eq!(owner.asks(), 0);
}

#[tokio::test]
async fn ask_mode_asks_with_the_whole_command_and_remembers_only_that_command() {
    let owner = testutil::Owner::new(Approval::deny());
    let core = testutil::core(owner.clone());
    let shared = core.shared();
    on(&core, ShellMode::Ask);
    let dir = tmp("ask");
    let cwd = dir.to_string_lossy().to_string();

    // No.
    let e = exec_now(&shared, json!({"command": "echo one", "cwd": cwd, "reason": "to trim the video"})).await.unwrap_err();
    assert_eq!(e, "denied: the owner did not allow this command");
    let asked = owner.last();
    assert_eq!(asked.capability, "shell");
    assert_eq!(asked.scope, "echo one");
    assert_eq!(asked.reason, "to trim the video");
    assert_eq!(asked.remember, Remember::AlwaysOnly);
    assert!(asked.details.contains(&("command".to_string(), "echo one".to_string())));
    assert!(asked.details.contains(&("cwd".to_string(), cwd.clone())));
    let last = core.recent_journal().pop().unwrap();
    assert_eq!((last.capability.as_str(), last.outcome.as_str()), ("shell", "denied: owner"));

    // Always, for this exact command.
    owner.next(Approval { allow: true, duration: Duration_::Always });
    let v = exec_now(&shared, json!({"command": "echo one", "cwd": cwd})).await.unwrap();
    assert_eq!(stdout_of(&v).trim(), "one");
    assert_eq!(owner.asks(), 2);
    exec_now(&shared, json!({"command": "echo one", "cwd": cwd})).await.unwrap();
    assert_eq!(owner.asks(), 2, "the remembered command asked again");
    // Another command is another question.
    let _ = exec_now(&shared, json!({"command": "echo two", "cwd": cwd})).await;
    assert_eq!(owner.asks(), 3);
    // A program over several lines can only be allowed this once.
    let _ = exec_now(&shared, json!({"command": "echo a\necho b", "cwd": cwd})).await;
    assert_eq!(owner.last().remember, Remember::Never);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn auto_mode_runs_and_the_journal_has_no_output() {
    let owner = testutil::Owner::new(Approval::deny());
    let core = testutil::core(owner.clone());
    let shared = core.shared();
    on(&core, ShellMode::Auto);
    let dir = tmp("auto");
    let v = exec_now(&shared, json!({"command": "echo secret-output", "cwd": dir.to_string_lossy()})).await.unwrap();
    assert_eq!(owner.asks(), 0);
    assert_eq!(v["exit_code"], 0);
    assert_eq!(v["shell"], os_shell().label.as_str());
    assert!(stdout_of(&v).contains("secret-output"));
    let e = core.recent_journal().pop().unwrap();
    assert_eq!(e.capability, "shell");
    assert_eq!(e.action, "shell.exec");
    assert_eq!(e.target, "echo secret-output");
    assert_eq!(e.outcome, "ok");
    assert_eq!(e.exit_code, Some(0));
    assert_eq!(e.cwd.as_deref(), Some(dir.to_string_lossy().as_ref()));
    assert!(e.duration_ms.is_some());
    assert!(e.bytes.unwrap() >= "secret-output".len() as u64);
    // The output is never written down: only the command names it.
    let line = serde_json::to_string(&e).unwrap();
    assert_eq!(line.matches("secret-output").count(), 1);

    let v = exec_now(&shared, json!({"command": "exit 7"})).await.unwrap();
    assert_eq!(v["exit_code"], 7);
    assert_eq!(core.recent_journal().pop().unwrap().outcome, "exit 7");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_bad_request_is_refused_before_anyone_is_asked() {
    let owner = testutil::Owner::new(Approval::once());
    let core = testutil::core(owner.clone());
    let shared = core.shared();
    on(&core, ShellMode::Ask);
    for (params, starts) in [
        (json!({}), "bad request: params.command is required"),
        (json!({"command": "  "}), "bad request: params.command is empty"),
        (json!({"command": "x".repeat(MAX_COMMAND + 1)}), "bad request: the command is"),
        (json!({"command": "echo", "cwd": "relative/dir"}), "bad request: cwd must be an absolute path"),
        (json!({"command": "echo", "stdin_b64": "%%%"}), "bad request: stdin_b64"),
    ] {
        let e = exec_now(&shared, params).await.unwrap_err();
        assert!(e.starts_with(starts), "{e}");
    }
    let missing = std::env::temp_dir().join("connect-shell-no-such-folder-here");
    let e = exec_now(&shared, json!({"command": "echo", "cwd": missing.to_string_lossy()})).await.unwrap_err();
    assert!(e.starts_with("not found: "), "{e}");
    assert_eq!(owner.asks(), 0);
}

#[tokio::test]
async fn turning_it_off_or_pausing_stops_a_running_command() {
    let core = testutil::core(testutil::Owner::new(Approval::deny()));
    let shared = core.shared();
    on(&core, ShellMode::Auto);
    let sleeper = json!({"command": pick("sleep 30", "Start-Sleep -Seconds 30")});
    let running = {
        let shared = shared.clone();
        let sleeper = sleeper.clone();
        tokio::spawn(async move { exec_now(&shared, sleeper).await })
    };
    tokio::time::sleep(Duration::from_millis(700)).await;
    let started = Instant::now();
    core.pause().await;
    let e = running.await.unwrap().unwrap_err();
    assert!(e.starts_with("denied: the owner stopped the command"), "{e}");
    assert!(started.elapsed() < Duration::from_secs(15));
    assert_eq!(core.recent_journal().pop().unwrap().outcome, "stopped: paused by the owner");

    core.resume();
    let running = {
        let shared = shared.clone();
        tokio::spawn(async move { exec_now(&shared, sleeper).await })
    };
    tokio::time::sleep(Duration::from_millis(700)).await;
    core.mutate_settings(|s| s.shell.enabled = false);
    assert!(running.await.unwrap().unwrap_err().starts_with("denied: the owner stopped the command"));
}

#[tokio::test]
async fn a_preset_turns_it_off_and_permission_request_cannot_turn_it_on() {
    let core = testutil::core(testutil::Owner::new(Approval::always()));
    on(&core, ShellMode::Auto);
    core.apply_preset(crate::settings::Preset::Full);
    assert!(!core.settings().shell.enabled);
    assert_eq!(core.settings().shell.mode, ShellMode::Auto, "the mode is kept for next time");
    let e = crate::rpc::call_for_test(&core.shared(), "permission.request", json!({"capability": "shell", "scope": "rm -rf /"}))
        .await
        .unwrap_err();
    assert!(e.starts_with("denied: commands are asked about one by one"), "{e}");
    assert!(!core.shared().grants().allows("shell", "rm -rf /"));
}
