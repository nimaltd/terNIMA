//! Writing what a session receives to a log file.
//!
//! A text log is for reading: color codes and other escape sequences are
//! removed, each line can start with the date and time, and connects and
//! disconnects get a line of their own. A raw log keeps every byte as it came.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::VERSION;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Readable text: colors and other escape codes removed.
    #[default]
    Text,
    /// Every byte as it came, for binary data.
    Raw,
}

/// Where and how one session's log is written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogSetup {
    pub folder: PathBuf,
    /// The session's name, which starts the file name.
    pub name: String,
    pub format: LogFormat,
    /// Puts the date and time at the start of every line of a text log.
    pub timestamps: bool,
}

/// Gives the time written at the start of lines.
type Clock = Box<dyn Fn() -> String + Send>;

/// Where an escape sequence that is being removed has got to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Escape {
    #[default]
    None,
    /// After ESC.
    Start,
    /// Inside ESC [ ... until a final byte.
    Csi,
    /// Inside ESC ] ... until BEL or ESC \.
    Osc,
    /// After ESC inside an OSC sequence.
    OscEnd,
}

/// One open log file.
pub struct Logger {
    file: BufWriter<File>,
    path: PathBuf,
    format: LogFormat,
    timestamps: bool,
    clock: Clock,
    /// Whether the next byte starts a line.
    line_start: bool,
    escape: Escape,
    connected: bool,
    /// Whether the session has been connected before, to tell "again" apart.
    was_connected: bool,
}

impl Logger {
    /// Starts a new log file in the setup's folder, made if needed. Its name is
    /// the session's name and the time, such as "Nucleo 2026-10-25 14-02-11.log".
    pub fn create(setup: &LogSetup) -> io::Result<Self> {
        let stamp = chrono::Local::now().format("%Y-%m-%d %H-%M-%S").to_string();
        Self::create_with(setup, &stamp, Box::new(local_time))
    }

    fn create_with(setup: &LogSetup, stamp: &str, clock: Clock) -> io::Result<Self> {
        fs::create_dir_all(&setup.folder)?;
        let base = file_name(&setup.name);
        let mut path = setup.folder.join(format!("{base} {stamp}.log"));
        // Two logs started in the same second get a number.
        let mut number = 2;
        while path.exists() {
            path = setup.folder.join(format!("{base} {stamp} ({number}).log"));
            number += 1;
        }
        let file = File::options().write(true).create_new(true).open(&path)?;

        let mut logger = Self {
            file: BufWriter::new(file),
            path,
            format: setup.format,
            timestamps: setup.timestamps,
            clock,
            line_start: true,
            escape: Escape::None,
            connected: false,
            was_connected: false,
        };
        logger.marker(&format!("ternima {VERSION} log of {}", setup.name))?;
        Ok(logger)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes received bytes, and flushes them to the disk.
    pub fn data(&mut self, bytes: &[u8]) -> io::Result<()> {
        match self.format {
            LogFormat::Raw => self.file.write_all(bytes)?,
            LogFormat::Text => {
                let text = self.readable(bytes);
                self.file.write_all(&text)?;
            }
        }
        self.file.flush()
    }

    /// Notes that the session connected to `what`.
    pub fn connected(&mut self, what: &str) -> io::Result<()> {
        if self.connected {
            return Ok(());
        }
        let text = if self.was_connected {
            format!("connected again to {what}")
        } else {
            format!("connected to {what}")
        };
        self.connected = true;
        self.was_connected = true;
        self.marker(&text)
    }

    /// Notes that the session lost its device.
    pub fn disconnected(&mut self) -> io::Result<()> {
        if !self.connected {
            return Ok(());
        }
        self.connected = false;
        self.marker("disconnected")
    }

    /// Writes a marker line such as "---- disconnected ----" on a line of its
    /// own. Raw logs get none, so their bytes stay as they came.
    pub fn marker(&mut self, text: &str) -> io::Result<()> {
        if self.format == LogFormat::Raw {
            return Ok(());
        }
        let mut line = String::new();
        if !self.line_start {
            line.push('\n');
        }
        if self.timestamps {
            line.push_str(&format!("[{}] ", (self.clock)()));
        }
        line.push_str(&format!("---- {text} ----\n"));
        self.line_start = true;
        self.file.write_all(line.as_bytes())?;
        self.file.flush()
    }

    /// Turns received bytes into readable text: escape sequences, CR and other
    /// control characters out, the time in front of each line.
    fn readable(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len() + 32);
        for &byte in bytes {
            self.escape = match (self.escape, byte) {
                (Escape::None, 0x1B) => Escape::Start,
                (Escape::None, _) => {
                    self.text_byte(byte, &mut out);
                    Escape::None
                }
                (Escape::Start, b'[') => Escape::Csi,
                (Escape::Start, b']') => Escape::Osc,
                // A two byte sequence, such as ESC 7.
                (Escape::Start, _) => Escape::None,
                (Escape::Csi, 0x40..=0x7E) => Escape::None,
                (Escape::Csi, _) => Escape::Csi,
                (Escape::Osc, 0x07) => Escape::None,
                (Escape::Osc, 0x1B) => Escape::OscEnd,
                (Escape::Osc, _) => Escape::Osc,
                (Escape::OscEnd, b'\\') => Escape::None,
                (Escape::OscEnd, _) => Escape::Osc,
            };
        }
        out
    }

    fn text_byte(&mut self, byte: u8, out: &mut Vec<u8>) {
        match byte {
            b'\n' => {
                out.push(b'\n');
                self.line_start = true;
            }
            // Tabs stay; CR, backspace, bell and the other control characters go.
            b'\t' => self.printable(byte, out),
            0x00..=0x1F | 0x7F => {}
            _ => self.printable(byte, out),
        }
    }

    fn printable(&mut self, byte: u8, out: &mut Vec<u8>) {
        if self.line_start {
            if self.timestamps {
                out.extend_from_slice(format!("[{}] ", (self.clock)()).as_bytes());
            }
            self.line_start = false;
        }
        out.push(byte);
    }
}

/// "2026-10-25 14:02:11.204"
fn local_time() -> String {
    chrono::Local::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string()
}

/// A session name made safe for a file name on every OS.
fn file_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .take(80)
        .collect();
    let safe = safe.trim().trim_end_matches(['.', ' ']);
    if safe.is_empty() {
        "session".into()
    } else {
        safe.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tomlfile::tests::TempDir;

    fn setup(dir: &TempDir, format: LogFormat, timestamps: bool) -> LogSetup {
        LogSetup {
            folder: dir.file("logs"),
            name: "H503 RTT".into(),
            format,
            timestamps,
        }
    }

    fn logger(dir: &TempDir, format: LogFormat, timestamps: bool) -> Logger {
        Logger::create_with(
            &setup(dir, format, timestamps),
            "2026-10-25 01-30-00",
            Box::new(|| "T".into()),
        )
        .unwrap()
    }

    /// The file's text after the first line, which names the session.
    fn body(logger: &Logger) -> String {
        let text = fs::read_to_string(logger.path()).unwrap();
        text.split_once('\n')
            .map(|(_, rest)| rest.to_string())
            .unwrap_or_default()
    }

    #[test]
    fn colors_go_and_lines_get_the_time() {
        let dir = TempDir::new("log-colors");
        let mut log = logger(&dir, LogFormat::Text, true);
        log.data(b"\x1b[1;32mh503:~$ \x1b[0mhelp\r\n  up 4.2 s\r\n")
            .unwrap();
        assert_eq!(body(&log), "[T] h503:~$ help\n[T]   up 4.2 s\n");
    }

    #[test]
    fn without_timestamps_lines_are_as_they_came() {
        let dir = TempDir::new("log-plain");
        let mut log = logger(&dir, LogFormat::Text, false);
        log.data(b"a\tb\r\nc\n").unwrap();
        assert_eq!(body(&log), "a\tb\nc\n");
    }

    #[test]
    fn a_sequence_split_between_reads_is_still_removed() {
        let dir = TempDir::new("log-split");
        let mut log = logger(&dir, LogFormat::Text, true);
        log.data(b"\x1b[3").unwrap();
        log.data(b"1mred\x1b[0m and ").unwrap();
        log.data(b"more\r\n").unwrap();
        assert_eq!(body(&log), "[T] red and more\n");
    }

    #[test]
    fn titles_and_two_byte_sequences_are_removed() {
        let dir = TempDir::new("log-osc");
        let mut log = logger(&dir, LogFormat::Text, false);
        log.data(b"\x1b]0;window title\x07\x1b7hi\x1b]2;x\x1b\\!\n")
            .unwrap();
        assert_eq!(body(&log), "hi!\n");
    }

    #[test]
    fn markers_get_a_line_of_their_own() {
        let dir = TempDir::new("log-markers");
        let mut log = logger(&dir, LogFormat::Text, true);
        log.connected("STM32H503RB").unwrap();
        log.data(b"tick 1").unwrap();
        log.disconnected().unwrap();
        log.disconnected().unwrap();
        log.connected("STM32H503RB").unwrap();
        assert_eq!(
            body(&log),
            "[T] ---- connected to STM32H503RB ----\n\
             [T] tick 1\n\
             [T] ---- disconnected ----\n\
             [T] ---- connected again to STM32H503RB ----\n"
        );
    }

    #[test]
    fn raw_logs_keep_every_byte_and_no_markers() {
        let dir = TempDir::new("log-raw");
        let mut log = logger(&dir, LogFormat::Raw, true);
        log.connected("COM6").unwrap();
        log.data(&[0x1B, b'[', b'1', b'm', 0x00, 0xFF, b'\r', b'\n'])
            .unwrap();
        assert_eq!(
            fs::read(log.path()).unwrap(),
            [0x1B, b'[', b'1', b'm', 0x00, 0xFF, b'\r', b'\n']
        );
    }

    #[test]
    fn the_first_line_names_the_session() {
        let dir = TempDir::new("log-header");
        let log = logger(&dir, LogFormat::Text, false);
        let text = fs::read_to_string(log.path()).unwrap();
        assert_eq!(
            text,
            format!("---- ternima {VERSION} log of H503 RTT ----\n")
        );
    }

    #[test]
    fn files_are_named_after_the_session_and_never_overwritten() {
        let dir = TempDir::new("log-names");
        let first = logger(&dir, LogFormat::Text, false);
        let second = logger(&dir, LogFormat::Text, false);
        let name = |log: &Logger| {
            log.path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        };
        assert_eq!(name(&first), "H503 RTT 2026-10-25 01-30-00.log");
        assert_eq!(name(&second), "H503 RTT 2026-10-25 01-30-00 (2).log");
    }

    #[test]
    fn names_are_made_safe_for_files() {
        assert_eq!(file_name("COM6: 115200/8N1?"), "COM6_ 115200_8N1_");
        assert_eq!(file_name("  ...  "), "session");
        assert_eq!(file_name("Board."), "Board");
    }
}
