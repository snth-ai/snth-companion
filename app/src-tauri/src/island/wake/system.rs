//! What the system says: does another app use the microphone, is the
//! screen locked, which input devices there are (`input_devices`, for
//! `device::choose`).
//!
//! The microphone: macOS 14 and later list every process that uses audio
//! as a CoreAudio process object (`kAudioHardwarePropertyProcessObjectList`)
//! that says whether it records right now
//! (`kAudioProcessPropertyIsRunningInput`) and on which input devices
//! (`kAudioProcessPropertyDevices`, input scope). Any process but this one
//! recording from a device means a call (Zoom, Telegram, FaceTime, a browser's Meet) or
//! a dictation. Before macOS 14 the list does not exist: then a known call
//! app in front counts instead.
//!
//! The device matters: on this M1 Air (macOS 26) `avconferenced` (the
//! FaceTime engine) has said "running input" for days with no input device
//! and the microphone running nowhere; that is not a call. A real capture
//! (ffmpeg, a call app) lists its device.
//!
//! The lock: the window server's session dictionary
//! (`CGSessionCopyCurrentDictionary`) says `CGSSessionScreenIsLocked`
//! while the screen is locked, and the session is off the console while
//! another user is switched in.

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::{c_char, c_void, CStr};
    use std::panic::AssertUnwindSafe;

    use objc2::exception;
    use objc2::rc::autoreleasepool;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::msg_send;

    #[repr(C)]
    struct Address {
        selector: u32,
        scope: u32,
        element: u32,
    }

    const fn fourcc(s: &[u8; 4]) -> u32 {
        ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | s[3] as u32
    }

    const SYSTEM_OBJECT: u32 = 1;
    const SCOPE_GLOBAL: u32 = fourcc(b"glob");
    const SCOPE_INPUT: u32 = fourcc(b"inpt");
    const PROCESS_DEVICES: u32 = fourcc(b"pdv#");
    const HARDWARE_DEVICES: u32 = fourcc(b"dev#");
    const DEFAULT_INPUT: u32 = fourcc(b"dIn ");
    const DEVICE_STREAMS: u32 = fourcc(b"stm#");
    const DEVICE_UID: u32 = fourcc(b"uid ");
    const OBJECT_NAME: u32 = fourcc(b"lnam");
    const DEVICE_TRANSPORT: u32 = fourcc(b"tran");
    const ELEMENT_MAIN: u32 = 0;
    const PROCESS_OBJECT_LIST: u32 = fourcc(b"prs#");
    const PROCESS_PID: u32 = fourcc(b"ppid");
    const PROCESS_RUNNING_INPUT: u32 = fourcc(b"piri");
    const PROCESS_BUNDLE_ID: u32 = fourcc(b"pbid");

    type CFTypeRef = *const c_void;

    #[link(name = "CoreAudio", kind = "framework")]
    extern "C" {
        fn AudioObjectGetPropertyDataSize(id: u32, addr: *const Address, qsize: u32, qdata: *const c_void, out: *mut u32) -> i32;
        fn AudioObjectGetPropertyData(id: u32, addr: *const Address, qsize: u32, qdata: *const c_void, size: *mut u32, out: *mut c_void) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: CFTypeRef);
        fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, encoding: u32) -> CFTypeRef;
        fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, size: isize, encoding: u32) -> bool;
        fn CFDictionaryGetValue(d: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        fn CFBooleanGetValue(b: CFTypeRef) -> bool;
        fn CFGetTypeID(cf: CFTypeRef) -> usize;
        fn CFBooleanGetTypeID() -> usize;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGSessionCopyCurrentDictionary() -> CFTypeRef;
    }

    const UTF8: u32 = 0x0800_0100;

    fn get<T: Copy>(id: u32, selector: u32) -> Option<T> {
        let addr = Address { selector, scope: SCOPE_GLOBAL, element: ELEMENT_MAIN };
        let mut out = std::mem::MaybeUninit::<T>::uninit();
        let mut size = std::mem::size_of::<T>() as u32;
        let rc = unsafe { AudioObjectGetPropertyData(id, &addr, 0, std::ptr::null(), &mut size, out.as_mut_ptr().cast()) };
        (rc == 0 && size as usize == std::mem::size_of::<T>()).then(|| unsafe { out.assume_init() })
    }

    fn cf_string(s: CFTypeRef) -> String {
        let mut buf = [0 as c_char; 256];
        if s.is_null() || !unsafe { CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as isize, UTF8) } {
            return String::new();
        }
        unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
    }

    fn count(id: u32, selector: u32, scope: u32) -> u32 {
        let addr = Address { selector, scope, element: ELEMENT_MAIN };
        let mut size = 0u32;
        if unsafe { AudioObjectGetPropertyDataSize(id, &addr, 0, std::ptr::null(), &mut size) } != 0 {
            return 0;
        }
        size / 4
    }

    fn string(id: u32, selector: u32) -> String {
        get::<CFTypeRef>(id, selector)
            .map(|s| {
                let out = cf_string(s);
                if !s.is_null() {
                    unsafe { CFRelease(s) };
                }
                out
            })
            .unwrap_or_default()
    }

    /// Every device with an input stream, the default input marked.
    pub fn input_devices() -> Vec<crate::island::wake::device::InputDevice> {
        let addr = Address { selector: HARDWARE_DEVICES, scope: SCOPE_GLOBAL, element: ELEMENT_MAIN };
        let mut size = 0u32;
        if unsafe { AudioObjectGetPropertyDataSize(SYSTEM_OBJECT, &addr, 0, std::ptr::null(), &mut size) } != 0 {
            return Vec::new();
        }
        let mut ids = vec![0u32; size as usize / 4];
        let mut got = (ids.len() * 4) as u32;
        if unsafe { AudioObjectGetPropertyData(SYSTEM_OBJECT, &addr, 0, std::ptr::null(), &mut got, ids.as_mut_ptr().cast()) } != 0 {
            return Vec::new();
        }
        ids.truncate(got as usize / 4);
        let default = get::<u32>(SYSTEM_OBJECT, DEFAULT_INPUT).unwrap_or(0);
        ids.into_iter()
            .filter(|id| count(*id, DEVICE_STREAMS, SCOPE_INPUT) > 0)
            .map(|id| crate::island::wake::device::InputDevice {
                uid: string(id, DEVICE_UID),
                name: string(id, OBJECT_NAME),
                transport: get::<u32>(id, DEVICE_TRANSPORT).unwrap_or(0),
                is_default: id == default,
            })
            .filter(|d| !d.uid.is_empty())
            .collect()
    }

    /// How many input devices this process object uses.
    fn process_inputs(id: u32) -> u32 {
        count(id, PROCESS_DEVICES, SCOPE_INPUT)
    }

    /// The processes other than this one that record now: (pid, bundle
    /// id). None where the list does not exist (before macOS 14).
    pub fn recording_elsewhere() -> Option<Vec<(i32, String)>> {
        let addr = Address { selector: PROCESS_OBJECT_LIST, scope: SCOPE_GLOBAL, element: ELEMENT_MAIN };
        let mut size = 0u32;
        if unsafe { AudioObjectGetPropertyDataSize(SYSTEM_OBJECT, &addr, 0, std::ptr::null(), &mut size) } != 0 {
            return None;
        }
        let mut ids = vec![0u32; size as usize / 4];
        let mut got = (ids.len() * 4) as u32;
        if unsafe { AudioObjectGetPropertyData(SYSTEM_OBJECT, &addr, 0, std::ptr::null(), &mut got, ids.as_mut_ptr().cast()) } != 0 {
            return None;
        }
        ids.truncate(got as usize / 4);
        let me = std::process::id() as i32;
        let mut out = Vec::new();
        for id in ids {
            if get::<u32>(id, PROCESS_RUNNING_INPUT).unwrap_or(0) == 0 || process_inputs(id) == 0 {
                continue;
            }
            let pid = get::<i32>(id, PROCESS_PID).unwrap_or(-1);
            if pid == me {
                continue;
            }
            let bundle = get::<CFTypeRef>(id, PROCESS_BUNDLE_ID)
                .map(|s| {
                    let b = cf_string(s);
                    if !s.is_null() {
                        unsafe { CFRelease(s) };
                    }
                    b
                })
                .unwrap_or_default();
            out.push((pid, bundle));
        }
        Some(out)
    }

    /// The front app's bundle id (the fallback before macOS 14).
    pub fn front_app() -> Option<String> {
        let class = AnyClass::get(c"NSWorkspace")?;
        exception::catch(AssertUnwindSafe(|| {
            autoreleasepool(|_| {
                let ws: *mut AnyObject = unsafe { msg_send![class, sharedWorkspace] };
                if ws.is_null() {
                    return None;
                }
                let app: *mut AnyObject = unsafe { msg_send![&*ws, frontmostApplication] };
                if app.is_null() {
                    return None;
                }
                let id: *mut AnyObject = unsafe { msg_send![&*app, bundleIdentifier] };
                if id.is_null() {
                    return None;
                }
                let utf8: *const c_char = unsafe { msg_send![&*id, UTF8String] };
                (!utf8.is_null()).then(|| unsafe { CStr::from_ptr(utf8) }.to_string_lossy().into_owned())
            })
        }))
        .ok()
        .flatten()
    }

    fn key(name: &CStr) -> CFTypeRef {
        unsafe { CFStringCreateWithCString(std::ptr::null(), name.as_ptr(), UTF8) }
    }

    fn flag(dict: CFTypeRef, name: &CStr) -> Option<bool> {
        let k = key(name);
        if k.is_null() {
            return None;
        }
        let v = unsafe { CFDictionaryGetValue(dict, k) };
        unsafe { CFRelease(k) };
        if v.is_null() || unsafe { CFGetTypeID(v) != CFBooleanGetTypeID() } {
            return None;
        }
        Some(unsafe { CFBooleanGetValue(v) })
    }

    pub fn locked() -> bool {
        let dict = unsafe { CGSessionCopyCurrentDictionary() };
        // No session dictionary: no one is logged in at the console.
        if dict.is_null() {
            return true;
        }
        let locked = flag(dict, c"CGSSessionScreenIsLocked").unwrap_or(false);
        let on_console = flag(dict, c"kCGSSessionOnConsoleKey").unwrap_or(true);
        unsafe { CFRelease(dict) };
        locked || !on_console
    }
}

/// Call apps whose being in front counts as a call where the process list
/// does not exist (before macOS 14).
const CALL_APPS: &[&str] = &[
    "us.zoom.xos",
    "com.apple.FaceTime",
    "ru.keepcoder.Telegram",
    "org.telegram.desktop",
    "com.microsoft.teams",
    "com.microsoft.teams2",
    "com.hnc.Discord",
    "com.tinyspeck.slackmacgap",
    "com.skype.skype",
    "net.whatsapp.WhatsApp",
    "desktop.WhatsApp",
];

/// Another app records now (with the bundle ids, for the log line).
pub fn mic_busy() -> (bool, Vec<String>) {
    #[cfg(target_os = "macos")]
    {
        match mac::recording_elsewhere() {
            Some(list) => {
                let ids: Vec<String> = list.into_iter().map(|(pid, b)| if b.is_empty() { process_name(pid) } else { b }).collect();
                (!ids.is_empty(), ids)
            }
            None => match mac::front_app() {
                Some(id) if CALL_APPS.contains(&id.as_str()) => (true, vec![id]),
                _ => (false, Vec::new()),
            },
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        (false, Vec::new())
    }
}

/// A process's name (a tool without a bundle id, like ffmpeg).
#[cfg(target_os = "macos")]
fn process_name(pid: i32) -> String {
    let mut buf = [0u8; 256];
    let n = unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return format!("pid {pid}");
    }
    format!("{} (pid {pid})", String::from_utf8_lossy(&buf[..n as usize]))
}

/// The input devices (empty where they cannot be read).
pub fn input_devices() -> Vec<super::device::InputDevice> {
    #[cfg(target_os = "macos")]
    {
        mac::input_devices()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

pub fn locked() -> bool {
    #[cfg(target_os = "macos")]
    {
        mac::locked()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::island::wake::pause::{self, Inputs, Pause, Status};
    use std::time::{Duration, Instant};

    /// The pause chain on this Mac: the microphone open for the listener,
    /// another program starts recording (ffmpeg, from 3 s to 8 s), the
    /// probe sees it within its second, the listener closes the
    /// microphone, and opens it again once ffmpeg is done. Needs ffmpeg
    /// and the microphone allowed for the terminal.
    ///
    /// `cargo test -p snth-connect --release -- --ignored wake_real_pause --nocapture`
    #[test]
    #[ignore]
    fn wake_real_pause_while_another_app_records() {
        let base = Inputs { on: true, available: true, island: true, synth_up: true, mic_granted: true, model_ready: true, ears_loaded: true, ..Inputs::default() };
        let t0 = Instant::now();
        let mut ffmpeg = None;
        let mut capture: Option<crate::island::audio::Recording> = None;
        let mut log = Vec::new();
        let mut last = None;
        while t0.elapsed() < Duration::from_secs(13) {
            let at = t0.elapsed().as_secs_f32();
            if ffmpeg.is_none() && at >= 3.0 {
                ffmpeg = Some(
                    connect_core::proc::std_command("ffmpeg")
                        .args(["-loglevel", "error", "-y", "-f", "avfoundation", "-i", ":0", "-t", "5", "/tmp/wake-pause-test.wav"])
                        .spawn()
                        .expect("ffmpeg"),
                );
            }
            let (busy, who) = mic_busy();
            let status = pause::status(&Inputs { mic_busy: busy, ..base });
            if status.listens() && capture.is_none() {
                capture = Some(crate::island::audio::start(std::sync::Arc::new(|_| {})).expect("the microphone"));
            } else if !status.listens() {
                capture = None;
            }
            if last != Some(status) {
                println!("{at:.1} s: {status:?} {who:?}, microphone {}", if capture.is_some() { "open" } else { "closed" });
                log.push((at, status));
                last = Some(status);
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        if let Some(mut f) = ffmpeg {
            let _ = f.wait();
        }
        let paused = log.iter().find(|(_, s)| *s == Status::Paused { reason: Pause::MicBusy }).map(|(t, _)| *t);
        let back = log.iter().rev().find(|(_, s)| *s == Status::Listening).map(|(t, _)| *t);
        assert!(paused.is_some_and(|t| (3.0..5.5).contains(&t)), "paused when ffmpeg started: {log:?}");
        assert!(back.is_some_and(|t| t > 8.0), "listening again after ffmpeg: {log:?}");
    }

    /// What this Mac says right now (a look, not a check: run it with and
    /// without another app recording).
    /// `cargo test -p snth-connect -- --ignored wake_system_now --nocapture`
    #[test]
    #[ignore]
    fn wake_system_now() {
        println!("process list: {:?}", mac::recording_elsewhere());
        println!("mic busy: {:?}", mic_busy());
        println!("front app: {:?}", mac::front_app());
        println!("locked: {}", locked());
        for d in input_devices() {
            println!("input: {:?} {:?} transport {}{}", d.name, d.uid, crate::island::wake::device::transport::name(d.transport), if d.is_default { " (default)" } else { "" });
        }
        let mic = crate::island::wake::device::choose(&input_devices());
        println!("the listener uses: {mic:?}");
        // cpal finds the chosen device by its CoreAudio UID and opens it.
        if let Some(uid) = mic.uid() {
            let rec = crate::island::audio::start_on(Some(uid.to_string()), std::sync::Arc::new(|_| {}));
            println!("opened {uid} through cpal: {}", rec.is_ok());
            assert!(rec.is_ok(), "{:?}", rec.err());
        }
        assert!(crate::island::audio::start_on(Some("no-such-device".into()), std::sync::Arc::new(|_| {})).is_err(), "an unknown UID is an error, not the default");
    }
}
