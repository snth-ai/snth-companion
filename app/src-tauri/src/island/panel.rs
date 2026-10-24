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
//! island again. Its `visibleFrame` (the screen less the menu bar and a
//! Dock that is not hidden) is the usable area for the spots other than
//! the default top centre (`geometry::place`, the owner's `Spot`). Nothing
//! promises that notification for the Dock moving, resizing or hiding, so
//! while the island floats (any spot but the notch) the poller in
//! `mod.rs` also asks for the placement again every `REPLACE_EVERY`; the
//! notch placement never polls.
//!
//! Windows: an always-on-top tool window (no taskbar button, no Alt-Tab)
//! with `WS_EX_NOACTIVATE`, so a click does not take the focus; the line
//! of text lifts that for as long as it is open. Out of screen capture
//! (`WDA_EXCLUDEFROMCAPTURE`, through Tauri's `content_protected`).
//!
//! Where it goes on Windows: there is no menu bar and the top of the
//! screen is browser tabs and title bars, so by default the island floats
//! at the bottom centre of the primary monitor, just above the taskbar
//! (`geometry::place`, `edge: Bottom`; a taskbar on the top edge puts it
//! just under the taskbar, `edge: Top`). The top centre, when the owner
//! chooses it, is the drawn notch flush with the monitor's top edge (under
//! a taskbar on the top edge), as on a Mac without a notch; the other
//! spots float. `screen_info` reads the
//! monitor's frame and its work area (Tauri's `Monitor::work_area`, the
//! `rcWork` of `GetMonitorInfoW`: the screen less the taskbar) in
//! physical pixels and turns both into logical points at the monitor's
//! own scale (`geometry::from_physical`); `place` turns the window's
//! rectangle back into pixels at that same scale (not the window's
//! current one: before the first move the window may sit on another
//! monitor with another DPI). The taskbar moving, auto-hide switched,
//! a resolution or scale change: the poller in `mod.rs` asks for the
//! placement again every few seconds (`REPLACE_EVERY`) and the window
//! moves only when it changed.
//!
//! The pointer: macOS `CGEventGetLocation` of a fresh event (any thread,
//! global points, top-left origin), Windows `GetCursorPos`.
//!
//! Full screen (`cover`): the island hides while the screen it is on
//! shows a full-screen app, and comes back with the menu bar. macOS: the
//! window list (`CGWindowListCopyWindowInfo`, on-screen windows only, any
//! thread, about 0.3 ms). Measured on macOS 26: in a normal Space the
//! system's menu bar is a window of "Window Server" at layer 24 across
//! the top of the screen; in a full-screen Space it is not in the list at
//! all until the pointer reaches the top edge, then it slides in from
//! above (first at y -50, then 0, about 0.25 s after the pointer got
//! there) and leaves the list again when the pointer goes down. The
//! full-screen app's window is a layer-0 window as large as the screen
//! (below the notch on a Mac with one). Only regular apps count: an
//! accessory app's overlay may cover the screen in every Space (one
//! did, on the Mac this was measured on). Space switches and app
//! activations (`NSWorkspace` notifications) ask for a fresh look at
//! once. Windows: the foreground window has no caption and covers the
//! primary monitor (a full-screen video, a game); there is no menu bar to
//! follow.

use tauri::WebviewWindow;

#[cfg(not(target_os = "macos"))]
use super::geometry;
use super::geometry::{Cover, Placement, ScreenInfo};

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

    use super::super::geometry::{self, Cover, Rect, ScreenInfo};

    const NS_NONACTIVATING_PANEL: usize = 1 << 7;
    const NS_MAIN_MENU_WINDOW_LEVEL: isize = 24;
    const LEVEL: isize = NS_MAIN_MENU_WINDOW_LEVEL + 3;
    // canJoinAllSpaces | stationary | ignoresCycle | fullScreenAuxiliary
    const BEHAVIOR: usize = (1 << 0) | (1 << 4) | (1 << 6) | (1 << 8);

    // kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements
    const LIST_ON_SCREEN: u32 = (1 << 0) | (1 << 4);
    // kCGMainMenuWindowLevel: the system's menu bar.
    const MENU_BAR_LAYER: isize = 24;
    const NS_APPLICATION_ACTIVATION_POLICY_REGULAR: isize = 0;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGSize {
        width: f64,
        height: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreate(source: *const std::ffi::c_void) -> *mut std::ffi::c_void;
        fn CGEventGetLocation(event: *mut std::ffi::c_void) -> CGPoint;
        fn CGGetDisplaysWithPoint(point: CGPoint, max: u32, displays: *mut u32, count: *mut u32) -> i32;
        fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> *const std::ffi::c_void;
        fn CGRectMakeWithDictionaryRepresentation(dict: *const AnyObject, rect: *mut CGRect) -> bool;
        static kCGWindowLayer: &'static AnyObject;
        static kCGWindowBounds: &'static AnyObject;
        static kCGWindowOwnerPID: &'static AnyObject;
        static kCGWindowOwnerName: &'static AnyObject;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *const std::ffi::c_void);
    }
    #[link(name = "AppKit", kind = "framework")]
    extern "C" {
        static NSApplicationDidChangeScreenParametersNotification: &'static AnyObject;
        static NSWorkspaceActiveSpaceDidChangeNotification: &'static AnyObject;
        static NSWorkspaceDidActivateApplicationNotification: &'static AnyObject;
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
            // The visible frame (the screen less the menu bar and a Dock
            // that is not hidden, wherever it is): where the floating
            // spots go. The default spot (the notch) does not use it.
            Some(ScreenInfo { frame: top_left(frame), notch, bar_h, work: Some(top_left(visible)), menu_bar: true })
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

    /// Run `changed` on the main thread for every `name` posted to this
    /// notification centre. The observer lives as long as the app.
    fn observe(center: &AnyObject, name: &AnyObject, changed: std::rc::Rc<dyn Fn() + 'static>) {
        let Some(queue) = AnyClass::get(c"NSOperationQueue") else { return };
        let queue: *mut AnyObject = unsafe { msg_send![class_object(queue), mainQueue] };
        let Some(queue) = (unsafe { queue.as_ref() }) else { return };
        let block = RcBlock::new(move |_note: *mut AnyObject| changed());
        let token: *mut AnyObject = unsafe {
            msg_send![center, addObserverForName: name, object: std::ptr::null::<AnyObject>(), queue: queue, usingBlock: &*block]
        };
        // Kept for the life of the app.
        if !token.is_null() {
            let _: *mut AnyObject = unsafe { msg_send![&*token, retain] };
        }
    }

    /// Call `changed` (on the main thread) whenever screens or their
    /// resolution change. The observer lives as long as the app.
    pub fn watch_screens(changed: Box<dyn Fn() + 'static>) {
        guarded("watching the screens", || {
            let Some(center) = AnyClass::get(c"NSNotificationCenter") else { return };
            let center: *mut AnyObject = unsafe { msg_send![class_object(center), defaultCenter] };
            let Some(center) = (unsafe { center.as_ref() }) else { return };
            let name = unsafe { NSApplicationDidChangeScreenParametersNotification };
            observe(center, name, changed.into());
        });
    }

    /// Call `changed` (on the main thread) when another Space shows or
    /// another app comes to the front: a full-screen app may have come
    /// or gone. These are posted to the workspace's own centre.
    pub fn watch_spaces(changed: Box<dyn Fn() + 'static>) {
        guarded("watching the Spaces", || {
            let Some(workspace) = AnyClass::get(c"NSWorkspace") else { return };
            let workspace: *mut AnyObject = unsafe { msg_send![class_object(workspace), sharedWorkspace] };
            let Some(workspace) = (unsafe { workspace.as_ref() }) else { return };
            let center: *mut AnyObject = unsafe { msg_send![workspace, notificationCenter] };
            let Some(center) = (unsafe { center.as_ref() }) else { return };
            let changed: std::rc::Rc<dyn Fn() + 'static> = changed.into();
            observe(center, unsafe { NSWorkspaceActiveSpaceDidChangeNotification }, changed.clone());
            observe(center, unsafe { NSWorkspaceDidActivateApplicationNotification }, changed);
        });
    }

    fn integer(dict: &AnyObject, key: &AnyObject) -> Option<isize> {
        let n: *mut AnyObject = unsafe { msg_send![dict, objectForKey: key] };
        let n = unsafe { n.as_ref()? };
        Some(unsafe { msg_send![n, integerValue] })
    }

    /// A window's bounds, already in top-left global points.
    fn bounds(dict: &AnyObject) -> Option<Rect> {
        let b: *mut AnyObject = unsafe { msg_send![dict, objectForKey: kCGWindowBounds] };
        let b = unsafe { b.as_ref()? };
        let mut r = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 0.0, height: 0.0 } };
        unsafe { CGRectMakeWithDictionaryRepresentation(b, &mut r) }.then_some(Rect { x: r.origin.x, y: r.origin.y, w: r.size.width, h: r.size.height })
    }

    fn owned_by_window_server(dict: &AnyObject) -> bool {
        let name: *mut AnyObject = unsafe { msg_send![dict, objectForKey: kCGWindowOwnerName] };
        let Some(name) = (unsafe { name.as_ref() }) else { return false };
        let utf8: *const std::ffi::c_char = unsafe { msg_send![name, UTF8String] };
        !utf8.is_null() && unsafe { std::ffi::CStr::from_ptr(utf8) }.to_bytes() == b"Window Server"
    }

    /// A regular app (in the Dock, can go full screen), not an accessory
    /// app's overlay.
    fn regular_app(pid: i32) -> bool {
        let Some(class) = AnyClass::get(c"NSRunningApplication") else { return false };
        let app: *mut AnyObject = unsafe { msg_send![class_object(class), runningApplicationWithProcessIdentifier: pid] };
        let Some(app) = (unsafe { app.as_ref() }) else { return false };
        let policy: isize = unsafe { msg_send![app, activationPolicy] };
        policy == NS_APPLICATION_ACTIVATION_POLICY_REGULAR
    }

    /// What the screen shows now (see the module's notes). Any thread.
    pub fn cover(screen: &ScreenInfo) -> Option<Cover> {
        let list = unsafe { CGWindowListCopyWindowInfo(LIST_ON_SCREEN, 0) };
        if list.is_null() {
            return None;
        }
        let found = guarded("reading the window list", || {
            // A CFArray of CFDictionaries is an NSArray of NSDictionaries.
            let list = unsafe { &*(list as *const AnyObject) };
            let count: usize = unsafe { msg_send![list, count] };
            let mut cover = Cover::default();
            for i in 0..count {
                let w: *mut AnyObject = unsafe { msg_send![list, objectAtIndex: i] };
                let Some(w) = (unsafe { w.as_ref() }) else { continue };
                let Some(layer) = integer(w, unsafe { kCGWindowLayer }) else { continue };
                if layer != 0 && layer != MENU_BAR_LAYER {
                    continue;
                }
                let Some(r) = bounds(w) else { continue };
                if layer == MENU_BAR_LAYER {
                    if !cover.bar && geometry::bar_on_screen(screen, r) && owned_by_window_server(w) {
                        cover.bar = true;
                    }
                } else if !cover.covered && geometry::fills_screen(screen, r) {
                    let pid = integer(w, unsafe { kCGWindowOwnerPID }).unwrap_or(0) as i32;
                    cover.covered = pid > 0 && regular_app(pid);
                }
            }
            cover
        });
        unsafe { CFRelease(list) };
        found
    }
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONULL, MONITOR_DEFAULTTOPRIMARY};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, GetDesktopWindow, GetForegroundWindow, GetShellWindow, GetWindowLongPtrW, GetWindowRect, SetWindowLongPtrW, GWL_EXSTYLE, GWL_STYLE,
        WS_CAPTION, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
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

    /// The foreground window fills the primary monitor (where the island
    /// is) and has no caption: a full-screen video, a game, a slide show.
    /// A maximized window keeps its caption; the desktop and the shell
    /// are not apps. Any thread.
    pub fn full_screen() -> bool {
        unsafe {
            let fg = GetForegroundWindow();
            if fg.is_null() || fg == GetDesktopWindow() || fg == GetShellWindow() {
                return false;
            }
            let style = GetWindowLongPtrW(fg, GWL_STYLE) as u32;
            if style & WS_CAPTION == WS_CAPTION {
                return false;
            }
            let monitor = MonitorFromWindow(fg, MONITOR_DEFAULTTONULL);
            if monitor.is_null() || monitor != MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) {
                return false;
            }
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            if GetMonitorInfoW(monitor, &mut info) == 0 || GetWindowRect(fg, &mut r) == 0 {
                return false;
            }
            let m = info.rcMonitor;
            r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
        }
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
        let frame = geometry::from_physical(pos.x, pos.y, size.width, size.height, scale);
        // Windows: the work area makes the island float above the
        // taskbar (and keeps a chosen top centre off a top taskbar).
        // Elsewhere (Linux) the drawn notch as before.
        #[cfg(windows)]
        let work = {
            let wa = monitor.work_area();
            Some(geometry::from_physical(wa.position.x, wa.position.y, wa.size.width, wa.size.height, scale))
        };
        #[cfg(not(windows))]
        let work = None;
        Some(ScreenInfo { frame, notch: None, bar_h: 0.0, work, menu_bar: false })
    }
}

/// The primary monitor's scale, the one `screen_info` measured with.
#[cfg(not(target_os = "macos"))]
fn primary_scale(window: &WebviewWindow) -> f64 {
    window
        .primary_monitor()
        .ok()
        .flatten()
        .map(|m| m.scale_factor())
        .or_else(|| window.scale_factor().ok())
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or(1.0)
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
        // Pixels at the primary monitor's scale, where the placement was
        // measured. Moved first: a move onto a monitor with another DPI
        // resizes the window by itself, the size set after it is the one
        // that stays; moved again in case the resize shifted it.
        let scale = primary_scale(window);
        let pos = tauri::PhysicalPosition::new((p.window.x * scale).round() as i32, (p.window.y * scale).round() as i32);
        let _ = window.set_position(pos);
        let _ = window.set_size(tauri::PhysicalSize::new((p.window.w * scale).round() as u32, (p.window.h * scale).round() as u32));
        let _ = window.set_position(pos);
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

/// Call `changed` on the main thread when another Space shows or another
/// app comes to the front (macOS). Elsewhere the poller's own look is
/// enough.
pub fn watch_spaces(changed: Box<dyn Fn() + 'static>) {
    #[cfg(target_os = "macos")]
    mac::watch_spaces(changed);
    #[cfg(not(target_os = "macos"))]
    let _ = changed;
}

/// Does the island's screen show a full-screen app now, and the menu bar
/// on it? None: the system did not say. Any thread.
pub fn cover(screen: &ScreenInfo) -> Option<Cover> {
    #[cfg(target_os = "macos")]
    {
        mac::cover(screen)
    }
    #[cfg(windows)]
    {
        let _ = screen;
        Some(Cover { covered: win::full_screen(), bar: false })
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = screen;
        None
    }
}
