//! Everything in ternimal that does not draw on the screen.
//!
//! Transports, sessions, config and parsers live here, apart from the window, so
//! they can be tested on their own.

mod banner;
pub mod detect;
mod hex;
pub mod logfile;
pub mod probe;
pub mod rtt;
pub mod serial;
pub mod settings;
pub mod swo;
mod tomlfile;
pub mod workspace;

pub use banner::{Background, welcome_banner};

/// The version of ternimal, from the workspace's `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
