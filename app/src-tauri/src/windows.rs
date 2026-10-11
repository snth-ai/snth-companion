//! The two windows: the main one (hidden to the tray on close) and the
//! small always-on-top approval window.

use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub const MAIN: &str = "main";
pub const APPROVAL: &str = "approval";

/// Only the app's own pages may load in a window: the bundled assets, or
/// the local dev server in a debug build. Everything else is refused, so
/// a stray link can never put remote content next to the command bridge.
fn is_app_url(url: &Url) -> bool {
    match url.scheme() {
        "tauri" | "about" => true,
        "http" | "https" => match url.host_str() {
            Some("tauri.localhost") => true,
            Some("127.0.0.1") | Some("localhost") => {
                cfg!(debug_assertions) && url.port() == Some(1420)
            }
            _ => false,
        },
        _ => false,
    }
}

pub fn create_main(app: &AppHandle, visible: bool) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App("index.html".into()))
        .title("SNTH Connect")
        .inner_size(760.0, 560.0)
        .resizable(false)
        .maximizable(false)
        .fullscreen(false)
        .center()
        .visible(visible)
        .on_navigation(is_app_url)
        .build()
}

/// Bring the main window up (from the tray, the dock, a second launch or a
/// deep link).
pub fn show_main(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    let window = match app.get_webview_window(MAIN) {
        Some(w) => w,
        None => match create_main(app, true) {
            Ok(w) => w,
            Err(_) => return,
        },
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

/// Closing the main window keeps the app in the tray. On macOS the dock
/// icon goes with the window.
pub fn hide_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.hide();
    }
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
}

/// Show the approval window, creating it on first use. Never called on
/// the main thread (window creation from there can deadlock on Windows).
pub fn open_approval(app: &AppHandle) {
    let window = match app.get_webview_window(APPROVAL) {
        Some(w) => w,
        None => {
            let built = WebviewWindowBuilder::new(
                app,
                APPROVAL,
                WebviewUrl::App("approval.html".into()),
            )
            .title("SNTH Connect")
            .inner_size(440.0, 500.0)
            .resizable(false)
            .maximizable(false)
            .minimizable(false)
            .fullscreen(false)
            .always_on_top(true)
            .visible_on_all_workspaces(true)
            .skip_taskbar(true)
            .center()
            .visible(true)
            .on_navigation(is_app_url)
            .build();
            match built {
                Ok(w) => w,
                Err(_) => return,
            }
        }
    };
    let _ = window.show();
    let _ = window.set_focus();
}

pub fn hide_approval(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(APPROVAL) {
        let _ = w.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_app_pages_may_load() {
        let ok = |s: &str| is_app_url(&Url::parse(s).unwrap());
        assert!(ok("tauri://localhost/index.html"));
        assert!(ok("http://tauri.localhost/approval.html"));
        assert!(ok("https://tauri.localhost/index.html"));
        assert!(ok("about:blank"));
        assert!(!ok("https://example.com/"));
        assert!(!ok("https://hub.snth.ai/"));
        assert!(!ok("file:///etc/passwd"));
        assert!(!ok("http://127.0.0.1:8080/"));
        // The dev server is reachable in a debug build only.
        assert_eq!(ok("http://127.0.0.1:1420/"), cfg!(debug_assertions));
    }
}
