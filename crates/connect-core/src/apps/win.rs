//! Windows: what works on every Windows 10 and 11 without installing or
//! configuring anything, and without administrator rights.
//!
//! - notifications: the host's `Notifier` (a toast)
//! - the clipboard: the Win32 clipboard, text only (`CF_UNICODETEXT`)
//! - open: a web address, a document in a shared folder, or an app from
//!   the Start menu by its name. `ShellExecuteW` does the opening; the
//!   Start menu is read with PowerShell's `Get-StartApps`.
//!
//! None of the Mac apps exist here and none is offered. What else could
//! be driven on Windows (Outlook and so on) is in the README, with why it
//! is not built.

use std::process::Stdio;
use std::sync::Arc;

use serde_json::json;

use super::request::{self, Request};
use super::{AppError, Backend, Call, CallFuture, Notifier, Offer};

pub struct WinBackend {
    notifier: Arc<dyn Notifier>,
}

impl WinBackend {
    pub fn new(notifier: Arc<dyn Notifier>) -> WinBackend {
        WinBackend { notifier }
    }
}

impl Backend for WinBackend {
    fn offered(&self) -> Vec<Offer> {
        vec![Offer::all("notify"), Offer::all("clipboard"), Offer::all("open")]
    }

    fn call(&self, call: Call) -> CallFuture {
        let notifier = self.notifier.clone();
        Box::pin(async move {
            match call.request {
                Request::NotifyShow { title, body } => {
                    let title = format!("{}: {title}", call.synth_name);
                    notifier.show(title, body).await.map_err(AppError::Unavailable)?;
                    Ok(json!({"shown": true}))
                }
                Request::ClipboardRead => {
                    let text = blocking(clipboard::read).await?;
                    let (text, truncated) = request::cut(&text, request::CLIPBOARD_MAX);
                    Ok(json!({"text": text, "truncated": truncated}))
                }
                Request::ClipboardWrite { text } => {
                    let bytes = text.len();
                    blocking(move || clipboard::write(&text)).await?;
                    Ok(json!({"bytes": bytes}))
                }
                Request::OpenUrl { url, .. } => {
                    blocking(move || shell_open(&url)).await?;
                    Ok(json!({"opened": true}))
                }
                Request::OpenFile { path } => {
                    let shown = path.to_string_lossy().to_string();
                    let target = shown.clone();
                    blocking(move || shell_open(&target)).await?;
                    Ok(json!({"opened": true, "path": shown}))
                }
                Request::OpenApp { name } => {
                    let id = start_app_id(&name).await?;
                    blocking(move || shell_open(&format!("shell:AppsFolder\\{id}"))).await?;
                    Ok(json!({"opened": true, "name": name}))
                }
                _ => Err(AppError::Unavailable("that app does not exist on Windows".into())),
            }
        })
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, AppError> + Send + 'static) -> Result<T, AppError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| AppError::Failed(format!("the call stopped: {e}")))?
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Hand a target to the Windows shell ("open"): a web address goes to the
/// default browser, a document to its app, `shell:AppsFolder\<id>` starts
/// a Start menu app.
pub fn shell_open(target: &str) -> Result<(), AppError> {
    use std::ffi::c_void;
    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(hwnd: *mut c_void, verb: *const u16, file: *const u16, params: *const u16, dir: *const u16, show: i32) -> isize;
    }
    const SW_SHOWNORMAL: i32 = 1;
    let verb = wide("open");
    let file = wide(target);
    let code = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
    // ShellExecute answers above 32 when it worked.
    if code > 32 {
        Ok(())
    } else {
        Err(AppError::Failed(format!("Windows could not open it (code {code})")))
    }
}

/// Lists the Start menu as `Name<TAB>AppID` lines.
const START_APPS_SCRIPT: &str = "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; Get-StartApps | ForEach-Object { $_.Name + \"`t\" + $_.AppID }";

/// The Start menu apps as (name, id).
pub async fn start_apps() -> Result<Vec<(String, String)>, AppError> {
    let run = crate::proc::command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", START_APPS_SCRIPT])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(std::time::Duration::from_secs(30), run)
        .await
        .map_err(|_| AppError::Unavailable("Windows did not list its apps in time".into()))?
        .map_err(|e| AppError::Unavailable(format!("cannot run PowerShell: {e}")))?;
    if !out.status.success() {
        return Err(AppError::Unavailable("Windows could not list its apps".into()));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim_end().split_once('\t'))
        .map(|(n, id)| (n.trim().to_string(), id.trim().to_string()))
        .filter(|(n, id)| !n.is_empty() && !id.is_empty())
        .collect())
}

/// The Start menu id of the app with this name (letter case ignored).
async fn start_app_id(name: &str) -> Result<String, AppError> {
    let want = name.to_lowercase();
    start_apps()
        .await?
        .into_iter()
        .find(|(n, _)| n.to_lowercase() == want)
        .map(|(_, id)| id)
        .ok_or_else(|| AppError::NotFound(format!("no app named {name:?} in the Start menu")))
}

pub mod clipboard {
    use super::{wide, AppError};
    use std::ffi::c_void;

    #[link(name = "user32")]
    extern "system" {
        fn OpenClipboard(owner: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn GetClipboardData(format: u32) -> *mut c_void;
        fn SetClipboardData(format: u32, mem: *mut c_void) -> *mut c_void;
        fn IsClipboardFormatAvailable(format: u32) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
        fn GlobalLock(mem: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(mem: *mut c_void) -> i32;
        fn GlobalFree(mem: *mut c_void) -> *mut c_void;
        fn GlobalSize(mem: *mut c_void) -> usize;
    }

    const CF_UNICODETEXT: u32 = 13;
    const GMEM_MOVEABLE: u32 = 0x0002;

    /// Another program may hold the clipboard for a moment: try a few
    /// times before giving up.
    fn open() -> Result<(), AppError> {
        for _ in 0..20 {
            if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        Err(AppError::Unavailable("another program is using the clipboard".into()))
    }

    /// The clipboard as text. Empty when it holds no text.
    pub fn read() -> Result<String, AppError> {
        open()?;
        let text = unsafe {
            let mut text = String::new();
            if IsClipboardFormatAvailable(CF_UNICODETEXT) != 0 {
                let mem = GetClipboardData(CF_UNICODETEXT);
                if !mem.is_null() {
                    let p = GlobalLock(mem) as *const u16;
                    if !p.is_null() {
                        let max = GlobalSize(mem) / 2;
                        let mut len = 0;
                        while len < max && *p.add(len) != 0 {
                            len += 1;
                        }
                        text = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
                        GlobalUnlock(mem);
                    }
                }
            }
            CloseClipboard();
            text
        };
        Ok(text)
    }

    /// Replace the clipboard with this text.
    pub fn write(text: &str) -> Result<(), AppError> {
        // Windows line ends, as every Windows program expects them.
        let data = wide(&text.replace("\r\n", "\n").replace('\n', "\r\n"));
        open()?;
        let ok = unsafe {
            EmptyClipboard();
            let mem = GlobalAlloc(GMEM_MOVEABLE, data.len() * 2);
            let mut ok = false;
            if !mem.is_null() {
                let p = GlobalLock(mem) as *mut u16;
                if !p.is_null() {
                    std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
                    GlobalUnlock(mem);
                    // On success the system owns the memory.
                    ok = !SetClipboardData(CF_UNICODETEXT, mem).is_null();
                }
                if !ok {
                    GlobalFree(mem);
                }
            }
            CloseClipboard();
            ok
        };
        if ok {
            Ok(())
        } else {
            Err(AppError::Failed("Windows did not accept the clipboard text".into()))
        }
    }
}

