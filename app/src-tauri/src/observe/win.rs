//! The observe probes on Windows.
//!
//!  - The microphone (`Mic`): the microphone's consent store
//!    (`wake::system::recorders`: the apps with a start and no stop,
//!    this program left out by its own path), the call apps among the
//!    running programs (Toolhelp). Nothing is opened or recorded.
//!  - The app in front (`Front`): `GetForegroundWindow`, its title
//!    (`GetWindowTextW`) and its program (`GetWindowThreadProcessId`,
//!    `QueryFullProcessImageNameW`). No permission is involved. The
//!    session locked: nothing.
//!  - The Wi-Fi network (`Wifi`): the WLAN API, the current connection of
//!    a connected interface (`WlanQueryInterface`
//!    `wlan_intf_opcode_current_connection`). Windows 11 24H2 lets a
//!    desktop app read it only with location access for desktop apps on
//!    (Settings > Privacy & security > Location); without it the call
//!    fails with ERROR_ACCESS_DENIED, shown as "not allowed".

use std::ffi::c_void;

use connect_core::observe::sources::{call_app_of, windows_app_name, Front as FrontApp, FrontProbe, MicProbe, MicSample, Wifi as WifiNow, WifiProbe};
use connect_core::observe::AppMark;
use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, ERROR_SUCCESS, HANDLE, HWND, LPARAM};
use windows_sys::Win32::NetworkManagement::WiFi::{
    wlan_interface_state_connected, wlan_intf_opcode_current_connection, WlanCloseHandle, WlanEnumInterfaces, WlanFreeMemory, WlanOpenHandle, WlanQueryInterface, WLAN_CONNECTION_ATTRIBUTES,
    WLAN_INTERFACE_INFO, WLAN_INTERFACE_INFO_LIST,
};
use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible};

use crate::island::wake::system;

/// The program file name of a process ("Code.exe"). None when Windows
/// does not say (a process of another user, an elevated one).
fn exe_of(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let h: HANDLE = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return None;
    }
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) } != 0;
    unsafe { CloseHandle(h) };
    if !ok {
        return None;
    }
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    path.rsplit(['\\', '/']).next().map(str::to_string).filter(|s| !s.is_empty())
}

fn title_of(hwnd: HWND) -> Option<String> {
    let n = unsafe { GetWindowTextLengthW(hwnd) };
    if n <= 0 {
        return None;
    }
    let mut buf = vec![0u16; (n as usize).min(4096) + 1];
    let got = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    (got > 0).then(|| String::from_utf16_lossy(&buf[..got as usize]))
}

fn pid_of(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    pid
}

pub struct Mic;

impl MicProbe for Mic {
    fn sample(&mut self) -> MicSample {
        let recorders = system::recorders();
        let busy = recorders.as_ref().is_some_and(|r| !r.is_empty());
        let mut running: Vec<String> = Vec::new();
        if busy {
            running = system::running_programs().unwrap_or_default().into_iter().filter(|exe| call_app_of(exe).is_some()).collect();
            running.sort();
            running.dedup();
            let hwnd = unsafe { GetForegroundWindow() };
            if let Some(front) = (!hwnd.is_null()).then(|| exe_of(pid_of(hwnd))).flatten().map(|e| e.to_lowercase()) {
                if running.contains(&front) {
                    running.retain(|e| *e != front);
                    running.insert(0, front);
                }
            }
        }
        MicSample { recorders, device_running: false, own_capture: crate::island::audio::own_capture(), running }
    }
}

pub struct Front;

impl FrontProbe for Front {
    fn front(&mut self) -> Option<FrontApp> {
        if system::locked() {
            return None;
        }
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_null() {
            return None;
        }
        let exe = exe_of(pid_of(hwnd))?;
        let title = title_of(hwnd);
        // A Store app's window belongs to its frame host: its title is
        // the app's name.
        let name = if exe.eq_ignore_ascii_case("applicationframehost.exe") {
            title.clone().unwrap_or_else(|| windows_app_name(&exe))
        } else {
            windows_app_name(&exe)
        };
        Some(FrontApp { id: exe, name, title })
    }
}

unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let out = unsafe { &mut *(lparam as *mut Vec<u32>) };
    if unsafe { IsWindowVisible(hwnd) } != 0 && unsafe { GetWindowTextLengthW(hwnd) } > 0 {
        let pid = pid_of(hwnd);
        if !out.contains(&pid) {
            out.push(pid);
        }
    }
    1
}

/// The programs with a visible, titled window (the exclusion picker).
pub fn running_apps() -> Vec<AppMark> {
    let mut pids: Vec<u32> = Vec::new();
    unsafe { EnumWindows(Some(collect_window), &mut pids as *mut Vec<u32> as LPARAM) };
    let me = std::process::id();
    let mut out: Vec<AppMark> = pids
        .into_iter()
        .filter(|p| *p != me)
        .filter_map(exe_of)
        .filter(|e| !e.eq_ignore_ascii_case("explorer.exe") && !e.eq_ignore_ascii_case("applicationframehost.exe"))
        .map(|e| AppMark { name: windows_app_name(&e), id: e })
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    out.dedup_by(|a, b| a.id.eq_ignore_ascii_case(&b.id));
    out
}

// --- Wi-Fi ---

struct Wlan(HANDLE);

impl Drop for Wlan {
    fn drop(&mut self) {
        unsafe { WlanCloseHandle(self.0, std::ptr::null()) };
    }
}

pub fn wifi_now() -> WifiNow {
    let mut version = 0u32;
    let mut h: HANDLE = std::ptr::null_mut();
    // No WLAN service (a desktop without Wi-Fi): no network.
    if unsafe { WlanOpenHandle(2, std::ptr::null(), &mut version, &mut h) } != ERROR_SUCCESS {
        return WifiNow::NotConnected;
    }
    let wlan = Wlan(h);
    let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
    if unsafe { WlanEnumInterfaces(wlan.0, std::ptr::null(), &mut list) } != ERROR_SUCCESS || list.is_null() {
        return WifiNow::NotConnected;
    }
    let mut out = WifiNow::NotConnected;
    let count = unsafe { (*list).dwNumberOfItems } as usize;
    let first = unsafe { std::ptr::addr_of!((*list).InterfaceInfo) } as *const WLAN_INTERFACE_INFO;
    for i in 0..count.min(16) {
        let info = unsafe { &*first.add(i) };
        if info.isState != wlan_interface_state_connected {
            continue;
        }
        let mut size = 0u32;
        let mut data: *mut c_void = std::ptr::null_mut();
        let rc = unsafe { WlanQueryInterface(wlan.0, &info.InterfaceGuid, wlan_intf_opcode_current_connection, std::ptr::null(), &mut size, &mut data, std::ptr::null_mut()) };
        if rc == ERROR_ACCESS_DENIED {
            out = WifiNow::Denied;
            continue;
        }
        if rc != ERROR_SUCCESS || data.is_null() {
            continue;
        }
        if size as usize >= std::mem::size_of::<WLAN_CONNECTION_ATTRIBUTES>() {
            let attrs = unsafe { &*(data as *const WLAN_CONNECTION_ATTRIBUTES) };
            let ssid = attrs.wlanAssociationAttributes.dot11Ssid;
            let n = (ssid.uSSIDLength as usize).min(ssid.ucSSID.len());
            let name = String::from_utf8_lossy(&ssid.ucSSID[..n]).into_owned();
            unsafe { WlanFreeMemory(data) };
            if !name.is_empty() {
                out = WifiNow::Network(name);
                break;
            }
            continue;
        }
        unsafe { WlanFreeMemory(data) };
    }
    unsafe { WlanFreeMemory(list as *const c_void) };
    out
}

pub struct Wifi;

impl WifiProbe for Wifi {
    fn read(&mut self) -> WifiNow {
        wifi_now()
    }
}
