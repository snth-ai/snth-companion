//! Linux: only what common desktop tools give for free, and only when
//! the tool is installed. Never used on a real machine yet.
//!
//! - notifications: the host's `Notifier` (`notify-send` in the
//!   fallback), offered when `notify-send` exists
//! - the clipboard: `wl-copy` / `wl-paste` on Wayland, else `xclip`,
//!   else `xsel`
//! - open: a web address or a document in a shared folder with
//!   `xdg-open`. Starting an app by its name is not built (there is no
//!   one way to find it).

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use serde_json::json;
use tokio::io::AsyncWriteExt;

use super::request::{self, Request};
use super::{AppError, Backend, Call, CallFuture, Notifier, Offer};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Clip {
    Wayland,
    Xclip,
    Xsel,
}

pub struct LinuxBackend {
    notifier: Arc<dyn Notifier>,
    notify: bool,
    clip: Option<Clip>,
    open: bool,
}

fn in_path(tool: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| PathBuf::from(dir).join(tool).is_file()))
        .unwrap_or(false)
}

impl LinuxBackend {
    pub fn detect(notifier: Arc<dyn Notifier>) -> LinuxBackend {
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        let clip = if wayland && in_path("wl-copy") && in_path("wl-paste") {
            Some(Clip::Wayland)
        } else if in_path("xclip") {
            Some(Clip::Xclip)
        } else if in_path("xsel") {
            Some(Clip::Xsel)
        } else {
            None
        };
        LinuxBackend { notifier, notify: in_path("notify-send"), clip, open: in_path("xdg-open") }
    }
}

/// Run a tool. With `stdin` the tool is a writer: its output is not
/// collected, because `xclip` and `wl-copy` leave a helper behind that
/// keeps the output open for as long as it holds the clipboard.
async fn tool(program: &str, args: &[&str], stdin: Option<&[u8]>) -> Result<Vec<u8>, AppError> {
    let writer = stdin.is_some();
    let mut child = crate::proc::command(program)
        .args(args)
        .stdin(if writer { Stdio::piped() } else { Stdio::null() })
        .stdout(if writer { Stdio::null() } else { Stdio::piped() })
        .stderr(Stdio::null())
        .kill_on_drop(!writer)
        .spawn()
        .map_err(|e| AppError::Unavailable(format!("cannot run {program}: {e}")))?;
    if let (Some(mut pipe), Some(bytes)) = (child.stdin.take(), stdin) {
        let _ = pipe.write_all(bytes).await;
        let _ = pipe.shutdown().await;
    }
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| AppError::Failed(format!("{program} did not finish: {e}")))?;
    if !out.status.success() {
        return Err(AppError::Failed(format!("{program} failed")));
    }
    Ok(out.stdout)
}

impl Backend for LinuxBackend {
    fn offered(&self) -> Vec<Offer> {
        let mut out = Vec::new();
        if self.notify {
            out.push(Offer::all("notify"));
        }
        if self.clip.is_some() {
            out.push(Offer::all("clipboard"));
        }
        if self.open {
            out.push(Offer { app: "open", actions: vec!["url", "file"] });
        }
        out
    }

    fn call(&self, call: Call) -> CallFuture {
        let notifier = self.notifier.clone();
        let clip = self.clip;
        Box::pin(async move {
            match call.request {
                Request::NotifyShow { title, body } => {
                    let title = format!("{}: {title}", call.synth_name);
                    notifier.show(title, body).await.map_err(AppError::Unavailable)?;
                    Ok(json!({"shown": true}))
                }
                Request::ClipboardRead => {
                    let out = match clip {
                        Some(Clip::Wayland) => tool("wl-paste", &["--no-newline"], None).await?,
                        Some(Clip::Xclip) => tool("xclip", &["-selection", "clipboard", "-o"], None).await?,
                        Some(Clip::Xsel) => tool("xsel", &["--clipboard", "--output"], None).await?,
                        None => return Err(AppError::Unavailable("no clipboard tool is installed".into())),
                    };
                    let text = String::from_utf8_lossy(&out).to_string();
                    let (text, truncated) = request::cut(&text, request::CLIPBOARD_MAX);
                    Ok(json!({"text": text, "truncated": truncated}))
                }
                Request::ClipboardWrite { text } => {
                    match clip {
                        Some(Clip::Wayland) => tool("wl-copy", &[], Some(text.as_bytes())).await?,
                        Some(Clip::Xclip) => tool("xclip", &["-selection", "clipboard", "-i"], Some(text.as_bytes())).await?,
                        Some(Clip::Xsel) => tool("xsel", &["--clipboard", "--input"], Some(text.as_bytes())).await?,
                        None => return Err(AppError::Unavailable("no clipboard tool is installed".into())),
                    };
                    Ok(json!({"bytes": text.len()}))
                }
                Request::OpenUrl { url, .. } => {
                    tool("xdg-open", &[url.as_str()], None).await?;
                    Ok(json!({"opened": true}))
                }
                Request::OpenFile { path } => {
                    let shown = path.to_string_lossy().to_string();
                    tool("xdg-open", &[shown.as_str()], None).await?;
                    Ok(json!({"opened": true, "path": shown}))
                }
                _ => Err(AppError::Unavailable("that app does not exist on this system".into())),
            }
        })
    }
}
