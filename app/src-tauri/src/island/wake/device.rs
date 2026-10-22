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
//!
//! Windows has no transport codes of its own: `windows_transport` turns
//! the bus Windows names for the device under an input
//! (`PKEY_Device_EnumeratorName`) into CoreAudio's codes, so `choose`
//! decides the same way there. A headset's microphone on Windows is the
//! hands-free endpoint (BTHHFENUM), opening which switches the headset to
//! call audio exactly as on a Mac; a laptop's own microphone sits on
//! HDAUDIO or INTELAUDIO (Intel Smart Sound), a desktop's jack on
//! HDAUDIO, a USB microphone on USB.

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
    pub const PCI: u32 = fourcc(b"pci ");

    pub fn name(t: u32) -> String {
        if t == UNKNOWN {
            return "unknown".into();
        }
        t.to_be_bytes().iter().map(|b| *b as char).collect::<String>().trim().to_string()
    }
}

/// The transport code of a Windows input from the bus of its device
/// (`PKEY_Device_EnumeratorName`) and its name. Bluetooth is every bus
/// whose name starts with BTH (BTHENUM, BTHHFENUM: classic headsets,
/// BTHLE*: LE Audio), and, when the bus could not be read, a name that
/// says "Hands-Free" or "Bluetooth": a Bluetooth microphone is the one
/// thing never to open. An unknown bus is UNKNOWN: used when it is the
/// default, never as a stand-in.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn windows_transport(bus: &str, name: &str) -> u32 {
    let bus = bus.trim().to_ascii_uppercase();
    let low = name.to_lowercase();
    if bus.starts_with("BTHLE") {
        return transport::BLUETOOTH_LE;
    }
    if bus.starts_with("BTH") || low.contains("hands-free") || (bus.is_empty() && low.contains("bluetooth")) {
        return transport::BLUETOOTH;
    }
    match bus.as_str() {
        "USB" => transport::USB,
        // The board's codec (HD Audio), Intel Smart Sound's and AMD's
        // audio co-processor's digital microphones, a laptop's array on
        // ACPI.
        "HDAUDIO" | "INTELAUDIO" | "ACP" | "AMDACP" | "ACPI" => transport::BUILT_IN,
        // A sound card.
        "PCI" => transport::PCI,
        // Software: a virtual cable, a streaming app's microphone.
        "SW" | "SWD" | "ROOT" | "MMDEVAPI" => transport::VIRTUAL,
        _ => transport::UNKNOWN,
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
    fn windows_buses_map_to_the_transports() {
        // A classic headset: its microphone is the hands-free endpoint.
        assert_eq!(windows_transport("BTHHFENUM", "Headset (WH-1000XM4 Hands-Free AG Audio)"), BLUETOOTH);
        assert_eq!(windows_transport("BTHENUM", "Headset (AirPods Pro)"), BLUETOOTH);
        assert_eq!(windows_transport("BTHLEDEVICE", "Headset (Galaxy Buds2 Pro)"), BLUETOOTH_LE);
        assert_eq!(windows_transport("bthenum", "x"), BLUETOOTH, "case does not matter");
        // The bus unread: the name still gives a headset away.
        assert_eq!(windows_transport("", "Headset (Jabra Evolve2 Hands-Free AG Audio)"), BLUETOOTH);
        assert_eq!(windows_transport("", "Microphone (Bluetooth Audio)"), BLUETOOTH);
        assert_eq!(windows_transport("", "Microphone Array"), UNKNOWN);
        // Wired.
        assert_eq!(windows_transport("HDAUDIO", "Microphone Array (Realtek(R) Audio)"), BUILT_IN);
        assert_eq!(windows_transport("INTELAUDIO", "Microphone Array (Intel® Smart Sound Technology for Digital Microphones)"), BUILT_IN);
        assert_eq!(windows_transport("USB", "Microphone (Yeti Stereo Microphone)"), USB);
        assert_eq!(windows_transport("PCI", "Line In (Sound Blaster)"), PCI);
        // Software.
        assert_eq!(windows_transport("ROOT", "CABLE Output (VB-Audio Virtual Cable)"), VIRTUAL);
        assert_eq!(windows_transport("SWD", "Steam Streaming Microphone"), VIRTUAL);
        assert_eq!(windows_transport("SOMETHINGNEW", "Mic"), UNKNOWN);
    }

    #[test]
    fn a_windows_laptop_with_a_headset_listens_on_its_own_microphone() {
        // As system::input_devices lists a laptop with AirPods connected
        // and the hands-free microphone made the default.
        let win = |uid: &str, bus: &str, name: &str, is_default: bool| InputDevice { uid: uid.into(), name: name.into(), transport: windows_transport(bus, name), is_default };
        let devices = [
            win("{0.0.1.00000000}.{aa}", "BTHHFENUM", "Headset (AirPods Pro Hands-Free AG Audio)", true),
            win("{0.0.1.00000000}.{bb}", "INTELAUDIO", "Microphone Array (Intel® Smart Sound Technology for Digital Microphones)", false),
            win("{0.0.1.00000000}.{cc}", "ROOT", "CABLE Output (VB-Audio Virtual Cable)", false),
        ];
        assert_eq!(
            choose(&devices),
            Mic::Use {
                uid: "{0.0.1.00000000}.{bb}".into(),
                name: "Microphone Array (Intel® Smart Sound Technology for Digital Microphones)".into(),
                instead_of: Some("Headset (AirPods Pro Hands-Free AG Audio)".into())
            }
        );
        // A desktop with only the headset (and a virtual cable): pause.
        assert_eq!(choose(&[devices[0].clone(), devices[2].clone()]), Mic::BluetoothOnly { name: "Headset (AirPods Pro Hands-Free AG Audio)".into() });
        // A USB microphone stands in before a sound card's line in.
        let devices = [
            win("bt", "BTHENUM", "Headset", true),
            win("pci", "PCI", "Line In", false),
            win("usb", "USB", "Yeti", false),
        ];
        assert_eq!(uses(&choose(&devices)), Some("usb"));
    }

    #[test]
    fn transports_print_as_their_codes() {
        assert_eq!(transport::name(BLUETOOTH), "blue");
        assert_eq!(transport::name(USB), "usb");
        assert_eq!(transport::name(UNKNOWN), "unknown");
    }
}
