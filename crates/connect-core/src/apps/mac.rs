//! The Mac's own apps.
//!
//! How each one is reached, and why:
//!
//! - Contacts, Calendar, Reminders, Notes, sending in Messages:
//!   AppleScript run by `/usr/bin/osascript`. The app itself reads and
//!   writes its data, so the only macOS permission involved is Automation
//!   ("SNTH Connect wants to control Calendar"), asked per app by the
//!   system on first use. The system frameworks (EventKit, Contacts)
//!   would need an Objective-C bridge, four more usage strings and their
//!   own prompts, and could not be exercised without a real signed app;
//!   the scripts are the ones the old companion ran for months.
//!   Known limit of Calendar's scripting: a repeating event is found by
//!   the date of its FIRST occurrence only.
//! - Reading Messages: the history database
//!   (`~/Library/Messages/chat.db`), read-only. Messages has no scripting
//!   for reading. This needs Full Disk Access for the app; without it the
//!   call fails with a label that says so and the Apps screen shows the
//!   step. Sending does not need it.
//! - Shortcuts: `/usr/bin/shortcuts`. Clipboard: `pbpaste` / `pbcopy`.
//!   Opening: `/usr/bin/open`. Notifications: the host's `Notifier`.
//! - Opening an app also checks that a window came up (`WindowCheck`):
//!   `open` activates a running app and asks it to reopen, and some apps
//!   answer with no window at all (TextEdit with its last document
//!   closed). Then one explicit reopen goes to it (only when SNTH Connect
//!   may already send that app Apple events: no prompt), and the answer
//!   says `"window": false, "status": "no_window"` when there is still
//!   none, instead of a plain "opened". Finder's desktop (the icons on
//!   the wallpaper) is never counted as a Finder window, and a Finder
//!   with no window gets one by opening the owner's Desktop folder
//!   (`open ~/Desktop`: no Apple event, so no Automation prompt) before
//!   the reopen.
//!
//! Every script is a constant. What the synth sent travels as ARGUMENTS
//! of the script (`on run argv`), never as script text, so there is
//! nothing to escape and nothing to inject. A script answers in records
//! separated by control characters (0x1e between records, 0x1f between
//! fields), which no title or name contains.
//!
//! An error of a script is reported by its number only: its text can
//! quote what the app holds.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

use super::request::{self, MsgService, Request};
use super::time::{self, Local, When};
use super::{AppError, Backend, Call, CallFuture, Notifier, Offer, Permission, PermissionKind, PermissionState, APPS};

const FS: char = '\u{1f}';
const RS: char = '\u{1e}';
const GS: char = '\u{1d}';

/// The most records one script may print. The gate's own limits are
/// lower; this only keeps a script from walking a whole library.
const SCRIPT_HARD_CAP: usize = 500;

/// Handlers shared by every script. Plain AppleScript, no app involved.
const PRELUDE: &str = r#"
on mkDate(y, m, d, hh, mm, ss)
	set dt to current date
	set day of dt to 1
	set year of dt to y
	set month of dt to m
	set day of dt to d
	set time of dt to (hh * 3600 + mm * 60 + ss)
	return dt
end mkDate

on argDate(argv, i)
	return my mkDate((item i of argv) as integer, (item (i + 1) of argv) as integer, (item (i + 2) of argv) as integer, (item (i + 3) of argv) as integer, (item (i + 4) of argv) as integer, (item (i + 5) of argv) as integer)
end argDate

on pad2(v)
	return text -2 thru -1 of ("0" & (v as string))
end pad2

on iso(d)
	try
		if d is missing value then return ""
		set secs to time of d
		return ((year of d) as string) & "-" & my pad2(month of d as integer) & "-" & my pad2(day of d) & "T" & my pad2(secs div 3600) & ":" & my pad2((secs mod 3600) div 60) & ":" & my pad2(secs mod 60)
	on error
		return ""
	end try
end iso

on txt(v)
	try
		if v is missing value then return ""
		return v as string
	on error
		return ""
	end try
end txt

on joinList(xs, sep)
	try
		if (count of xs) = 0 then return ""
		set saved to AppleScript's text item delimiters
		set AppleScript's text item delimiters to sep
		set joined to xs as string
		set AppleScript's text item delimiters to saved
		return joined
	on error
		return ""
	end try
end joinList
"#;

const CONTACTS_SEARCH: &str = r#"
on run argv
	set theQuery to item 1 of argv
	set maxN to (item 2 of argv) as integer
	set FS to character id 31
	set RS to character id 30
	set GS to character id 29
	set acc to ""
	set n to 0
	tell application "Contacts"
		set hitList to (every person whose name contains theQuery or organization contains theQuery or first name contains theQuery or last name contains theQuery)
		if theQuery contains "@" then
			try
				set hitList to hitList & (every person whose value of emails contains theQuery)
			end try
		end if
		repeat with p in hitList
			if n >= maxN then exit repeat
			set ph to ""
			try
				set ph to my joinList(value of every phone of p, GS)
			end try
			set em to ""
			try
				set em to my joinList(value of every email of p, GS)
			end try
			set acc to acc & my txt(name of p) & FS & my txt(organization of p) & FS & ph & FS & em & RS
			set n to n + 1
		end repeat
	end tell
	return acc
end run
"#;

/// Names and phone numbers only, for a search by number: the filter runs
/// in this app, and only the matches leave it.
const CONTACTS_PHONES: &str = r#"
on run argv
	set FS to character id 31
	set RS to character id 30
	set GS to character id 29
	set acc to ""
	tell application "Contacts"
		set nameList to name of every person
		set phoneLists to value of phones of every person
	end tell
	repeat with i from 1 to (count of nameList)
		set phs to item i of phoneLists
		if (count of phs) > 0 then
			set acc to acc & my txt(item i of nameList) & FS & my joinList(phs, GS) & RS
		end if
	end repeat
	return acc
end run
"#;

const CALENDAR_LIST: &str = r#"
on run argv
	set startDate to my argDate(argv, 1)
	set endDate to my argDate(argv, 7)
	set calName to item 13 of argv
	set maxN to (item 14 of argv) as integer
	set FS to character id 31
	set RS to character id 30
	set acc to ""
	set n to 0
	tell application "Calendar"
		if calName is "" then
			set calList to every calendar
		else
			set calList to (every calendar whose name is calName)
			if (count of calList) = 0 then error "no such calendar" number -1728
		end if
		repeat with cal in calList
			if n >= maxN then exit repeat
			set cn to name of cal
			set evs to (every event of cal whose start date >= startDate and start date < endDate)
			repeat with ev in evs
				if n >= maxN then exit repeat
				set acc to acc & (uid of ev) & FS & my txt(summary of ev) & FS & my iso(start date of ev) & FS & my iso(end date of ev) & FS & cn & FS & my txt(location of ev) & FS & my txt(description of ev) & FS & ((allday event of ev) as string) & RS
				set n to n + 1
			end repeat
		end repeat
	end tell
	return acc
end run
"#;

const CALENDAR_CREATE: &str = r#"
on run argv
	set startDate to my argDate(argv, 1)
	set endDate to my argDate(argv, 7)
	set calName to item 13 of argv
	set theTitle to item 14 of argv
	set theLocation to item 15 of argv
	set theNotes to item 16 of argv
	set isAllDay to ((item 17 of argv) is "true")
	set FS to character id 31
	tell application "Calendar"
		if calName is "" then
			set cal to first calendar whose writable is true
		else
			set cal to first calendar whose name is calName
		end if
		tell cal
			set newEv to make new event with properties {summary:theTitle, start date:startDate, end date:endDate, allday event:isAllDay}
		end tell
		if theLocation is not "" then set location of newEv to theLocation
		if theNotes is not "" then set description of newEv to theNotes
		return (uid of newEv) & FS & (summary of newEv) & FS & (name of cal)
	end tell
end run
"#;

const REMINDERS_LIST: &str = r#"
on run argv
	set listName to item 1 of argv
	set inclDone to ((item 2 of argv) is "true")
	set maxN to (item 3 of argv) as integer
	set FS to character id 31
	set RS to character id 30
	set acc to ""
	set n to 0
	tell application "Reminders"
		if listName is "" then
			set theLists to every list
		else
			set theLists to (every list whose name is listName)
			if (count of theLists) = 0 then error "no such list" number -1728
		end if
		repeat with lst in theLists
			if n >= maxN then exit repeat
			set ln to name of lst
			if inclDone then
				set rms to every reminder of lst
			else
				set rms to (every reminder of lst whose completed is false)
			end if
			repeat with r in rms
				if n >= maxN then exit repeat
				set dueStr to ""
				try
					set dueStr to my iso(due date of r)
				end try
				set acc to acc & (id of r) & FS & my txt(name of r) & FS & ln & FS & dueStr & FS & ((completed of r) as string) & FS & my txt(body of r) & RS
				set n to n + 1
			end repeat
		end repeat
	end tell
	return acc
end run
"#;

const REMINDERS_CREATE: &str = r#"
on run argv
	set theTitle to item 1 of argv
	set listName to item 2 of argv
	set theNotes to item 3 of argv
	set hasDue to ((item 4 of argv) is "true")
	if hasDue then set dueDate to my argDate(argv, 5)
	set FS to character id 31
	tell application "Reminders"
		if listName is "" then
			set r to make new reminder with properties {name:theTitle}
		else
			tell (first list whose name is listName)
				set r to make new reminder with properties {name:theTitle}
			end tell
		end if
		if theNotes is not "" then set body of r to theNotes
		if hasDue then set due date of r to dueDate
		set ln to ""
		try
			set ln to name of (container of r)
		end try
		return (id of r) & FS & (name of r) & FS & ln
	end tell
end run
"#;

const REMINDERS_COMPLETE: &str = r#"
on run argv
	set theId to item 1 of argv
	set FS to character id 31
	tell application "Reminders"
		set r to reminder id theId
		set completed of r to true
		return (id of r) & FS & (name of r)
	end tell
end run
"#;

/// Titles first, then the text of notes, without repeats. Locked notes
/// are skipped.
const NOTES_SEARCH: &str = r#"
on run argv
	set theQuery to item 1 of argv
	set folderName to item 2 of argv
	set maxN to (item 3 of argv) as integer
	set FS to character id 31
	set RS to character id 30
	set acc to ""
	set n to 0
	set seen to {}
	tell application "Notes"
		if folderName is not "" then set theFolder to first folder whose name is folderName
		repeat with stepNo from 1 to 2
			set hitList to {}
			if n < maxN then
				try
					if stepNo is 1 then
						if folderName is "" then
							set hitList to (every note whose name contains theQuery)
						else
							set hitList to (every note of theFolder whose name contains theQuery)
						end if
					else
						if folderName is "" then
							set hitList to (every note whose plaintext contains theQuery)
						else
							set hitList to (every note of theFolder whose plaintext contains theQuery)
						end if
					end if
				end try
			end if
			repeat with nt in hitList
				if n >= maxN then exit repeat
				try
					set noteId to (id of nt) as string
					if seen does not contain noteId then
						set end of seen to noteId
						if not (password protected of nt) then
							set fn to ""
							try
								set fn to name of (container of nt)
							end try
							set snip to ""
							try
								set snip to plaintext of nt
								if (count of snip) > 240 then set snip to text 1 thru 240 of snip
							end try
							set acc to acc & noteId & FS & my txt(name of nt) & FS & fn & FS & my iso(modification date of nt) & FS & snip & RS
							set n to n + 1
						end if
					end if
				end try
			end repeat
		end repeat
	end tell
	return acc
end run
"#;

const NOTES_READ: &str = r#"
on run argv
	set theId to item 1 of argv
	set theTitle to item 2 of argv
	set FS to character id 31
	tell application "Notes"
		if theId is not "" then
			set nt to note id theId
		else
			set nt to first note whose name is theTitle
		end if
		if password protected of nt then error "locked" number 9001
		set fn to ""
		try
			set fn to name of (container of nt)
		end try
		return ((id of nt) as string) & FS & my txt(name of nt) & FS & fn & FS & my iso(modification date of nt) & FS & (plaintext of nt)
	end tell
end run
"#;

const NOTES_CREATE: &str = r#"
on run argv
	set theTitle to item 1 of argv
	set theBody to item 2 of argv
	set folderName to item 3 of argv
	set FS to character id 31
	tell application "Notes"
		if folderName is "" then
			set nt to make new note with properties {name:theTitle, body:theBody}
		else
			set nt to make new note at (first folder whose name is folderName) with properties {name:theTitle, body:theBody}
		end if
		return ((id of nt) as string) & FS & my txt(name of nt)
	end tell
end run
"#;

const MESSAGES_SEND: &str = r#"
on run argv
	set theHandle to item 1 of argv
	set theText to item 2 of argv
	set svcName to item 3 of argv
	tell application "Messages"
		if svcName is "sms" then
			set acct to first account whose service type is SMS
		else
			set acct to first account whose service type is iMessage
		end if
		send theText to participant theHandle of acct
	end tell
	return "ok"
end run
"#;

/// The first harmless question to each app: it makes macOS show its
/// "wants to control" window (the app is opened for it). Counts only.
fn probe_script(app: &str) -> Option<&'static str> {
    Some(match app {
        "messages" => "tell application \"Messages\" to count accounts\n",
        "contacts" => "tell application \"Contacts\" to count groups\n",
        "calendar" => "tell application \"Calendar\" to count calendars\n",
        "reminders" => "tell application \"Reminders\" to count lists\n",
        "notes" => "tell application \"Notes\" to count folders\n",
        _ => return None,
    })
}

/// The bundle id of the app an Automation permission is about.
fn bundle_id(app: &str) -> Option<&'static str> {
    Some(match app {
        "messages" => "com.apple.MobileSMS",
        "contacts" => "com.apple.AddressBook",
        "calendar" => "com.apple.iCal",
        "reminders" => "com.apple.reminders",
        "notes" => "com.apple.Notes",
        _ => return None,
    })
}

fn display_name(app: &str) -> &'static str {
    super::app_title(app)
}

/// Asks a running app to reopen: kAEReopenApplication, what a click on
/// its Dock icon sends (it shows a window, or makes one). A standard
/// command: no terminology of the app is needed.
const APP_REOPEN: &str = r#"
on run argv
	tell application id (item 1 of argv) to reopen
end run
"#;

/// Counts an app's windows by its name (WindowCheck::count).
pub type WindowCount = Arc<dyn Fn(&str) -> Option<usize> + Send + Sync>;

/// Whether opening an app gave it a window, and the one reopen when it
/// did not. The parts are fields so a test can stand in for them;
/// `WindowCheck::system` uses the real ones.
#[derive(Clone)]
pub struct WindowCheck {
    /// The app's windows a person can work in (on screen, the normal
    /// window layer, at least 50 points a side), by the app's name as the
    /// Window Server names its owner. None: the app cannot be told among
    /// the windows (no window of that owner name at all, on any layer).
    pub count: WindowCount,
    /// Whether this app may already send Apple events to the app with
    /// this bundle id (asked without a prompt).
    pub may_script: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    /// How long a window is waited for after `open` and after the reopen.
    pub wait: Duration,
    /// The folder a Finder with no window is shown in (the owner's
    /// Desktop). None: Finder gets only the reopen, like any app.
    pub finder_folder: Option<PathBuf>,
}

impl WindowCheck {
    pub fn system() -> WindowCheck {
        WindowCheck {
            count: Arc::new(app_windows),
            may_script: Arc::new(|bundle: &str| automation_state(bundle) == PermissionState::Granted),
            wait: Duration::from_millis(1500),
            finder_folder: directories::BaseDirs::new().map(|d| d.home_dir().join("Desktop")),
        }
    }
}

/// Finder by the name or the bundle id an open names.
fn is_finder(name: &str, bundle_id: Option<&str>) -> bool {
    bundle_id.is_some_and(|id| id.eq_ignore_ascii_case("com.apple.finder")) || name.trim().eq_ignore_ascii_case("finder")
}

/// A rectangle in global display points (the Window Server's bounds).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// One window of the Window Server's list, as the window check reads it
/// (no title: that would need Screen Recording).
#[derive(Clone, Debug)]
pub struct ListedWindow {
    pub owner: String,
    pub layer: i32,
    pub on_screen: bool,
    pub bounds: Frame,
}

/// The windows of `name` a person can work in: owner name equal to
/// `name` (case does not matter), the normal window layer, on screen, at
/// least 50 points a side, and for Finder never its desktop (a window of
/// the size of a whole display: the icons on the wallpaper, which would
/// otherwise pass for "Finder shows a window" on every desktop). None
/// when no window of any layer or state has that owner name.
pub fn workable_windows(name: &str, windows: &[ListedWindow], displays: &[Frame]) -> Option<usize> {
    let want = name.trim().to_lowercase();
    if want.is_empty() {
        return None;
    }
    let finder = want == "finder";
    let whole_display = |b: &Frame| {
        displays.iter().any(|d| (b.x - d.x).abs() < 1.0 && (b.y - d.y).abs() < 1.0 && (b.w - d.w).abs() < 1.0 && (b.h - d.h).abs() < 1.0)
    };
    let mut known = false;
    let mut count = 0usize;
    for w in windows {
        if w.owner.to_lowercase() != want {
            continue;
        }
        known = true;
        if w.layer != 0 || !w.on_screen || w.bounds.w < 50.0 || w.bounds.h < 50.0 {
            continue;
        }
        if finder && whole_display(&w.bounds) {
            continue;
        }
        count += 1;
    }
    known.then_some(count)
}

/// The Mac backend. The program paths are fields so a test can point
/// them at stand-ins; `system` uses the real ones.
pub struct MacBackend {
    pub osascript: PathBuf,
    pub shortcuts: PathBuf,
    pub pbcopy: PathBuf,
    pub pbpaste: PathBuf,
    pub open: PathBuf,
    /// The Messages history database.
    pub chat_db: PathBuf,
    pub notifier: Arc<dyn Notifier>,
    pub windows: WindowCheck,
}

impl MacBackend {
    pub fn system(notifier: Arc<dyn Notifier>) -> MacBackend {
        let home = directories::BaseDirs::new()
            .map(|d| d.home_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("/var/empty"));
        MacBackend {
            osascript: "/usr/bin/osascript".into(),
            shortcuts: "/usr/bin/shortcuts".into(),
            pbcopy: "/usr/bin/pbcopy".into(),
            pbpaste: "/usr/bin/pbpaste".into(),
            open: "/usr/bin/open".into(),
            chat_db: home.join("Library/Messages/chat.db"),
            notifier,
            windows: WindowCheck::system(),
        }
    }

    fn runner(&self) -> Runner {
        Runner {
            osascript: self.osascript.clone(),
            shortcuts: self.shortcuts.clone(),
            pbcopy: self.pbcopy.clone(),
            pbpaste: self.pbpaste.clone(),
            open: self.open.clone(),
            chat_db: self.chat_db.clone(),
            notifier: self.notifier.clone(),
            windows: self.windows.clone(),
        }
    }
}

impl Backend for MacBackend {
    fn offered(&self) -> Vec<Offer> {
        APPS.iter().map(|a| Offer::all(a)).collect()
    }

    fn call(&self, call: Call) -> CallFuture {
        let r = self.runner();
        Box::pin(async move { r.run(call).await })
    }

    fn permissions(&self, app: &str) -> Vec<Permission> {
        let mut out = Vec::new();
        if let Some(bundle) = bundle_id(app) {
            out.push(Permission { kind: PermissionKind::Automation, state: automation_state(bundle) });
        }
        if app == "messages" {
            out.push(Permission { kind: PermissionKind::FullDisk, state: full_disk_state(&self.chat_db) });
        }
        out
    }

    fn request_permission(&self, app: &str, kind: PermissionKind) -> Result<(), String> {
        match kind {
            // Nothing can ask for Full Disk Access: the owner adds the app
            // in System Settings (the host opens that page).
            PermissionKind::FullDisk => Ok(()),
            PermissionKind::Automation => {
                let script = probe_script(app).ok_or("this app needs no permission")?;
                let mut child = crate::proc::std_command(&self.osascript)
                    .arg("-")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| format!("cannot run osascript: {e}"))?;
                if let Some(mut stdin) = child.stdin.take() {
                    use std::io::Write;
                    let _ = stdin.write_all(script.as_bytes());
                }
                // The system window stays up until the owner answers.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                Ok(())
            }
        }
    }
}

/// Is this app allowed to automate the app with this bundle id? Asks the
/// system WITHOUT prompting.
fn automation_state(bundle: &str) -> PermissionState {
    use std::ffi::c_void;

    // AEDataModel.h declares AEDesc under `#pragma pack(push, 2)`.
    #[repr(C, packed(2))]
    struct AEDesc {
        descriptor_type: u32,
        data_handle: *mut c_void,
    }

    #[link(name = "CoreServices", kind = "framework")]
    extern "C" {
        fn AECreateDesc(type_code: u32, data: *const c_void, size: isize, result: *mut AEDesc) -> i16;
        fn AEDisposeDesc(desc: *mut AEDesc) -> i16;
        fn AEDeterminePermissionToAutomateTarget(target: *const AEDesc, event_class: u32, event_id: u32, ask_user: u8) -> i32;
    }

    const TYPE_APPLICATION_BUNDLE_ID: u32 = u32::from_be_bytes(*b"bund");
    const TYPE_WILDCARD: u32 = u32::from_be_bytes(*b"****");

    let mut desc = AEDesc { descriptor_type: 0, data_handle: std::ptr::null_mut() };
    let status = unsafe {
        if AECreateDesc(TYPE_APPLICATION_BUNDLE_ID, bundle.as_ptr() as *const c_void, bundle.len() as isize, &mut desc) != 0 {
            return PermissionState::Unknown;
        }
        let status = AEDeterminePermissionToAutomateTarget(&desc, TYPE_WILDCARD, TYPE_WILDCARD, 0);
        AEDisposeDesc(&mut desc);
        status
    };
    match status {
        0 => PermissionState::Granted,
        -1743 => PermissionState::Denied,
        -1744 => PermissionState::NotAsked,
        // -600: the target is not running, so the system cannot say.
        _ => PermissionState::Unknown,
    }
}

/// The windows of an app a person can work in, from the Window Server's
/// list (CGWindowListCopyWindowInfo), judged by `workable_windows`.
/// Reading owner names, layers and bounds needs no permission (window
/// titles would need Screen Recording; none is read). The displays'
/// frames (CGGetActiveDisplayList, CGDisplayBounds) tell Finder's desktop
/// apart.
fn app_windows(name: &str) -> Option<usize> {
    use std::ffi::{c_char, c_void, CStr};

    #[repr(C)]
    #[derive(Default)]
    struct CGRect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        static kCGWindowOwnerName: *const c_void;
        static kCGWindowLayer: *const c_void;
        static kCGWindowIsOnscreen: *const c_void;
        static kCGWindowBounds: *const c_void;
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> *const c_void;
        fn CGRectMakeWithDictionaryRepresentation(dict: *const c_void, rect: *mut CGRect) -> bool;
        fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
        fn CGDisplayBounds(display: u32) -> CGRect;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFArrayGetCount(array: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(array: *const c_void, i: isize) -> *const c_void;
        fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
        fn CFStringGetCString(s: *const c_void, buf: *mut c_char, len: isize, encoding: u32) -> u8;
        fn CFNumberGetValue(n: *const c_void, kind: isize, out: *mut c_void) -> u8;
        fn CFBooleanGetValue(b: *const c_void) -> u8;
        fn CFRelease(cf: *const c_void);
    }
    // kCGWindowListOptionAll | kCGWindowListExcludeDesktopElements: every
    // window, without the desktop and its icons (Finder's desktop is not
    // a window to work in; workable_windows also drops a Finder window of
    // a whole display's size, in case a macOS version lists it on the
    // normal layer).
    const OPTIONS: u32 = 16;
    const UTF8: u32 = 0x0800_0100;
    const SINT32: isize = 3;

    if name.trim().is_empty() {
        return None;
    }
    let mut windows = Vec::new();
    let mut displays = Vec::new();
    unsafe {
        let mut ids = [0u32; 16];
        let mut n = 0u32;
        if CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut n) == 0 {
            for id in ids.iter().take(n as usize) {
                let r = CGDisplayBounds(*id);
                displays.push(Frame { x: r.x, y: r.y, w: r.w, h: r.h });
            }
        }
        let list = CGWindowListCopyWindowInfo(OPTIONS, 0);
        if list.is_null() {
            return None;
        }
        for i in 0..CFArrayGetCount(list) {
            let w = CFArrayGetValueAtIndex(list, i);
            let owner = CFDictionaryGetValue(w, kCGWindowOwnerName);
            if owner.is_null() {
                continue;
            }
            let mut buf = [0 as c_char; 256];
            if CFStringGetCString(owner, buf.as_mut_ptr(), buf.len() as isize, UTF8) == 0 {
                continue;
            }
            let owner = CStr::from_ptr(buf.as_ptr()).to_string_lossy().to_string();
            let mut layer: i32 = -1;
            let l = CFDictionaryGetValue(w, kCGWindowLayer);
            if l.is_null() || CFNumberGetValue(l, SINT32, &mut layer as *mut i32 as *mut c_void) == 0 {
                layer = -1;
            }
            let on = CFDictionaryGetValue(w, kCGWindowIsOnscreen);
            let on_screen = !on.is_null() && CFBooleanGetValue(on) != 0;
            let mut r = CGRect::default();
            let b = CFDictionaryGetValue(w, kCGWindowBounds);
            if b.is_null() || !CGRectMakeWithDictionaryRepresentation(b, &mut r) {
                r = CGRect::default();
            }
            windows.push(ListedWindow { owner, layer, on_screen, bounds: Frame { x: r.x, y: r.y, w: r.w, h: r.h } });
        }
        CFRelease(list);
    }
    workable_windows(name, &windows, &displays)
}

/// Full Disk Access shows as being able to open the Messages database.
/// Opening it reads nothing and never prompts.
fn full_disk_state(chat_db: &Path) -> PermissionState {
    match std::fs::File::open(chat_db) {
        Ok(_) => PermissionState::Granted,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => PermissionState::Denied,
        Err(_) => PermissionState::Unknown,
    }
}

struct Runner {
    osascript: PathBuf,
    shortcuts: PathBuf,
    pbcopy: PathBuf,
    pbpaste: PathBuf,
    open: PathBuf,
    chat_db: PathBuf,
    notifier: Arc<dyn Notifier>,
    windows: WindowCheck,
}

/// The number at the end of an osascript error line: "... (-1728)".
fn script_error_number(stderr: &str) -> Option<i64> {
    let line = stderr.lines().rev().find(|l| !l.trim().is_empty())?;
    let open = line.rfind('(')?;
    let close = line[open..].find(')')? + open;
    line[open + 1..close].trim().parse().ok()
}

/// Turn a failed script into a label. `missing` is what a "no such
/// object" error means for this call.
fn script_error(app: &str, stderr: &str, missing: &str) -> AppError {
    let name = display_name(app);
    match script_error_number(stderr) {
        Some(-1743) | Some(-1744) => AppError::Unavailable(format!(
            "SNTH Connect is not allowed to control {name} on this Mac; the owner can allow it in the app (Apps) or in System Settings, Privacy & Security, Automation"
        )),
        Some(-600) | Some(-609) | Some(-10810) | Some(-1712) => {
            AppError::Unavailable(format!("{name} is not running or did not answer"))
        }
        Some(-1728) | Some(-1719) => AppError::NotFound(missing.to_string()),
        Some(9001) => AppError::Denied("that note is locked with a password".into()),
        Some(n) if stderr.contains("syntax error") => AppError::Failed(format!(
            "the {name} script does not work on this version of macOS (error {n})"
        )),
        Some(n) => AppError::Failed(format!("{name} reported error {n}")),
        None => AppError::Failed(format!("{name} could not do it")),
    }
}

fn records(out: &str) -> Vec<Vec<String>> {
    out.trim_end_matches('\n')
        .split(RS)
        .filter(|r| !r.trim().is_empty())
        .map(|r| r.split(FS).map(str::to_string).collect())
        .collect()
}

fn date_args(l: &Local) -> [String; 6] {
    [l.year.to_string(), l.month.to_string(), l.day.to_string(), l.hour.to_string(), l.minute.to_string(), l.second.to_string()]
}

/// A local time a script printed -> RFC 3339 with this Mac's offset.
fn script_time(text: &str) -> String {
    time::parse_local_parts(text).map(|l| time::rfc3339_local(&l)).unwrap_or_default()
}

fn digits(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

/// Plain text -> the simple HTML Notes stores.
fn note_html(text: &str) -> String {
    let mut out = String::new();
    for line in text.split('\n') {
        if line.is_empty() {
            out.push_str("<div><br></div>");
        } else {
            out.push_str("<div>");
            out.push_str(&line.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;"));
            out.push_str("</div>");
        }
    }
    out
}

impl Runner {
    /// Whether the app shows a window, waiting up to `windows.wait` for
    /// one: Some(true) as soon as one is there, Some(false) when the app
    /// is known among the windows and shows none, None when it cannot be
    /// told.
    async fn window_after_open(&self, name: &str) -> Option<bool> {
        let deadline = Instant::now() + self.windows.wait;
        let mut seen = None;
        loop {
            match (self.windows.count)(name) {
                Some(n) if n > 0 => return Some(true),
                Some(_) => seen = Some(false),
                None => {}
            }
            if Instant::now() >= deadline {
                return seen;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Run one constant script with these arguments; stdout on success.
    async fn osa(&self, app: &str, script: &str, args: &[String], missing: &str) -> Result<String, AppError> {
        let mut child = crate::proc::command(&self.osascript)
            .arg("-")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| AppError::Unavailable(format!("cannot run osascript: {e}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            let source = format!("{PRELUDE}\n{script}");
            let _ = stdin.write_all(source.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }
        let out = child
            .wait_with_output()
            .await
            .map_err(|e| AppError::Failed(format!("osascript did not finish: {e}")))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            // The number only: the text can quote what the app holds.
            crate::logln!("apps: {app} script failed (error {:?})", script_error_number(&stderr));
            return Err(script_error(app, &stderr, missing));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string())
    }

    async fn run(&self, call: Call) -> Result<Value, AppError> {
        match call.request {
            Request::MessagesRecent { chat, limit } => {
                let db = self.chat_db.clone();
                tokio::task::spawn_blocking(move || read_messages(&db, chat.as_deref(), limit))
                    .await
                    .map_err(|e| AppError::Failed(format!("reading messages stopped: {e}")))?
            }
            Request::MessagesSend { to, text, service } => {
                let svc = match service {
                    MsgService::IMessage => "imessage",
                    MsgService::Sms => "sms",
                };
                self.osa(
                    "messages",
                    MESSAGES_SEND,
                    &[to.clone(), text, svc.to_string()],
                    "Messages has no account for that service, or cannot reach that recipient",
                )
                .await?;
                Ok(json!({"sent": true, "to": to, "service": svc}))
            }
            Request::ContactsSearch { query, limit } => self.contacts(&query, limit).await,
            Request::CalendarList { start, end, calendar, query, limit } => {
                let mut args: Vec<String> = Vec::new();
                args.extend(date_args(&start.local()));
                args.extend(date_args(&end.local()));
                args.push(calendar.clone().unwrap_or_default());
                args.push(SCRIPT_HARD_CAP.to_string());
                let missing = format!("no calendar named {:?}", calendar.unwrap_or_default());
                let out = self.osa("calendar", CALENDAR_LIST, &args, &missing).await?;
                let q = query.unwrap_or_default().to_lowercase();
                let mut events: Vec<(String, Value)> = Vec::new();
                for f in records(&out) {
                    if f.len() < 8 {
                        continue;
                    }
                    if !q.is_empty() && ![&f[1], &f[5], &f[6]].iter().any(|s| s.to_lowercase().contains(&q)) {
                        continue;
                    }
                    events.push((
                        f[2].clone(),
                        json!({
                            "id": f[0], "title": f[1], "start": script_time(&f[2]), "end": script_time(&f[3]),
                            "all_day": f[7] == "true", "calendar": f[4], "location": f[5], "notes": f[6],
                        }),
                    ));
                }
                events.sort_by(|a, b| a.0.cmp(&b.0));
                let truncated = events.len() > limit;
                let events: Vec<Value> = events.into_iter().take(limit).map(|(_, v)| v).collect();
                Ok(json!({"events": events, "truncated": truncated}))
            }
            Request::CalendarCreate { title, start, end, calendar, location, notes, all_day } => {
                let mut args: Vec<String> = Vec::new();
                args.extend(date_args(&start.local()));
                args.extend(date_args(&end.local()));
                args.push(calendar.clone().unwrap_or_default());
                args.push(title);
                args.push(location);
                args.push(notes);
                args.push(all_day.to_string());
                let missing = match &calendar {
                    Some(c) => format!("no calendar named {c:?}"),
                    None => "no calendar on this Mac can be written to".to_string(),
                };
                let out = self.osa("calendar", CALENDAR_CREATE, &args, &missing).await?;
                let f: Vec<&str> = out.split(FS).collect();
                if f.len() < 3 {
                    return Err(AppError::Failed("Calendar did not confirm the new event".into()));
                }
                Ok(json!({"id": f[0], "title": f[1], "calendar": f[2]}))
            }
            Request::RemindersList { list, include_completed, limit } => {
                let args = [list.clone().unwrap_or_default(), include_completed.to_string(), (limit + 1).to_string()];
                let missing = format!("no reminder list named {:?}", list.unwrap_or_default());
                let out = self.osa("reminders", REMINDERS_LIST, &args, &missing).await?;
                let mut reminders: Vec<Value> = records(&out)
                    .into_iter()
                    .filter(|f| f.len() >= 6)
                    .map(|f| json!({"id": f[0], "title": f[1], "list": f[2], "due": script_time(&f[3]), "completed": f[4] == "true", "notes": f[5]}))
                    .collect();
                let truncated = reminders.len() > limit;
                reminders.truncate(limit);
                Ok(json!({"reminders": reminders, "truncated": truncated}))
            }
            Request::RemindersCreate { title, due, list, notes } => {
                let mut args = vec![title, list.clone().unwrap_or_default(), notes, due.is_some().to_string()];
                args.extend(date_args(&due.map(|d: When| d.local()).unwrap_or(Local { year: 2001, month: 1, day: 1, hour: 0, minute: 0, second: 0 })));
                let missing = format!("no reminder list named {:?}", list.unwrap_or_default());
                let out = self.osa("reminders", REMINDERS_CREATE, &args, &missing).await?;
                let f: Vec<&str> = out.split(FS).collect();
                if f.len() < 3 {
                    return Err(AppError::Failed("Reminders did not confirm the new reminder".into()));
                }
                Ok(json!({"id": f[0], "title": f[1], "list": f[2]}))
            }
            Request::RemindersComplete { id } => {
                let out = self
                    .osa("reminders", REMINDERS_COMPLETE, &[id.clone()], &format!("no reminder with id {id}"))
                    .await?;
                let f: Vec<&str> = out.split(FS).collect();
                if f.len() < 2 {
                    return Err(AppError::Failed("Reminders did not confirm the change".into()));
                }
                Ok(json!({"id": f[0], "title": f[1], "completed": true}))
            }
            Request::NotesSearch { query, folder, limit } => {
                let args = [query, folder.clone().unwrap_or_default(), (limit + 1).to_string()];
                let missing = format!("no Notes folder named {:?}", folder.unwrap_or_default());
                let out = self.osa("notes", NOTES_SEARCH, &args, &missing).await?;
                let mut notes: Vec<Value> = records(&out)
                    .into_iter()
                    .filter(|f| f.len() >= 5)
                    .map(|f| json!({"id": f[0], "title": f[1], "folder": f[2], "modified": script_time(&f[3]), "snippet": f[4].split_whitespace().collect::<Vec<_>>().join(" ")}))
                    .collect();
                let truncated = notes.len() > limit;
                notes.truncate(limit);
                Ok(json!({"notes": notes, "truncated": truncated}))
            }
            Request::NotesRead { id, title } => {
                let args = [id.unwrap_or_default(), title.unwrap_or_default()];
                let out = self.osa("notes", NOTES_READ, &args, "no such note").await?;
                let f: Vec<&str> = out.splitn(5, FS).collect();
                if f.len() < 5 {
                    return Err(AppError::Failed("Notes did not return the note".into()));
                }
                Ok(json!({"id": f[0], "title": f[1], "folder": f[2], "modified": script_time(f[3]), "text": f[4]}))
            }
            Request::NotesCreate { title, text, folder } => {
                let args = [title.clone(), note_html(&text), folder.clone().unwrap_or_default()];
                let missing = format!("no Notes folder named {:?}", folder.unwrap_or_default());
                let out = self.osa("notes", NOTES_CREATE, &args, &missing).await?;
                let f: Vec<&str> = out.split(FS).collect();
                Ok(json!({"id": f.first().copied().unwrap_or(""), "title": title}))
            }
            Request::ShortcutsList => {
                let names = self.shortcut_names().await?;
                Ok(json!({"shortcuts": names}))
            }
            Request::ShortcutsRun { name, input } => self.shortcut_run(&name, &input).await,
            Request::NotifyShow { title, body } => {
                // The synth's name leads, so a notification cannot pose
                // as the system or as this app.
                let title = format!("{}: {title}", call.synth_name);
                self.notifier.show(title, body).await.map_err(AppError::Unavailable)?;
                Ok(json!({"shown": true}))
            }
            Request::ClipboardRead => {
                let out = self.tool(&self.pbpaste, &[], None, "pbpaste").await?;
                let text = String::from_utf8_lossy(&out).to_string();
                let (text, truncated) = request::cut(&text, request::CLIPBOARD_MAX);
                Ok(json!({"text": text, "truncated": truncated}))
            }
            Request::ClipboardWrite { text } => {
                self.tool(&self.pbcopy, &[], Some(text.as_bytes()), "pbcopy").await?;
                Ok(json!({"bytes": text.len()}))
            }
            Request::OpenUrl { url, .. } => {
                self.tool(&self.open, &[url], None, "open").await?;
                Ok(json!({"opened": true}))
            }
            Request::OpenFile { path } => {
                let shown = path.to_string_lossy().to_string();
                self.tool(&self.open, &[shown.clone()], None, "open").await?;
                Ok(json!({"opened": true, "path": shown}))
            }
            Request::OpenApp { name, bundle_id } => {
                // `open` activates the app and asks it to reopen (a running
                // app shows its main window; macOS switches to the app's
                // Space when the owner's Mission Control setting allows
                // it). A bundle id names exactly one app (Finder lives
                // outside /Applications).
                let args = match &bundle_id {
                    Some(id) => vec!["-b".to_string(), id.clone()],
                    None => vec!["-a".to_string(), name.clone()],
                };
                self.tool(&self.open, &args, None, "open").await.map_err(|_| match &bundle_id {
                    Some(id) => AppError::NotFound(format!("no app with the bundle id {id:?} on this Mac")),
                    None => AppError::NotFound(format!("no app named {name:?} on this Mac")),
                })?;
                // "Opened" is not "shown": did a window come up? If not,
                // one explicit reopen, then the honest answer.
                let mut window = self.window_after_open(&name).await;
                let mut reopened = false;
                let mut desktop_folder = false;
                if window == Some(false) && is_finder(&name, bundle_id.as_deref()) {
                    // Finder with only its desktop: a Finder window on the
                    // owner's Desktop folder (what a task there needs),
                    // with no Apple event and so no Automation prompt.
                    if let Some(dir) = self.windows.finder_folder.as_ref() {
                        let folder = dir.to_string_lossy().to_string();
                        if self.tool(&self.open, &[folder], None, "open").await.is_ok() {
                            desktop_folder = true;
                            window = self.window_after_open(&name).await;
                        }
                    }
                }
                if window == Some(false) {
                    if let Some(id) = bundle_id.as_ref().filter(|id| (self.windows.may_script)(id)) {
                        reopened = self.osa("open", APP_REOPEN, std::slice::from_ref(id), "the app did not answer").await.is_ok();
                        window = self.window_after_open(&name).await;
                    }
                }
                let mut out = json!({"opened": true, "name": name, "bundle_id": bundle_id, "window": window, "reopened": reopened});
                if desktop_folder {
                    out["desktop_folder"] = json!(true);
                }
                if window == Some(false) {
                    out["status"] = json!("no_window");
                    out["note"] = json!(format!(
                        "{name} is running but shows no window; opening it{} brought none up",
                        if reopened { " and asking it to reopen" } else { "" }
                    ));
                }
                Ok(out)
            }
        }
    }

    /// Run a small system tool: optional stdin, stdout back. The UTF-8
    /// locale matters: without it pbcopy and pbpaste mangle anything that
    /// is not ASCII when the app was started by the system and has no
    /// LANG of its own.
    async fn tool(&self, program: &Path, args: &[String], stdin: Option<&[u8]>, what: &str) -> Result<Vec<u8>, AppError> {
        let mut child = crate::proc::command(program)
            .args(args)
            .env("LC_ALL", "en_US.UTF-8")
            .env("LANG", "en_US.UTF-8")
            .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| AppError::Unavailable(format!("cannot run {what}: {e}")))?;
        if let (Some(mut pipe), Some(bytes)) = (child.stdin.take(), stdin) {
            let _ = pipe.write_all(bytes).await;
            let _ = pipe.shutdown().await;
        }
        let out = child
            .wait_with_output()
            .await
            .map_err(|e| AppError::Failed(format!("{what} did not finish: {e}")))?;
        if !out.status.success() {
            return Err(AppError::Failed(format!("{what} failed")));
        }
        Ok(out.stdout)
    }

    async fn contacts(&self, query: &str, limit: usize) -> Result<Value, AppError> {
        let letters = query.chars().any(char::is_alphabetic);
        let mut found: Vec<Value> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        if letters {
            let args = [query.to_string(), SCRIPT_HARD_CAP.to_string()];
            let out = self.osa("contacts", CONTACTS_SEARCH, &args, "nothing matched").await?;
            for f in records(&out) {
                if f.len() < 4 {
                    continue;
                }
                let key = format!("{}|{}|{}", f[0], f[2], f[3]);
                if seen.contains(&key) {
                    continue;
                }
                seen.push(key);
                let list = |s: &str| -> Vec<String> { s.split(GS).filter(|x| !x.is_empty() && *x != "missing value").map(str::to_string).collect() };
                found.push(json!({"name": f[0], "organization": f[1], "phones": list(&f[2]), "emails": list(&f[3])}));
            }
        } else {
            // A search by number: names and numbers are compared here and
            // only the matches go on.
            let want = digits(query);
            let out = self.osa("contacts", CONTACTS_PHONES, &[], "nothing matched").await?;
            for f in records(&out) {
                if f.len() < 2 {
                    continue;
                }
                let phones: Vec<String> = f[1].split(GS).filter(|x| !x.is_empty()).map(str::to_string).collect();
                if phones.iter().any(|p| digits(p).contains(&want)) {
                    found.push(json!({"name": f[0], "organization": "", "phones": phones, "emails": []}));
                }
            }
        }
        let truncated = found.len() > limit;
        found.truncate(limit);
        Ok(json!({"contacts": found, "truncated": truncated}))
    }

    async fn shortcut_names(&self) -> Result<Vec<String>, AppError> {
        let out = self.tool(&self.shortcuts, &["list".to_string()], None, "shortcuts").await?;
        Ok(String::from_utf8_lossy(&out)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect())
    }

    async fn shortcut_run(&self, name: &str, input: &str) -> Result<Value, AppError> {
        // Only a shortcut that exists, by its exact name: the name is an
        // argument of a command line tool.
        if !self.shortcut_names().await?.iter().any(|n| n == name) {
            return Err(AppError::NotFound(format!("no shortcut named {name:?}")));
        }
        let dir = std::env::temp_dir();
        let tag = format!("{}-{:x}", std::process::id(), rand::random::<u64>());
        let out_path = dir.join(format!("snth-shortcut-out-{tag}"));
        let in_path = dir.join(format!("snth-shortcut-in-{tag}"));
        let mut args = vec!["run".to_string(), name.to_string(), "--output-path".to_string(), out_path.to_string_lossy().to_string()];
        if !input.is_empty() {
            write_private(&in_path, input.as_bytes())
                .map_err(|e| AppError::Failed(format!("cannot prepare the shortcut input: {e}")))?;
            args.push("--input-path".to_string());
            args.push(in_path.to_string_lossy().to_string());
        }
        let started = Instant::now();
        let ran = self.tool(&self.shortcuts, &args, None, "shortcuts").await;
        let output = std::fs::read(&out_path).unwrap_or_default();
        let _ = std::fs::remove_file(&out_path);
        let _ = std::fs::remove_file(&in_path);
        ran.map_err(|_| AppError::Failed(format!("the shortcut {name:?} failed or was stopped")))?;
        let text = String::from_utf8_lossy(&output).to_string();
        let (text, truncated) = request::cut(&text, request::SHORTCUT_OUTPUT_MAX);
        Ok(json!({"name": name, "output": text, "truncated": truncated, "duration_ms": started.elapsed().as_millis() as u64}))
    }
}

/// Create a file only this user can read, and write to it.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
    f.write_all(bytes)
}

/// The text of a message whose `text` column is empty: newer versions of
/// macOS keep it only in `attributedBody`, an archived attributed string.
/// The string itself sits after the `NSString` class name as a length and
/// UTF-8 bytes; that much is read here and the styling is ignored.
pub(crate) fn attributed_text(blob: &[u8]) -> Option<String> {
    let marker = b"NSString";
    let at = blob.windows(marker.len()).position(|w| w == marker)? + marker.len();
    let rest = &blob[at..];
    // The string follows a '+' within the next few bytes.
    let plus = rest.iter().take(16).position(|b| *b == b'+')?;
    let mut p = &rest[plus + 1..];
    let len = match *p.first()? {
        0x81 => {
            let n = u16::from_le_bytes([*p.get(1)?, *p.get(2)?]) as usize;
            p = &p[3..];
            n
        }
        0x82 => {
            let n = u32::from_le_bytes([*p.get(1)?, *p.get(2)?, *p.get(3)?, *p.get(4)?]) as usize;
            p = &p[5..];
            n
        }
        n if n < 0x80 => {
            p = &p[1..];
            n as usize
        }
        _ => return None,
    };
    let bytes = p.get(..len)?;
    let text = String::from_utf8_lossy(bytes).to_string();
    (!text.is_empty()).then_some(text)
}

/// Messages dates are nanoseconds since 2001 (seconds on old systems).
fn message_unix(date: i64) -> i64 {
    const EPOCH_2001: i64 = 978_307_200;
    if date > 100_000_000_000 {
        date / 1_000_000_000 + EPOCH_2001
    } else {
        date + EPOCH_2001
    }
}

/// The most recent messages, of one conversation or across all of them.
/// Read-only; reactions and group events are skipped; an attachment is a
/// count, never a file.
pub(crate) fn read_messages(db: &Path, chat: Option<&str>, limit: usize) -> Result<Value, AppError> {
    use rusqlite::{Connection, OpenFlags};
    let no_access = || {
        AppError::Unavailable(
            "reading messages needs Full Disk Access for SNTH Connect on this Mac; the owner can give it in System Settings, Privacy & Security, Full Disk Access (the app's Apps screen has the button)".into(),
        )
    };
    if !db.exists() {
        // Without Full Disk Access the file cannot even be seen.
        return match std::fs::metadata(db) {
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Err(no_access()),
            _ => Err(AppError::Unavailable("Messages has no history on this Mac".into())),
        };
    }
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|_| no_access())?;
    let _ = conn.busy_timeout(std::time::Duration::from_secs(2));
    let want = chat.unwrap_or("");
    // More rows than asked: reactions and empty rows are dropped below.
    let fetch = (limit * 3 + 10) as i64;
    let sql = |body: &str| {
        format!(
            "SELECT m.date, m.text, {body}, m.is_from_me, COALESCE(h.id, ''), COALESCE(c.chat_identifier, ''), \
             COALESCE(c.display_name, ''), COALESCE(m.service, ''), COALESCE(m.cache_has_attachments, 0), \
             COALESCE(m.associated_message_type, 0), COALESCE(m.item_type, 0) \
             FROM message m \
             LEFT JOIN handle h ON m.handle_id = h.ROWID \
             LEFT JOIN chat_message_join cmj ON cmj.message_id = m.ROWID \
             LEFT JOIN chat c ON cmj.chat_id = c.ROWID \
             WHERE (?1 = '' OR h.id = ?1 OR c.chat_identifier = ?1) \
             ORDER BY m.date DESC LIMIT ?2"
        )
    };
    // An old database has no attributedBody column.
    let mut stmt = match conn.prepare(&sql("m.attributedBody")) {
        Ok(s) => s,
        Err(_) => conn.prepare(&sql("NULL")).map_err(|e| {
            if e.to_string().contains("authorization denied") || e.to_string().contains("unable to open") {
                no_access()
            } else {
                AppError::Failed("the Messages history has a layout this version cannot read".into())
            }
        })?,
    };
    let rows = stmt
        .query_map(rusqlite::params![want, fetch], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<Vec<u8>>>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, i64>(8)?,
                r.get::<_, i64>(9)?,
                r.get::<_, i64>(10)?,
            ))
        })
        .map_err(|_| no_access())?;
    let mut messages: Vec<Value> = Vec::new();
    let mut more = false;
    for row in rows.flatten() {
        let (date, text, body, from_me, handle, chat_id, chat_name, service, has_attachment, reaction, item_type) = row;
        if reaction != 0 || item_type != 0 {
            continue;
        }
        let mut text = text.filter(|t| !t.trim().is_empty()).or_else(|| body.as_deref().and_then(attributed_text)).unwrap_or_default();
        // The placeholder character Messages puts where a file sits.
        text = text.replace('\u{FFFC}', "").trim().to_string();
        if text.is_empty() {
            if has_attachment == 0 {
                continue;
            }
            text = "[attachment]".to_string();
        }
        if messages.len() == limit {
            more = true;
            break;
        }
        let (text, _) = request::cut(&text, request::MESSAGE_TEXT_MAX);
        let mut m = json!({
            "time": time::rfc3339_utc(message_unix(date)),
            "from": if from_me != 0 { "me".to_string() } else { handle.clone() },
            "chat": if chat_id.is_empty() { handle } else { chat_id },
            "text": text,
            "service": service,
        });
        if !chat_name.is_empty() {
            m["chat_name"] = Value::String(chat_name);
        }
        if has_attachment != 0 {
            m["attachments"] = json!(1);
        }
        messages.push(m);
    }
    Ok(json!({"messages": messages, "truncated": more}))
}

#[cfg(test)]
mod tests;
