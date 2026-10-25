//! What the send box and macros send, and sending a file at a pace the link
//! can take.
//!
//! Text can hold escapes such as `\r` and `\x1b` for bytes that are hard to
//! type; hex is written as pairs of digits, such as `AA 55 01`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// Turns what was typed into the bytes to send: hex digits, or text with escapes.
pub fn encode(text: &str, hex: bool) -> Result<Vec<u8>, String> {
    if hex { parse_hex(text) } else { unescape(text) }
}

/// Turns text into bytes. `\r`, `\n`, `\t`, `\0`, `\e` (escape), `\\` and
/// `\xHH` stand for the bytes they name; everything else is sent as UTF-8.
pub fn unescape(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            let mut utf8 = [0; 4];
            out.extend_from_slice(ch.encode_utf8(&mut utf8).as_bytes());
            continue;
        }
        let byte = match chars.next() {
            Some('r') => b'\r',
            Some('n') => b'\n',
            Some('t') => b'\t',
            Some('0') => 0,
            Some('e') => 0x1B,
            Some('\\') => b'\\',
            Some('x') => {
                let digits: String = chars.by_ref().take(2).collect();
                if digits.len() != 2 || !digits.chars().all(|ch| ch.is_ascii_hexdigit()) {
                    return Err(format!(
                        "\\x needs two hex digits after it, such as \\x1b, not \\x{digits}."
                    ));
                }
                u8::from_str_radix(&digits, 16).expect("checked above")
            }
            Some(other) => {
                return Err(format!(
                    "\\{other} is not an escape. Write \\\\ for a backslash; the escapes are \\r \\n \\t \\0 \\e \\xHH."
                ));
            }
            None => return Err("A \\ at the end needs a letter after it, or write \\\\.".into()),
        };
        out.push(byte);
    }
    Ok(out)
}

/// Reads hex bytes, such as "AA 55 01", "aa5501" or "0xAA, 0x55". Each word
/// is one or more pairs of digits, or a single digit for one byte.
pub fn parse_hex(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let words = text
        .split(|ch: char| ch.is_whitespace() || matches!(ch, ',' | ':' | ';' | '-'))
        .filter(|word| !word.is_empty());
    for word in words {
        let digits = word
            .strip_prefix("0x")
            .or_else(|| word.strip_prefix("0X"))
            .unwrap_or(word);
        if let Some(bad) = digits.chars().find(|ch| !ch.is_ascii_hexdigit()) {
            return Err(format!("\"{bad}\" in \"{word}\" is not a hex digit."));
        }
        match digits.len() {
            0 => return Err(format!("\"{word}\" has no digits after 0x.")),
            1 => out.push(u8::from_str_radix(digits, 16).expect("checked above")),
            len if len % 2 == 1 => {
                return Err(format!(
                    "\"{word}\" has an odd number of digits: write 0{word} or split it."
                ));
            }
            _ => {
                for pair in digits.as_bytes().chunks(2) {
                    let pair = std::str::from_utf8(pair).expect("hex digits are ASCII");
                    out.push(u8::from_str_radix(pair, 16).expect("checked above"));
                }
            }
        }
    }
    Ok(out)
}

/// How a file is cut up and how long to wait between the pieces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pace {
    pub chunk: usize,
    pub delay: Duration,
}

impl Pace {
    /// For a link that carries `bytes_per_second`: about 20 pieces a second.
    /// With 0 the session takes the pieces as fast as it can, as TCP does.
    pub fn for_rate(bytes_per_second: u32) -> Self {
        if bytes_per_second == 0 {
            return Self {
                chunk: 4096,
                delay: Duration::ZERO,
            };
        }
        let chunk = (bytes_per_second as usize / 20).clamp(16, 4096);
        Self {
            chunk,
            delay: Duration::from_secs_f64(chunk as f64 / f64::from(bytes_per_second)),
        }
    }
}

/// Why sending a file stopped before the end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stopped {
    /// The user stopped it.
    Cancelled,
    /// The session is no longer there to send to.
    Gone,
}

/// Sends `data` through `write` in pieces, at `pace`, telling `progress` how
/// many bytes went after each piece. `write` returns false when the session
/// is gone. Setting `cancel` stops it between two pieces.
pub fn send_paced(
    data: &[u8],
    pace: Pace,
    cancel: &AtomicBool,
    mut write: impl FnMut(&[u8]) -> bool,
    mut progress: impl FnMut(usize),
) -> Result<(), Stopped> {
    let start = Instant::now();
    let mut sent = 0;
    for (index, chunk) in data.chunks(pace.chunk.max(1)).enumerate() {
        // Waits are counted from the start, so they do not add up to drift.
        let due = start + pace.delay * index as u32;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Stopped::Cancelled);
            }
            let left = due.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            thread::sleep(left.min(Duration::from_millis(20)));
        }
        if !write(chunk) {
            return Err(Stopped::Gone);
        }
        sent += chunk.len();
        progress(sent);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_escapes_name_bytes() {
        assert_eq!(unescape("help\\r\\n").unwrap(), b"help\r\n");
        assert_eq!(unescape("\\e[0m\\t\\0\\\\").unwrap(), b"\x1b[0m\t\0\\");
        assert_eq!(unescape("\\x41\\xff").unwrap(), [b'A', 0xFF]);
        assert_eq!(unescape("نیما").unwrap(), "نیما".as_bytes());
    }

    #[test]
    fn bad_escapes_say_what_is_wrong() {
        assert!(unescape("C:\\temp\\q").unwrap_err().contains("\\q"));
        assert!(unescape("\\x4").unwrap_err().contains("two hex digits"));
        assert!(unescape("\\xZZ").is_err());
        assert!(unescape("end\\").is_err());
    }

    #[test]
    fn hex_in_the_usual_ways() {
        let bytes = [0xAA, 0x55, 0x01];
        assert_eq!(parse_hex("AA 55 01").unwrap(), bytes);
        assert_eq!(parse_hex("aa5501").unwrap(), bytes);
        assert_eq!(parse_hex("0xAA, 0x55, 0x01").unwrap(), bytes);
        assert_eq!(parse_hex("AA:55-01").unwrap(), bytes);
        assert_eq!(parse_hex("1 2 a").unwrap(), [1, 2, 10]);
        assert_eq!(parse_hex("  ").unwrap(), []);
    }

    #[test]
    fn bad_hex_says_what_is_wrong() {
        assert!(parse_hex("AG").unwrap_err().contains("\"G\""));
        assert!(parse_hex("ABC").unwrap_err().contains("odd"));
        assert!(parse_hex("0x").unwrap_err().contains("no digits"));
    }

    #[test]
    fn encode_picks_the_mode() {
        assert_eq!(encode("41", true).unwrap(), [0x41]);
        assert_eq!(encode("41", false).unwrap(), b"41");
    }

    #[test]
    fn pace_follows_the_rate() {
        // 115200 baud with 10 bits a byte.
        let pace = Pace::for_rate(11_520);
        assert_eq!(pace.chunk, 576);
        assert_eq!(pace.delay, Duration::from_millis(50));
        // Slow links still send pieces of at least 16 bytes.
        assert_eq!(Pace::for_rate(96).chunk, 16);
        assert_eq!(Pace::for_rate(0).delay, Duration::ZERO);
    }

    #[test]
    fn sends_every_byte_in_order_with_progress() {
        let data: Vec<u8> = (0..=255).cycle().take(1000).collect();
        let pace = Pace {
            chunk: 300,
            delay: Duration::from_millis(1),
        };
        let mut received = Vec::new();
        let mut reports = Vec::new();
        let done = send_paced(
            &data,
            pace,
            &AtomicBool::new(false),
            |chunk| {
                received.extend_from_slice(chunk);
                true
            },
            |sent| reports.push(sent),
        );
        assert_eq!(done, Ok(()));
        assert_eq!(received, data);
        assert_eq!(reports, [300, 600, 900, 1000]);
    }

    #[test]
    fn waits_between_pieces() {
        let pace = Pace {
            chunk: 10,
            delay: Duration::from_millis(30),
        };
        let start = Instant::now();
        send_paced(&[0; 40], pace, &AtomicBool::new(false), |_| true, |_| {}).unwrap();
        // Four pieces: three waits.
        assert!(start.elapsed() >= Duration::from_millis(90));
    }

    #[test]
    fn stops_when_cancelled_or_gone() {
        let pace = Pace {
            chunk: 10,
            delay: Duration::ZERO,
        };
        let cancel = AtomicBool::new(false);
        let mut pieces = 0;
        let stopped = send_paced(
            &[0; 100],
            pace,
            &cancel,
            |_| {
                pieces += 1;
                if pieces == 3 {
                    cancel.store(true, Ordering::Relaxed);
                }
                true
            },
            |_| {},
        );
        assert_eq!(stopped, Err(Stopped::Cancelled));
        assert_eq!(pieces, 3);

        let gone = send_paced(&[0; 100], pace, &AtomicBool::new(false), |_| false, |_| {});
        assert_eq!(gone, Err(Stopped::Gone));
    }
}
