//! Everything in ternima that does not draw on the screen.
//!
//! Transports, sessions, config and parsers live here, apart from the window, so
//! they can be tested on their own.

mod ansi;
mod banner;
pub mod detect;
pub mod elf;
mod hex;
pub mod lines;
pub mod logfile;
pub mod net;
pub mod openocd;
pub mod probe;
pub mod record;
pub mod replay;
pub mod rtt;
pub mod send;
pub mod serial;
pub mod settings;
pub mod ssh;
pub mod swo;
mod tomlfile;
pub mod watch;
pub mod workspace;

pub use banner::{Background, welcome_banner};

/// The version of ternima, from the workspace's `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
