//! What is open in the window, kept in `workspace.toml`, so the app opens the
//! way it was left: the same tabs, connected again to the same devices.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::rtt::RttConfig;
use crate::serial::{PortId, SerialConfig};
use crate::swo::SwoConfig;
use crate::tomlfile;

const HEADER: &str =
    "# ternimal workspace: the tabs that were open. ternimal writes it on every change.\n\n";

/// What the Enter key sends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineEnding {
    #[default]
    Cr,
    Lf,
    CrLf,
}

/// How the terminal treats what is typed and received. Missing values take their defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalOptions {
    pub enter: LineEnding,
    /// Shows what is typed, for devices that do not echo it back.
    pub local_echo: bool,
    /// Starts a new line on a received LF alone, for devices that send LF without CR.
    pub lf_is_newline: bool,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            enter: LineEnding::Cr,
            local_echo: false,
            lf_is_newline: true,
        }
    }
}

/// What a tab is connected to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    #[default]
    Serial,
    Rtt,
    Swo,
}

/// One tab: a session and how it is set up. Only the settings of its kind
/// are written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSetup {
    pub name: String,
    #[serde(default)]
    pub kind: SessionKind,
    /// Whether it was connected when the app closed. If so, it connects again at start.
    #[serde(default)]
    pub connected: bool,
    /// Whether what it receives is written to a log file.
    #[serde(default)]
    pub log: bool,
    #[serde(default, skip_serializing_if = "is_default")]
    pub port: PortId,
    #[serde(default, skip_serializing_if = "is_default")]
    pub serial: SerialConfig,
    #[serde(default, skip_serializing_if = "is_default")]
    pub rtt: RttConfig,
    #[serde(default, skip_serializing_if = "is_default")]
    pub swo: SwoConfig,
    #[serde(default)]
    pub terminal: TerminalOptions,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// Everything in `workspace.toml`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    /// The tab in front, counted from 0.
    pub active: usize,
    pub sessions: Vec<SessionSetup>,
}

impl Workspace {
    /// Reads the workspace from `path`; see [`tomlfile::load`] for missing and broken files.
    pub fn load(path: &Path) -> Self {
        tomlfile::load::<Workspace>(path).checked()
    }

    /// Writes the workspace to `path`, safely; see [`tomlfile::save`].
    pub fn save(&self, path: &Path) -> io::Result<()> {
        tomlfile::save(self, path, HEADER)
    }

    /// The same workspace with the tab in front pointing at a tab that exists.
    pub fn checked(mut self) -> Self {
        self.active = self.active.min(self.sessions.len().saturating_sub(1));
        self
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::serial::UsbId;
    use crate::tomlfile::tests::TempDir;

    fn nucleo() -> SessionSetup {
        SessionSetup {
            name: "Nucleo".into(),
            kind: SessionKind::Serial,
            connected: true,
            log: true,
            port: PortId {
                name: "COM6".into(),
                usb: Some(UsbId {
                    vid: 0x0483,
                    pid: 0x3754,
                    serial: Some("003600333132511238363431".into()),
                    interface: Some(1),
                    location: None,
                }),
            },
            serial: SerialConfig {
                baud: 921_600,
                ..SerialConfig::default()
            },
            rtt: RttConfig::default(),
            swo: SwoConfig::default(),
            terminal: TerminalOptions::default(),
        }
    }

    fn rtt_tab() -> SessionSetup {
        SessionSetup {
            name: "H503 RTT".into(),
            kind: SessionKind::Rtt,
            connected: true,
            log: false,
            port: PortId::default(),
            serial: SerialConfig::default(),
            rtt: RttConfig {
                chip: "STM32H503RB".into(),
                address: Some(0x2000_0400),
                ..RttConfig::default()
            },
            swo: SwoConfig::default(),
            terminal: TerminalOptions::default(),
        }
    }

    #[test]
    fn each_kind_writes_only_its_own_settings() {
        let dir = TempDir::new("workspace-kinds");
        let path = dir.file("workspace.toml");
        let workspace = Workspace {
            active: 0,
            sessions: vec![nucleo(), rtt_tab()],
        };
        workspace.save(&path).unwrap();
        assert_eq!(Workspace::load(&path), workspace);

        let text = fs::read_to_string(&path).unwrap();
        let (serial, rtt) = text.split_at(text.find("H503 RTT").unwrap());
        assert!(
            serial.contains("baud = 921600") && !serial.contains("chip ="),
            "{text}"
        );
        assert!(
            rtt.contains("kind = \"rtt\"") && rtt.contains("chip = \"STM32H503RB\""),
            "{text}"
        );
        assert!(!rtt.contains("baud"), "{text}");
    }

    #[test]
    fn saved_workspace_loads_back_the_same() {
        let dir = TempDir::new("workspace-round-trip");
        let path = dir.file("workspace.toml");
        let workspace = Workspace {
            active: 1,
            sessions: vec![
                nucleo(),
                SessionSetup {
                    name: "Debug UART".into(),
                    connected: false,
                    ..nucleo()
                },
            ],
        };
        workspace.save(&path).unwrap();
        assert_eq!(Workspace::load(&path), workspace);

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("[[sessions]]"), "{text}");
        assert!(text.contains("vid = \"0483\""), "{text}");
    }

    #[test]
    fn missing_file_gives_an_empty_workspace() {
        let dir = TempDir::new("workspace-missing");
        assert_eq!(
            Workspace::load(&dir.file("workspace.toml")),
            Workspace::default()
        );
    }

    #[test]
    fn short_session_takes_the_defaults() {
        let dir = TempDir::new("workspace-short");
        let path = dir.file("workspace.toml");
        fs::write(
            &path,
            "[[sessions]]\nname = \"Board\"\nport = { name = \"/dev/ttyUSB0\" }\n",
        )
        .unwrap();
        let workspace = Workspace::load(&path);
        let session = &workspace.sessions[0];
        assert_eq!(session.port.name, "/dev/ttyUSB0");
        assert_eq!(session.serial, SerialConfig::default());
        assert_eq!(session.terminal.enter, LineEnding::Cr);
        assert!(!session.connected);
    }

    #[test]
    fn tab_in_front_always_exists() {
        let workspace = Workspace {
            active: 7,
            sessions: vec![nucleo()],
        };
        assert_eq!(workspace.checked().active, 0);
        assert_eq!(
            Workspace {
                active: 3,
                sessions: vec![]
            }
            .checked()
            .active,
            0
        );
    }
}
