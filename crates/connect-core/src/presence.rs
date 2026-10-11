//! Presence: how long since the owner last touched the machine, and
//! whether the screen is locked.
//!
//! macOS has real values (Quartz). Other platforms are stubs that report
//! "active, unlocked" (the conservative answer: the synth treats the
//! owner as present) with a TODO.

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

#[cfg(not(target_os = "macos"))]
mod imp {
    // TODO(P2): real values per OS. Windows: GetLastInputInfo +
    // WTSRegisterSessionNotification / session lock state. Linux X11:
    // XScreenSaverQueryInfo + a lock-detection heuristic. Wayland: no
    // portable idle API.
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
