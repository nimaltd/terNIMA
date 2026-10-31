//! How an RTT session finds its probe, its chip and its control block.

use serde::{Deserialize, Serialize};

use crate::probe::ProbeId;

/// Everything about an RTT session. A value missing from a file takes its default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RttConfig {
    /// The probe to use. Without one, the only probe plugged in is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe: Option<ProbeId>,
    /// The chip, as probe-rs names it, such as "STM32H503RB".
    pub chip: String,
    /// The SWD clock in kHz, or 0 for the probe's own default.
    pub speed_khz: u32,
    /// Holds the chip in reset while attaching, for firmware that sleeps or
    /// turns the debug pins off. This resets the chip.
    pub under_reset: bool,
    /// The firmware's ELF file, to find the control block from its
    /// `_SEGGER_RTT` symbol, which is quicker and exact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elf: Option<String>,
    /// Where the control block is, if known. Without it and without an ELF
    /// file, RAM is searched.
    #[serde(with = "crate::hex::address", skip_serializing_if = "Option::is_none")]
    pub address: Option<u64>,
    /// The channel shown in the terminal. Channel 0 is usually the terminal.
    pub up_channel: usize,
    /// The channel that gets what is typed.
    pub down_channel: usize,
    /// Only the watched variables are read, for firmware without RTT: the
    /// control block is not looked for.
    #[serde(skip_serializing_if = "is_false")]
    pub watch_only: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl RttConfig {
    /// Says what is wrong, if the session cannot start as it is.
    pub fn problem(&self) -> Option<String> {
        if self.chip.trim().is_empty() {
            return Some("Choose the chip on the board.".into());
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_with_hex_ids_and_address() {
        let config = RttConfig {
            probe: Some(ProbeId {
                vid: 0x0483,
                pid: 0x3754,
                serial: Some("0036".into()),
            }),
            chip: "STM32H503RB".into(),
            address: Some(0x2000_0400),
            ..RttConfig::default()
        };
        let text = toml::to_string(&config).unwrap();
        assert!(text.contains("address = \"0x20000400\""), "{text}");
        assert!(text.contains("vid = \"0483\""), "{text}");
        assert!(!text.contains("elf"), "{text}");
        // Off, as it is for RTT, it is left out.
        assert!(!text.contains("watch_only"), "{text}");
        assert_eq!(toml::from_str::<RttConfig>(&text).unwrap(), config);

        let watching = RttConfig {
            watch_only: true,
            ..config
        };
        let text = toml::to_string(&watching).unwrap();
        assert!(text.contains("watch_only = true"), "{text}");
        assert_eq!(toml::from_str::<RttConfig>(&text).unwrap(), watching);
    }

    #[test]
    fn empty_address_means_none() {
        let config: RttConfig = toml::from_str("chip = \"X\"\naddress = \"\"\n").unwrap();
        assert_eq!(config.address, None);
    }

    #[test]
    fn chip_is_needed() {
        assert!(RttConfig::default().problem().is_some());
        let config = RttConfig {
            chip: "STM32H503RB".into(),
            ..RttConfig::default()
        };
        assert_eq!(config.problem(), None);
    }
}
