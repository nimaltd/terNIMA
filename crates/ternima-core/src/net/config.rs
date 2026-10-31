//! Where a TCP session connects, and what speaks at the other end.

use serde::{Deserialize, Serialize};

use crate::openocd::TCL_PORT;

/// What is at the other end of the connection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    /// Plain bytes both ways, as from a serial to network bridge or a
    /// device's own socket.
    #[default]
    Raw,
    /// Telnet, as a device's shell or a terminal server speaks it: its option
    /// talk is answered and kept out of the terminal.
    Telnet,
    /// The RTT server J-Link's software opens while it is connected to a chip:
    /// J-Link Commander, GDB Server, Ozone, RTT Viewer or a debugger in an IDE.
    Jlink,
    /// The RTT server OpenOCD opens with `rtt server start`.
    Openocd,
}

impl Protocol {
    /// The port the other end usually listens on, if there is a usual one.
    pub fn usual_port(self) -> Option<u16> {
        match self {
            Protocol::Raw => None,
            Protocol::Telnet => Some(23),
            Protocol::Jlink => Some(19021),
            Protocol::Openocd => Some(9090),
        }
    }

    /// The protocol's name for people.
    pub fn name(self) -> &'static str {
        match self {
            Protocol::Raw => "TCP",
            Protocol::Telnet => "Telnet",
            Protocol::Jlink => "J-Link RTT",
            Protocol::Openocd => "OpenOCD RTT",
        }
    }
}

/// Everything about a TCP session. A value missing from a file takes its default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TcpConfig {
    /// A host name or an IP address.
    pub host: String,
    pub port: u16,
    pub protocol: Protocol,
    /// The RTT channel J-Link's server uses both ways. Channel 0 is J-Link's
    /// own choice, so nothing is asked for then. The other protocols leave it alone.
    #[serde(skip_serializing_if = "is_zero")]
    pub channel: usize,
    /// Where OpenOCD's Tcl server listens on `host`, through which the Watch
    /// view reads the variables. The other protocols leave it alone.
    #[serde(skip_serializing_if = "is_tcl_port")]
    pub tcl_port: u16,
    /// The firmware's ELF file, where the Watch view finds its variables.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elf: Option<String>,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

fn is_tcl_port(port: &u16) -> bool {
    *port == TCL_PORT
}

impl Default for TcpConfig {
    fn default() -> Self {
        Self {
            host: "localhost".into(),
            port: 0,
            protocol: Protocol::Raw,
            channel: 0,
            tcl_port: TCL_PORT,
            elf: None,
        }
    }
}

impl TcpConfig {
    /// Says what is wrong, if the session cannot start as it is.
    pub fn problem(&self) -> Option<String> {
        if self.host.trim().is_empty() {
            return Some("Enter the host: a name or an IP address, such as localhost.".into());
        }
        if self.port == 0 {
            return Some("Enter the port: a number from 1 to 65535.".into());
        }
        None
    }

    /// "localhost:19021", or "[::1]:23" for an IPv6 address.
    pub fn address(&self) -> String {
        let host = self.host.trim();
        if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]:{}", self.port)
        } else {
            format!("{host}:{}", self.port)
        }
    }

    /// What J-Link's RTT server is told right after connecting, if anything.
    /// It takes this within its first 100 ms to pick an RTT channel.
    pub fn greeting(&self) -> Option<Vec<u8>> {
        (self.protocol == Protocol::Jlink && self.channel != 0)
            .then(|| format!("$$SEGGER_TELNET_ConfigStr=RTTCh;{}$$", self.channel).into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jlink(channel: usize) -> TcpConfig {
        TcpConfig {
            port: 19021,
            protocol: Protocol::Jlink,
            channel,
            ..TcpConfig::default()
        }
    }

    #[test]
    fn written_with_lowercase_protocol() {
        let config = jlink(1);
        let text = toml::to_string(&config).unwrap();
        assert!(text.contains("protocol = \"jlink\""), "{text}");
        assert_eq!(toml::from_str::<TcpConfig>(&text).unwrap(), config);
    }

    #[test]
    fn host_and_port_are_needed() {
        assert!(TcpConfig::default().problem().is_some());
        let no_host = TcpConfig {
            host: " ".into(),
            ..jlink(0)
        };
        assert!(no_host.problem().is_some());
        assert_eq!(jlink(0).problem(), None);
    }

    #[test]
    fn address_puts_ipv6_in_brackets() {
        assert_eq!(jlink(0).address(), "localhost:19021");
        let v6 = TcpConfig {
            host: "::1".into(),
            port: 23,
            ..TcpConfig::default()
        };
        assert_eq!(v6.address(), "[::1]:23");
    }

    #[test]
    fn only_jlink_on_another_channel_is_greeted() {
        assert_eq!(jlink(0).greeting(), None);
        assert_eq!(
            jlink(2).greeting().unwrap(),
            b"$$SEGGER_TELNET_ConfigStr=RTTCh;2$$"
        );
        let telnet = TcpConfig {
            protocol: Protocol::Telnet,
            channel: 2,
            ..jlink(0)
        };
        assert_eq!(telnet.greeting(), None);
    }

    #[test]
    fn usual_ports() {
        assert_eq!(Protocol::Jlink.usual_port(), Some(19021));
        assert_eq!(Protocol::Raw.usual_port(), None);
    }
}
