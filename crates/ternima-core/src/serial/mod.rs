//! Serial ports: listing them, finding the same one again, and keeping one open.

mod config;
mod matching;
mod ports;
mod session;
mod system;

pub use config::{FlowControl, Parity, SerialConfig};
pub use matching::{PortId, find_port};
pub use ports::{PortInfo, UsbId, list_ports};
pub use session::{Event, ModemLines, Session, SessionState};
pub use system::{SerialLink, SerialSystem, SystemPorts};
