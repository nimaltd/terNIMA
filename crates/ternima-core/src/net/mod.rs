//! TCP sessions: plain sockets, Telnet, and the RTT servers of J-Link's
//! software and OpenOCD, for when a debugger already has the probe.

mod config;
mod session;
mod telnet;

pub use config::{Protocol, TcpConfig};
pub use session::{Event, NetState, Session};
