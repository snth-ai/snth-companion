//! The observe probes on macOS.
//!
//!  - The microphone (`Mic`): the processes other than this one that
//!    record now (`wake::system::recorders`, CoreAudio's process objects,
//!    macOS 14 and later), else whether the default input runs at all
//!    (`kAudioDevicePropertyDeviceIsRunningSomewhere`) next to whether
//!    this app holds it (`audio::own_capture`). Nothing is opened or
//!    recorded. The call apps that run: `NSWorkspace.runningApplications`.
//!  - The app in front (`Front`): `NSWorkspace.frontmostApplication`
//!    (name, bundle id, pid) and its focused window's title through
//!    Accessibility (`AXFocusedWindow`, `AXTitle`), only while the app
//!    already has Accessibility (the permission computer control asks for;
//!    observe never asks for it). The screen locked: nothing.
//!  - The Wi-Fi network (`Wifi`): CoreWLAN's `CWWiFiClient` interface
//!    `ssid`. Since macOS 14 it is nil unless the app may use Location
//!    Services (`CLLocationManager`): asked only when the owner switches
//!    the place on (`request_location`, on the main thread, with
//!    `NSLocationUsageDescription` in Info.plist and the hardened
//!    runtime's location entitlement).
//!
//! Every Objective-C send runs inside `exception::catch` (see safari.rs).

use std::ffi::{c_char, c_void, CStr};
use std::panic::AssertUnwindSafe;

use connect_core::observe::sources::{call_app_of, Front as FrontApp, FrontProbe, MicProbe, MicSample, Wifi as WifiNow, WifiProbe};
use connect_core::observe::AppMark;
use objc2::exception;
use objc2::rc::autoreleasepool;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{msg_send, sel};

use crate::island::wake::system;

type CFTypeRef = *const c_void;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    fn AXUIElementCopyAttributeValue(element: CFTypeRef, attribute: CFTypeRef, value: *mut CFTypeRef) -> i32;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, seconds: f32) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, encoding: u32) -> CFTypeRef;
    fn CFStringGetLength(s: CFTypeRef) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, size: isize, encoding: u32) -> bool;
}

// The two frameworks are reached through the Objective-C runtime; linking
// them makes their classes exist.
#[link(name = "CoreWLAN", kind = "framework")]
extern "C" {}
#[link(name = "CoreLocation", kind = "framework")]
extern "C" {}

const UTF8: u32 = 0x0800_0100;
/// A busy app answers Accessibility within this, or its title is skipped.
const AX_TIMEOUT_S: f32 = 0.25;
/// NSApplicationActivationPolicyRegular: an app with a Dock icon.
const POLICY_REGULAR: isize = 0;

fn ns_string(s: *mut AnyObject) -> Option<String> {
    if s.is_null() {
        return None;
    }
    let utf8: *const c_char = unsafe { msg_send![&*s, UTF8String] };
    (!utf8.is_null()).then(|| unsafe { CStr::from_ptr(utf8) }.to_string_lossy().into_owned())
}

fn class_object(name: &CStr) -> Option<&'static AnyObject> {
    let class = AnyClass::get(name)?;
    Some(unsafe { &*(class as *const AnyClass as *const AnyObject) })
}

/// One running app: pid, bundle id, name, activation policy.
struct App {
    pid: i32,
    bundle: String,
    name: String,
    policy: isize,
}

fn app_of(app: *mut AnyObject) -> Option<App> {
    if app.is_null() {
        return None;
    }
    let app = unsafe { &*app };
    let pid: i32 = unsafe { msg_send![app, processIdentifier] };
    let bundle: *mut AnyObject = unsafe { msg_send![app, bundleIdentifier] };
    let name: *mut AnyObject = unsafe { msg_send![app, localizedName] };
    let policy: isize = unsafe { msg_send![app, activationPolicy] };
    Some(App { pid, bundle: ns_string(bundle).unwrap_or_default(), name: ns_string(name).unwrap_or_default(), policy })
}

fn workspace() -> Option<*mut AnyObject> {
    let class = class_object(c"NSWorkspace")?;
    let ws: *mut AnyObject = unsafe { msg_send![class, sharedWorkspace] };
    (!ws.is_null()).then_some(ws)
}

fn frontmost() -> Option<App> {
    exception::catch(AssertUnwindSafe(|| {
        autoreleasepool(|_| {
            let ws = workspace()?;
            let app: *mut AnyObject = unsafe { msg_send![&*ws, frontmostApplication] };
            app_of(app)
        })
    }))
    .ok()
    .flatten()
}

fn running() -> Vec<App> {
    exception::catch(AssertUnwindSafe(|| {
        autoreleasepool(|_| {
            let Some(ws) = workspace() else { return Vec::new() };
            let list: *mut AnyObject = unsafe { msg_send![&*ws, runningApplications] };
            if list.is_null() {
                return Vec::new();
            }
            let n: usize = unsafe { msg_send![&*list, count] };
            (0..n.min(1000))
                .filter_map(|i| {
                    let app: *mut AnyObject = unsafe { msg_send![&*list, objectAtIndex: i] };
                    app_of(app)
                })
                .collect()
        })
    }))
    .unwrap_or_default()
}

/// May this app read other apps' windows (Accessibility)? Read-only,
/// never shows anything.
pub fn titles_allowed() -> bool {
    unsafe { AXIsProcessTrusted() != 0 }
}

fn cf_key(name: &CStr) -> CFTypeRef {
    unsafe { CFStringCreateWithCString(std::ptr::null(), name.as_ptr(), UTF8) }
}

fn cf_to_string(s: CFTypeRef) -> Option<String> {
    if s.is_null() || unsafe { CFGetTypeID(s) != CFStringGetTypeID() } {
        return None;
    }
    let len = unsafe { CFStringGetLength(s) };
    let size = unsafe { CFStringGetMaximumSizeForEncoding(len, UTF8) }.clamp(1, 16 * 1024) + 1;
    let mut buf = vec![0 as c_char; size as usize];
    if !unsafe { CFStringGetCString(s, buf.as_mut_ptr(), size, UTF8) } {
        return None;
    }
    Some(unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned())
}

/// The title of an app's focused window, through Accessibility.
fn window_title(pid: i32) -> Option<String> {
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return None;
    }
    unsafe { AXUIElementSetMessagingTimeout(app, AX_TIMEOUT_S) };
    let (k_window, k_title) = (cf_key(c"AXFocusedWindow"), cf_key(c"AXTitle"));
    let mut out = None;
    if !k_window.is_null() && !k_title.is_null() {
        let mut window: CFTypeRef = std::ptr::null();
        if unsafe { AXUIElementCopyAttributeValue(app, k_window, &mut window) } == 0 && !window.is_null() {
            let mut title: CFTypeRef = std::ptr::null();
            if unsafe { AXUIElementCopyAttributeValue(window, k_title, &mut title) } == 0 && !title.is_null() {
                out = cf_to_string(title);
                unsafe { CFRelease(title) };
            }
            unsafe { CFRelease(window) };
        }
    }
    for k in [k_window, k_title] {
        if !k.is_null() {
            unsafe { CFRelease(k) };
        }
    }
    unsafe { CFRelease(app) };
    out
}

pub struct Mic;

impl MicProbe for Mic {
    fn sample(&mut self) -> MicSample {
        let recorders = system::recorders();
        // Without the list: the device, and whether we hold it.
        let device_running = recorders.is_none() && system::input_running();
        let busy = recorders.as_ref().map_or(device_running, |r| !r.is_empty());
        // The running call apps matter only while the microphone is busy.
        let mut running: Vec<String> = Vec::new();
        if busy {
            running = self::running().into_iter().map(|a| a.bundle).filter(|b| call_app_of(b).is_some()).collect();
            if let Some(f) = frontmost().map(|a| a.bundle).filter(|f| running.contains(f)) {
                running.retain(|b| *b != f);
                running.insert(0, f);
            }
        }
        MicSample { recorders, device_running, own_capture: crate::island::audio::own_capture(), running }
    }
}

pub struct Front;

impl FrontProbe for Front {
    fn front(&mut self) -> Option<FrontApp> {
        if system::locked() {
            return None;
        }
        let app = frontmost()?;
        let title = if titles_allowed() { window_title(app.pid) } else { None };
        Some(FrontApp { id: app.bundle, name: app.name, title })
    }
}

/// The apps with a Dock icon that run now (the exclusion picker).
pub fn running_apps() -> Vec<AppMark> {
    let mut out: Vec<AppMark> = running()
        .into_iter()
        .filter(|a| a.policy == POLICY_REGULAR && !a.bundle.is_empty() && a.pid != std::process::id() as i32)
        .map(|a| AppMark { id: a.bundle, name: a.name })
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    out.dedup_by(|a, b| a.id == b.id);
    out
}

// --- Wi-Fi and Location Services ---

/// CLAuthorizationStatus.
const LOCATION_NOT_DETERMINED: i32 = 0;
const LOCATION_RESTRICTED: i32 = 1;
const LOCATION_DENIED: i32 = 2;

fn location_status() -> Option<(bool, i32)> {
    exception::catch(AssertUnwindSafe(|| {
        autoreleasepool(|_| {
            let class = class_object(c"CLLocationManager")?;
            let on: bool = unsafe { msg_send![class, locationServicesEnabled] };
            let status: i32 = unsafe { msg_send![class, authorizationStatus] };
            Some((on, status))
        })
    }))
    .ok()
    .flatten()
}

fn ssid() -> Option<String> {
    exception::catch(AssertUnwindSafe(|| {
        autoreleasepool(|_| {
            let class = class_object(c"CWWiFiClient")?;
            let client: *mut AnyObject = unsafe { msg_send![class, sharedWiFiClient] };
            if client.is_null() {
                return None;
            }
            let iface: *mut AnyObject = unsafe { msg_send![&*client, interface] };
            if iface.is_null() {
                return None;
            }
            let ssid: *mut AnyObject = unsafe { msg_send![&*iface, ssid] };
            ns_string(ssid).filter(|s| !s.is_empty())
        })
    }))
    .ok()
    .flatten()
}

/// The network now. A name the system gives counts whatever the
/// permission says (before macOS 14 none is needed); without one the
/// permission tells "not asked" and "not allowed" from "no Wi-Fi".
pub fn wifi_now() -> WifiNow {
    if let Some(s) = ssid() {
        return WifiNow::Network(s);
    }
    match location_status() {
        None => WifiNow::Unsupported,
        Some((false, _)) => WifiNow::Denied,
        Some((_, LOCATION_NOT_DETERMINED)) => WifiNow::NotAsked,
        Some((_, LOCATION_RESTRICTED | LOCATION_DENIED)) => WifiNow::Denied,
        Some(_) => WifiNow::NotConnected,
    }
}

pub struct Wifi;

impl WifiProbe for Wifi {
    fn read(&mut self) -> WifiNow {
        wifi_now()
    }
}

/// The one location manager (kept for the life of the app: the system
/// answers through it).
static MANAGER: std::sync::Mutex<usize> = std::sync::Mutex::new(0);

/// Make macOS ask whether the app may use Location Services (once; later
/// calls change nothing). Main thread only.
pub fn request_location() {
    let _ = exception::catch(AssertUnwindSafe(|| {
        autoreleasepool(|_| {
            let Some(class) = class_object(c"CLLocationManager") else { return };
            let mut kept = MANAGER.lock().unwrap();
            if *kept == 0 {
                let m: *mut AnyObject = unsafe { msg_send![class, alloc] };
                if m.is_null() {
                    return;
                }
                let m: *mut AnyObject = unsafe { msg_send![m, init] };
                if m.is_null() {
                    return;
                }
                *kept = m as usize;
            }
            let m = *kept as *mut AnyObject;
            let responds: bool = unsafe { msg_send![&*m, respondsToSelector: sel!(requestWhenInUseAuthorization)] };
            if responds {
                let _: () = unsafe { msg_send![&*m, requestWhenInUseAuthorization] };
            } else {
                let _: () = unsafe { msg_send![&*m, requestAlwaysAuthorization] };
            }
        })
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probes on this Mac, printed (no assertion on what they see:
    /// that depends on the machine). `cargo test -p snth-connect
    /// observe_probes_now -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn observe_probes_now() {
        println!("front: {:?}", Front.front());
        println!("titles allowed (Accessibility): {}", titles_allowed());
        println!("microphone: {:?}", Mic.sample());
        println!("wifi: {:?}", Wifi.read());
        println!("running apps: {}", running_apps().len());
    }
}
