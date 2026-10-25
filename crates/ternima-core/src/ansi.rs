//! Taking escape sequences, such as colors, out of what a device sends, for
//! readable logs and for lines of text.

/// Where an escape sequence that is being taken out has got to.
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

/// Takes escape sequences out of a byte stream, also one split between reads.
#[derive(Debug, Default)]
pub struct EscapeFilter {
    escape: Escape,
}

impl EscapeFilter {
    /// Calls `text` with each byte that is not part of an escape sequence.
    pub fn filter(&mut self, bytes: &[u8], mut text: impl FnMut(u8)) {
        for &byte in bytes {
            self.escape = match (self.escape, byte) {
                (Escape::None, 0x1B) => Escape::Start,
                (Escape::None, _) => {
                    text(byte);
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(filter: &mut EscapeFilter, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        filter.filter(bytes, |byte| out.push(byte));
        out
    }

    #[test]
    fn colors_titles_and_two_byte_sequences_go() {
        let mut filter = EscapeFilter::default();
        let out = text(
            &mut filter,
            b"\x1b[1;31mred\x1b[0m \x1b]0;title\x07\x1b7ok\x1b]2;x\x1b\\!",
        );
        assert_eq!(out, b"red ok!");
    }

    #[test]
    fn a_sequence_split_between_reads_still_goes() {
        let mut filter = EscapeFilter::default();
        let mut out = text(&mut filter, b"a\x1b[3");
        out.extend(text(&mut filter, b"1mb"));
        assert_eq!(out, b"ab");
    }
}
