//! The island's window as each system needs it.
//!
//! macOS: the Tauri window is turned into a non-activating NSPanel, the
//! way Spotlight-like panels are made: its class is switched to a
//! subclass of NSPanel made at run time (`SNTHIslandPanel`, no ivars of
//! its own, `canBecomeKeyWindow` YES for the line of text and
//! `canBecomeMainWindow` NO), its style gets `NonactivatingPanel`, so a
//! click on it never activates the app and never takes the focus from
//! the app the owner works in. Then: a level above the menu bar
//! (`NSMainMenuWindowLevel + 3`, below the menus that drop from it),
//! every Space, over full-screen apps, out of Cmd-Tab and Exposé
//! (`canJoinAllSpaces | stationary | ignoresCycle | fullScreenAuxiliary`),
//! never hidden when the app is not active, no shadow, out of screen
//! capture. This is what the `tauri-nspanel` crate does; it is done here
//! with objc2 directly (already in the build, see safari.rs) instead of a
//! git dependency, and every send runs inside `exception::catch`: an
//! Objective-C exception must never reach Rust (the 0.1.7 crash).
//! All of it runs on the main thread.
//!
//! The notch: `NSScreen.safeAreaInsets.top` (its height) and the two
//! `auxiliaryTopLeftArea` / `auxiliaryTopRightArea` beside it (its edges),
//! macOS 12 and later, asked with `respondsToSelector:` first. The screen
//! is the one with the menu bar; a change of screens or resolution
//! (`NSApplicationDidChangeScreenParametersNotification`) places the
//! island again.
//!
//! Windows: an always-on-top tool window (no taskbar button, no Alt-Tab)
//! with `WS_EX_NOACTIVATE`, so a click does not take the focus; the line
//! of text lifts that for as long as it is open. Out of screen capture
//! (`WDA_EXCLUDEFROMCAPTURE`, through Tauri's `content_protected`).
//!
//! The pointer: macOS `CGEventGetLocation` of a fresh event (any thread,
//! global points, top-left origin), Windows `GetCursorPos`.

use tauri::WebviewWindow;

#[cfg(not(target_os = "macos"))]
use super::geometry::Rect;
use super::geometry::{Placement, ScreenInfo};

#[cfg(target_os = "macos")]
mod mac {
    use std::panic::AssertUnwindSafe;
    use std::sync::OnceLock;

    use block2::RcBlock;
    use objc2::exception;
    use objc2::rc::autoreleasepool;
    use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
    use objc2::{msg_send, sel};
    use objc2_foundation::{NSEdgeInsets, NSPoint, NSRect, NSSize};

    use super::super::geometry::{Rect, ScreenInfo};

    const NS_NONACTIVATING_PANEL: usize = 1 << 7;
    const NS_MAIN_MENU_WINDOW_LEVEL: isize = 24;
    const LEVEL: isize = NS_MAIN_MENU_WINDOW_LEVEL + 3;
    // canJoinAllSpaces | stationary | ignoresCycle | fullScreenAuxiliary
    const BEHAVIOR: usize = (1 << 0) | (1 << 4) | (1 << 6) | (1 << 8);

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreate(source: *const std::ffi::c_void) -> *mut std::ffi::c_void;
        fn CGEventGetLocation(event: *mut std::ffi::c_void) -> CGPoint;
        fn CGGetDisplaysWithPoint(point: CGPoint, max: u32, displays: *mut u32, count: *mut u32) -> i32;
        fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *const std::ffi::c_void);
    }
    #[link(name = "AppKit", kind = "framework")]
    extern "C" {
        static NSApplicationDidChangeScreenParametersNotification: &'static AnyObject;
    }

    fn guarded<R>(what: &str, call: impl FnOnce() -> R) -> Option<R> {
        match exception::catch(AssertUnwindSafe(|| autoreleasepool(|_| call()))) {
            Ok(v) => Some(v),
            Err(e) => {
                let why = e.as_deref().map_or_else(|| "nil".to_string(), |e| format!("{e:?}"));
                connect_core::logln!("island: {what} raised an Objective-C exception: {why}");
                None
            }
        }
    }

    fn responds(receiver: &AnyObject, selector: Sel) -> bool {
        unsafe { msg_send![receiver, respondsToSelector: selector] }
    }

    fn class_object(class: &AnyClass) -> &AnyObject {
        unsafe { &*(class as *const AnyClass as *const AnyObject) }
    }

    extern "C-unwind" fn yes(_this: &AnyObject, _sel: Sel) -> Bool {
        Bool::YES
    }
    extern "C-unwind" fn no(_this: &AnyObject, _sel: Sel) -> Bool {
        Bool::NO
    }

    /// The NSPanel subclass, made once.
    fn panel_class() -> Option<&'static AnyClass> {
        static CLASS: OnceLock<Option<usize>> = OnceLock::new();
        let made = CLASS.get_or_init(|| {
            let panel = AnyClass::get(c"NSPanel")?;
            if let Some(have) = AnyClass::get(c"SNTHIslandPanel") {
                return Some(have as *const AnyClass as usize);
            }
            let mut builder = ClassBuilder::new(c"SNTHIslandPanel", panel)?;
            unsafe {
                builder.add_method(sel!(canBecomeKeyWindow), yes as extern "C-unwind" fn(_, _) -> _);
                builder.add_method(sel!(canBecomeMainWindow), no as extern "C-unwind" fn(_, _) -> _);
            }
            Some(builder.register() as *const AnyClass as usize)
        });
        made.map(|p| unsafe { &*(p as *const AnyClass) })
    }

    /// Turn the window into the island's panel. Main thread.
    pub fn make_panel(ns_window: usize) -> bool {
        let Some(class) = panel_class() else {
            return false;
        };
        let window = unsafe { &*(ns_window as *const AnyObject) };
        guarded("making the island panel", || {
            unsafe { objc2::ffi::object_setClass(ns_window as *mut AnyObject, class) };
            let mask: usize = unsafe { msg_send![window, styleMask] };
            let _: () = unsafe { msg_send![window, setStyleMask: mask | NS_NONACTIVATING_PANEL] };
            // The window server's own "never activate" flag: what the
            // style above sets when a panel is created with it.
            if responds(window, sel!(_setPreventsActivation:)) {
                let _: () = unsafe { msg_send![window, _setPreventsActivation: Bool::YES] };
            }
            let _: () = unsafe { msg_send![window, setLevel: LEVEL] };
            let _: () = unsafe { msg_send![window, setCollectionBehavior: BEHAVIOR] };
            let _: () = unsafe { msg_send![window, setHidesOnDeactivate: Bool::NO] };
            let _: () = unsafe { msg_send![window, setBecomesKeyOnlyIfNeeded: Bool::YES] };
            let _: () = unsafe { msg_send![window, setHasShadow: Bool::NO] };
            let _: () = unsafe { msg_send![window, setMovable: Bool::NO] };
            // NSWindowSharingNone: not in screenshots or recordings.
            let _: () = unsafe { msg_send![window, setSharingType: 0usize] };
            true
        })
        .unwrap_or(false)
    }

    fn primary_height() -> Option<f64> {
        let class = class_object(AnyClass::get(c"NSScreen")?);
        let screens: *mut AnyObject = unsafe { msg_send![class, screens] };
        let screens = unsafe { screens.as_ref()? };
        let count: usize = unsafe { msg_send![screens, count] };
        if count == 0 {
            return None;
        }
        let first: *mut AnyObject = unsafe { msg_send![screens, objectAtIndex: 0usize] };
        let frame: NSRect = unsafe { msg_send![&*first, frame] };
        Some(frame.size.height)
    }

    /// The screen with the menu bar, in top-left global points. Main
    /// thread.
    pub fn screen_info() -> Option<ScreenInfo> {
        guarded("reading the screen", || {
            let class = class_object(AnyClass::get(c"NSScreen")?);
            let screens: *mut AnyObject = unsafe { msg_send![class, screens] };
            let screens = unsafe { screens.as_ref()? };
            let count: usize = unsafe { msg_send![screens, count] };
            if count == 0 {
                return None;
            }
            let screen: *mut AnyObject = unsafe { msg_send![screens, objectAtIndex: 0usize] };
            let screen = unsafe { screen.as_ref()? };
            let frame: NSRect = unsafe { msg_send![screen, frame] };
            let visible: NSRect = unsafe { msg_send![screen, visibleFrame] };
            let h = frame.size.height;
            let top_left = |r: NSRect| Rect { x: r.origin.x, y: h - (r.origin.y + r.size.height), w: r.size.width, h: r.size.height };
            let mut bar_h = (frame.origin.y + frame.size.height) - (visible.origin.y + visible.size.height);
            if bar_h <= 0.0 {
                // The menu bar hides itself: the status bar says how tall
                // it is when shown.
                if let Some(bar) = AnyClass::get(c"NSStatusBar") {
                    let bar: *mut AnyObject = unsafe { msg_send![class_object(bar), systemStatusBar] };
                    if let Some(bar) = unsafe { bar.as_ref() } {
                        bar_h = unsafe { msg_send![bar, thickness] };
                    }
                }
            }
            let mut notch = None;
            if responds(screen, sel!(safeAreaInsets)) && responds(screen, sel!(auxiliaryTopLeftArea)) && responds(screen, sel!(auxiliaryTopRightArea)) {
                let insets: NSEdgeInsets = unsafe { msg_send![screen, safeAreaInsets] };
                if insets.top > 0.0 {
                    let left: NSRect = unsafe { msg_send![screen, auxiliaryTopLeftArea] };
                    let right: NSRect = unsafe { msg_send![screen, auxiliaryTopRightArea] };
                    if left.size.width > 0.0 && right.size.width > 0.0 {
                        notch = Some((left.origin.x + left.size.width, right.origin.x, insets.top));
                    }
                }
            }
            Some(ScreenInfo { frame: top_left(frame), notch, bar_h })
        })
        .flatten()
    }

    /// Put the window at this rectangle (top-left global points). Main
    /// thread. Set directly: a borderless window may cover the menu bar.
    pub fn set_frame(ns_window: usize, r: Rect) {
        let window = unsafe { &*(ns_window as *const AnyObject) };
        guarded("placing the island", || {
            let Some(h) = primary_height() else { return };
            let frame = NSRect::new(NSPoint::new(r.x, h - (r.y + r.h)), NSSize::new(r.w, r.h));
            let _: () = unsafe { msg_send![window, setFrame: frame, display: Bool::YES] };
        });
    }

    /// Show without making it key or activating the app. Main thread.
    pub fn show(ns_window: usize) {
        let window = unsafe { &*(ns_window as *const AnyObject) };
        guarded("showing the island", || {
            let _: () = unsafe { msg_send![window, orderFrontRegardless] };
        });
    }

    pub fn set_alpha(ns_window: usize, alpha: f64) {
        let window = unsafe { &*(ns_window as *const AnyObject) };
        guarded("fading the island", || {
            let _: () = unsafe { msg_send![window, setAlphaValue: alpha] };
        });
    }

    /// The line of text takes the keyboard (without activating the app),
    /// or gives it back.
    pub fn set_key(ns_window: usize, key: bool) {
        let window = unsafe { &*(ns_window as *const AnyObject) };
        guarded("keyboard for the island", || {
            if key {
                let _: () = unsafe { msg_send![window, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()] };
            } else {
                let is_key: bool = unsafe { msg_send![window, isKeyWindow] };
                if is_key {
                    let _: () = unsafe { msg_send![window, resignKeyWindow] };
                }
            }
        });
    }

    pub fn cursor() -> Option<(f64, f64)> {
        unsafe {
            let event = CGEventCreate(std::ptr::null());
            if event.is_null() {
                return None;
            }
            let p = CGEventGetLocation(event);
            CFRelease(event);
            Some((p.x, p.y))
        }
    }

    /// The display under the pointer, numbered as `screencapture -D` takes
    /// it (1 = the main display).
    pub fn display_under_cursor() -> Option<u32> {
        let (x, y) = cursor()?;
        unsafe {
            let mut id = 0u32;
            let mut n = 0u32;
            if CGGetDisplaysWithPoint(CGPoint { x, y }, 1, &mut id, &mut n) != 0 || n == 0 {
                return None;
            }
            let mut all = [0u32; 16];
            let mut count = 0u32;
            if CGGetActiveDisplayList(16, all.as_mut_ptr(), &mut count) != 0 {
                return None;
            }
            all[..count as usize].iter().position(|d| *d == id).map(|i| i as u32 + 1)
        }
    }

    /// Call `changed` (on the main thread) whenever screens or their
    /// resolution change. The observer lives as long as the app.
    pub fn watch_screens(changed: Box<dyn Fn() + 'static>) {
        guarded("watching the screens", || {
            let Some(center) = AnyClass::get(c"NSNotificationCenter") else { return };
            let Some(queue) = AnyClass::get(c"NSOperationQueue") else { return };
            let center: *mut AnyObject = unsafe { msg_send![class_object(center), defaultCenter] };
            let queue: *mut AnyObject = unsafe { msg_send![class_object(queue), mainQueue] };
            let (Some(center), Some(queue)) = (unsafe { center.as_ref() }, unsafe { queue.as_ref() }) else { return };
            let block = RcBlock::new(move |_note: *mut AnyObject| changed());
            let name = unsafe { NSApplicationDidChangeScreenParametersNotification };
            let token: *mut AnyObject = unsafe {
                msg_send![center, addObserverForName: name, object: std::ptr::null::<AnyObject>(), queue: queue, usingBlock: &*block]
            };
            // Kept for the life of the app.
            if !token.is_null() {
                let _: *mut AnyObject = unsafe { msg_send![&*token, retain] };
            }
        });
    }
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::{HWND, POINT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };

    pub fn set_noactivate(hwnd: isize, on: bool) {
        let hwnd = hwnd as HWND;
        unsafe {
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let flags = (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) as isize;
            let next = if on { ex | flags } else { (ex & !(WS_EX_NOACTIVATE as isize)) | WS_EX_TOOLWINDOW as isize };
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next);
        }
    }

    /// Physical pixels.
    pub fn cursor() -> Option<(f64, f64)> {
        let mut p = POINT { x: 0, y: 0 };
        (unsafe { GetCursorPos(&mut p) } != 0).then(|| (f64::from(p.x), f64::from(p.y)))
    }
}

/// Make the window the island's: a panel on macOS, a tool window that is
/// never activated on Windows. Call once, on the main thread on macOS.
pub fn setup(window: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        if let Ok(ns) = window.ns_window() {
            if !mac::make_panel(ns as usize) {
                connect_core::logln!("island: the window could not be made a panel");
            }
        }
    }
    #[cfg(windows)]
    {
        if let Ok(hwnd) = window.hwnd() {
            win::set_noactivate(hwnd.0 as isize, true);
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = window;
}

/// The screen the island goes on. macOS: main thread.
pub fn screen_info(window: &WebviewWindow) -> Option<ScreenInfo> {
    #[cfg(target_os = "macos")]
    {
        let _ = window;
        mac::screen_info()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let monitor = window.primary_monitor().ok().flatten()?;
        let scale = monitor.scale_factor();
        let pos = monitor.position();
        let size = monitor.size();
        Some(ScreenInfo {
            frame: Rect { x: f64::from(pos.x) / scale, y: f64::from(pos.y) / scale, w: f64::from(size.width) / scale, h: f64::from(size.height) / scale },
            notch: None,
            bar_h: 0.0,
        })
    }
}

/// Put the window where the placement says. macOS: main thread.
pub fn place(window: &WebviewWindow, p: &Placement) {
    #[cfg(target_os = "macos")]
    {
        if let Ok(ns) = window.ns_window() {
            mac::set_frame(ns as usize, p.window);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let scale = window.scale_factor().unwrap_or(1.0);
        let _ = window.set_size(tauri::LogicalSize::new(p.window.w, p.window.h));
        let _ = window.set_position(tauri::PhysicalPosition::new((p.window.x * scale).round() as i32, (p.window.y * scale).round() as i32));
    }
}

pub fn show(window: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        if let Ok(ns) = window.ns_window() {
            mac::show(ns as usize);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window.show();
    }
}

/// Invisible (for the moment of a screenshot) or visible again.
pub fn set_invisible(window: &WebviewWindow, invisible: bool) {
    #[cfg(target_os = "macos")]
    {
        if let Ok(ns) = window.ns_window() {
            mac::set_alpha(ns as usize, if invisible { 0.0 } else { 1.0 });
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Out of capture already (content protection); nothing to do.
        let _ = (window, invisible);
    }
}

/// The line of text takes the keyboard, or gives it back.
pub fn set_keyboard(window: &WebviewWindow, on: bool) {
    #[cfg(target_os = "macos")]
    {
        if let Ok(ns) = window.ns_window() {
            mac::set_key(ns as usize, on);
        }
    }
    #[cfg(windows)]
    {
        if let Ok(hwnd) = window.hwnd() {
            win::set_noactivate(hwnd.0 as isize, !on);
        }
        if on {
            let _ = window.set_focus();
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = (window, on);
}

/// The pointer, relative to the window's top-left corner, in the
/// webview's units (CSS pixels). `window_tl`: where the window is (top-left
/// global points on macOS; ignored elsewhere, where the window reports
/// its own position).
pub fn cursor_local(window: &WebviewWindow, window_tl: (f64, f64)) -> Option<(f64, f64)> {
    #[cfg(target_os = "macos")]
    {
        let _ = window;
        let (x, y) = mac::cursor()?;
        Some((x - window_tl.0, y - window_tl.1))
    }
    #[cfg(windows)]
    {
        let _ = window_tl;
        let (x, y) = win::cursor()?;
        let pos = window.outer_position().ok()?;
        let scale = window.scale_factor().unwrap_or(1.0);
        Some(((x - f64::from(pos.x)) / scale, (y - f64::from(pos.y)) / scale))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (window, window_tl);
        None
    }
}

/// The synth's pointer (screen points, top-left global, as the driver
/// takes them) relative to the window.
pub fn point_local(window: &WebviewWindow, window_tl: (f64, f64), point: (f64, f64)) -> (f64, f64) {
    #[cfg(windows)]
    {
        if let (Ok(pos), Ok(scale)) = (window.outer_position(), window.scale_factor()) {
            return (point.0 - f64::from(pos.x) / scale, point.1 - f64::from(pos.y) / scale);
        }
    }
    let _ = window;
    (point.0 - window_tl.0, point.1 - window_tl.1)
}

/// The display under the pointer for `screencapture -D` (macOS).
pub fn display_under_cursor() -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        mac::display_under_cursor()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Call `changed` on the main thread when the screens change (macOS).
/// Elsewhere the island checks the monitor now and then (`mod.rs`).
pub fn watch_screens(changed: Box<dyn Fn() + 'static>) {
    #[cfg(target_os = "macos")]
    mac::watch_screens(changed);
    #[cfg(not(target_os = "macos"))]
    let _ = changed;
}
