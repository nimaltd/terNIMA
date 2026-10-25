//! Finding the same port again, when the OS may have given it another name.

use serde::{Deserialize, Serialize};

use super::ports::{PortInfo, UsbId};

/// What a session remembers about its port, to find it again later.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortId {
    /// The name it had last time, such as COM6.
    pub name: String,
    /// Set for USB ports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usb: Option<UsbId>,
}

impl From<&PortInfo> for PortId {
    fn from(port: &PortInfo) -> Self {
        PortId {
            name: port.name.clone(),
            usb: port.usb.clone(),
        }
    }
}

/// Finds the port a session used before among the ports there are now.
///
/// In this order:
/// 1. A USB device with a serial number is found by its VID, PID, serial number
///    and interface, whatever its port is called now. If it is not plugged in,
///    nothing matches, even if another device now has its old name.
/// 2. A USB device without a serial number (many CH340s) is found by VID, PID
///    and interface: the one in the same USB socket, else the one with the same
///    name, else the only one there is.
/// 3. A port that is not USB, such as /dev/ttyS0, is found by its name.
pub fn find_port<'a>(wanted: &PortId, ports: &'a [PortInfo]) -> Option<&'a PortInfo> {
    let Some(usb) = &wanted.usb else {
        return ports.iter().find(|port| port.name == wanted.name);
    };

    let same_kind: Vec<&PortInfo> = ports
        .iter()
        .filter(|port| {
            port.usb
                .as_ref()
                .is_some_and(|other| same_device_kind(usb, other))
        })
        .collect();

    if let Some(serial) = &usb.serial {
        return same_kind
            .into_iter()
            .find(|port| port.usb.as_ref().and_then(|u| u.serial.as_ref()) == Some(serial));
    }

    let same_socket = usb.location.as_ref().and_then(|location| {
        same_kind
            .iter()
            .find(|port| port.usb.as_ref().and_then(|u| u.location.as_ref()) == Some(location))
    });
    let same_name = || same_kind.iter().find(|port| port.name == wanted.name);
    let only_one = || match same_kind.as_slice() {
        [port] => Some(port),
        _ => None,
    };
    same_socket.or_else(same_name).or_else(only_one).copied()
}

/// Same VID and PID, and the same interface when both are known.
fn same_device_kind(a: &UsbId, b: &UsbId) -> bool {
    let interface_matches = match (a.interface, b.interface) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    };
    a.vid == b.vid && a.pid == b.pid && interface_matches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usb(
        name: &str,
        vid: u16,
        pid: u16,
        serial: Option<&str>,
        location: Option<&str>,
    ) -> PortInfo {
        PortInfo {
            name: name.into(),
            usb: Some(UsbId {
                vid,
                pid,
                serial: serial.map(String::from),
                interface: Some(1),
                location: location.map(String::from),
            }),
            manufacturer: None,
            product: None,
        }
    }

    fn plain(name: &str) -> PortInfo {
        PortInfo {
            name: name.into(),
            usb: None,
            manufacturer: None,
            product: None,
        }
    }

    const STLINK: (u16, u16) = (0x0483, 0x3754);
    const CH340: (u16, u16) = (0x1a86, 0x7523);

    #[test]
    fn device_with_serial_is_found_under_its_new_name() {
        let before = PortId::from(&usb("COM6", STLINK.0, STLINK.1, Some("AAA"), None));
        let now = [
            usb("COM3", CH340.0, CH340.1, None, None),
            usb("COM7", STLINK.0, STLINK.1, Some("AAA"), None),
        ];
        assert_eq!(find_port(&before, &now).unwrap().name, "COM7");
    }

    #[test]
    fn device_with_serial_does_not_match_another_board_of_the_same_kind() {
        let before = PortId::from(&usb("COM6", STLINK.0, STLINK.1, Some("AAA"), None));
        let now = [usb("COM6", STLINK.0, STLINK.1, Some("BBB"), None)];
        assert_eq!(find_port(&before, &now), None);
    }

    #[test]
    fn interface_tells_two_ports_of_one_device_apart() {
        let mut first = usb("COM8", STLINK.0, STLINK.1, Some("AAA"), None);
        let mut second = usb("COM9", STLINK.0, STLINK.1, Some("AAA"), None);
        first.usb.as_mut().unwrap().interface = Some(0);
        second.usb.as_mut().unwrap().interface = Some(2);
        let before = PortId::from(&second);
        second.name = "COM12".into();
        assert_eq!(find_port(&before, &[first, second]).unwrap().name, "COM12");
    }

    #[test]
    fn device_without_serial_is_found_in_the_same_socket() {
        let before = PortId::from(&usb("/dev/ttyUSB0", CH340.0, CH340.1, None, Some("1-2.1")));
        let now = [
            usb("/dev/ttyUSB0", CH340.0, CH340.1, None, Some("1-2.3")),
            usb("/dev/ttyUSB1", CH340.0, CH340.1, None, Some("1-2.1")),
        ];
        assert_eq!(find_port(&before, &now).unwrap().name, "/dev/ttyUSB1");
    }

    #[test]
    fn device_without_serial_falls_back_to_its_name() {
        let before = PortId::from(&usb("COM4", CH340.0, CH340.1, None, None));
        let now = [
            usb("COM3", CH340.0, CH340.1, None, None),
            usb("COM4", CH340.0, CH340.1, None, None),
        ];
        assert_eq!(find_port(&before, &now).unwrap().name, "COM4");
    }

    #[test]
    fn device_without_serial_is_found_when_it_is_the_only_one() {
        let before = PortId::from(&usb("COM4", CH340.0, CH340.1, None, Some("1-1")));
        let now = [
            plain("COM1"),
            usb("COM9", CH340.0, CH340.1, None, Some("1-5")),
        ];
        assert_eq!(find_port(&before, &now).unwrap().name, "COM9");
    }

    #[test]
    fn two_unknown_devices_without_serial_match_nothing() {
        let before = PortId::from(&usb("COM4", CH340.0, CH340.1, None, None));
        let now = [
            usb("COM8", CH340.0, CH340.1, None, None),
            usb("COM9", CH340.0, CH340.1, None, None),
        ];
        assert_eq!(find_port(&before, &now), None);
    }

    #[test]
    fn port_that_is_not_usb_is_found_by_name() {
        let before = PortId::from(&plain("/dev/ttyS0"));
        let now = [plain("/dev/ttyS1"), plain("/dev/ttyS0")];
        assert_eq!(find_port(&before, &now).unwrap().name, "/dev/ttyS0");
        assert_eq!(find_port(&before, &[plain("/dev/ttyS1")]), None);
    }
}
