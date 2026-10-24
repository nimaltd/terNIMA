//! Everything in ternimal that does not draw on the screen.
//!
//! Transports, sessions, config and parsers live here, apart from the window, so
//! they can be tested on their own.

mod banner;
pub mod settings;

pub use banner::{Background, welcome_banner};

/// The version of ternimal, from the workspace's `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
