//! Owner-controlled macOS power assertions. These do not unlock a session
//! or wake a CPU that is already asleep. Idle assertions have an OS timeout
//! as well as Rust ownership, so a stalled runtime cannot hold one forever.

use std::time::{Duration, Instant};
use tokio::sync::watch;

#[cfg(target_os = "macos")]
const ASSERTION_TTL: Duration = Duration::from_secs(30);
const RENEW_AFTER: Duration = Duration::from_secs(10);

#[derive(Clone, Copy)]
pub enum Kind {
    Reachable,
    Display,
}

/// Dropping a hold releases it synchronously, including on task abortion.
pub struct Hold {
    inner: platform::Assertion,
    renewed: Instant,
}

impl Hold {
    pub fn acquire(kind: Kind) -> Result<Self, String> {
        Ok(Self {
            inner: platform::Assertion::new(kind)?,
            renewed: Instant::now(),
        })
    }

    pub fn renew(&mut self, kind: Kind) -> Result<(), String> {
        if self.renewed.elapsed() >= RENEW_AFTER {
            // Acquire before releasing the previous assertion. Its expiry
            // remains bounded even if the app stops making progress here.
            self.inner = platform::Assertion::new(kind)?;
            self.renewed = Instant::now();
        }
        Ok(())
    }
}

fn availability_enabled(paired: bool, paused: bool, enabled: bool, opted_in: bool) -> bool {
    cfg!(target_os = "macos") && paired && !paused && enabled && opted_in
}

/// Called once by the desktop shell. A locked screen may stay reachable,
/// but waking the display is reserved for an independently permitted call.
pub async fn run_availability(core: crate::Core, mut shutdown: watch::Receiver<bool>) {
    if !cfg!(target_os = "macos") { return; }
    let mut held: Option<Hold> = None;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let shared = core.shared();
    let mut settings = shared.settings_tx.subscribe();
    let mut control = shared.control_tx.subscribe();
    let mut reported_failure = false;
    loop {
        if *shutdown.borrow() {
            break;
        }
        let s = shared.settings();
        if availability_enabled(
            core.is_paired(),
            shared.paused(),
            s.computer.enabled,
            s.computer.keep_awake,
        ) {
            let result = match held.as_mut() {
                Some(hold) => hold.renew(Kind::Reachable),
                None => Hold::acquire(Kind::Reachable).map(|hold| held = Some(hold)),
            };
            if let Err(error) = result {
                held = None;
                if !reported_failure {
                    crate::logln!("power: {error}");
                    reported_failure = true;
                }
            } else {
                reported_failure = false;
            }
        } else {
            held = None;
            reported_failure = false;
        }
        tokio::select! {
            _ = tick.tick() => {},
            _ = settings.changed() => {},
            _ = control.changed() => {},
            _ = shutdown.changed() => break,
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{Kind, ASSERTION_TTL};
    use std::ffi::{c_char, c_void, CString};

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            text: *const c_char,
            encoding: u32,
        ) -> *const c_void;
        fn CFRelease(value: *const c_void);
    }
    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMAssertionCreateWithDescription(
            kind: *const c_void,
            name: *const c_void,
            details: *const c_void,
            reason: *const c_void,
            bundle: *const c_void,
            timeout: f64,
            action: *const c_void,
            id: *mut u32,
        ) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
        fn IOPMAssertionDeclareUserActivity(
            name: *const c_void,
            user_type: u32,
            id: *mut u32,
        ) -> i32;
        #[cfg(test)]
        fn IOPMAssertionCopyProperties(id: u32) -> *const c_void;
    }

    struct Text(*const c_void);
    impl Text {
        fn new(text: &str) -> Result<Self, String> {
            let text = CString::new(text).map_err(|_| "invalid power assertion name")?;
            let value =
                unsafe { CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), 0x0800_0100) };
            if value.is_null() {
                Err("could not create power assertion name".into())
            } else {
                Ok(Self(value))
            }
        }
    }
    impl Drop for Text {
        fn drop(&mut self) {
            unsafe {
                CFRelease(self.0);
            }
        }
    }

    pub struct Assertion {
        id: u32,
        activity: Option<u32>,
    }
    impl Assertion {
        pub fn new(kind: Kind) -> Result<Self, String> {
            let (typ, label) = match kind {
                Kind::Reachable => (
                    "PreventUserIdleSystemSleep",
                    "SNTH Connect: available for permitted tasks",
                ),
                Kind::Display => (
                    "PreventUserIdleDisplaySleep",
                    "SNTH Connect: active computer task",
                ),
            };
            let typ = Text::new(typ)?;
            let name = Text::new(label)?;
            let action = Text::new("TimeoutActionTurnOff")?;
            let mut id = 0;
            let null = std::ptr::null();
            let status = unsafe {
                IOPMAssertionCreateWithDescription(
                    typ.0,
                    name.0,
                    null,
                    null,
                    null,
                    ASSERTION_TTL.as_secs_f64(),
                    action.0,
                    &mut id,
                )
            };
            if status != 0 {
                return Err(format!("macOS power assertion failed ({status})"));
            }
            let mut result = Self { id, activity: None };
            if matches!(kind, Kind::Display) {
                let mut activity = 0;
                // kIOPMUserActiveRemote: report remote activity, not local
                // HID input. The sleep assertion above bounds the hold.
                let status = unsafe { IOPMAssertionDeclareUserActivity(name.0, 1, &mut activity) };
                if status != 0 {
                    return Err(format!("macOS display wake failed ({status})"));
                }
                result.activity = Some(activity);
            }
            Ok(result)
        }
    }
    impl Drop for Assertion {
        fn drop(&mut self) {
            unsafe {
                if let Some(id) = self.activity {
                    IOPMAssertionRelease(id);
                }
                IOPMAssertionRelease(self.id);
            }
        }
    }

    #[cfg(test)]
    pub fn verify_release() {
        let assertion = Assertion::new(Kind::Reachable).expect("IOKit assertion");
        let id = assertion.id;
        unsafe {
            let properties = IOPMAssertionCopyProperties(id);
            assert!(
                !properties.is_null(),
                "IOKit must report the held assertion"
            );
            CFRelease(properties);
        }
        drop(assertion);
        unsafe {
            let properties = IOPMAssertionCopyProperties(id);
            let released = properties.is_null();
            if !released {
                CFRelease(properties);
            }
            assert!(
                released,
                "IOKit must no longer report the released assertion"
            );
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub struct Assertion;
    impl Assertion {
        pub fn new(_: super::Kind) -> Result<Self, String> {
            Err("Mac power management is unavailable on this platform".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_requires_every_owner_condition() {
        for mask in 0..16 {
            let paired = mask & 1 != 0;
            let paused = mask & 2 != 0;
            let enabled = mask & 4 != 0;
            let opted_in = mask & 8 != 0;
            assert_eq!(
                availability_enabled(paired, paused, enabled, opted_in),
                cfg!(target_os = "macos") && mask == 13
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "live IOKit check; creates an assertion briefly, without waking the display"]
    fn native_hold_releases_on_drop() {
        platform::verify_release();
    }
}
