//! How a serial port is set up: speed, frame and flow control.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Parity {
    #[default]
    None,
    Odd,
    Even,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FlowControl {
    #[default]
    None,
    /// Hardware flow control on the RTS and CTS lines.
    RtsCts,
    /// Software flow control with the XON and XOFF characters.
    XonXoff,
}

/// Everything about how a port is opened. A value missing from a file takes its default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SerialConfig {
    pub baud: u32,
    /// 5, 6, 7 or 8.
    pub data_bits: u8,
    pub parity: Parity,
    /// 1 or 2.
    pub stop_bits: u8,
    pub flow: FlowControl,
    /// The DTR line's state. Some boards use it, or RTS, to reset or to enter their bootloader.
    pub dtr: bool,
    /// The RTS line's state, when flow control is not using it.
    pub rts: bool,
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            baud: 115_200,
            data_bits: 8,
            parity: Parity::None,
            stop_bits: 1,
            flow: FlowControl::None,
            dtr: true,
            rts: true,
        }
    }
}

impl SerialConfig {
    /// The usual short form, such as "115200 8N1".
    pub fn summary(&self) -> String {
        let parity = match self.parity {
            Parity::None => 'N',
            Parity::Odd => 'O',
            Parity::Even => 'E',
        };
        format!("{} {}{parity}{}", self.baud, self.data_bits, self.stop_bits)
    }

    /// Says what is wrong, if a value is one no port can use.
    pub fn problem(&self) -> Option<String> {
        if self.baud == 0 {
            return Some("The baud rate must be more than 0.".into());
        }
        if !(5..=8).contains(&self.data_bits) {
            return Some(format!(
                "{} data bits is not possible: use 5, 6, 7 or 8.",
                self.data_bits
            ));
        }
        if !(1..=2).contains(&self.stop_bits) {
            return Some(format!(
                "{} stop bits is not possible: use 1 or 2.",
                self.stop_bits
            ));
        }
        None
    }

    pub(crate) fn builder(&self, name: &str) -> serialport::SerialPortBuilder {
        let data_bits = match self.data_bits {
            5 => serialport::DataBits::Five,
            6 => serialport::DataBits::Six,
            7 => serialport::DataBits::Seven,
            _ => serialport::DataBits::Eight,
        };
        let parity = match self.parity {
            Parity::None => serialport::Parity::None,
            Parity::Odd => serialport::Parity::Odd,
            Parity::Even => serialport::Parity::Even,
        };
        let stop_bits = match self.stop_bits {
            2 => serialport::StopBits::Two,
            _ => serialport::StopBits::One,
        };
        let flow = match self.flow {
            FlowControl::None => serialport::FlowControl::None,
            FlowControl::RtsCts => serialport::FlowControl::Hardware,
            FlowControl::XonXoff => serialport::FlowControl::Software,
        };
        serialport::new(name, self.baud)
            .data_bits(data_bits)
            .parity(parity)
            .stop_bits(stop_bits)
            .flow_control(flow)
            // Opening sets DTR the way the session wants it, so a board that
            // resets on DTR does not reset just because the port opened.
            .dtr_on_open(self.dtr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_is_the_usual_short_form() {
        assert_eq!(SerialConfig::default().summary(), "115200 8N1");
        let config = SerialConfig {
            baud: 9600,
            data_bits: 7,
            parity: Parity::Even,
            stop_bits: 2,
            ..SerialConfig::default()
        };
        assert_eq!(config.summary(), "9600 7E2");
    }

    #[test]
    fn impossible_values_are_reported() {
        assert_eq!(SerialConfig::default().problem(), None);
        let bad_bits = SerialConfig {
            data_bits: 9,
            ..SerialConfig::default()
        };
        assert!(bad_bits.problem().unwrap().contains("9 data bits"));
        let bad_stop = SerialConfig {
            stop_bits: 3,
            ..SerialConfig::default()
        };
        assert!(bad_stop.problem().unwrap().contains("3 stop bits"));
    }

    #[test]
    fn written_with_readable_names() {
        let config = SerialConfig {
            flow: FlowControl::RtsCts,
            ..SerialConfig::default()
        };
        let text = toml::to_string(&config).unwrap();
        assert!(text.contains("flow = \"rtscts\""), "{text}");
        assert!(text.contains("parity = \"none\""), "{text}");
        assert_eq!(toml::from_str::<SerialConfig>(&text).unwrap(), config);
    }
}
