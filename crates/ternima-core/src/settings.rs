//! The user's preferences, kept in `settings.toml` in the OS's config folder.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::logfile::LogFormat;
use crate::tomlfile;

/// Smallest terminal text size, in pixels.
pub const FONT_SIZE_MIN: u8 = 8;
/// Largest terminal text size, in pixels.
pub const FONT_SIZE_MAX: u8 = 32;
/// Terminal text size for a new install, in pixels.
pub const FONT_SIZE_DEFAULT: u8 = 14;

/// Written at the top of the file, so someone who opens it knows what it is.
const HEADER: &str = "# ternima settings. Edit them while ternima is closed.\n\n";

/// Which colors the app uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Dark or light, following the OS.
    #[default]
    System,
    Dark,
    Light,
}

/// How log files are written. Each tab turns its own log on or off.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogSettings {
    /// New tabs start with their log on.
    pub new_tabs: bool,
    /// Where log files go. Empty means a "ternima logs" folder in Documents.
    pub folder: String,
    pub format: LogFormat,
    /// Puts the date and time at the start of every line of a text log.
    pub timestamps: bool,
}

impl Default for LogSettings {
    fn default() -> Self {
        Self {
            new_tabs: false,
            folder: String::new(),
            format: LogFormat::Text,
            timestamps: true,
        }
    }
}

/// What comes back when ternima starts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StartupSettings {
    /// Opens the tabs from last time, with their settings and split view.
    pub restore_tabs: bool,
    /// Connects the tabs that were connected when ternima closed.
    pub reconnect: bool,
    /// Opens the window where it was, at the size it had.
    pub restore_window: bool,
}

impl Default for StartupSettings {
    fn default() -> Self {
        Self {
            restore_tabs: true,
            reconnect: true,
            restore_window: true,
        }
    }
}

/// What the mouse does in the terminal and the hex view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MouseSettings {
    /// Selecting text copies it, as in PuTTY.
    pub select_copies: bool,
    /// A right-click pastes.
    pub right_click_pastes: bool,
}

impl Default for MouseSettings {
    fn default() -> Self {
        Self {
            select_copies: true,
            right_click_pastes: true,
        }
    }
}

/// Looking on GitHub for a newer terNIMA.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    /// Looks when terNIMA starts.
    pub check: bool,
    /// A version not to tell about again, such as "1.0.2"; empty for none.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub skipped: String,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            check: true,
            skipped: String::new(),
        }
    }
}

/// A command kept for one click, or Alt with its number.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Macro {
    pub name: String,
    /// Text with escapes such as `\r`, or hex digits; see [`crate::send::encode`].
    pub text: String,
    #[serde(default)]
    pub hex: bool,
    /// Sends the tab's Enter after the text.
    #[serde(default = "yes")]
    pub enter: bool,
}

fn yes() -> bool {
    true
}

/// Macros sent one after another with a click, a wait after each: a number
/// of times, or until it is stopped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sequence {
    pub name: String,
    /// The names of the macros it sends, in order.
    #[serde(default)]
    pub steps: Vec<String>,
    /// How long it waits after each macro, in ms.
    #[serde(default = "half_a_second")]
    pub delay_ms: u32,
    /// How many times it goes through its macros; 0 goes on until it is stopped.
    #[serde(default = "once")]
    pub repeat: u32,
}

fn half_a_second() -> u32 {
    500
}

fn once() -> u32 {
    1
}

/// The colors a highlight rule can mark lines in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HighlightColor {
    Red,
    Orange,
    #[default]
    Yellow,
    Green,
    Blue,
    Purple,
}

/// Lines with this text are marked in a color in the Lines view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Highlight {
    pub text: String,
    /// Whether `text` is a regular expression.
    #[serde(default)]
    pub regex: bool,
    /// Whether upper and lower case must match.
    #[serde(default)]
    pub case: bool,
    #[serde(default)]
    pub color: HighlightColor,
}

/// Everything in `settings.toml`. A value missing from the file takes its default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: Theme,
    pub font_size: u8,
    /// Shows a line under each terminal to type a whole command and send it at once.
    pub send_box: bool,
    /// Shows where code addresses are under the lines that have them, from the tab's ELF file.
    pub decode_addresses: bool,
    pub startup: StartupSettings,
    pub log: LogSettings,
    pub mouse: MouseSettings,
    pub updates: UpdateSettings,
    /// The macros every tab can send, in the order they are shown.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub macros: Vec<Macro>,
    /// Macros sent one after another, in the order they are shown.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sequences: Vec<Sequence>,
    /// The Lines view's highlight rules; the first that matches a line marks it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub highlights: Vec<Highlight>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            font_size: FONT_SIZE_DEFAULT,
            send_box: true,
            decode_addresses: true,
            startup: StartupSettings::default(),
            log: LogSettings::default(),
            mouse: MouseSettings::default(),
            updates: UpdateSettings::default(),
            macros: Vec::new(),
            sequences: Vec::new(),
            highlights: Vec::new(),
        }
    }
}

impl Settings {
    /// Reads the settings from `path`; see [`tomlfile::load`] for missing and broken files.
    pub fn load(path: &Path) -> Self {
        tomlfile::load::<Settings>(path).checked()
    }

    /// Writes the settings to `path`, safely; see [`tomlfile::save`].
    pub fn save(&self, path: &Path) -> io::Result<()> {
        tomlfile::save(self, path, HEADER)
    }

    /// The same settings with every value moved into its allowed range.
    pub fn checked(mut self) -> Self {
        self.font_size = self.font_size.clamp(FONT_SIZE_MIN, FONT_SIZE_MAX);
        self
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::tomlfile::tests::TempDir;

    #[test]
    fn missing_file_gives_defaults() {
        let dir = TempDir::new("missing");
        assert_eq!(
            Settings::load(&dir.file("settings.toml")),
            Settings::default()
        );
    }

    #[test]
    fn saved_settings_load_back_the_same() {
        let dir = TempDir::new("round-trip");
        let path = dir.file("nested/folder/settings.toml");
        let settings = Settings {
            theme: Theme::Light,
            font_size: 18,
            send_box: false,
            decode_addresses: false,
            startup: StartupSettings {
                restore_tabs: true,
                reconnect: false,
                restore_window: false,
            },
            log: LogSettings {
                new_tabs: true,
                folder: "D:/logs".into(),
                format: LogFormat::Raw,
                timestamps: false,
            },
            mouse: MouseSettings {
                select_copies: false,
                right_click_pastes: false,
            },
            updates: UpdateSettings {
                check: false,
                skipped: "1.0.2".into(),
            },
            macros: vec![
                Macro {
                    name: "Reboot".into(),
                    text: "kernel reboot cold".into(),
                    hex: false,
                    enter: true,
                },
                Macro {
                    name: "Ping".into(),
                    text: "AA 55 01".into(),
                    hex: true,
                    enter: false,
                },
            ],
            sequences: vec![Sequence {
                name: "Boot test".into(),
                steps: vec!["Reboot".into(), "Ping".into()],
                delay_ms: 2000,
                repeat: 0,
            }],
            highlights: vec![Highlight {
                text: "timeout|fail".into(),
                regex: true,
                case: false,
                color: HighlightColor::Red,
            }],
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), settings);
        assert!(!path.with_extension("toml.tmp").exists());
    }

    #[test]
    fn a_macro_sends_enter_unless_told_not_to() {
        let settings: Settings =
            toml::from_str("[[macros]]\nname = \"Help\"\ntext = \"help\"\n").unwrap();
        assert_eq!(
            settings.macros,
            [Macro {
                name: "Help".into(),
                text: "help".into(),
                hex: false,
                enter: true,
            }]
        );
    }

    #[test]
    fn a_sequence_waits_half_a_second_and_runs_once_unless_told_otherwise() {
        let settings: Settings =
            toml::from_str("[[sequences]]\nname = \"Ping twice\"\nsteps = [\"Ping\", \"Ping\"]\n")
                .unwrap();
        assert_eq!(
            settings.sequences,
            [Sequence {
                name: "Ping twice".into(),
                steps: vec!["Ping".into(), "Ping".into()],
                delay_ms: 500,
                repeat: 1,
            }]
        );
    }

    #[test]
    fn saved_file_is_readable_toml() {
        let dir = TempDir::new("readable");
        let path = dir.file("settings.toml");
        Settings::default().save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# ternima settings"));
        assert!(text.contains("theme = \"system\""));
        assert!(text.contains("font_size = 14"));
    }

    #[test]
    fn missing_values_take_their_defaults() {
        let dir = TempDir::new("partial");
        let path = dir.file("settings.toml");
        fs::write(&path, "theme = \"dark\"\n").unwrap();
        let settings = Settings::load(&path);
        assert_eq!(settings.theme, Theme::Dark);
        assert_eq!(settings.font_size, FONT_SIZE_DEFAULT);
        // From before the mouse and the updates had settings: on, as for a new install.
        assert!(settings.mouse.select_copies && settings.mouse.right_click_pastes);
        assert!(settings.updates.check);
        assert_eq!(settings.updates.skipped, "");
    }

    #[test]
    fn unknown_values_are_ignored() {
        let dir = TempDir::new("unknown");
        let path = dir.file("settings.toml");
        fs::write(&path, "theme = \"light\"\nfrom_the_future = 1\n").unwrap();
        assert_eq!(Settings::load(&path).theme, Theme::Light);
    }

    #[test]
    fn broken_file_gives_defaults_and_is_kept() {
        let dir = TempDir::new("broken");
        let path = dir.file("settings.toml");
        fs::write(&path, "theme = \"purple\"\n").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
        let kept = fs::read_to_string(dir.file("settings.bad.toml")).unwrap();
        assert_eq!(kept, "theme = \"purple\"\n");
    }

    #[test]
    fn font_size_is_kept_in_range() {
        let small = Settings {
            font_size: 2,
            ..Settings::default()
        };
        let large = Settings {
            font_size: 200,
            ..Settings::default()
        };
        assert_eq!(small.checked().font_size, FONT_SIZE_MIN);
        assert_eq!(large.checked().font_size, FONT_SIZE_MAX);
    }
}
