//! Presence: how long since the owner last touched the machine, and
//! whether the screen is locked.
//!
//! macOS has real values (Quartz). Windows has the idle time
//! (GetLastInputInfo) but not yet the lock state. Linux is a stub that
//! reports "active, unlocked" (the conservative answer: the synth treats
//! the owner as present) with a TODO.
//!
//! Also here: whether the owner asked this computer to reduce motion
//! (`reduced_motion`), which the synth's cursor follows.

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
        fn CFPreferencesGetAppBooleanValue(
            key: *const c_void,
            app: *const c_void,
            key_exists_and_is_valid: *mut u8,
        ) -> u8;
        fn CFPreferencesAppSynchronize(app: *const c_void) -> u8;
    }

    /// System Settings > Accessibility > Display > Reduce motion: the
    /// `reduceMotion` key of `com.apple.universalaccess`. A read of a
    /// preference; nothing is shown and no permission is involved.
    pub fn reduced_motion() -> bool {
        unsafe {
            let key = CFStringCreateWithCString(
                std::ptr::null(),
                c"reduceMotion".as_ptr(),
                UTF8,
            );
            let app = CFStringCreateWithCString(
                std::ptr::null(),
                c"com.apple.universalaccess".as_ptr(),
                UTF8,
            );
            let mut valid: u8 = 0;
            let on = if key.is_null() || app.is_null() {
                false
            } else {
                // The app runs for days: without this the answer is the
                // one from the first read.
                CFPreferencesAppSynchronize(app);
                CFPreferencesGetAppBooleanValue(key, app, &mut valid) != 0 && valid != 0
            };
            if !key.is_null() {
                CFRelease(key);
            }
            if !app.is_null() {
                CFRelease(app);
            }
            on
        }
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
        fn SystemParametersInfoW(action: u32, param: u32, value: *mut std::ffi::c_void, flags: u32) -> i32;
    }

    /// Settings > Accessibility > Visual effects > Animation effects, off:
    /// `SPI_GETCLIENTAREAANIMATION` answers false.
    pub fn reduced_motion() -> bool {
        const SPI_GETCLIENTAREAANIMATION: u32 = 0x1042;
        let mut animations: i32 = 1;
        let ok = unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                &mut animations as *mut i32 as *mut std::ffi::c_void,
                0,
            )
        };
        ok != 0 && animations == 0
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

    /// GNOME and the desktops that follow its setting: "animations" off.
    /// Where `gsettings` is missing or says nothing, motion is not reduced.
    pub fn reduced_motion() -> bool {
        crate::proc::std_command("gsettings")
            .args(["get", "org.gnome.desktop.interface", "enable-animations"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .is_some_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "false")
    }
}

/// Read the current presence.
pub fn read() -> Presence {
    Presence {
        idle_seconds: imp::idle_seconds(),
        screen_locked: imp::screen_locked(),
    }
}

/// Did the owner ask this computer to reduce motion? Read when our driver
/// daemon starts, for the "follow the system" choice of the cursor's
/// animation: the driver's own `auto` does not look at the system.
pub fn reduced_motion() -> bool {
    imp::reduced_motion()
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
        let _ = reduced_motion();
    }
}
