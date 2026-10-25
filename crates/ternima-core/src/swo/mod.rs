//! SWO: what the chip writes to its ITM stimulus ports, read from the trace
//! pin through a debug probe.

mod config;
mod itm;
mod session;

pub use config::SwoConfig;
pub use itm::{ItmDecoder, ItmPacket};
pub use session::{Event, ProbeRs, Session, SwoBackend, SwoLink, SwoState};
