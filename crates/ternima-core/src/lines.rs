//! What a session receives, as lines of text, with Zephyr's log lines read
//! into their parts:
//!
//! ```text
//! [00:00:01.234,567] <err> sensor: read failed
//!  time               level module  message
//! ```
//!
//! Colors are taken out first, as Zephyr colors whole lines. A line that is
//! not a log line, such as a shell prompt or a printk, keeps only its text.

use serde::{Deserialize, Serialize};

use crate::ansi::EscapeFilter;

/// The most bytes a line keeps. Data without line ends, such as binary data,
/// is cut there rather than growing a line without end.
const LINE_MAX: usize = 4096;

/// A Zephyr log level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Err,
    Wrn,
    Inf,
    Dbg,
}

impl Level {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "err" => Some(Level::Err),
            "wrn" => Some(Level::Wrn),
            "inf" => Some(Level::Inf),
            "dbg" => Some(Level::Dbg),
            _ => None,
        }
    }
}

/// One received line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Line {
    /// The whole line, without colors and control characters.
    pub text: String,
    /// The time as the log wrote it, such as "00:00:01.234,567".
    pub time: Option<String>,
    pub level: Option<Level>,
    pub module: Option<String>,
    /// What follows the module; the whole text for a line that is not a log line.
    pub message: String,
}

/// The parts of a log line a line after it can carry on.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    time: Option<String>,
    level: Level,
    module: Option<String>,
}

/// Cuts a byte stream into lines, and reads the log lines among them.
#[derive(Debug, Default)]
pub struct LineSplitter {
    escape: EscapeFilter,
    current: Vec<u8>,
    /// Just after a CR. An LF then ends the line; anything else starts it
    /// over, as a terminal draws over a line after a CR, the way a shell
    /// clears its prompt before it prints a log line.
    after_cr: bool,
    /// The last log line, which an indented line after it carries on, as the
    /// lines of a LOG_HEXDUMP do.
    last: Option<Entry>,
}

impl LineSplitter {
    /// Takes received bytes, and gives back the lines they finished. Empty
    /// lines are left out.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Line> {
        let mut text = Vec::with_capacity(bytes.len());
        self.escape.filter(bytes, |byte| text.push(byte));
        let mut lines = Vec::new();
        for byte in text {
            match byte {
                b'\n' => {
                    self.after_cr = false;
                    if let Some(line) = self.finish() {
                        lines.push(line);
                    }
                }
                b'\r' => self.after_cr = true,
                _ => {
                    if self.after_cr {
                        self.after_cr = false;
                        self.current.clear();
                    }
                    match byte {
                        // A shell takes back a typed character with "\b \b".
                        0x08 => {
                            self.current.pop();
                        }
                        b'\t' => self.push(b' '),
                        0x00..=0x1F | 0x7F => {}
                        _ => self.push(byte),
                    }
                }
            }
        }
        lines
    }

    fn push(&mut self, byte: u8) {
        if self.current.len() < LINE_MAX {
            self.current.push(byte);
        }
    }

    /// Ends the line being read, if it has any text.
    fn finish(&mut self) -> Option<Line> {
        let text = String::from_utf8_lossy(&self.current)
            .trim_end()
            .to_string();
        self.current.clear();
        if text.is_empty() {
            return None;
        }
        if let Some((entry, message)) = parse(&text) {
            self.last = Some(entry.clone());
            return Some(Line {
                text,
                time: entry.time,
                level: Some(entry.level),
                module: entry.module,
                message,
            });
        }
        // An indented line right after a log line carries it on.
        if text.starts_with("  ")
            && let Some(entry) = &self.last
        {
            return Some(Line {
                message: text.trim_start().to_string(),
                time: entry.time.clone(),
                level: Some(entry.level),
                module: entry.module.clone(),
                text,
            });
        }
        self.last = None;
        Some(Line {
            message: text.clone(),
            text,
            time: None,
            level: None,
            module: None,
        })
    }
}

/// Reads a Zephyr log line: an optional "[time]", a "<level>", then
/// "module: message". The module is left out when there is no "word: ".
fn parse(text: &str) -> Option<(Entry, String)> {
    let mut rest = text.trim_start();
    let mut time = None;
    if let Some(after) = rest.strip_prefix('[') {
        let end = after.find(']').filter(|&end| end <= 40)?;
        time = Some(after[..end].trim().to_string());
        rest = after[end + 1..].trim_start();
    }
    let after = rest.strip_prefix('<')?;
    let end = after.find('>').filter(|&end| end <= 4)?;
    let level = Level::from_name(&after[..end])?;
    rest = after[end + 1..].trim_start();

    let (module, message) = match rest.split_once(": ") {
        Some((module, message)) if is_module(module) => (Some(module.to_string()), message),
        _ => match rest.strip_suffix(':') {
            Some(module) if is_module(module) => (Some(module.to_string()), ""),
            _ => (None, rest),
        },
    };
    Some((
        Entry {
            time,
            level,
            module,
        },
        message.to_string(),
    ))
}

/// Whether a word can be a log module's name, such as "net_if" or "app/main".
fn is_module(word: &str) -> bool {
    !word.is_empty()
        && word.len() <= 64
        && word
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '/' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(chunks: &[&[u8]]) -> Vec<Line> {
        let mut splitter = LineSplitter::default();
        chunks
            .iter()
            .flat_map(|chunk| splitter.feed(chunk))
            .collect()
    }

    fn one(text: &str) -> Line {
        let found = lines(&[text.as_bytes(), b"\r\n"]);
        assert_eq!(found.len(), 1, "{found:?}");
        found.into_iter().next().unwrap()
    }

    #[test]
    fn a_log_line_is_read_into_its_parts() {
        let line = one("[00:00:01.234,567] <err> sensor: read failed: -5");
        assert_eq!(line.time.as_deref(), Some("00:00:01.234,567"));
        assert_eq!(line.level, Some(Level::Err));
        assert_eq!(line.module.as_deref(), Some("sensor"));
        assert_eq!(line.message, "read failed: -5");
    }

    #[test]
    fn colors_are_taken_out_first() {
        let line = one("\x1b[1;31m[00:00:02.000,000] <wrn> net_if: no carrier\x1b[0m");
        assert_eq!(line.level, Some(Level::Wrn));
        assert_eq!(line.text, "[00:00:02.000,000] <wrn> net_if: no carrier");
    }

    #[test]
    fn time_and_module_can_be_missing() {
        let line = one("<inf> main: Hello");
        assert_eq!((line.time, line.level), (None, Some(Level::Inf)));
        assert_eq!(line.module.as_deref(), Some("main"));

        let cycles = one("[00012345] <dbg> app/ble: conn_cb: connected");
        assert_eq!(cycles.time.as_deref(), Some("00012345"));
        assert_eq!(cycles.module.as_deref(), Some("app/ble"));
        assert_eq!(cycles.message, "conn_cb: connected");

        let no_module = one("<inf> Booting Zephyr OS");
        assert_eq!(no_module.module, None);
        assert_eq!(no_module.message, "Booting Zephyr OS");
    }

    #[test]
    fn other_lines_keep_their_text() {
        let line = one("*** Booting Zephyr OS build v4.2.0 ***");
        assert_eq!(line.level, None);
        assert_eq!(line.message, "*** Booting Zephyr OS build v4.2.0 ***");
        assert_eq!(one("<unknown> x: y").level, None);
    }

    #[test]
    fn an_indented_line_carries_on_the_log_line_before_it() {
        let found = lines(&[
            b"[00:00:03.000,000] <inf> spi: rx\r\n",
            b"                 01 02 03 04 |....\r\n",
            b"uart:~$ help\r\n",
            b"  not a hexdump\r\n",
        ]);
        assert_eq!(found[1].level, Some(Level::Inf));
        assert_eq!(found[1].module.as_deref(), Some("spi"));
        assert_eq!(found[1].message, "01 02 03 04 |....");
        assert_eq!(found[2].level, None);
        assert_eq!(found[3].level, None);
    }

    #[test]
    fn lines_split_between_reads_come_out_whole() {
        let found = lines(&[b"[00:00:01.000,000] <in", b"f> main: hel", b"lo\n"]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].message, "hello");
    }

    #[test]
    fn a_line_drawn_over_after_a_cr_starts_again() {
        let found = lines(&[b"uart:~$ \r\x1b[K[00:00:01.000,000] <inf> main: tick\r\n"]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].module.as_deref(), Some("main"));
    }

    #[test]
    fn backspace_takes_back_a_character_and_empty_lines_are_left_out() {
        let found = lines(&[b"helx\x08 \x08lo\r\n\r\n\n"]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "hello");
    }

    #[test]
    fn very_long_lines_are_cut() {
        let long = vec![b'x'; LINE_MAX + 100];
        let found = lines(&[&long, b"\n"]);
        assert_eq!(found[0].text.len(), LINE_MAX);
    }
}
