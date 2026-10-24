//! Observe on this computer: the shell's half of connect-core's `observe`
//! (UC source contract P2, connect.md "Observe"). The core decides and
//! sends; this reads the system.
//!
//! One thread ("observe") runs a round every `sources::TICK` (2 s): the
//! sources some linked synth has switched on (`Core::observe_wanted`) are
//! read through this system's probes (mac.rs, win.rs) and their values go
//! to `Core::observe_report`. While the app is paused nothing is read and
//! the last values stay as they are (the links send nothing while paused;
//! a resume then sends no spurious null).
//!
//! Permissions, each only where a source needs it:
//!
//!  - the call: none on either system (the microphone is never opened:
//!    the system is asked who records);
//!  - the activity's window titles: macOS Accessibility, the permission
//!    computer control already asks for. Observe never asks for it: the
//!    page says so and opens the system's settings. Windows: none;
//!  - the place: macOS Location Services (since macOS 14 the network's
//!    name needs it), asked when the owner switches the place on.
//!    Windows 11 24H2 and later: location access for desktop apps.
//!
//! Linux: no probe; the page says the sources are not available here.

#[cfg(target_os = "macos")]
mod mac;
#[cfg(windows)]
mod win;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[cfg(not(any(target_os = "macos", windows)))]
use connect_core::observe::sources::{FrontProbe, MicProbe, MicSample, WifiProbe};
use connect_core::observe::sources::{Sampler, Wifi, TICK};
use connect_core::observe::{AppMark, ObserveView, ACTIVITY, CALL, PLACE};
use serde::Serialize;

use crate::state::Ctx;

#[cfg(target_os = "macos")]
use mac as sys;
#[cfg(windows)]
use win as sys;

/// The sources this system can read.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Support {
    pub call: bool,
    pub activity: bool,
    pub place: bool,
}

pub fn support() -> Support {
    let on = cfg!(any(target_os = "macos", windows));
    Support { call: on, activity: on, place: on }
}

/// What the observe thread saw last, for the page.
#[derive(Debug, Clone, Serialize)]
pub struct Seen {
    /// The Wi-Fi network (its name stays on this computer: the page shows
    /// it to the owner, the synth never gets it).
    pub wifi: Wifi,
    /// The apps in front lately (the exclusion picker).
    pub recent: Vec<AppMark>,
}

impl Default for Seen {
    fn default() -> Self {
        Seen { wifi: Wifi::Unknown, recent: Vec::new() }
    }
}

#[derive(Default)]
pub struct Observe {
    seen: Mutex<Seen>,
    /// Read the network at the next round (the owner answered the
    /// location question, or switched the place on).
    wifi_again: AtomicBool,
}

/// The "what the synth sees" page.
#[derive(Debug, Clone, Serialize)]
pub struct ObserveDto {
    #[serde(flatten)]
    pub core: ObserveView,
    pub support: Support,
    /// macOS: may the app read window titles (Accessibility)? None where
    /// no permission is involved.
    pub titles_allowed: Option<bool>,
    #[serde(flatten)]
    pub seen: Seen,
}

pub fn dto(ctx: &Ctx) -> ObserveDto {
    #[cfg(target_os = "macos")]
    let titles_allowed = Some(mac::titles_allowed());
    #[cfg(not(target_os = "macos"))]
    let titles_allowed = None;
    ObserveDto { core: ctx.core.observe_view(), support: support(), titles_allowed, seen: ctx.observe.seen.lock().unwrap().clone() }
}

/// The apps with a window that run now (the exclusion picker).
pub fn running_apps() -> Vec<AppMark> {
    #[cfg(any(target_os = "macos", windows))]
    {
        sys::running_apps()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Vec::new()
    }
}

/// Switch one source for one synth. Switching the place on asks for
/// what the system needs to tell the network (macOS: Location Services,
/// once; the answer arrives later and the next rounds read it).
pub fn set(ctx: &Arc<Ctx>, synth_id: &str, resource: &str, on: bool) -> Result<(), String> {
    if !ctx.core.link_ids().iter().any(|id| id == synth_id) {
        return Err("unknown_synth".into());
    }
    let s = support();
    let supported = match resource {
        CALL => s.call,
        ACTIVITY => s.activity,
        PLACE => s.place,
        _ => return Err("unknown_resource".into()),
    };
    if on && !supported {
        return Err("unsupported".into());
    }
    let mut err = None;
    ctx.core.set_observe(|o| err = o.set_on(synth_id, resource, on).err());
    if let Some(e) = err {
        return Err(e);
    }
    if on && resource == PLACE {
        ask_location(ctx);
    }
    Ok(())
}

/// Ask the system for the network's name (macOS: Location Services).
/// Nothing to ask elsewhere.
pub fn ask_location(ctx: &Arc<Ctx>) {
    #[cfg(target_os = "macos")]
    {
        if matches!(mac::wifi_now(), Wifi::NotAsked) {
            let _ = ctx.app.run_on_main_thread(mac::request_location);
        }
    }
    ctx.observe.wifi_again.store(true, Ordering::Release);
}

// A system without probes (Linux): every source reads nothing.
#[cfg(not(any(target_os = "macos", windows)))]
struct NoMic;
#[cfg(not(any(target_os = "macos", windows)))]
impl MicProbe for NoMic {
    fn sample(&mut self) -> MicSample {
        MicSample::default()
    }
}
#[cfg(not(any(target_os = "macos", windows)))]
struct NoFront;
#[cfg(not(any(target_os = "macos", windows)))]
impl FrontProbe for NoFront {
    fn front(&mut self) -> Option<connect_core::observe::sources::Front> {
        None
    }
}
#[cfg(not(any(target_os = "macos", windows)))]
struct NoWifi;
#[cfg(not(any(target_os = "macos", windows)))]
impl WifiProbe for NoWifi {
    fn read(&mut self) -> Wifi {
        Wifi::Unsupported
    }
}

fn sampler() -> Sampler {
    #[cfg(any(target_os = "macos", windows))]
    {
        Sampler::new(Box::new(sys::Mic), Box::new(sys::Front), Box::new(sys::Wifi))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Sampler::new(Box::new(NoMic), Box::new(NoFront), Box::new(NoWifi))
    }
}

/// Start the observe thread. It ends with the app.
pub fn spawn(ctx: Arc<Ctx>) {
    let _ = std::thread::Builder::new().name("observe".into()).spawn(move || {
        let mut sampler = sampler();
        let shutdown = ctx.shutdown.subscribe();
        let mut wifi_shown = Wifi::Unknown;
        loop {
            if *shutdown.borrow() {
                break;
            }
            let start = Instant::now();
            if !ctx.core.shared().paused() {
                round(&ctx, &mut sampler, &mut wifi_shown);
            }
            std::thread::sleep(TICK.saturating_sub(start.elapsed()));
        }
    });
}

fn round(ctx: &Arc<Ctx>, sampler: &mut Sampler, wifi_shown: &mut Wifi) {
    let wanted = ctx.core.observe_wanted();
    if ctx.observe.wifi_again.swap(false, Ordering::AcqRel) {
        sampler.wifi_again();
    }
    let settings = ctx.core.observe_settings();
    let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let out = sampler.tick(Instant::now(), unix, &wanted, &settings);
    for (resource, value) in out {
        ctx.core.observe_report(resource, value);
    }
    let mut seen = ctx.observe.seen.lock().unwrap();
    seen.wifi = sampler.wifi().clone();
    seen.recent = sampler.recent().to_vec();
    // A new network or a new answer about the permission: the page shows
    // it at once (the rest it asks for while it is open).
    if seen.wifi != *wifi_shown {
        *wifi_shown = seen.wifi.clone();
        drop(seen);
        crate::ui::emit(&ctx.app, crate::ui::UiEvent::State);
    }
}
