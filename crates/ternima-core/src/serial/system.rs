//! The operating system's serial ports, behind a trait so tests can use pretend ones.

use std::io::{self, Read, Write};
use std::time::Duration;

use super::config::{FlowControl, SerialConfig};
use super::ports::{PortInfo, list_ports};
use super::session::ModemLines;

/// How long one read waits for data before it gives up.
const READ_WAIT: Duration = Duration::from_millis(10);

/// Lists ports and opens them.
pub trait SerialSystem: Send + Sync + 'static {
    fn ports(&self) -> Vec<PortInfo>;
    fn open(&self, name: &str, config: &SerialConfig) -> io::Result<Box<dyn SerialLink>>;
}

/// One open port.
pub trait SerialLink: Send {
    /// Reads what has arrived, waiting a short while. Gives `ErrorKind::TimedOut`
    /// when nothing came, and any other error when the port is gone.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    fn write_all(&mut self, data: &[u8]) -> io::Result<()>;
    fn set_dtr(&mut self, on: bool) -> io::Result<()>;
    fn set_rts(&mut self, on: bool) -> io::Result<()>;
    fn set_break(&mut self, on: bool) -> io::Result<()>;
    fn modem_lines(&mut self) -> io::Result<ModemLines>;
}

/// The real serial ports of this computer.
pub struct SystemPorts;

impl SerialSystem for SystemPorts {
    fn ports(&self) -> Vec<PortInfo> {
        list_ports()
    }

    fn open(&self, name: &str, config: &SerialConfig) -> io::Result<Box<dyn SerialLink>> {
        let mut port = config
            .builder(name)
            .timeout(READ_WAIT)
            .open()
            .map_err(io::Error::from)?;
        // With RTS/CTS flow control the driver owns RTS.
        if config.flow != FlowControl::RtsCts {
            port.write_request_to_send(config.rts)?;
        }
        Ok(Box::new(SystemLink(port)))
    }
}

struct SystemLink(Box<dyn serialport::SerialPort>);

impl SerialLink for SystemLink {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }

    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        self.0.write_all(data)?;
        self.0.flush()
    }

    fn set_dtr(&mut self, on: bool) -> io::Result<()> {
        Ok(self.0.write_data_terminal_ready(on)?)
    }

    fn set_rts(&mut self, on: bool) -> io::Result<()> {
        Ok(self.0.write_request_to_send(on)?)
    }

    fn set_break(&mut self, on: bool) -> io::Result<()> {
        if on {
            Ok(self.0.set_break()?)
        } else {
            Ok(self.0.clear_break()?)
        }
    }

    fn modem_lines(&mut self) -> io::Result<ModemLines> {
        Ok(ModemLines {
            cts: self.0.read_clear_to_send()?,
            dsr: self.0.read_data_set_ready()?,
            dcd: self.0.read_carrier_detect()?,
            ri: self.0.read_ring_indicator()?,
        })
    }
}
