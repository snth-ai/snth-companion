//! Which microphone the listener keeps open.
//!
//! An always-open Bluetooth microphone switches AirPods (and every other
//! Bluetooth headset) to call audio (HFP): music turns into a phone line
//! for as long as the mode is on. So the listener never opens a Bluetooth
//! input. When the default input is Bluetooth it listens on the built-in
//! microphone instead (a MacBook has one), or a USB or other wired input;
//! with nothing else (a Mac mini with AirPods) it pauses. Hold-to-talk is
//! not affected: a hold is short and uses the default input as before.
//!
//! The device list comes from `system::input_devices`; the decision is
//! here, apart from CoreAudio, so it can be tested. The listener asks
//! again once a second and opens the microphone afresh when the answer
//! changes (the default input changed, a device came or went).

/// CoreAudio's `kAudioDeviceTransportType*` values (four-char codes).
pub mod transport {
    const fn fourcc(s: &[u8; 4]) -> u32 {
        ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | s[3] as u32
    }
    pub const UNKNOWN: u32 = 0;
    pub const BUILT_IN: u32 = fourcc(b"bltn");
    pub const AGGREGATE: u32 = fourcc(b"grup");
    pub const AUTO_AGGREGATE: u32 = fourcc(b"fgrp");
    pub const VIRTUAL: u32 = fourcc(b"virt");
    pub const USB: u32 = fourcc(b"usb ");
    pub const BLUETOOTH: u32 = fourcc(b"blue");
    pub const BLUETOOTH_LE: u32 = fourcc(b"blea");
    pub const CONTINUITY_WIRED: u32 = fourcc(b"ccwd");
    pub const CONTINUITY_WIRELESS: u32 = fourcc(b"ccwl");

    pub fn name(t: u32) -> String {
        if t == UNKNOWN {
            return "unknown".into();
        }
        t.to_be_bytes().iter().map(|b| *b as char).collect::<String>().trim().to_string()
    }
}

/// An input device, as CoreAudio lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDevice {
    /// The CoreAudio UID (cpal's device id).
    pub uid: String,
    pub name: String,
    pub transport: u32,
    pub is_default: bool,
}

/// Where the listener listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mic {
    /// This device. `instead_of`: the Bluetooth default it stands in for.
    Use { uid: String, name: String, instead_of: Option<String> },
    /// The default input is Bluetooth and there is no other microphone.
    BluetoothOnly { name: String },
    /// The list could not be read: the default input, as a hold uses it.
    Default,
}

pub fn is_bluetooth(transport: u32) -> bool {
    transport == transport::BLUETOOTH || transport == transport::BLUETOOTH_LE
}

/// A microphone that may stand in for a Bluetooth default: a real one on
/// this Mac or plugged into it. Not a virtual device (BlackHole hears no
/// room), not an aggregate (it may hold the headset itself), not an
/// iPhone's microphone over Continuity.
fn stands_in(transport: u32) -> bool {
    !is_bluetooth(transport)
        && !matches!(
            transport,
            transport::UNKNOWN | transport::VIRTUAL | transport::AGGREGATE | transport::AUTO_AGGREGATE | transport::CONTINUITY_WIRED | transport::CONTINUITY_WIRELESS
        )
}

/// The microphone for the listener, from the input devices.
pub fn choose(devices: &[InputDevice]) -> Mic {
    let Some(default) = devices.iter().find(|d| d.is_default) else {
        return Mic::Default;
    };
    if !is_bluetooth(default.transport) {
        return Mic::Use { uid: default.uid.clone(), name: default.name.clone(), instead_of: None };
    }
    let other = devices
        .iter()
        .filter(|d| stands_in(d.transport))
        .min_by_key(|d| match d.transport {
            transport::BUILT_IN => 0,
            transport::USB => 1,
            _ => 2,
        });
    match other {
        Some(d) => Mic::Use { uid: d.uid.clone(), name: d.name.clone(), instead_of: Some(default.name.clone()) },
        None => Mic::BluetoothOnly { name: default.name.clone() },
    }
}

impl Mic {
    /// The device to open (None: the default input).
    pub fn uid(&self) -> Option<&str> {
        match self {
            Mic::Use { uid, .. } => Some(uid),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::transport::*;
    use super::*;

    fn dev(uid: &str, transport: u32, is_default: bool) -> InputDevice {
        InputDevice { uid: uid.into(), name: uid.into(), transport, is_default }
    }

    fn uses(m: &Mic) -> Option<&str> {
        m.uid()
    }

    #[test]
    fn a_wired_default_is_used_as_it_is() {
        let devices = [dev("builtin", BUILT_IN, true), dev("blackhole", VIRTUAL, false)];
        assert_eq!(choose(&devices), Mic::Use { uid: "builtin".into(), name: "builtin".into(), instead_of: None });
        let devices = [dev("builtin", BUILT_IN, false), dev("usb-mic", USB, true)];
        assert_eq!(uses(&choose(&devices)), Some("usb-mic"), "the owner's choice of a wired microphone stands");
    }

    #[test]
    fn airpods_as_the_default_send_the_listener_to_the_built_in_microphone() {
        let devices = [dev("usb-mic", USB, false), dev("airpods", BLUETOOTH, true), dev("builtin", BUILT_IN, false), dev("blackhole", VIRTUAL, false)];
        assert_eq!(choose(&devices), Mic::Use { uid: "builtin".into(), name: "builtin".into(), instead_of: Some("airpods".into()) });
        // Bluetooth LE headsets too.
        let devices = [dev("le", BLUETOOTH_LE, true), dev("builtin", BUILT_IN, false)];
        assert_eq!(uses(&choose(&devices)), Some("builtin"));
    }

    #[test]
    fn without_a_built_in_microphone_another_wired_one_stands_in() {
        let devices = [dev("airpods", BLUETOOTH, true), dev("usb-mic", USB, false)];
        assert_eq!(uses(&choose(&devices)), Some("usb-mic"));
    }

    #[test]
    fn usb_comes_before_other_wired_inputs() {
        let devices = [dev("airpods", BLUETOOTH, true), dev("thunderbolt-dock", fourcc_thun(), false), dev("usb-mic", USB, false)];
        assert_eq!(uses(&choose(&devices)), Some("usb-mic"));
    }

    fn fourcc_thun() -> u32 {
        u32::from_be_bytes(*b"thun")
    }

    #[test]
    fn a_mac_mini_with_airpods_pauses() {
        // Only the headset, a virtual device, an aggregate, an iPhone.
        let devices = [
            dev("airpods", BLUETOOTH, true),
            dev("blackhole", VIRTUAL, false),
            dev("agg", AGGREGATE, false),
            dev("iphone", CONTINUITY_WIRELESS, false),
            dev("other-bt", BLUETOOTH_LE, false),
            dev("odd", UNKNOWN, false),
        ];
        assert_eq!(choose(&devices), Mic::BluetoothOnly { name: "airpods".into() });
        assert_eq!(choose(&devices).uid(), None);
    }

    #[test]
    fn an_unreadable_list_falls_back_to_the_default() {
        assert_eq!(choose(&[]), Mic::Default);
        assert_eq!(choose(&[dev("builtin", BUILT_IN, false)]), Mic::Default, "no default known");
    }

    #[test]
    fn transports_print_as_their_codes() {
        assert_eq!(transport::name(BLUETOOTH), "blue");
        assert_eq!(transport::name(USB), "usb");
        assert_eq!(transport::name(UNKNOWN), "unknown");
    }
}
