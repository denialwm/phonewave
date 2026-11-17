//! What accessory-pair decides, apart from talking to BlueZ and udev: the
//! addresses, the events the kernel sends and the steps that bring a
//! Bluetooth accessory where it belongs.

use std::{fmt, str::FromStr};

pub mod agent;
pub mod bluez;

/// The uevent variables a dock driver sends when an accessory comes or goes.
pub const ENV_ADDRESS: &str = "ACCESSORY_BT_ADDRESS";
pub const ENV_PRESENT: &str = "ACCESSORY_PRESENT";
pub const ENV_TYPE: &str = "ACCESSORY_TYPE";
/// The sysfs attributes of the reporting device: the address of the docked
/// accessory, and where its Bluetooth battery level goes while it is docked.
pub const ATTR_ADDRESS: &str = "bt_address";
pub const ATTR_CAPACITY: &str = "bt_capacity";
/// The attribute of a touch controller that senses a stylus: the stylus
/// type it looks for, 0 for none.
pub const ATTR_PEN_TYPE: &str = "pen_type";
/// The stylus type a touch controller looks for when it senses one again.
pub const PEN_TYPE_DEFAULT: u8 = 1;

/// A Bluetooth device address, most significant byte first.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Address(pub [u8; 6]);

impl Address {
    /// The same bytes in the other order, for a dock that reports the
    /// address as it travels on the air.
    pub fn reversed(self) -> Address {
        let mut a = self.0;
        a.reverse();
        Address(a)
    }

    /// The LE address type: a static random address has the two top bits set.
    pub fn le_type(self) -> &'static str {
        if self.0[0] & 0xc0 == 0xc0 {
            "random"
        } else {
            "public"
        }
    }

    /// The address of a BlueZ device object, `/org/bluez/hci0/dev_C0_87_06_07_FE_69`.
    pub fn from_object_path(path: &str) -> Option<Address> {
        let name = path.rsplit('/').next()?.strip_prefix("dev_")?;
        name.replace('_', ":").parse().ok()
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let a = self.0;
        write!(
            f,
            "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            a[0], a[1], a[2], a[3], a[4], a[5]
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct BadAddress;

impl fmt::Display for BadAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not a Bluetooth address")
    }
}

impl std::error::Error for BadAddress {}

impl FromStr for Address {
    type Err = BadAddress;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let mut a = [0u8; 6];
        let mut parts = s.split(':');
        for byte in &mut a {
            let part = parts.next().ok_or(BadAddress)?;
            if part.len() != 2 {
                return Err(BadAddress);
            }
            *byte = u8::from_str_radix(part, 16).map_err(|_| BadAddress)?;
        }
        if parts.next().is_some() || a == [0; 6] {
            return Err(BadAddress);
        }
        Ok(Address(a))
    }
}

/// An accessory came or went, as a dock driver reported it.
#[derive(Debug, PartialEq, Eq)]
pub struct DockEvent {
    pub address: Address,
    pub present: bool,
    /// What the driver calls it (stylus, keyboard), for the log.
    pub kind: String,
}

impl DockEvent {
    /// The event in a uevent's variables, if it is one.
    pub fn from_env<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Option<DockEvent> {
        let address = get(ENV_ADDRESS)?.parse().ok()?;
        let present = match get(ENV_PRESENT)? {
            "1" => true,
            "0" => false,
            _ => return None,
        };
        let kind = get(ENV_TYPE).unwrap_or("accessory").to_owned();
        Some(DockEvent {
            address,
            present,
            kind,
        })
    }
}

/// What BlueZ knows about an accessory.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeviceView {
    pub exists: bool,
    pub paired: bool,
    pub trusted: bool,
    pub connected: bool,
    pub blocked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Scan LE until the accessory shows up.
    Discover,
    /// Pair through our NoInputNoOutput agent: Just Works, no prompt.
    Pair,
    /// Let it reconnect on its own.
    Trust,
    Connect,
    /// Refuse its Bluetooth connections, keeping the bond.
    Block,
    Unblock,
}

/// The steps from what BlueZ knows to where the accessory belongs.
///
/// Every accessory gets paired and trusted. One whose input arrives over the
/// dock while it is docked (`wired_input`, the keyboard) is kept off
/// Bluetooth while docked, so no key arrives twice, and connected when it
/// leaves. Any other (the stylus) stays connected, docked or not; an undocked
/// one connects by itself when it is used.
pub fn plan(docked: bool, wired_input: bool, d: &DeviceView) -> Vec<Step> {
    let keep_off = docked && wired_input;
    let mut steps = Vec::new();
    if !d.paired {
        if d.blocked {
            steps.push(Step::Unblock);
        }
        if !d.exists {
            steps.push(Step::Discover);
        }
        steps.push(Step::Pair);
    }
    if !d.paired || !d.trusted {
        steps.push(Step::Trust);
    }
    if keep_off {
        if !d.paired || !d.blocked {
            steps.push(Step::Block);
        }
    } else {
        if d.paired && d.blocked {
            steps.push(Step::Unblock);
        }
        let blocked_after = d.blocked && !steps.contains(&Step::Unblock);
        if (docked || wired_input) && !blocked_after && (!d.connected || !d.paired) {
            steps.push(Step::Connect);
        }
    }
    steps
}

/// Whether the touch controller should look for the stylus: while it is
/// docked (charging, maybe not connected yet) or connected over Bluetooth,
/// as the stock Android turns pen sensing off when the stylus link drops.
pub fn pen_sensing(docked: bool, connected: bool) -> bool {
    docked || connected
}

/// What to write to `pen_type`, which reads `current`, so that the touch
/// controller senses the stylus or not. A stylus type set by someone else
/// stays as long as sensing stays on.
pub fn pen_type_change(current: u8, sense: bool) -> Option<u8> {
    match (current, sense) {
        (0, true) => Some(PEN_TYPE_DEFAULT),
        (1.., false) => Some(0),
        _ => None,
    }
}

/// The blocked-addresses file: one address per line.
pub fn parse_address_list(text: &str) -> Vec<Address> {
    text.lines().filter_map(|l| l.parse().ok()).collect()
}

pub fn format_address_list<'a>(list: impl IntoIterator<Item = &'a Address>) -> String {
    list.into_iter().map(|a| format!("{a}\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const PEN: Address = Address([0xc0, 0x87, 0x06, 0x07, 0xfe, 0x68]);

    #[test]
    fn address_round_trip() {
        let a: Address = "c0:87:06:07:fe:68".parse().unwrap();
        assert_eq!(a, PEN);
        assert_eq!(a.to_string(), "C0:87:06:07:FE:68");
        assert_eq!(" C0:87:06:07:FE:68\n".parse::<Address>(), Ok(PEN));
    }

    #[test]
    fn address_rejects_garbage() {
        for s in [
            "",
            "c0:87:06:07:fe",
            "c0:87:06:07:fe:68:00",
            "c0:87:06:07:fe:6",
            "c0-87-06-07-fe-68",
            "g0:87:06:07:fe:68",
            "00:00:00:00:00:00",
        ] {
            assert!(s.parse::<Address>().is_err(), "{s}");
        }
    }

    #[test]
    fn address_type_and_order() {
        assert_eq!(PEN.le_type(), "random");
        assert_eq!(PEN.reversed().to_string(), "68:FE:07:06:87:C0");
        assert_eq!(PEN.reversed().le_type(), "public");
    }

    #[test]
    fn address_from_object_path() {
        assert_eq!(
            Address::from_object_path("/org/bluez/hci0/dev_C0_87_06_07_FE_69"),
            Some(PEN)
        );
        assert_eq!(Address::from_object_path("/org/bluez/hci0"), None);
        assert_eq!(
            Address::from_object_path("/org/bluez/hci0/dev_C0_87_06_07_FE_69/service0010"),
            None
        );
    }

    #[test]
    fn dock_event_from_env() {
        let env: HashMap<&str, &str> = [
            ("ACCESSORY_TYPE", "stylus"),
            ("ACCESSORY_PRESENT", "1"),
            ("ACCESSORY_BT_ADDRESS", "c0:87:06:07:fe:68"),
        ]
        .into();
        let e = DockEvent::from_env(|k| env.get(k).copied()).unwrap();
        assert_eq!(
            e,
            DockEvent {
                address: PEN,
                present: true,
                kind: "stylus".into()
            }
        );
        let mut env = env;
        env.insert("ACCESSORY_PRESENT", "2");
        assert_eq!(DockEvent::from_env(|k| env.get(k).copied()), None);
        env.remove("ACCESSORY_PRESENT");
        assert_eq!(DockEvent::from_env(|k| env.get(k).copied()), None);
        let none: HashMap<&str, &str> = HashMap::new();
        assert_eq!(DockEvent::from_env(|k| none.get(k).copied()), None);
    }

    const NEW: DeviceView = DeviceView {
        exists: false,
        paired: false,
        trusted: false,
        connected: false,
        blocked: false,
    };
    const BONDED: DeviceView = DeviceView {
        exists: true,
        paired: true,
        trusted: true,
        connected: true,
        blocked: false,
    };

    #[test]
    fn docked_stylus_gets_paired_and_connected() {
        use Step::*;
        assert_eq!(plan(true, false, &NEW), [Discover, Pair, Trust, Connect]);
        let seen = DeviceView {
            exists: true,
            ..NEW
        };
        assert_eq!(plan(true, false, &seen), [Pair, Trust, Connect]);
        assert_eq!(plan(true, false, &BONDED), []);
        let asleep = DeviceView {
            connected: false,
            ..BONDED
        };
        assert_eq!(plan(true, false, &asleep), [Connect]);
    }

    #[test]
    fn undocked_stylus_is_left_alone_once_bonded() {
        use Step::*;
        let asleep = DeviceView {
            connected: false,
            ..BONDED
        };
        assert_eq!(plan(false, false, &asleep), []);
        // never paired: pair it now, it connects by itself afterwards
        assert_eq!(plan(false, false, &NEW), [Discover, Pair, Trust]);
    }

    #[test]
    fn docked_keyboard_is_bonded_but_kept_off() {
        use Step::*;
        assert_eq!(plan(true, true, &NEW), [Discover, Pair, Trust, Block]);
        assert_eq!(plan(true, true, &BONDED), [Block]);
        let blocked = DeviceView {
            blocked: true,
            connected: false,
            ..BONDED
        };
        assert_eq!(plan(true, true, &blocked), []);
        // blocked from an earlier session but the bond is gone
        let stale = DeviceView {
            exists: true,
            blocked: true,
            ..NEW
        };
        assert_eq!(plan(true, true, &stale), [Unblock, Pair, Trust, Block]);
    }

    #[test]
    fn undocked_keyboard_comes_back_on_bluetooth() {
        use Step::*;
        let blocked = DeviceView {
            blocked: true,
            connected: false,
            ..BONDED
        };
        assert_eq!(plan(false, true, &blocked), [Unblock, Connect]);
        assert_eq!(plan(false, true, &BONDED), []);
        // could not be paired while docked: pair it now
        assert_eq!(plan(false, true, &NEW), [Discover, Pair, Trust, Connect]);
    }

    #[test]
    fn pen_is_sensed_while_docked_or_connected() {
        assert!(pen_sensing(true, false));
        assert!(pen_sensing(true, true));
        assert!(pen_sensing(false, true));
        assert!(!pen_sensing(false, false));
    }

    #[test]
    fn pen_type_written_only_on_a_change() {
        assert_eq!(pen_type_change(0, true), Some(1));
        assert_eq!(pen_type_change(1, true), None);
        assert_eq!(pen_type_change(3, true), None);
        assert_eq!(pen_type_change(1, false), Some(0));
        assert_eq!(pen_type_change(3, false), Some(0));
        assert_eq!(pen_type_change(0, false), None);
    }

    #[test]
    fn address_list_round_trip() {
        let list = [PEN, PEN.reversed()];
        let text = format_address_list(&list);
        assert_eq!(text, "C0:87:06:07:FE:68\n68:FE:07:06:87:C0\n");
        assert_eq!(parse_address_list(&format!("{text}junk\n\n")), list);
    }
}
