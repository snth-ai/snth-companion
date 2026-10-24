//! Showing a notification on this computer.
//!
//! The app shell shows notifications as itself (its own name and icon in
//! the system's notification settings), so it implements `Notifier` with
//! its own means. `SystemNotifier` is the fallback for a host without a
//! window system of its own (the `connectd` CLI, the self-test): it uses
//! what every system ships.
//!
//! - macOS: `osascript` `display notification` (shown under "Script
//!   Editor" in the notification settings)
//! - Windows: a toast through Windows PowerShell and the WinRT
//!   notification types, under PowerShell's own app identity
//! - Linux: `notify-send`, when it is installed

use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

pub type NotifyFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

/// Shows one notification. The texts are checked and bounded already.
pub trait Notifier: Send + Sync {
    fn show(&self, title: String, body: String) -> NotifyFuture;
}

/// The system's own tools (see the top of this file).
pub struct SystemNotifier;

impl Notifier for SystemNotifier {
    fn show(&self, title: String, body: String) -> NotifyFuture {
        Box::pin(show_with_system_tools(title, body))
    }
}

const TIMEOUT: Duration = Duration::from_secs(20);

#[cfg(target_os = "macos")]
async fn show_with_system_tools(title: String, body: String) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    // The script comes on stdin and the texts are its arguments: nothing
    // the synth wrote is ever part of the program.
    const SCRIPT: &str = "on run argv\n\tdisplay notification (item 2 of argv) with title (item 1 of argv)\nend run\n";
    let mut child = crate::proc::command("/usr/bin/osascript")
        .args(["-", &title, &body])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("cannot run osascript: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(SCRIPT.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }
    match tokio::time::timeout(TIMEOUT, child.wait()).await {
        Ok(Ok(st)) if st.success() => Ok(()),
        Ok(Ok(_)) => Err("the system refused to show the notification".into()),
        Ok(Err(e)) => Err(format!("cannot run osascript: {e}")),
        Err(_) => Err("the notification was not shown in time".into()),
    }
}

/// The PowerShell program that shows a toast. The texts arrive in
/// environment variables, never inside the program text.
#[cfg(any(windows, test))]
pub(crate) const WINDOWS_TOAST_SCRIPT: &str = r#"$ErrorActionPreference = 'Stop'
[void][Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime]
[void][Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime]
$title = [System.Security.SecurityElement]::Escape($env:SNTH_TOAST_TITLE)
$body = [System.Security.SecurityElement]::Escape($env:SNTH_TOAST_BODY)
$xml = New-Object Windows.Data.Xml.Dom.XmlDocument
$xml.LoadXml("<toast><visual><binding template='ToastGeneric'><text>$title</text><text>$body</text></binding></visual></toast>")
$toast = New-Object Windows.UI.Notifications.ToastNotification $xml
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($env:SNTH_TOAST_APP).Show($toast)
Write-Output 'shown'
"#;

/// The app identity Windows PowerShell itself is registered under: a
/// toast needs a registered identity to appear at all.
#[cfg(windows)]
const POWERSHELL_APP_ID: &str = r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe";

#[cfg(windows)]
async fn show_with_system_tools(title: String, body: String) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let mut child = crate::proc::command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", "-"])
        .env("SNTH_TOAST_TITLE", &title)
        .env("SNTH_TOAST_BODY", &body)
        .env("SNTH_TOAST_APP", POWERSHELL_APP_ID)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("cannot run PowerShell: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(WINDOWS_TOAST_SCRIPT.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }
    match tokio::time::timeout(TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(out)) if out.status.success() && String::from_utf8_lossy(&out.stdout).contains("shown") => Ok(()),
        Ok(Ok(out)) => {
            let err = String::from_utf8_lossy(&out.stderr);
            let first = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("no reason given");
            Err(format!("Windows did not show the notification: {}", first.trim()))
        }
        Ok(Err(e)) => Err(format!("cannot run PowerShell: {e}")),
        Err(_) => Err("the notification was not shown in time".into()),
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
async fn show_with_system_tools(title: String, body: String) -> Result<(), String> {
    let run = crate::proc::command("notify-send")
        .args(["--app-name", "SNTH Connect", "--", &title, &body])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status();
    match tokio::time::timeout(TIMEOUT, run).await {
        Ok(Ok(st)) if st.success() => Ok(()),
        Ok(Ok(_)) => Err("the desktop refused to show the notification".into()),
        Ok(Err(e)) => Err(format!("cannot run notify-send: {e}")),
        Err(_) => Err("the notification was not shown in time".into()),
    }
}

#[cfg(not(any(unix, windows)))]
async fn show_with_system_tools(_title: String, _body: String) -> Result<(), String> {
    Err("notifications are not supported on this system".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_toast_program_takes_its_texts_from_the_environment() {
        // Nothing the synth writes may become program text.
        let s = super::WINDOWS_TOAST_SCRIPT;
        assert!(s.contains("$env:SNTH_TOAST_TITLE") && s.contains("$env:SNTH_TOAST_BODY"));
        assert!(s.contains("SecurityElement]::Escape"), "the texts must be escaped before they enter the XML");
        assert!(!s.contains('\u{2014}'));
    }
}
