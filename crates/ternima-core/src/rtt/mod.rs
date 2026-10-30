//! SEGGER RTT through a debug probe: CMSIS-DAP, J-Link, ST-Link and the others
//! probe-rs supports.

mod config;
mod link;
mod session;

pub use config::RttConfig;
pub use link::{Attached, ChannelInfo, LinkProblem, ProbeRs, RttBackend, RttLink};
pub use session::{Event, RttState, Session, Watched};
