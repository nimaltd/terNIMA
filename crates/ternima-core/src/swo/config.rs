//! How an SWO session reaches its chip, and how fast the trace pin runs.

use serde::{Deserialize, Serialize};

use crate::probe::ProbeId;

/// Everything about an SWO session. A value missing from a file takes its default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SwoConfig {
    /// The probe to use. Without one, the only probe plugged in is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe: Option<ProbeId>,
    /// The chip, as probe-rs names it, such as "STM32H503RB".
    pub chip: String,
    /// The SWD clock in kHz, or 0 for the probe's own default.
    pub speed_khz: u32,
    /// Holds the chip in reset while attaching. This resets the chip.
    pub under_reset: bool,
    /// The clock of the chip's trace unit in Hz, which is usually the core
    /// clock. The SWO baud rate is divided from it.
    pub clock_hz: u32,
    /// The SWO baud rate.
    pub baud: u32,
    /// The ITM stimulus port shown, 0 to 31. Port 0 is where printf usually goes.
    pub port: u8,
    /// Shows every stimulus port instead of only `port`.
    pub all_ports: bool,
    /// The firmware's ELF file, where the Watch view finds its variables.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elf: Option<String>,
}

impl Default for SwoConfig {
    fn default() -> Self {
        Self {
            probe: None,
            chip: String::new(),
            speed_khz: 0,
            under_reset: false,
            clock_hz: 0,
            baud: 2_000_000,
            port: 0,
            all_ports: false,
            elf: None,
        }
    }
}

impl SwoConfig {
    /// Says what is wrong, if the session cannot start as it is.
    pub fn problem(&self) -> Option<String> {
        if self.chip.trim().is_empty() {
            return Some("Choose the chip on the board.".into());
        }
        if self.clock_hz == 0 {
            return Some(
                "Enter the chip's core clock in Hz: the SWO speed is divided from it.".into(),
            );
        }
        if self.baud == 0 || self.baud > self.clock_hz {
            return Some(format!(
                "The SWO baud rate must be between 1 and the clock, {} Hz.",
                self.clock_hz
            ));
        }
        if !self.clock_hz.is_multiple_of(self.baud) {
            // The fastest rate at or below the one asked for that divides the clock exactly.
            let near = (self.clock_hz.div_ceil(self.baud)..=self.clock_hz)
                .find(|divider| self.clock_hz.is_multiple_of(*divider))
                .map_or(1, |divider| self.clock_hz / divider);
            return Some(format!(
                "{} Hz cannot be divided to {} baud exactly. Try {near} baud.",
                self.clock_hz, self.baud
            ));
        }
        if self.port > 31 {
            return Some("ITM has stimulus ports 0 to 31.".into());
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(clock_hz: u32, baud: u32) -> SwoConfig {
        SwoConfig {
            chip: "STM32H503RB".into(),
            clock_hz,
            baud,
            ..SwoConfig::default()
        }
    }

    #[test]
    fn a_whole_divider_is_fine() {
        assert_eq!(config(32_000_000, 2_000_000).problem(), None);
        assert_eq!(config(64_000_000, 1_000_000).problem(), None);
    }

    #[test]
    fn missing_clock_is_reported() {
        assert!(
            config(0, 2_000_000)
                .problem()
                .unwrap()
                .contains("core clock")
        );
    }

    #[test]
    fn a_baud_rate_that_does_not_divide_gets_a_suggestion() {
        let problem = config(48_000_000, 2_250_000).problem().unwrap();
        assert!(problem.contains("Try 2000000 baud"), "{problem}");
    }

    #[test]
    fn baud_above_the_clock_is_refused() {
        assert!(config(1_000_000, 2_000_000).problem().is_some());
    }
}
