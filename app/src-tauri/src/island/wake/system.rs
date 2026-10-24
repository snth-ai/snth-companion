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
//!
//! On Windows (`win`):
//!
//!  - The microphone: Windows keeps, per user, a record of every app that
//!    used the microphone (the one behind the "in use" icon of the
//!    taskbar and Settings > Privacy & security > Microphone > Recent
//!    activity): `HKCU\Software\Microsoft\Windows\CurrentVersion\
//!    CapabilityAccessManager\ConsentStore\microphone`, a subkey per
//!    Store app (its package family name) and under `NonPackaged` one per
//!    desktop program (its path, `\` written as `#`). Each has
//!    `LastUsedTimeStart` and `LastUsedTimeStop` (FILETIMEs); a start with
//!    no stop is an app recording now. This program is left out (its own
//!    path), and so is a desktop program that is not running (a crash
//!    may leave a start without a stop). `consent` reads the entries apart
//!    from the registry, so the decision is tested anywhere.
//!  - The lock: `WTSQuerySessionInformation(WTSSessionInfoEx)` gives the
//!    session's lock flag and whether it is the active one (a session
//!    switched away from, or a disconnected remote one, counts as locked,
//!    as a Mac's session off the console does). Where the flag is unknown,
//!    the input desktop decides: it cannot be opened while the lock
//!    screen (Winlogon's desktop) is up.
//!  - The input devices: the MMDevice API, each active capture endpoint
//!    with its id (cpal's device id), its name and the bus of the device
//!    under it (`PKEY_Device_EnumeratorName`: BTHENUM, BTHHFENUM for a
//!    headset's hands-free microphone, USB, HDAUDIO, ...), which
//!    `device::windows_transport` turns into the Mac's transport codes, so
//!    `device::choose` decides the same way on both systems.
//!  - The front app: not asked. The Mac looks at it only before macOS 14,
//!    where the process list does not exist; Windows' consent store is
//!    there since Windows 10 1903.

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
    /// kAudioDevicePropertyDeviceIsRunningSomewhere.
    const DEVICE_RUNNING_SOMEWHERE: u32 = fourcc(b"gone");

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

    /// The default input device runs for some process (this app
    /// included): all that macOS before 14 says.
    pub fn default_input_running() -> bool {
        let Some(dev) = get::<u32>(SYSTEM_OBJECT, DEFAULT_INPUT).filter(|d| *d != 0) else {
            return false;
        };
        get::<u32>(dev, DEVICE_RUNNING_SOMEWHERE).unwrap_or(0) != 0
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

/// The microphone's consent store, apart from the registry (Windows
/// writes it; the decision is tested anywhere).
#[cfg_attr(not(windows), allow(dead_code))]
pub mod consent {
    /// One app the store knows.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Entry {
        /// The subkey: a package family name
        /// (`Microsoft.WindowsSoundRecorder_8wekyb3d8bbwe`), or for a
        /// desktop program its path with `#` for `\`
        /// (`C:#Program Files#Zoom#bin#Zoom.exe`).
        pub key: String,
        /// Under `NonPackaged` (a desktop program).
        pub desktop: bool,
        /// FILETIMEs; 0 when absent.
        pub start: u64,
        pub stop: u64,
    }

    /// A path as the store writes it: `\` (and `/`) as `#`, lower case.
    pub fn path_key(path: &str) -> String {
        let p = path.strip_prefix(r"\\?\").unwrap_or(path);
        p.chars().map(|c| if c == '\\' || c == '/' { '#' } else { c }).collect::<String>().to_lowercase()
    }

    /// The program's file name of a desktop entry ("Zoom.exe").
    pub fn exe_name(key: &str) -> &str {
        key.rsplit('#').next().unwrap_or(key)
    }

    /// The name shown in the log: the file name of a desktop program, the
    /// package name of a Store app (before the publisher hash).
    pub fn shown(e: &Entry) -> String {
        if e.desktop {
            exe_name(&e.key).to_string()
        } else {
            e.key.split('_').next().unwrap_or(&e.key).to_string()
        }
    }

    /// The apps recording now: a start with no stop, not this program
    /// (`me`: its path), and for a desktop program only while a program of
    /// that file name runs (`running`, lower case; None: not known, every
    /// entry counts).
    pub fn recording(entries: &[Entry], me: &str, running: Option<&[String]>) -> Vec<String> {
        let me = path_key(me);
        let mut out: Vec<String> = Vec::new();
        for e in entries {
            if e.start == 0 || e.stop != 0 {
                continue;
            }
            if e.desktop {
                if e.key.to_lowercase() == me {
                    continue;
                }
                let exe = exe_name(&e.key).to_lowercase();
                if running.is_some_and(|r| !r.iter().any(|n| *n == exe)) {
                    continue;
                }
            }
            let name = shown(e);
            if !out.contains(&name) {
                out.push(name);
            }
        }
        out
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::Registry::{RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ, RRF_RT_REG_QWORD};
    use windows_sys::Win32::System::RemoteDesktop::{WTSFreeMemory, WTSQuerySessionInformationW, WTSSessionInfoEx, WTSActive, WTSINFOEXW, WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION, WTS_SESSIONSTATE_LOCK, WTS_SESSIONSTATE_UNLOCK};
    use windows_sys::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS};

    use super::consent::Entry;
    use crate::island::wake::device::{self, InputDevice};

    const STORE: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    struct Key(HKEY);

    impl Drop for Key {
        fn drop(&mut self) {
            unsafe { RegCloseKey(self.0) };
        }
    }

    fn open(parent: HKEY, path: &str) -> Option<Key> {
        let mut k: HKEY = std::ptr::null_mut();
        let w = wide(path);
        (unsafe { RegOpenKeyExW(parent, w.as_ptr(), 0, KEY_READ, &mut k) } == ERROR_SUCCESS).then_some(Key(k))
    }

    fn subkeys(k: &Key) -> Vec<String> {
        let mut out = Vec::new();
        let mut buf = [0u16; 512];
        for i in 0.. {
            let mut len = buf.len() as u32;
            let rc = unsafe { RegEnumKeyExW(k.0, i, buf.as_mut_ptr(), &mut len, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
            if rc == ERROR_NO_MORE_ITEMS {
                break;
            }
            if rc == ERROR_SUCCESS {
                out.push(String::from_utf16_lossy(&buf[..len as usize]));
            }
            // A name too long for the buffer: skipped, the next one read.
            if i > 4096 {
                break;
            }
        }
        out
    }

    fn qword(k: &Key, sub: &str, value: &str) -> u64 {
        let (sub, value) = (wide(sub), wide(value));
        let mut v = 0u64;
        let mut size = 8u32;
        let rc = unsafe { RegGetValueW(k.0, sub.as_ptr(), value.as_ptr(), RRF_RT_REG_QWORD, std::ptr::null_mut(), (&mut v as *mut u64).cast::<c_void>(), &mut size) };
        if rc == ERROR_SUCCESS {
            v
        } else {
            0
        }
    }

    fn entries_under(root: &Key, desktop: bool, out: &mut Vec<Entry>) {
        for name in subkeys(root) {
            if !desktop && name.eq_ignore_ascii_case("NonPackaged") {
                continue;
            }
            let start = qword(root, &name, "LastUsedTimeStart");
            let stop = qword(root, &name, "LastUsedTimeStop");
            out.push(Entry { key: name, desktop, start, stop });
        }
    }

    /// Every app in the store. None when the store cannot be read.
    pub fn consent_entries() -> Option<Vec<Entry>> {
        consent_entries_at(STORE)
    }

    /// The same, from a store at this path under HKCU (the test's own).
    pub fn consent_entries_at(path: &str) -> Option<Vec<Entry>> {
        let root = open(HKEY_CURRENT_USER, path)?;
        let mut out = Vec::new();
        entries_under(&root, false, &mut out);
        if let Some(np) = open(root.0, "NonPackaged") {
            entries_under(&np, true, &mut out);
        }
        Some(out)
    }

    /// The file names of the running programs, lower case. None when they
    /// cannot be listed.
    pub fn running() -> Option<Vec<String>> {
        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snap == INVALID_HANDLE_VALUE || snap.is_null() {
            return None;
        }
        let mut out = Vec::new();
        let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = unsafe { Process32FirstW(snap, &mut e) } != 0;
        while ok {
            let n = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
            out.push(String::from_utf16_lossy(&e.szExeFile[..n]).to_lowercase());
            ok = unsafe { Process32NextW(snap, &mut e) } != 0;
        }
        unsafe { CloseHandle(snap) };
        Some(out)
    }

    /// The session's lock: Some(true/false) from its flags, None when
    /// Windows does not say.
    fn session_locked() -> Option<bool> {
        let mut buf: windows_sys::core::PWSTR = std::ptr::null_mut();
        let mut bytes = 0u32;
        let ok = unsafe { WTSQuerySessionInformationW(WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION, WTSSessionInfoEx, &mut buf, &mut bytes) } != 0;
        if !ok || buf.is_null() {
            return None;
        }
        let out = if (bytes as usize) >= std::mem::size_of::<WTSINFOEXW>() {
            let info = unsafe { &*(buf as *const WTSINFOEXW) };
            if info.Level != 1 {
                None
            } else {
                let l1 = unsafe { info.Data.WTSInfoExLevel1 };
                if l1.SessionState != WTSActive {
                    // Switched away from, or a remote session disconnected.
                    Some(true)
                } else if l1.SessionFlags == WTS_SESSIONSTATE_LOCK as i32 {
                    Some(true)
                } else if l1.SessionFlags == WTS_SESSIONSTATE_UNLOCK as i32 {
                    Some(false)
                } else {
                    None
                }
            }
        } else {
            None
        };
        unsafe { WTSFreeMemory(buf.cast()) };
        out
    }

    /// The input desktop can be opened (it cannot while the lock screen
    /// is up).
    fn input_desktop_open() -> bool {
        let d = unsafe { OpenInputDesktop(0, 0, DESKTOP_READOBJECTS) };
        if d.is_null() {
            return false;
        }
        unsafe { CloseDesktop(d) };
        true
    }

    pub fn locked() -> bool {
        match session_locked() {
            Some(l) => l,
            None => !input_desktop_open(),
        }
    }

    // --- the input devices (COM: the MMDevice API) ---

    use windows::core::{GUID, PWSTR};
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::{eCapture, eConsole, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE};
    use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PROPVARIANT};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ};
    use windows::Win32::System::Variant::VT_LPWSTR;
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

    /// PKEY_Device_FriendlyName ("Microphone Array (Realtek(R) Audio)").
    const FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0), pid: 14 };
    /// PKEY_Device_EnumeratorName: the bus the device sits on ("USB",
    /// "BTHHFENUM", "HDAUDIO").
    const ENUMERATOR: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0), pid: 24 };

    thread_local! {
        /// COM on this thread, once (the wake thread lives as long as the
        /// app). Already set up another way is fine too.
        static COM: () = {
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        };
    }

    fn take(p: PWSTR) -> String {
        let s = unsafe { p.to_string() }.unwrap_or_default();
        unsafe { CoTaskMemFree(Some(p.0 as *const c_void)) };
        s
    }

    fn string(store: &IPropertyStore, key: &PROPERTYKEY) -> Option<String> {
        let mut v: PROPVARIANT = unsafe { store.GetValue(key) }.ok()?;
        let inner = unsafe { &v.Anonymous.Anonymous };
        let out = if inner.vt == VT_LPWSTR {
            let p = unsafe { inner.Anonymous.pwszVal };
            (!p.is_null()).then(|| unsafe { p.to_string() }.unwrap_or_default())
        } else {
            None
        };
        let _ = unsafe { PropVariantClear(&mut v) };
        out
    }

    fn id_of(d: &IMMDevice) -> Option<String> {
        unsafe { d.GetId() }.ok().map(take)
    }

    pub fn input_devices() -> Vec<InputDevice> {
        COM.with(|_| {});
        let run = || -> windows::core::Result<Vec<InputDevice>> {
            let en: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }?;
            let default = unsafe { en.GetDefaultAudioEndpoint(eCapture, eConsole) }.ok().and_then(|d| id_of(&d));
            let all = unsafe { en.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) }?;
            let mut out = Vec::new();
            for i in 0..unsafe { all.GetCount() }? {
                let Ok(d) = (unsafe { all.Item(i) }) else { continue };
                let Some(uid) = id_of(&d) else { continue };
                let store = unsafe { d.OpenPropertyStore(STGM_READ) }.ok();
                let name = store.as_ref().and_then(|s| string(s, &FRIENDLY_NAME)).unwrap_or_default();
                let bus = store.as_ref().and_then(|s| string(s, &ENUMERATOR)).unwrap_or_default();
                out.push(InputDevice { is_default: default.as_deref() == Some(uid.as_str()), transport: device::windows_transport(&bus, &name), uid, name });
            }
            Ok(out)
        };
        run().unwrap_or_default()
    }

    /// The bus names as Windows gives them, for the look at a real PC
    /// (`wake_system_now`).
    #[cfg(test)]
    pub fn buses() -> Vec<(String, String)> {
        COM.with(|_| {});
        let run = || -> windows::core::Result<Vec<(String, String)>> {
            let en: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }?;
            let all = unsafe { en.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) }?;
            let mut out = Vec::new();
            for i in 0..unsafe { all.GetCount() }? {
                let Ok(d) = (unsafe { all.Item(i) }) else { continue };
                let store = unsafe { d.OpenPropertyStore(STGM_READ) }.ok();
                let name = store.as_ref().and_then(|s| string(s, &FRIENDLY_NAME)).unwrap_or_default();
                let bus = store.as_ref().and_then(|s| string(s, &ENUMERATOR)).unwrap_or_default();
                out.push((name, bus));
            }
            Ok(out)
        };
        run().unwrap_or_default()
    }
}

/// Call apps whose being in front counts as a call where the process list
/// does not exist (before macOS 14).
#[cfg(target_os = "macos")]
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
    #[cfg(windows)]
    {
        let Some(entries) = win::consent_entries() else {
            return (false, Vec::new());
        };
        let me = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let running = win::running();
        let who = consent::recording(&entries, &me, running.as_deref());
        (!who.is_empty(), who)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        (false, Vec::new())
    }
}

/// The processes other than this app that record now, by id: a bundle id
/// (else the process name) on macOS 14 and later, a program's file name or
/// a Store package's name on Windows. None where the system does not list
/// them (macOS before 14, other systems). Observe's call source.
pub fn recorders() -> Option<Vec<String>> {
    #[cfg(target_os = "macos")]
    {
        mac::recording_elsewhere().map(|list| list.into_iter().map(|(pid, b)| if b.is_empty() { process_file(pid) } else { b }).collect())
    }
    #[cfg(windows)]
    {
        let entries = win::consent_entries()?;
        let me = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let running = win::running();
        Some(consent::recording(&entries, &me, running.as_deref()))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        None
    }
}

/// The default input device runs for some process, this app included
/// (macOS; false elsewhere, where `recorders` answers).
pub fn input_running() -> bool {
    #[cfg(target_os = "macos")]
    {
        mac::default_input_running()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// The file names of the running programs, lower case (None when they
/// cannot be listed).
#[cfg(windows)]
pub fn running_programs() -> Option<Vec<String>> {
    win::running()
}

/// A process's file name without the pid (a tool without a bundle id).
#[cfg(target_os = "macos")]
fn process_file(pid: i32) -> String {
    let mut buf = [0u8; 256];
    let n = unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf8_lossy(&buf[..n as usize]).into_owned()
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
    #[cfg(windows)]
    {
        win::input_devices()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Vec::new()
    }
}

pub fn locked() -> bool {
    #[cfg(target_os = "macos")]
    {
        mac::locked()
    }
    #[cfg(windows)]
    {
        win::locked()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
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

#[cfg(test)]
mod consent_tests {
    use super::consent::{path_key, recording, Entry};

    fn app(key: &str, desktop: bool, start: u64, stop: u64) -> Entry {
        Entry { key: key.into(), desktop, start, stop }
    }

    /// The store as a PC in a Zoom call has it, with this app recording
    /// too, an old Teams call, a recorder that crashed mid-recording, and
    /// a Store app recording.
    fn store() -> Vec<Entry> {
        vec![
            app("Microsoft.WindowsSoundRecorder_8wekyb3d8bbwe", false, 133_000_000_000_000_000, 133_000_000_100_000_000),
            app("MSTeams_8wekyb3d8bbwe", false, 133_400_000_000_000_000, 0),
            app("NonPackaged", false, 0, 0),
            app("C:#Program Files#Zoom#bin#Zoom.exe", true, 133_500_000_000_000_000, 0),
            app("C:#Users#sasha#AppData#Local#Programs#SNTH Connect#snth-connect.exe", true, 133_500_000_000_000_000, 0),
            app("C:#Users#sasha#AppData#Roaming#Telegram Desktop#Telegram.exe", true, 133_300_000_000_000_000, 133_300_000_900_000_000),
            app("C:#Tools#OldRecorder#rec.exe", true, 133_100_000_000_000_000, 0),
        ]
    }

    #[test]
    fn a_start_without_a_stop_is_an_app_recording_now() {
        let me = r"C:\Users\sasha\AppData\Local\Programs\SNTH Connect\snth-connect.exe";
        let running: Vec<String> = ["zoom.exe", "snth-connect.exe", "telegram.exe", "explorer.exe"].iter().map(|s| s.to_string()).collect();
        assert_eq!(recording(&store(), me, Some(&running)), vec!["MSTeams".to_string(), "Zoom.exe".into()], "not this app, not a stopped one, not one that is not running");
        // The program list unknown: every open start counts.
        assert_eq!(recording(&store(), me, None), vec!["MSTeams".to_string(), "Zoom.exe".into(), "rec.exe".into()]);
        // This app alone recording (its own wake listener): not busy.
        let mine = [app("C:#Users#sasha#AppData#Local#Programs#SNTH Connect#snth-connect.exe", true, 5, 0)];
        assert!(recording(&mine, me, Some(&running)).is_empty());
        // Nothing open: not busy; nothing known: not busy.
        let quiet: Vec<Entry> = store().into_iter().map(|mut e| {
            if e.start != 0 {
                e.stop = e.start + 1;
            }
            e
        }).collect();
        assert!(recording(&quiet, me, Some(&running)).is_empty());
        assert!(recording(&[], me, None).is_empty());
    }

    #[test]
    fn this_apps_path_matches_the_way_the_store_writes_it() {
        assert_eq!(path_key(r"C:\Program Files\SNTH Connect\snth-connect.exe"), "c:#program files#snth connect#snth-connect.exe");
        assert_eq!(path_key(r"\\?\C:\Program Files\SNTH Connect\snth-connect.exe"), "c:#program files#snth connect#snth-connect.exe", "a verbatim path from current_exe");
        // Case differs between the store and current_exe: still this app.
        let mine = [app("C:#PROGRAM FILES#SNTH Connect#SNTH-CONNECT.EXE", true, 5, 0)];
        assert!(recording(&mine, r"C:\Program Files\SNTH Connect\snth-connect.exe", None).is_empty());
    }
}

/// The Windows calls themselves, on the machine the tests run on (the
/// CI's runner: no microphone, no one at the console).
#[cfg(all(test, windows))]
mod win_tests {
    use super::consent::Entry;
    use super::*;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RegCloseKey, RegCreateKeyW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, REG_QWORD};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn put(path: &str, start: u64, stop: u64) {
        let mut k: HKEY = std::ptr::null_mut();
        assert_eq!(unsafe { RegCreateKeyW(HKEY_CURRENT_USER, wide(path).as_ptr(), &mut k) }, ERROR_SUCCESS, "{path}");
        for (name, v) in [("LastUsedTimeStart", start), ("LastUsedTimeStop", stop)] {
            let rc = unsafe { RegSetValueExW(k, wide(name).as_ptr(), 0, REG_QWORD, (&v as *const u64).cast(), 8) };
            assert_eq!(rc, ERROR_SUCCESS);
        }
        unsafe { RegCloseKey(k) };
    }

    /// A consent store laid out as Windows lays it out (a Store app, the
    /// NonPackaged key with programs under it, this test's own program),
    /// written under a key of the test's own and read back through the
    /// same code as the real one.
    #[test]
    fn the_consent_store_is_read_from_the_registry() {
        let root = format!(r"Software\SNTH Connect tests\{}\microphone", std::process::id());
        let me = std::env::current_exe().unwrap().to_string_lossy().into_owned();
        put(&format!(r"{root}\MSTeams_8wekyb3d8bbwe"), 133_400_000_000_000_000, 0);
        put(&format!(r"{root}\Microsoft.WindowsSoundRecorder_8wekyb3d8bbwe"), 133_000_000_000_000_000, 133_000_000_100_000_000);
        put(&format!(r"{root}\NonPackaged\C:#Program Files#Zoom#bin#Zoom.exe"), 133_500_000_000_000_000, 0);
        put(&format!(r"{root}\NonPackaged\{}", me.replace('\\', "#")), 133_500_000_000_000_000, 0);
        let entries = win::consent_entries_at(&root);
        let parent = format!(r"Software\SNTH Connect tests\{}", std::process::id());
        unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide(&parent).as_ptr()) };
        let entries = entries.expect("the store reads");
        assert!(entries.contains(&Entry { key: "MSTeams_8wekyb3d8bbwe".into(), desktop: false, start: 133_400_000_000_000_000, stop: 0 }), "{entries:?}");
        assert!(entries.contains(&Entry { key: "C:#Program Files#Zoom#bin#Zoom.exe".into(), desktop: true, start: 133_500_000_000_000_000, stop: 0 }), "{entries:?}");
        assert!(!entries.iter().any(|e| e.key == "NonPackaged"), "the folder of programs is not an app: {entries:?}");
        assert_eq!(entries.len(), 4, "{entries:?}");
        // Zoom is not running on the runner; this test is, and is left out.
        let running = win::running().expect("the programs are listed");
        let exe = std::path::Path::new(&me).file_name().unwrap().to_string_lossy().to_lowercase();
        assert!(running.contains(&exe), "this program is in the list: {exe}");
        assert_eq!(consent::recording(&entries, &me, Some(&running)), vec!["MSTeams".to_string()]);
        assert_eq!(consent::recording(&entries, &me, None), vec!["MSTeams".to_string(), "Zoom.exe".into()]);
    }

    /// What this machine says (a look: the runner has no audio devices and
    /// may run outside a console session). The calls must not fail.
    #[test]
    fn wake_system_now() {
        let t = std::time::Instant::now();
        let busy = mic_busy();
        println!("mic busy: {busy:?} ({} ms)", t.elapsed().as_millis());
        println!("locked: {}", locked());
        for (name, bus) in win::buses() {
            println!("input: {name:?} on {bus:?}");
        }
        for d in input_devices() {
            println!("input: {:?} {:?} transport {}{}", d.name, d.uid, super::super::device::transport::name(d.transport), if d.is_default { " (default)" } else { "" });
        }
        println!("the listener uses: {:?}", super::super::device::choose(&input_devices()));
    }
}
