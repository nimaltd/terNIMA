//! What is open in the window, kept in `workspace.toml`, so the app opens the
//! way it was left: the same tabs, connected again to the same devices.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::lines::Level;
use crate::net::TcpConfig;
use crate::rtt::RttConfig;
use crate::serial::{PortId, SerialConfig};
use crate::swo::SwoConfig;
use crate::tomlfile;

const HEADER: &str =
    "# ternima workspace: the tabs that were open. ternima writes it on every change.\n\n";

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
    /// Starts a new line when data comes after a pause this long, in ms, so
    /// each message starts on its own line; 0 never does. Only what is shown
    /// changes, in the terminal and in the hex view.
    pub pause_ms: u32,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            enter: LineEnding::Cr,
            local_echo: false,
            lf_is_newline: true,
            pause_ms: 0,
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
    /// A TCP connection: a plain socket, Telnet, or an RTT server.
    Tcp,
}

/// What a tab shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum View {
    /// The terminal, with colors and the cursor moving as the device says.
    #[default]
    Terminal,
    /// Every byte sent and received in hex, 16 to a line.
    Hex,
    /// Every line in a list, with Zephyr's logs read into level and module
    /// for filters, search, highlights and bookmarks.
    Lines,
}

/// What the Lines view leaves out. Its search and bookmarks are not kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LinesView {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hidden_levels: Vec<Level>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hidden_modules: Vec<String>,
    /// Shows the lines that are not log lines, such as the shell's.
    pub others: bool,
}

impl Default for LinesView {
    fn default() -> Self {
        Self {
            hidden_levels: Vec::new(),
            hidden_modules: Vec::new(),
            others: true,
        }
    }
}

/// How the hex view breaks its lines, besides the pause in [`TerminalOptions`].
/// It only changes what is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HexView {
    /// Bytes on a full line: 8, 16 or 32.
    pub width: u8,
    /// A byte that ends its line, such as 0x0A for LF or a frame's end byte.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_byte: Option<u8>,
}

impl Default for HexView {
    fn default() -> Self {
        Self {
            width: 16,
            end_byte: None,
        }
    }
}

/// The line widths the hex view offers.
const HEX_WIDTHS: [u8; 3] = [8, 16, 32];

/// A line sent from the send box, kept so ↑ brings it back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentLine {
    pub text: String,
    /// Whether `text` is hex digits rather than text.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hex: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

/// The most sent lines each tab keeps.
pub const HISTORY_MAX: usize = 100;

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
    #[serde(default, skip_serializing_if = "is_default")]
    pub tcp: TcpConfig,
    #[serde(default)]
    pub terminal: TerminalOptions,
    #[serde(default, skip_serializing_if = "is_default")]
    pub view: View,
    #[serde(default, skip_serializing_if = "is_default")]
    pub hex_view: HexView,
    #[serde(default, skip_serializing_if = "is_default")]
    pub lines_view: LinesView,
    /// The lines sent from the send box, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<SentLine>,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// The smallest window size kept; anything smaller is taken as a mistake.
const WINDOW_MIN: (u32, u32) = (200, 150);

/// Where the window was and how big, in screen pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowState {
    /// The window's top left corner, its frame included.
    pub x: i32,
    pub y: i32,
    /// The size of the window's inside, its frame not included.
    pub width: u32,
    pub height: u32,
    /// Whether it filled the screen. The place and size above are then the
    /// ones it goes back to.
    #[serde(default)]
    pub maximized: bool,
}

/// A rectangle on the screens, in pixels: a screen's usable area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl WindowState {
    /// Whether enough of the window's title bar is on one of the screens to
    /// grab it with the mouse. A screen may have been unplugged since, or set
    /// to another resolution.
    pub fn reachable(&self, screens: &[Area]) -> bool {
        // The top 30 pixels of the window: at least 100 by 20 of them on one screen.
        screens.iter().any(|screen| {
            let left = self.x.max(screen.x);
            let right = (self.x + self.width as i32).min(screen.x + screen.width as i32);
            let top = self.y.max(screen.y);
            let bottom = (self.y + 30).min(screen.y + screen.height as i32);
            right - left >= 100 && bottom - top >= 20
        })
    }
}

/// Where the second pane of a split view goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDirection {
    /// Side by side.
    #[default]
    Right,
    /// One above the other.
    Down,
}

/// Two tabs on screen at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Split {
    /// The tab in the first pane (left or top), counted from 0.
    pub first: usize,
    /// The tab in the second pane (right or bottom).
    pub second: usize,
    #[serde(default)]
    pub direction: SplitDirection,
    /// The first pane's share of the room, in percent.
    #[serde(default = "half")]
    pub ratio: u8,
}

fn half() -> u8 {
    50
}

/// The smallest share either pane of a split view gets, in percent.
const SPLIT_MIN: u8 = 15;

/// Everything in `workspace.toml`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    /// The tab in front, counted from 0. With a split view, it is one of the
    /// two tabs on screen: the one with the keyboard.
    pub active: usize,
    /// The window's place and size when it last moved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowState>,
    /// Two tabs on screen at once, if so.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub split: Option<Split>,
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

    /// The same workspace with the tab in front pointing at a tab that exists,
    /// a split view only of two tabs that exist, no more than [`HISTORY_MAX`]
    /// sent lines a tab, hex lines of a width offered, and without a window
    /// size too small to be real.
    pub fn checked(mut self) -> Self {
        for session in &mut self.sessions {
            let extra = session.history.len().saturating_sub(HISTORY_MAX);
            session.history.drain(..extra);
            if !HEX_WIDTHS.contains(&session.hex_view.width) {
                session.hex_view.width = HexView::default().width;
            }
        }
        self.active = self.active.min(self.sessions.len().saturating_sub(1));
        let count = self.sessions.len();
        self.split = self
            .split
            .filter(|split| {
                split.first < count && split.second < count && split.first != split.second
            })
            .map(|split| Split {
                ratio: split.ratio.clamp(SPLIT_MIN, 100 - SPLIT_MIN),
                ..split
            });
        if let Some(split) = self.split
            && self.active != split.first
            && self.active != split.second
        {
            self.active = split.first;
        }
        self.window = self
            .window
            .filter(|window| window.width >= WINDOW_MIN.0 && window.height >= WINDOW_MIN.1);
        self
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::net::Protocol;
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
            tcp: TcpConfig::default(),
            terminal: TerminalOptions::default(),
            view: View::Terminal,
            hex_view: HexView::default(),
            lines_view: LinesView::default(),
            history: Vec::new(),
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
            tcp: TcpConfig::default(),
            terminal: TerminalOptions::default(),
            view: View::Terminal,
            hex_view: HexView::default(),
            lines_view: LinesView::default(),
            history: Vec::new(),
        }
    }

    fn jlink_tab() -> SessionSetup {
        SessionSetup {
            name: "J-Link RTT".into(),
            kind: SessionKind::Tcp,
            tcp: TcpConfig {
                port: 19021,
                protocol: Protocol::Jlink,
                ..TcpConfig::default()
            },
            rtt: RttConfig::default(),
            ..rtt_tab()
        }
    }

    #[test]
    fn each_kind_writes_only_its_own_settings() {
        let dir = TempDir::new("workspace-kinds");
        let path = dir.file("workspace.toml");
        let workspace = Workspace {
            active: 0,
            window: None,
            split: None,
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
    fn hex_view_and_sent_lines_are_kept() {
        let dir = TempDir::new("workspace-history");
        let path = dir.file("workspace.toml");
        let line = |text: &str, hex| SentLine {
            text: text.into(),
            hex,
        };
        let workspace = Workspace {
            active: 0,
            window: None,
            split: None,
            sessions: vec![SessionSetup {
                view: View::Hex,
                hex_view: HexView {
                    width: 32,
                    end_byte: Some(0x7E),
                },
                terminal: TerminalOptions {
                    pause_ms: 50,
                    ..TerminalOptions::default()
                },
                history: vec![line("help", false), line("AA 55", true)],
                ..nucleo()
            }],
        };
        workspace.save(&path).unwrap();
        assert_eq!(Workspace::load(&path), workspace);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("view = \"hex\""), "{text}");
        assert!(text.contains("end_byte = 126"), "{text}");
        assert!(text.contains("pause_ms = 50"), "{text}");
    }

    #[test]
    fn lines_view_filters_are_kept() {
        let dir = TempDir::new("workspace-lines");
        let path = dir.file("workspace.toml");
        let workspace = Workspace {
            sessions: vec![SessionSetup {
                view: View::Lines,
                lines_view: LinesView {
                    hidden_levels: vec![Level::Dbg],
                    hidden_modules: vec!["net_if".into()],
                    others: false,
                },
                ..nucleo()
            }],
            ..Workspace::default()
        };
        workspace.save(&path).unwrap();
        assert_eq!(Workspace::load(&path), workspace);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("hidden_levels = [\"dbg\"]"), "{text}");
    }

    #[test]
    fn hex_lines_have_a_width_offered() {
        let workspace = Workspace {
            sessions: vec![SessionSetup {
                hex_view: HexView {
                    width: 13,
                    ..HexView::default()
                },
                ..nucleo()
            }],
            ..Workspace::default()
        }
        .checked();
        assert_eq!(workspace.sessions[0].hex_view.width, 16);
    }

    #[test]
    fn only_the_newest_sent_lines_are_kept() {
        let history: Vec<SentLine> = (0..HISTORY_MAX + 5)
            .map(|n| SentLine {
                text: n.to_string(),
                hex: false,
            })
            .collect();
        let workspace = Workspace {
            sessions: vec![SessionSetup {
                history,
                ..nucleo()
            }],
            ..Workspace::default()
        }
        .checked();
        let kept = &workspace.sessions[0].history;
        assert_eq!(kept.len(), HISTORY_MAX);
        assert_eq!(kept[0].text, "5");
    }

    #[test]
    fn tcp_tabs_are_kept_with_their_protocol() {
        let dir = TempDir::new("workspace-tcp");
        let path = dir.file("workspace.toml");
        let workspace = Workspace {
            active: 0,
            window: None,
            split: None,
            sessions: vec![jlink_tab()],
        };
        workspace.save(&path).unwrap();
        assert_eq!(Workspace::load(&path), workspace);

        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("kind = \"tcp\"")
                && text.contains("protocol = \"jlink\"")
                && text.contains("port = 19021"),
            "{text}"
        );
        assert!(!text.contains("chip"), "{text}");
    }

    #[test]
    fn saved_workspace_loads_back_the_same() {
        let dir = TempDir::new("workspace-round-trip");
        let path = dir.file("workspace.toml");
        let workspace = Workspace {
            active: 1,
            window: None,
            split: None,
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
            window: None,
            split: None,
            sessions: vec![nucleo()],
        };
        assert_eq!(workspace.checked().active, 0);
        assert_eq!(
            Workspace {
                active: 3,
                window: None,
                split: None,
                sessions: vec![]
            }
            .checked()
            .active,
            0
        );
    }

    fn window(x: i32, y: i32, width: u32, height: u32) -> WindowState {
        WindowState {
            x,
            y,
            width,
            height,
            maximized: false,
        }
    }

    const LAPTOP: Area = Area {
        x: 0,
        y: 0,
        width: 1920,
        height: 1040,
    };
    const RIGHT_SCREEN: Area = Area {
        x: 1920,
        y: 0,
        width: 2560,
        height: 1400,
    };

    #[test]
    fn window_place_and_size_are_kept() {
        let dir = TempDir::new("workspace-window");
        let path = dir.file("workspace.toml");
        let workspace = Workspace {
            active: 0,
            window: Some(WindowState {
                maximized: true,
                ..window(120, 80, 1200, 760)
            }),
            split: None,
            sessions: vec![nucleo()],
        };
        workspace.save(&path).unwrap();
        assert_eq!(Workspace::load(&path), workspace);
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("[window]") && text.contains("maximized = true"),
            "{text}"
        );
    }

    #[test]
    fn a_window_too_small_to_be_real_is_dropped() {
        let workspace = Workspace {
            window: Some(window(0, 0, 50, 20)),
            ..Workspace::default()
        };
        assert_eq!(workspace.checked().window, None);
    }

    #[test]
    fn a_window_on_a_screen_is_reachable() {
        assert!(window(100, 100, 1200, 760).reachable(&[LAPTOP]));
        assert!(window(2200, 50, 1200, 760).reachable(&[LAPTOP, RIGHT_SCREEN]));
        // Mostly off the left edge, but enough of the title bar is still there.
        assert!(window(-1000, 10, 1200, 760).reachable(&[LAPTOP]));
    }

    #[test]
    fn a_window_on_an_unplugged_screen_is_not_reachable() {
        assert!(!window(2200, 50, 1200, 760).reachable(&[LAPTOP]));
        // Its title bar is above the top of the screen.
        assert!(!window(100, -500, 1200, 760).reachable(&[LAPTOP]));
        assert!(!window(100, 100, 1200, 760).reachable(&[]));
    }

    fn three_tabs(split: Option<Split>, active: usize) -> Workspace {
        Workspace {
            active,
            window: None,
            split,
            sessions: vec![nucleo(), rtt_tab(), nucleo()],
        }
    }

    fn split(first: usize, second: usize, ratio: u8) -> Split {
        Split {
            first,
            second,
            direction: SplitDirection::Down,
            ratio,
        }
    }

    #[test]
    fn split_view_is_kept() {
        let dir = TempDir::new("workspace-split");
        let path = dir.file("workspace.toml");
        let workspace = three_tabs(Some(split(0, 2, 60)), 2);
        workspace.save(&path).unwrap();
        assert_eq!(Workspace::load(&path), workspace);
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("[split]") && text.contains("direction = \"down\""),
            "{text}"
        );
    }

    #[test]
    fn split_view_needs_two_different_tabs_that_exist() {
        assert_eq!(three_tabs(Some(split(0, 3, 50)), 0).checked().split, None);
        assert_eq!(three_tabs(Some(split(1, 1, 50)), 1).checked().split, None);
    }

    #[test]
    fn split_share_stays_usable() {
        let checked = three_tabs(Some(split(0, 1, 2)), 0).checked();
        assert_eq!(checked.split.unwrap().ratio, 15);
        let checked = three_tabs(Some(split(0, 1, 99)), 0).checked();
        assert_eq!(checked.split.unwrap().ratio, 85);
    }

    #[test]
    fn tab_in_front_is_one_of_the_split_tabs() {
        assert_eq!(three_tabs(Some(split(1, 2, 50)), 0).checked().active, 1);
        assert_eq!(three_tabs(Some(split(1, 2, 50)), 2).checked().active, 2);
    }
}
