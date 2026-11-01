//! SSH sessions: a shell on a server or a Linux board, in a real terminal.
//!
//! The server's key is checked against OpenSSH's own `known_hosts`, and what
//! is needed to log in comes from a key file, the SSH agent, or a password
//! asked each time. A password or a passphrase is never kept.

mod config;
mod known_hosts;
mod session;

pub use config::{Auth, SshConfig, usual_key_file};
pub use session::{Answer, Event, Question, Session, SshState};
