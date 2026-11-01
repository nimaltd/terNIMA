//! Where an SSH session connects, and how it logs in.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::net::host_and_port;

/// The port SSH servers listen on, unless told otherwise.
pub const SSH_PORT: u16 = 22;

/// How a session logs in. A password is asked each time, so it is not here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Auth {
    /// A password, or what the server asks instead, asked each time and never saved.
    #[default]
    Password,
    /// A private key file. A passphrase it has is asked each time and never saved.
    Key,
    /// The keys in the SSH agent.
    Agent,
}

/// Everything about an SSH session. A value missing from a file takes its default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SshConfig {
    /// A host name or an IP address.
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: Auth,
    /// The private key file, for [`Auth::Key`].
    #[serde(skip_serializing_if = "String::is_empty")]
    pub key_file: String,
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: SSH_PORT,
            user: String::new(),
            auth: Auth::default(),
            key_file: String::new(),
        }
    }
}

impl SshConfig {
    /// Says what is wrong, if the session cannot start as it is.
    pub fn problem(&self) -> Option<String> {
        if self.host.trim().is_empty() {
            return Some("Enter the host: a name or an IP address.".into());
        }
        if self.port == 0 {
            return Some("Enter the port: a number from 1 to 65535, usually 22.".into());
        }
        if self.user.trim().is_empty() {
            return Some("Enter the user to log in as, such as root.".into());
        }
        if self.auth == Auth::Key && self.key_file.trim().is_empty() {
            return Some("Choose the private key file to log in with.".into());
        }
        None
    }

    /// The host as it is looked up and as `known_hosts` has it: without the
    /// brackets of an IPv6 address, in small letters.
    pub(super) fn host_name(&self) -> String {
        self.host
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_ascii_lowercase()
    }

    /// "board:22", or "[::1]:22" for an IPv6 address.
    pub fn address(&self) -> String {
        host_and_port(&self.host, self.port)
    }

    /// "root@board:22"
    pub fn target(&self) -> String {
        format!("{}@{}", self.user.trim(), self.address())
    }

    /// The key file's path, with `~/` at its start meaning the home folder.
    pub(super) fn key_path(&self, home: &Path) -> PathBuf {
        let file = self.key_file.trim();
        match file.strip_prefix("~/").or(file.strip_prefix("~\\")) {
            Some(rest) => home.join(rest),
            None => PathBuf::from(file),
        }
    }
}

/// The key files OpenSSH looks for, in its order.
const USUAL_KEY_FILES: [&str; 3] = ["id_ed25519", "id_ecdsa", "id_rsa"];

/// The private key OpenSSH would use first, if one is in the home folder's `.ssh`.
pub fn usual_key_file() -> Option<String> {
    let folder = std::env::home_dir()?.join(".ssh");
    USUAL_KEY_FILES
        .iter()
        .map(|name| folder.join(name))
        .find(|path| path.is_file())
        .map(|path| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> SshConfig {
        SshConfig {
            host: "board".into(),
            user: "root".into(),
            ..SshConfig::default()
        }
    }

    #[test]
    fn host_user_and_key_file_are_needed() {
        assert_eq!(board().problem(), None);
        let mut no_host = board();
        no_host.host = " ".into();
        assert!(no_host.problem().unwrap().contains("host"));
        let mut no_user = board();
        no_user.user = String::new();
        assert!(no_user.problem().unwrap().contains("user"));
        let mut no_key = board();
        no_key.auth = Auth::Key;
        assert!(no_key.problem().unwrap().contains("key file"));
        no_key.key_file = "id_ed25519".into();
        assert_eq!(no_key.problem(), None);
    }

    #[test]
    fn says_where_it_connects() {
        assert_eq!(board().address(), "board:22");
        assert_eq!(board().target(), "root@board:22");
        let v6 = SshConfig {
            host: "::1".into(),
            port: 2222,
            ..board()
        };
        assert_eq!(v6.address(), "[::1]:2222");
        assert_eq!(v6.target(), "root@[::1]:2222");
    }

    #[test]
    fn known_hosts_has_the_host_without_brackets_in_small_letters() {
        let host = SshConfig {
            host: " [FE80::1] ".into(),
            ..board()
        };
        assert_eq!(host.host_name(), "fe80::1");
        let name = SshConfig {
            host: "Board.Local".into(),
            ..board()
        };
        assert_eq!(name.host_name(), "board.local");
    }

    #[test]
    fn a_key_file_can_start_at_the_home_folder() {
        let home = Path::new("/home/nima");
        let key = |file: &str| SshConfig {
            key_file: file.into(),
            ..board()
        };
        assert_eq!(key("~/.ssh/board").key_path(home), home.join(".ssh/board"));
        assert_eq!(key("/keys/board").key_path(home), Path::new("/keys/board"));
    }

    #[test]
    fn a_password_has_no_place_in_the_file() {
        let config = SshConfig {
            auth: Auth::Agent,
            ..board()
        };
        let text = toml::to_string(&config).unwrap();
        assert!(text.contains("auth = \"agent\""), "{text}");
        assert!(!text.contains("pass"), "{text}");
        assert_eq!(toml::from_str::<SshConfig>(&text).unwrap(), config);
    }
}
