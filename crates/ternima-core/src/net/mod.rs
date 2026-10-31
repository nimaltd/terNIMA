//! TCP sessions: plain sockets, Telnet, and the RTT servers of J-Link's
//! software and OpenOCD, for when a debugger already has the probe.

mod config;
mod session;
mod telnet;

pub use config::{Protocol, TcpConfig};
pub(crate) use session::addresses;
pub use session::{Event, NetState, Session};
