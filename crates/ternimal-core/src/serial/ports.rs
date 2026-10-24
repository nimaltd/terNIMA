//! The serial ports on this computer, and what each one is.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

/// What identifies a USB serial device, apart from the name the OS gave its port.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsbId {
    /// Vendor ID, written as four hex digits such as "0483".
    #[serde(with = "crate::hex::u16")]
    pub vid: u16,
    /// Product ID, written as four hex digits such as "3754".
    #[serde(with = "crate::hex::u16")]
    pub pid: u16,
    /// The device's serial number. Many cheap adapters, such as most CH340s, have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    /// Which of the device's ports this is, for devices with more than one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<u8>,
    /// Where the device is plugged in: the bus, then the hub ports, such as "1-2.4".
    /// Not every OS reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

/// One serial port on this computer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PortInfo {
    /// The name the OS gave it: COM6, /dev/ttyACM0, /dev/cu.usbmodem1101.
    pub name: String,
    /// Set for USB ports.
    pub usb: Option<UsbId>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
}

/// Every serial port on this computer, in a natural order: COM2 before COM10.
pub fn list_ports() -> Vec<PortInfo> {
    let mut ports: Vec<PortInfo> = serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .filter(|port| wanted(&port.port_name))
        .map(PortInfo::from)
        .collect();
    ports.sort_by(|a, b| natural_order(&a.name, &b.name));
    ports
}

/// macOS lists every port twice, as /dev/tty.* and /dev/cu.*. The cu one is
/// the one to open, as it does not wait for a modem's carrier signal.
fn wanted(name: &str) -> bool {
    !(cfg!(target_os = "macos") && name.starts_with("/dev/tty."))
}

impl From<serialport::SerialPortInfo> for PortInfo {
    fn from(port: serialport::SerialPortInfo) -> Self {
        let serialport::SerialPortType::UsbPort(usb) = port.port_type else {
            return PortInfo {
                name: port.port_name,
                usb: None,
                manufacturer: None,
                product: None,
            };
        };
        let location = usb.location.map(|location| {
            let chain: Vec<String> = location.port_chain().iter().map(u8::to_string).collect();
            format!("{}-{}", location.bus_id(), chain.join("."))
        });
        PortInfo {
            product: usb
                .product
                .map(|product| without_port_name(&product, &port.port_name)),
            name: port.port_name,
            usb: Some(UsbId {
                vid: usb.vid,
                pid: usb.pid,
                serial: usb.serial_number.filter(|serial| !serial.is_empty()),
                interface: usb.interface,
                location,
            }),
            manufacturer: usb.manufacturer,
        }
    }
}

/// Windows ends a product name with the port, as in "STLink Virtual COM Port (COM6)".
/// The name is shown next to it anyway, and changes when the port does.
fn without_port_name(product: &str, port: &str) -> String {
    product
        .strip_suffix(&format!(" ({port})"))
        .unwrap_or(product)
        .to_string()
}

/// Orders names by their text, then by the number at their end, so COM2 comes before COM10.
fn natural_order(a: &str, b: &str) -> Ordering {
    fn split(name: &str) -> (&str, u64) {
        let text = name.trim_end_matches(|c: char| c.is_ascii_digit());
        (text, name[text.len()..].parse().unwrap_or(0))
    }
    split(a).cmp(&split(b)).then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_sort_by_their_number() {
        let mut names = vec!["COM10", "COM2", "/dev/ttyUSB1", "COM1", "/dev/ttyACM0"];
        names.sort_by(|a, b| natural_order(a, b));
        assert_eq!(
            names,
            ["/dev/ttyACM0", "/dev/ttyUSB1", "COM1", "COM2", "COM10"]
        );
    }

    #[test]
    fn windows_port_suffix_is_removed_from_the_product() {
        assert_eq!(
            without_port_name("STLink Virtual COM Port (COM6)", "COM6"),
            "STLink Virtual COM Port"
        );
        assert_eq!(
            without_port_name("CP2102 USB to UART", "COM3"),
            "CP2102 USB to UART"
        );
    }

    #[test]
    fn usb_ids_are_hex_text() {
        let id = UsbId {
            vid: 0x0483,
            pid: 0x3754,
            serial: Some("0036".into()),
            interface: Some(1),
            location: None,
        };
        let text = toml::to_string(&id).unwrap();
        assert!(text.contains("vid = \"0483\""), "{text}");
        assert!(text.contains("pid = \"3754\""), "{text}");
        assert!(!text.contains("location"), "{text}");
        assert_eq!(toml::from_str::<UsbId>(&text).unwrap(), id);
    }
}
