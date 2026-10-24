//! The user's preferences, kept in `settings.toml` in the OS's config folder.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::tomlfile;

/// Smallest terminal text size, in pixels.
pub const FONT_SIZE_MIN: u8 = 8;
/// Largest terminal text size, in pixels.
pub const FONT_SIZE_MAX: u8 = 32;
/// Terminal text size for a new install, in pixels.
pub const FONT_SIZE_DEFAULT: u8 = 14;

/// Written at the top of the file, so someone who opens it knows what it is.
const HEADER: &str = "# ternimal settings. Edit them while ternimal is closed.\n\n";

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

/// Everything in `settings.toml`. A value missing from the file takes its default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: Theme,
    pub font_size: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            font_size: FONT_SIZE_DEFAULT,
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
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), settings);
        assert!(!path.with_extension("toml.tmp").exists());
    }

    #[test]
    fn saved_file_is_readable_toml() {
        let dir = TempDir::new("readable");
        let path = dir.file("settings.toml");
        Settings::default().save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# ternimal settings"));
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
