//! Presence: how long since the owner last touched the machine, and
//! whether the screen is locked.
//!
//! macOS has real values (Quartz). Windows has the idle time
//! (GetLastInputInfo) but not yet the lock state. Linux is a stub that
//! reports "active, unlocked" (the conservative answer: the synth treats
//! the owner as present) with a TODO.

/// Seconds since the last keyboard or pointer input, and whether the
/// screen is locked.
#[derive(Debug, Clone, Copy, Default)]
pub struct Presence {
    pub idle_seconds: u64,
    pub screen_locked: bool,
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_void;
    use std::os::raw::c_char;

    // Quartz event-source state and the "any input" event type.
    const HID_SYSTEM_STATE: u32 = 1;
    const ANY_INPUT_EVENT: u32 = 0xFFFF_FFFF;
    const UTF8: u32 = 0x0800_0100;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(state: u32, event_type: u32) -> f64;
        fn CGSessionCopyCurrentDictionary() -> *const c_void;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            cstr: *const c_char,
            encoding: u32,
        ) -> *const c_void;
        fn CFBooleanGetValue(b: *const c_void) -> u8;
        fn CFGetTypeID(cf: *const c_void) -> usize;
        fn CFBooleanGetTypeID() -> usize;
        fn CFRelease(cf: *const c_void);
    }

    pub fn idle_seconds() -> u64 {
        let s = unsafe { CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, ANY_INPUT_EVENT) };
        if s.is_finite() && s > 0.0 {
            s as u64
        } else {
            0
        }
    }

    pub fn screen_locked() -> bool {
        unsafe {
            let dict = CGSessionCopyCurrentDictionary();
            if dict.is_null() {
                return false;
            }
            let key_c = b"CGSSessionScreenIsLocked\0";
            let key = CFStringCreateWithCString(
                std::ptr::null(),
                key_c.as_ptr() as *const c_char,
                UTF8,
            );
            if key.is_null() {
                CFRelease(dict);
                return false;
            }
            let val = CFDictionaryGetValue(dict, key);
            // `val` is borrowed from `dict`; do not release it.
            let locked = if !val.is_null() && CFGetTypeID(val) == CFBooleanGetTypeID() {
                CFBooleanGetValue(val) != 0
            } else {
                false
            };
            CFRelease(key);
            CFRelease(dict);
            locked
        }
    }
}

#[cfg(windows)]
mod imp {
    #[repr(C)]
    struct LastInputInfo {
        cb_size: u32,
        dw_time: u32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetLastInputInfo(info: *mut LastInputInfo) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetTickCount() -> u32;
    }

    /// Time since the last keyboard or mouse input in this session. Both
    /// values are milliseconds since boot in 32 bits, so the difference
    /// is taken with wrap-around.
    pub fn idle_seconds() -> u64 {
        let mut info = LastInputInfo {
            cb_size: std::mem::size_of::<LastInputInfo>() as u32,
            dw_time: 0,
        };
        unsafe {
            if GetLastInputInfo(&mut info) == 0 {
                return 0;
            }
            u64::from(GetTickCount().wrapping_sub(info.dw_time)) / 1000
        }
    }

    // TODO(P2): the lock state (WTS session notifications). Until then
    // "unlocked": the synth treats the owner as possibly present.
    pub fn screen_locked() -> bool {
        false
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    // TODO(P2): real values. Linux X11: XScreenSaverQueryInfo + a
    // lock-detection heuristic. Wayland: no portable idle API.
    pub fn idle_seconds() -> u64 {
        0
    }
    pub fn screen_locked() -> bool {
        false
    }
}

/// Read the current presence.
pub fn read() -> Presence {
    Presence {
        idle_seconds: imp::idle_seconds(),
        screen_locked: imp::screen_locked(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_does_not_panic() {
        // Values are environment-dependent; this only proves the FFI path
        // links and returns.
        let p = read();
        let _ = p.idle_seconds;
        let _ = p.screen_locked;
    }
}
