//! Recordings: every byte a session received and sent, with when, to play
//! back later.
//!
//! A recording is a text file that can be read. It starts with `#` lines,
//! then has a line for each piece of data: the seconds from the start, `<`
//! for received or `>` for sent, and the bytes in quotes.
//!
//! ```text
//! # terNIMA recording
//! # name: Nucleo
//! # started: 2026-10-25 14:02:11.123 +02:00
//! # started ms: 1792930931123
//! 0.000000 < "booting\r\n"
//! 1.250000 > "help\r"
//! # 2.000000 disconnected
//! ```
//!
//! Text stays as it is, UTF-8 included. Other bytes are written as `\r`,
//! `\n`, `\t`, `\e`, `\0` or `\xHH`, and `\` and `"` as `\\` and `\"`.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::logfile::new_file;

/// What a recording's file name ends in.
pub const EXTENSION: &str = "trec";

const FIRST_LINE: &str = "# terNIMA recording";

/// How long written data may wait in memory before it goes to the disk.
const FLUSH_AFTER: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Received,
    Sent,
}

/// A piece of data as it was received or sent.
#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    /// Seconds from the start of the recording.
    pub at: f64,
    pub direction: Direction,
    pub bytes: Vec<u8>,
}

/// A recording read back from its file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recording {
    /// The name of the tab it was recorded from.
    pub name: String,
    /// When it started, in ms since 1970, if the file says.
    pub started_ms: Option<f64>,
    pub chunks: Vec<Chunk>,
}

impl Recording {
    /// How long it plays, in seconds: until its last piece.
    pub fn length(&self) -> f64 {
        self.chunks.last().map_or(0.0, |chunk| chunk.at)
    }
}

/// A recording being written.
pub struct Recorder {
    file: BufWriter<File>,
    path: PathBuf,
    start: Instant,
    flushed: Instant,
}

impl Recorder {
    /// Starts a recording in `folder`, made if needed, named after the
    /// session and the time, such as "Nucleo 2026-10-25 14-02-11.trec".
    pub fn create(folder: &Path, name: &str) -> io::Result<Self> {
        let now = chrono::Local::now();
        let stamp = now.format("%Y-%m-%d %H-%M-%S").to_string();
        let (file, path) = new_file(folder, name, &stamp, EXTENSION)?;
        let mut recorder = Self {
            file: BufWriter::new(file),
            path,
            start: Instant::now(),
            flushed: Instant::now(),
        };
        let header = format!(
            "{FIRST_LINE}\n# name: {}\n# started: {}\n# started ms: {}\n",
            name.replace(['\r', '\n'], " "),
            now.format("%Y-%m-%d %H:%M:%S%.3f %:z"),
            now.timestamp_millis(),
        );
        recorder.file.write_all(header.as_bytes())?;
        recorder.file.flush()?;
        Ok(recorder)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes bytes received or sent, with the time since the start.
    pub fn data(&mut self, direction: Direction, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let arrow = match direction {
            Direction::Received => '<',
            Direction::Sent => '>',
        };
        let line = format!("{:.6} {arrow} \"{}\"\n", self.seconds(), escape(bytes));
        self.file.write_all(line.as_bytes())?;
        self.flush_now_and_then()
    }

    /// Writes a note for whoever reads the file, such as "disconnected".
    /// Playing it back leaves notes out.
    pub fn note(&mut self, text: &str) -> io::Result<()> {
        let line = format!(
            "# {:.6} {}\n",
            self.seconds(),
            text.replace(['\r', '\n'], " ")
        );
        self.file.write_all(line.as_bytes())?;
        self.flush_now_and_then()
    }

    fn seconds(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Flushes a few times a second at most, so a crash loses little.
    fn flush_now_and_then(&mut self) -> io::Result<()> {
        if self.flushed.elapsed() >= FLUSH_AFTER {
            self.flushed = Instant::now();
            self.file.flush()?;
        }
        Ok(())
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.file.flush();
    }
}

/// Reads a recording file. An error says what is wrong in plain words.
pub fn read(path: &Path) -> Result<Recording, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    parse(&text).map_err(|problem| format!("{}: {problem}", path.display()))
}

/// Reads a recording from its text.
pub fn parse(text: &str) -> Result<Recording, String> {
    let mut lines = text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty());
    if lines.next().map(|(_, line)| line.trim_end()) != Some(FIRST_LINE) {
        return Err("this is not a terNIMA recording".into());
    }
    let mut recording = Recording::default();
    for (index, line) in lines {
        let line = line.trim_end();
        if let Some(header) = line.strip_prefix('#') {
            match header.trim_start().split_once(": ") {
                Some(("name", name)) => recording.name = name.to_string(),
                Some(("started ms", ms)) => recording.started_ms = ms.trim().parse().ok(),
                _ => {}
            }
            continue;
        }
        let chunk = parse_chunk(line)
            .ok_or_else(|| format!("line {} is not a line of a recording: {line}", index + 1))?;
        // Times only go forward, whatever the file says.
        let at = chunk
            .at
            .max(recording.chunks.last().map_or(0.0, |last| last.at));
        recording.chunks.push(Chunk { at, ..chunk });
    }
    Ok(recording)
}

fn parse_chunk(line: &str) -> Option<Chunk> {
    let (time, rest) = line.split_once(' ')?;
    let at: f64 = time
        .parse()
        .ok()
        .filter(|at: &f64| at.is_finite() && *at >= 0.0)?;
    let (arrow, quoted) = rest.split_once(' ')?;
    let direction = match arrow {
        "<" => Direction::Received,
        ">" => Direction::Sent,
        _ => return None,
    };
    let inner = quoted.strip_prefix('"')?.strip_suffix('"')?;
    Some(Chunk {
        at,
        direction,
        bytes: unescape(inner)?,
    })
}

/// Writes bytes as text: printable text as it is, the rest escaped.
fn escape(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() + 8);
    let mut rest = bytes;
    while !rest.is_empty() {
        let (text, bad) = match std::str::from_utf8(rest) {
            Ok(text) => (text, None),
            Err(error) => {
                let valid = error.valid_up_to();
                // Safe: the bytes up to `valid` are UTF-8.
                let text = std::str::from_utf8(&rest[..valid]).unwrap_or_default();
                (text, Some(rest[valid]))
            }
        };
        for ch in text.chars() {
            match ch {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '\r' => out.push_str("\\r"),
                '\n' => out.push_str("\\n"),
                '\t' => out.push_str("\\t"),
                '\0' => out.push_str("\\0"),
                '\x1b' => out.push_str("\\e"),
                ch if ch.is_control() => {
                    let mut utf8 = [0; 4];
                    for byte in ch.encode_utf8(&mut utf8).bytes() {
                        out.push_str(&format!("\\x{byte:02x}"));
                    }
                }
                ch => out.push(ch),
            }
        }
        rest = &rest[text.len()..];
        if let Some(byte) = bad {
            out.push_str(&format!("\\x{byte:02x}"));
            rest = &rest[1..];
        }
    }
    out
}

/// Reads escaped text back into bytes; None when an escape is broken.
fn unescape(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            let mut utf8 = [0; 4];
            out.extend_from_slice(ch.encode_utf8(&mut utf8).as_bytes());
            continue;
        }
        out.push(match chars.next()? {
            'r' => b'\r',
            'n' => b'\n',
            't' => b'\t',
            '0' => 0,
            'e' => 0x1B,
            '\\' => b'\\',
            '"' => b'"',
            'x' => {
                let digits: String = chars.by_ref().take(2).collect();
                u8::from_str_radix(&digits, 16)
                    .ok()
                    .filter(|_| digits.len() == 2)?
            }
            _ => return None,
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tomlfile::tests::TempDir;

    #[test]
    fn every_byte_comes_back_as_it_was() {
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(unescape(&escape(&all)).unwrap(), all);
        let text = "temp=21.5 \"ok\" \\ سلام\r\n".as_bytes();
        assert_eq!(unescape(&escape(text)).unwrap(), text);
    }

    #[test]
    fn text_stays_readable() {
        assert_eq!(escape(b"tick 1\r\n"), r"tick 1\r\n");
        assert_eq!(escape(b"\x1b[31mred\x1b[0m"), r"\e[31mred\e[0m");
        assert_eq!(escape("سلام".as_bytes()), "سلام");
        assert_eq!(escape(&[0xff, b'a', 0x7f]), r"\xffa\x7f");
        // Half of a UTF-8 letter, as when a read ends inside one.
        assert_eq!(escape(&"س".as_bytes()[..1]), r"\xd8");
    }

    #[test]
    fn a_recording_reads_back() {
        let text = "# terNIMA recording\n\
                    # name: Nucleo\n\
                    # started: 2026-10-25 14:02:11.123 +02:00\n\
                    # started ms: 1792930931123\n\
                    0.000000 < \"boot\\r\\n\"\n\
                    \n\
                    # 0.500000 disconnected\n\
                    1.250000 > \"help\\r\"\n\
                    1.000000 < \"\\x00\\\"\"\n";
        let recording = parse(text).unwrap();
        assert_eq!(recording.name, "Nucleo");
        assert_eq!(recording.started_ms, Some(1_792_930_931_123.0));
        assert_eq!(
            recording.chunks,
            vec![
                Chunk {
                    at: 0.0,
                    direction: Direction::Received,
                    bytes: b"boot\r\n".to_vec()
                },
                Chunk {
                    at: 1.25,
                    direction: Direction::Sent,
                    bytes: b"help\r".to_vec()
                },
                // Earlier than the one before: kept in order, at the same time.
                Chunk {
                    at: 1.25,
                    direction: Direction::Received,
                    bytes: b"\0\"".to_vec()
                },
            ]
        );
        assert_eq!(recording.length(), 1.25);
    }

    #[test]
    fn other_files_are_refused_with_the_reason() {
        assert!(
            parse("hello\n")
                .unwrap_err()
                .contains("not a terNIMA recording")
        );
        let broken = parse("# terNIMA recording\n0.5 < \"\\q\"\n").unwrap_err();
        assert!(broken.contains("line 2"), "{broken}");
        assert!(parse("# terNIMA recording\nnow < \"a\"\n").is_err());
        assert!(parse("# terNIMA recording\n0.5 = \"a\"\n").is_err());
    }

    #[test]
    fn a_recorder_writes_what_parse_reads() {
        let dir = TempDir::new("record");
        let path = {
            let mut recorder = Recorder::create(&dir.file("logs"), "COM6: test").unwrap();
            recorder.data(Direction::Received, b"tick 1\r\n").unwrap();
            recorder.data(Direction::Sent, b"").unwrap();
            recorder.data(Direction::Sent, b"help\r").unwrap();
            recorder.note("disconnected").unwrap();
            recorder.data(Direction::Received, &[0, 0xff]).unwrap();
            recorder.path().to_path_buf()
        };
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("COM6_ test ") && name.ends_with(".trec"),
            "{name}"
        );
        let recording = read(&path).unwrap();
        assert_eq!(recording.name, "COM6: test");
        assert!(recording.started_ms.is_some_and(|ms| ms > 1.7e12));
        let pieces: Vec<_> = recording
            .chunks
            .iter()
            .map(|chunk| (chunk.direction, chunk.bytes.clone()))
            .collect();
        assert_eq!(
            pieces,
            vec![
                (Direction::Received, b"tick 1\r\n".to_vec()),
                (Direction::Sent, b"help\r".to_vec()),
                (Direction::Received, vec![0, 0xff]),
            ]
        );
        assert!(
            recording
                .chunks
                .windows(2)
                .all(|pair| pair[0].at <= pair[1].at)
        );
    }
}
