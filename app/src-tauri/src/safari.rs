//! Safari (macOS): the extension that ships inside this app.
//!
//! Safari has no helper program and nothing to install: its extension is
//! an app extension inside `SNTH Connect.app`
//! (`Contents/PlugIns/SNTH Vault.appex`), and macOS shows it in Safari's
//! settings once the app has been opened. Two things remain for the owner,
//! and no program can do either: switch the extension on in Safari's
//! settings, and allow each site in Safari's own prompt.
//!
//! So the Logins page has to show where Safari stands even before the
//! extension ever talked to the app. This module finds that out:
//!
//!  - is the extension in this copy of the app, and will the app still be
//!    at this path tomorrow (not when it was opened from the disk image);
//!  - did the owner switch it on (Safari is asked; it answers the app the
//!    extension lives in and nobody else);
//!  - is the separate "SNTH Vault" app for Safari installed as well (then
//!    Safari lists two extensions and the page says which is which).
//!
//! It also opens Safari's settings at the extension, which is the one
//! thing the app can do to help. Nothing here reads anything of Safari's:
//! no cookie, no history, no setting.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;

/// Where the extension stands in Safari.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
// Only a Mac can say "off"; the other systems never ask.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum Enabled {
    /// Switched on in Safari's settings.
    On,
    /// In Safari's list, switched off (how every extension starts).
    Off,
    /// Safari did not say: not asked yet, or it does not know the
    /// extension (the app was never opened from where it is now).
    Unknown,
    /// The app runs from a temporary copy (opened in the disk image):
    /// Safari cannot keep an extension from there.
    Translocated,
}

/// What the Logins page shows about Safari besides the row itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SafariInfo {
    pub enabled: Enabled,
    /// The stand-alone "SNTH Vault" app for Safari is installed too.
    pub standalone: bool,
}

/// The extension inside the app bundle `exe` belongs to, when it is there.
/// A debug build and the Windows and Linux apps have none.
pub fn appex_for(exe: &Path) -> Option<PathBuf> {
    // .../SNTH Connect.app/Contents/MacOS/snth-connect
    let contents = exe.parent()?.parent()?;
    let appex = contents.join(connect_host::safari::APPEX_IN_BUNDLE);
    (contents.file_name()? == "Contents" && appex.join("Contents/Info.plist").is_file()).then_some(appex)
}

/// Is the stand-alone Safari edition of SNTH Vault installed? By the place
/// people put apps; nothing is opened or asked.
pub fn standalone_installed(home: Option<&Path>) -> bool {
    const APP: &str = "SNTH Vault.app";
    Path::new("/Applications").join(APP).is_dir() || home.is_some_and(|h| h.join("Applications").join(APP).is_dir())
}

/// What this run of the app knows about its Safari extension.
pub struct Safari {
    /// The extension, when this copy of the app carries one.
    appex: Option<PathBuf>,
    /// The app is where it will stay (see `logins::stays_put`).
    settled: bool,
    enabled: Mutex<Enabled>,
    /// Why the Safari door is not being served, when it is not.
    pub door: Mutex<Option<String>>,
}

impl Safari {
    pub fn detect(settled: impl Fn(&Path) -> bool) -> Safari {
        let exe = if cfg!(target_os = "macos") { std::env::current_exe().ok() } else { None };
        let appex = exe.as_deref().and_then(appex_for);
        let settled = exe.as_deref().is_some_and(settled);
        Safari { appex, settled, enabled: Mutex::new(Enabled::Unknown), door: Mutex::new(None) }
    }

    /// For tests: an app with (or without) the extension.
    #[cfg(test)]
    pub fn fixed(present: bool, settled: bool) -> Safari {
        Safari {
            appex: present.then(|| PathBuf::from("/x/SNTH Connect.app/Contents/PlugIns/SNTH Vault.appex")),
            settled,
            enabled: Mutex::new(Enabled::Unknown),
            door: Mutex::new(None),
        }
    }

    /// Does this copy of the app carry the extension at all?
    pub fn present(&self) -> bool {
        self.appex.is_some()
    }

    /// May Safari use it from here? Then the app serves the Safari door.
    pub fn usable(&self) -> bool {
        self.present() && self.settled
    }

    pub fn info(&self) -> Option<SafariInfo> {
        self.present().then(|| SafariInfo {
            enabled: if self.settled { *self.enabled.lock().unwrap() } else { Enabled::Translocated },
            standalone: standalone_installed(directories::BaseDirs::new().as_ref().map(|d| d.home_dir())),
        })
    }

    /// Ask Safari again whether the extension is switched on. Blocks for
    /// up to two seconds: call it off the async threads.
    pub fn refresh(&self) {
        if !self.usable() {
            return;
        }
        let now = ffi::extension_enabled(connect_host::safari::EXTENSION_ID, Duration::from_secs(2));
        *self.enabled.lock().unwrap() = now;
    }

    /// The extension said hello: it is on, whatever was known before.
    pub fn heard_from(&self) {
        if self.usable() {
            *self.enabled.lock().unwrap() = Enabled::On;
        }
    }

    /// Open Safari's settings at the extension. False when Safari could
    /// not be asked to (the caller then just opens Safari).
    pub fn show_settings(&self) -> bool {
        self.usable() && ffi::show_settings(connect_host::safari::EXTENSION_ID)
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    //! The two calls of SafariServices an app makes about its own
    //! extension. They are Objective-C class methods that answer through
    //! a block, on a queue of their own.
    //!
    //! NO OBJECTIVE-C EXCEPTION MAY REACH RUST. Rust cannot catch one: an
    //! exception that unwinds out of a message send runs into the
    //! `catch_unwind` of the task the call runs in (tokio's, Tauri's), and
    //! that aborts the whole app ("fatal runtime error: Rust cannot catch
    //! foreign exceptions", SIGABRT on a tokio-rt-worker). That was the
    //! 0.1.7 crash on opening Logins: the question to Safari runs on a
    //! tokio blocking thread. So every send here goes through `guarded`,
    //! asks `respondsToSelector:` first (a macOS whose Safari API moved
    //! answers "no" instead of raising "unrecognized selector"), and runs
    //! inside an autorelease pool (the tokio thread has none of its own).

    use std::ffi::CString;
    use std::panic::AssertUnwindSafe;
    use std::sync::mpsc;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::exception;
    use objc2::rc::autoreleasepool;
    use objc2::runtime::{AnyClass, AnyObject, Sel};
    use objc2::{msg_send, sel};

    use super::Enabled;

    #[link(name = "SafariServices", kind = "framework")]
    extern "C" {}

    pub(super) fn ns_string(text: &str) -> Option<*mut AnyObject> {
        let class = AnyClass::get(c"NSString")?;
        let text = CString::new(text).ok()?;
        // An autoreleased NSString; used at once, inside the caller's pool.
        let string: *mut AnyObject = unsafe { msg_send![class, stringWithUTF8String: text.as_ptr()] };
        (!string.is_null()).then_some(string)
    }

    /// Run Objective-C sends with any exception they raise caught on this
    /// side. None (and a log line) when one was raised.
    fn guarded<R>(what: &str, call: impl FnOnce() -> R) -> Option<R> {
        match exception::catch(AssertUnwindSafe(call)) {
            Ok(value) => Some(value),
            Err(e) => {
                let why = e.as_deref().map_or_else(|| "nil".to_string(), |e| format!("{e:?}"));
                connect_core::logln!("safari: {what} raised an Objective-C exception: {why}");
                None
            }
        }
    }

    /// Does `receiver` (an object, or a class for its class methods)
    /// answer `selector`? Asked inside `guarded`.
    fn responds(receiver: &AnyObject, selector: Sel) -> bool {
        unsafe { msg_send![receiver, respondsToSelector: selector] }
    }

    fn class_object(class: &AnyClass) -> &AnyObject {
        // A class is an object; its class methods are what it responds to.
        unsafe { &*(class as *const AnyClass as *const AnyObject) }
    }

    /// What one SFSafariExtensionState says.
    fn read_state(state: &AnyObject) -> Enabled {
        let on = guarded("-[SFSafariExtensionState isEnabled]", || {
            responds(state, sel!(isEnabled)).then(|| {
                let on: bool = unsafe { msg_send![state, isEnabled] };
                on
            })
        });
        match on.flatten() {
            Some(true) => Enabled::On,
            Some(false) => Enabled::Off,
            None => Enabled::Unknown,
        }
    }

    /// `+[<manager> getStateOfSafariExtensionWithIdentifier:completionHandler:]`
    /// on the class given, which is SFSafariExtensionManager outside tests.
    pub(super) fn enabled_from(manager: &AnyClass, identifier: &str, wait: Duration) -> Enabled {
        let (tx, rx) = mpsc::sync_channel::<Enabled>(1);
        // void (^)(SFSafariExtensionState *state, NSError *error). Safari
        // calls it on a queue of its own, or at once on this thread.
        let answer = RcBlock::new(move |state: *mut AnyObject, error: *mut AnyObject| {
            let got = match unsafe { state.as_ref() } {
                Some(state) if error.is_null() => autoreleasepool(|_| read_state(state)),
                _ => Enabled::Unknown,
            };
            // A late answer finds nobody waiting; that is fine.
            let _ = tx.try_send(got);
        });
        let asked = autoreleasepool(|_| {
            let Some(identifier) = ns_string(identifier) else {
                return false;
            };
            let manager = class_object(manager);
            guarded("+[SFSafariExtensionManager getStateOfSafariExtensionWithIdentifier:completionHandler:]", || {
                if !responds(manager, sel!(getStateOfSafariExtensionWithIdentifier:completionHandler:)) {
                    return false;
                }
                let _: () = unsafe { msg_send![manager, getStateOfSafariExtensionWithIdentifier: identifier, completionHandler: &*answer] };
                true
            })
            .unwrap_or(false)
        });
        if !asked {
            return Enabled::Unknown;
        }
        rx.recv_timeout(wait).unwrap_or(Enabled::Unknown)
    }

    pub(super) fn extension_enabled(identifier: &str, wait: Duration) -> Enabled {
        match AnyClass::get(c"SFSafariExtensionManager") {
            Some(manager) => enabled_from(manager, identifier, wait),
            None => Enabled::Unknown,
        }
    }

    /// `+[<application> showPreferencesForExtensionWithIdentifier:completionHandler:]`
    pub(super) fn show_settings_with(application: &AnyClass, identifier: &str) -> bool {
        // void (^)(NSError *error): nothing to do with the answer.
        let done = RcBlock::new(|_error: *mut AnyObject| {});
        autoreleasepool(|_| {
            let Some(identifier) = ns_string(identifier) else {
                return false;
            };
            let application = class_object(application);
            guarded("+[SFSafariApplication showPreferencesForExtensionWithIdentifier:completionHandler:]", || {
                if !responds(application, sel!(showPreferencesForExtensionWithIdentifier:completionHandler:)) {
                    return false;
                }
                let _: () = unsafe {
                    msg_send![application, showPreferencesForExtensionWithIdentifier: identifier, completionHandler: &*done]
                };
                true
            })
            .unwrap_or(false)
        })
    }

    pub(super) fn show_settings(identifier: &str) -> bool {
        match AnyClass::get(c"SFSafariApplication") {
            Some(application) => show_settings_with(application, identifier),
            None => false,
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod ffi {
    use std::time::Duration;

    use super::Enabled;

    pub(super) fn extension_enabled(_identifier: &str, _wait: Duration) -> Enabled {
        Enabled::Unknown
    }

    pub(super) fn show_settings(_identifier: &str) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_extension_is_looked_for_inside_the_app_bundle_only() {
        let root = std::env::temp_dir().join(format!("snth-connect-safari-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let app = root.join("SNTH Connect.app/Contents");
        std::fs::create_dir_all(app.join("MacOS")).unwrap();
        let exe = app.join("MacOS/snth-connect");
        // No extension in the bundle (a debug build, an old release).
        assert_eq!(appex_for(&exe), None);
        // A folder of that name without an Info.plist is not an extension.
        std::fs::create_dir_all(app.join("PlugIns/SNTH Vault.appex/Contents")).unwrap();
        assert_eq!(appex_for(&exe), None);
        std::fs::write(app.join("PlugIns/SNTH Vault.appex/Contents/Info.plist"), b"<plist/>").unwrap();
        assert_eq!(appex_for(&exe), Some(app.join("PlugIns/SNTH Vault.appex")));
        // A program that is not inside an app bundle has none.
        std::fs::create_dir_all(root.join("target/debug/PlugIns/SNTH Vault.appex/Contents")).unwrap();
        std::fs::write(root.join("target/debug/PlugIns/SNTH Vault.appex/Contents/Info.plist"), b"<plist/>").unwrap();
        assert_eq!(appex_for(&root.join("target/debug/bin/snth-connect")), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_app_without_the_extension_shows_no_safari_and_asks_nobody() {
        let none = Safari::fixed(false, true);
        assert!(!none.present() && !none.usable());
        assert_eq!(none.info(), None);
        none.refresh();
        none.heard_from();
        assert!(!none.show_settings());
        assert_eq!(none.info(), None);
    }

    #[test]
    fn an_app_opened_in_the_disk_image_says_so_and_asks_nobody() {
        let moving = Safari::fixed(true, false);
        assert!(moving.present() && !moving.usable());
        assert_eq!(moving.info().unwrap().enabled, Enabled::Translocated);
        moving.heard_from();
        assert_eq!(moving.info().unwrap().enabled, Enabled::Translocated);
        assert!(!moving.show_settings());
    }

    #[test]
    fn hearing_from_the_extension_means_it_is_on() {
        let safari = Safari::fixed(true, true);
        assert_eq!(safari.info().unwrap().enabled, Enabled::Unknown);
        safari.heard_from();
        assert_eq!(safari.info().unwrap().enabled, Enabled::On);
    }

    #[test]
    fn the_page_gets_plain_words() {
        let text = serde_json::to_string(&SafariInfo { enabled: Enabled::Translocated, standalone: true }).unwrap();
        assert_eq!(text, r#"{"enabled":"translocated","standalone":true}"#);
        assert_eq!(serde_json::to_string(&Enabled::Off).unwrap(), r#""off""#);
    }

    /// The calls into SafariServices cannot be made in a test (they talk to
    /// the owner's Safari). What can go wrong on this side of them is the
    /// calling convention: a class method that takes an NSString and a
    /// block, and a block that takes two objects. That is run here for
    /// real, against two classes of the test's own that answer the way
    /// Safari's do.
    #[cfg(target_os = "macos")]
    mod calls {
        use std::ffi::CStr;
        use std::time::Duration;

        use block2::DynBlock;
        use objc2::rc::Retained;
        use objc2::runtime::{AnyClass, AnyObject, NSObject};
        use objc2::{define_class, msg_send, ClassType};

        use super::super::ffi::{enabled_from, ns_string, show_settings_with};
        use super::super::Enabled;

        define_class!(
            #[unsafe(super(NSObject))]
            #[name = "SNTHConnectTestExtensionState"]
            struct OnState;

            impl OnState {
                #[unsafe(method(isEnabled))]
                fn is_enabled(&self) -> bool {
                    true
                }
            }
        );

        define_class!(
            #[unsafe(super(NSObject))]
            #[name = "SNTHConnectTestExtensionStateOff"]
            struct OffState;

            impl OffState {
                #[unsafe(method(isEnabled))]
                fn is_enabled(&self) -> bool {
                    false
                }
            }
        );

        define_class!(
            #[unsafe(super(NSObject))]
            #[name = "SNTHConnectTestExtensionStateThrows"]
            struct ThrowingState;

            impl ThrowingState {
                #[unsafe(method(isEnabled))]
                fn is_enabled(&self) -> bool {
                    raise("NSGenericException")
                }
            }
        );

        /// Raise an Objective-C exception, the way a system framework does.
        fn raise(name: &str) -> ! {
            let class = AnyClass::get(c"NSException").unwrap();
            let name = ns_string(name).unwrap();
            let reason = ns_string("thrown by the test").unwrap();
            let nothing = std::ptr::null_mut::<AnyObject>();
            let exception: *mut AnyObject = unsafe { msg_send![class, exceptionWithName: name, reason: reason, userInfo: nothing] };
            let _: () = unsafe { msg_send![exception, raise] };
            unreachable!("raise returned")
        }

        fn text(string: &AnyObject) -> String {
            let chars: *const std::ffi::c_char = unsafe { msg_send![string, UTF8String] };
            unsafe { CStr::from_ptr(chars) }.to_string_lossy().into_owned()
        }

        static SHOWN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

        define_class!(
            #[unsafe(super(NSObject))]
            #[name = "SNTHConnectTestSafari"]
            struct FakeSafari;

            impl FakeSafari {
                // The identifier chooses the answer, so one class plays
                // every case.
                #[unsafe(method(getStateOfSafariExtensionWithIdentifier:completionHandler:))]
                fn state(identifier: &AnyObject, handler: &DynBlock<dyn Fn(*mut AnyObject, *mut AnyObject)>) {
                    let nothing = std::ptr::null_mut::<AnyObject>();
                    match text(identifier).as_str() {
                        "ai.snth.test.on" => {
                            let state: Retained<OnState> = unsafe { msg_send![OnState::class(), new] };
                            handler.call((Retained::as_ptr(&state) as *mut AnyObject, nothing));
                        }
                        "ai.snth.test.off" => {
                            let state: Retained<OffState> = unsafe { msg_send![OffState::class(), new] };
                            handler.call((Retained::as_ptr(&state) as *mut AnyObject, nothing));
                        }
                        // Safari throws instead of answering (an Objective-C
                        // exception out of the call itself).
                        "ai.snth.test.throws" => raise("NSInternalInconsistencyException"),
                        // An answer that is not a state object at all.
                        "ai.snth.test.not-a-state" => {
                            let odd: Retained<NSObject> = unsafe { msg_send![NSObject::class(), new] };
                            handler.call((Retained::as_ptr(&odd) as *mut AnyObject, nothing));
                        }
                        // A state whose isEnabled throws, answered on the
                        // caller's own thread.
                        "ai.snth.test.state-throws" => {
                            let state: Retained<ThrowingState> = unsafe { msg_send![ThrowingState::class(), new] };
                            handler.call((Retained::as_ptr(&state) as *mut AnyObject, nothing));
                        }
                        // Safari does not know the extension: no state, an error.
                        "ai.snth.test.unknown" => {
                            let error: Retained<NSObject> = unsafe { msg_send![NSObject::class(), new] };
                            handler.call((nothing, Retained::as_ptr(&error) as *mut AnyObject));
                        }
                        // The answer comes later, from another thread, as Safari's does.
                        "ai.snth.test.later" => {
                            let handler = handler.copy();
                            struct Send(block2::RcBlock<dyn Fn(*mut AnyObject, *mut AnyObject)>);
                            unsafe impl std::marker::Send for Send {}
                            let handler = Send(handler);
                            std::thread::spawn(move || {
                                let handler = handler;
                                std::thread::sleep(Duration::from_millis(50));
                                let state: Retained<OnState> = unsafe { msg_send![OnState::class(), new] };
                                handler.0.call((Retained::as_ptr(&state) as *mut AnyObject, std::ptr::null_mut()));
                            });
                        }
                        // No answer at all.
                        _ => {}
                    }
                }

                #[unsafe(method(showPreferencesForExtensionWithIdentifier:completionHandler:))]
                fn show(identifier: &AnyObject, handler: &DynBlock<dyn Fn(*mut AnyObject)>) {
                    SHOWN.lock().unwrap().push(text(identifier));
                    handler.call((std::ptr::null_mut(),));
                }
            }
        );

        #[test]
        fn the_question_and_its_answer_cross_into_objective_c_and_back() {
            let safari = FakeSafari::class();
            let wait = Duration::from_secs(2);
            assert_eq!(enabled_from(safari, "ai.snth.test.on", wait), Enabled::On);
            assert_eq!(enabled_from(safari, "ai.snth.test.off", wait), Enabled::Off);
            assert_eq!(enabled_from(safari, "ai.snth.test.unknown", wait), Enabled::Unknown);
            assert_eq!(enabled_from(safari, "ai.snth.test.later", wait), Enabled::On);
            // Safari says nothing: the page is not held up for long.
            let started = std::time::Instant::now();
            assert_eq!(enabled_from(safari, "ai.snth.test.silent", Duration::from_millis(100)), Enabled::Unknown);
            assert!(started.elapsed() < Duration::from_secs(2));
            // A name that cannot be a C string is never sent.
            assert_eq!(enabled_from(safari, "bad\0name", wait), Enabled::Unknown);

            assert!(show_settings_with(safari, "ai.snth.connect.safari"));
            assert_eq!(*SHOWN.lock().unwrap(), ["ai.snth.connect.safari"]);
        }

        /// Set in the child process of the test below.
        const CHILD: &str = "SNTH_CONNECT_SAFARI_THROW_CHILD";

        /// The crash of 0.1.7 (opening Logins aborted the app): Safari's
        /// call raised an Objective-C exception on the tokio thread the
        /// question runs on. Rust cannot catch a foreign exception, so the
        /// task's own `catch_unwind` aborted the whole process ("Rust
        /// cannot catch foreign exceptions", SIGABRT on a tokio-rt-worker).
        ///
        /// The question is asked here exactly as the Logins page asks it
        /// (`spawn_blocking` on a tokio runtime), in a child process, so a
        /// regression shows up as this test failing with the child's
        /// signal instead of taking the whole test run down.
        #[test]
        fn safari_throwing_does_not_take_the_app_down() {
            let cases = [
                // The call itself throws.
                "ai.snth.test.throws",
                // The answer is not a state, and a state whose isEnabled
                // throws (both answered on the asking thread).
                "ai.snth.test.not-a-state",
                "ai.snth.test.state-throws",
            ];
            if std::env::var_os(CHILD).is_some() {
                let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
                for case in cases {
                    let got = rt
                        .block_on(rt.spawn_blocking(move || {
                            enabled_from(FakeSafari::class(), case, Duration::from_millis(200))
                        }))
                        .unwrap();
                    assert_eq!(got, Enabled::Unknown, "{case}");
                    println!("survived {case}");
                }
                // A class that has no such method at all (a macOS where
                // Safari's API moved): nothing is sent, nothing is thrown.
                let plain = rt
                    .block_on(rt.spawn_blocking(|| {
                        enabled_from(NSObject::class(), "ai.snth.test.on", Duration::from_millis(200))
                    }))
                    .unwrap();
                assert_eq!(plain, Enabled::Unknown);
                assert!(!show_settings_with(NSObject::class(), "ai.snth.test.on"));
                println!("survived a missing method");
                return;
            }
            let out = connect_core::proc::std_command(std::env::current_exe().unwrap())
                .args(["--exact", "safari::tests::calls::safari_throwing_does_not_take_the_app_down", "--nocapture", "--test-threads=1"])
                .env(CHILD, "1")
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(out.status.success(), "the child died: {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}", out.status);
            for case in cases {
                assert!(stdout.contains(&format!("survived {case}")), "{case} was not reached:\n{stdout}");
            }
            assert!(stdout.contains("survived a missing method"), "{stdout}");
        }
    }
}
