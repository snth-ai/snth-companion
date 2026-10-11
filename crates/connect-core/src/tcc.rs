//! macOS permissions for computer control, asked for and checked as the
//! HOST app.
//!
//! The app runs its own driver as a child (`cua-driver serve --embedded`).
//! macOS does not tie Accessibility and Screen Recording to the driver's
//! file: it ties them to the responsible process, the app at the top of
//! the launch chain. So the grants the embedded driver works with are
//! this app's, the entries in System Settings carry this app's name, and
//! it is this process that has to ask (the driver's embedding guide:
//! "Request Accessibility and Screen Recording from your app ... using
//! AXIsProcessTrustedWithOptions and CGRequestScreenCaptureAccess";
//! embedded mode itself never prompts).
//!
//! `cua-driver permissions grant|status` are the wrong tools here: they
//! work through `CuaDriver.app` and report the `com.trycua.driver`
//! identity, which is not the one our embedded driver runs under.
//!
//! Other systems have no such permissions: `status` is None there.

/// What macOS currently allows this app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostPermissions {
    /// May control other apps (click, type, read their windows).
    pub accessibility: bool,
    /// May capture the screen.
    pub screen_recording: bool,
}

#[cfg(target_os = "macos")]
mod imp {
    use super::HostPermissions;
    use std::ffi::c_void;

    #[repr(C)]
    struct Opaque {
        _private: [u8; 0],
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        static kAXTrustedCheckOptionPrompt: *const c_void;
        fn AXIsProcessTrusted() -> u8;
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> u8;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFBooleanTrue: *const c_void;
        static kCFTypeDictionaryKeyCallBacks: Opaque;
        static kCFTypeDictionaryValueCallBacks: Opaque;
        fn CFDictionaryCreate(
            allocator: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            count: isize,
            key_callbacks: *const Opaque,
            value_callbacks: *const Opaque,
        ) -> *const c_void;
        fn CFRelease(cf: *const c_void);
    }

    /// Read-only: neither call shows anything.
    pub fn status() -> Option<HostPermissions> {
        let (accessibility, screen_recording) =
            unsafe { (AXIsProcessTrusted() != 0, CGPreflightScreenCaptureAccess()) };
        Some(HostPermissions {
            accessibility,
            screen_recording,
        })
    }

    /// Makes macOS show its own dialogs for whatever is not allowed yet.
    pub fn request() {
        unsafe {
            if AXIsProcessTrusted() == 0 {
                let keys = [kAXTrustedCheckOptionPrompt];
                let values = [kCFBooleanTrue];
                let options = CFDictionaryCreate(
                    std::ptr::null(),
                    keys.as_ptr(),
                    values.as_ptr(),
                    1,
                    &kCFTypeDictionaryKeyCallBacks,
                    &kCFTypeDictionaryValueCallBacks,
                );
                if !options.is_null() {
                    let _ = AXIsProcessTrustedWithOptions(options);
                    CFRelease(options);
                }
            }
            if !CGPreflightScreenCaptureAccess() {
                let _ = CGRequestScreenCaptureAccess();
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::HostPermissions;

    pub fn status() -> Option<HostPermissions> {
        None
    }

    pub fn request() {}
}

/// What this app is allowed right now. Read-only, never prompts. None on
/// systems that have no such permissions.
pub fn status() -> Option<HostPermissions> {
    imp::status()
}

/// Ask macOS for the permissions that are still missing. This raises
/// system dialogs on the owner's screen: call it only from their click.
/// A no-op elsewhere.
pub fn request() {
    imp::request()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_read_only_and_does_not_panic() {
        // The answer depends on what runs the test; this proves the calls
        // link and return. `request` is never called from a test: it
        // would put a system dialog on the screen.
        let s = status();
        assert_eq!(s.is_some(), cfg!(target_os = "macos"));
    }
}
