//! Decoding the ITM packets that come out of the SWO pin.
//!
//! The stream mixes what the firmware writes to the stimulus ports with
//! timestamps, sync and overflow packets, and hardware packets from the DWT.
//! Only the stimulus port data is kept; the rest is skipped. Packets can be
//! split across reads, so the decoder keeps its place between calls.

/// What the decoder found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItmPacket {
    /// Bytes the firmware wrote to a stimulus port.
    Data { port: u8, bytes: Vec<u8> },
    /// The chip's trace FIFO overflowed, so some packets were lost.
    Overflow,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    /// Waiting for the next packet's header byte.
    #[default]
    Header,
    /// Collecting a stimulus port's payload, or skipping a hardware one.
    Payload {
        port: u8,
        software: bool,
        size: u8,
        got: u8,
        bytes: [u8; 4],
    },
    /// Skipping a packet whose bytes go on while their top bit is set.
    Continuation,
}

/// Turns raw SWO bytes into ITM packets.
#[derive(Clone, Debug, Default)]
pub struct ItmDecoder {
    state: State,
}

impl ItmDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes the next bytes. Packets that are not complete yet are kept
    /// for the next call.
    pub fn feed(&mut self, data: &[u8]) -> Vec<ItmPacket> {
        let mut packets = Vec::new();
        for &byte in data {
            if let Some(packet) = self.step(byte) {
                packets.push(packet);
            }
        }
        packets
    }

    fn step(&mut self, byte: u8) -> Option<ItmPacket> {
        match self.state {
            State::Header => self.header(byte),
            State::Payload {
                port,
                software,
                size,
                got,
                mut bytes,
            } => {
                bytes[got as usize] = byte;
                let got = got + 1;
                if got < size {
                    self.state = State::Payload {
                        port,
                        software,
                        size,
                        got,
                        bytes,
                    };
                    return None;
                }
                self.state = State::Header;
                software.then(|| ItmPacket::Data {
                    port,
                    bytes: bytes[..size as usize].to_vec(),
                })
            }
            State::Continuation => {
                if byte & 0x80 == 0 {
                    self.state = State::Header;
                }
                None
            }
        }
    }

    fn header(&mut self, byte: u8) -> Option<ItmPacket> {
        match byte {
            // Sync: zero bytes, ended by 0x80.
            0x00 | 0x80 => None,
            0x70 => Some(ItmPacket::Overflow),
            // Global timestamps 1 and 2: more bytes while their top bit is set.
            0x94 | 0xB4 => {
                self.state = State::Continuation;
                None
            }
            // Local timestamp: format 1 (top bits 11) goes on; format 2 is this byte alone.
            byte if byte & 0x0F == 0 => {
                if byte & 0xC0 == 0xC0 {
                    self.state = State::Continuation;
                }
                None
            }
            // Extension packet, such as a stimulus port page.
            byte if byte & 0x0B == 0x08 => {
                if byte & 0x80 != 0 {
                    self.state = State::Continuation;
                }
                None
            }
            // Source packets: bit 2 tells hardware (DWT) from software (stimulus port).
            byte if byte & 0x03 != 0 => {
                let size = match byte & 0x03 {
                    1 => 1,
                    2 => 2,
                    _ => 4,
                };
                self.state = State::Payload {
                    port: byte >> 3,
                    software: byte & 0x04 == 0,
                    size,
                    got: 0,
                    bytes: [0; 4],
                };
                None
            }
            // Reserved headers: skip the byte.
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(port: u8, bytes: &[u8]) -> ItmPacket {
        ItmPacket::Data {
            port,
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn stimulus_port_bytes_of_every_size() {
        let mut decoder = ItmDecoder::new();
        // Port 0, 1 byte "h"; port 0, 2 bytes "el"; port 0, 4 bytes "lo! ".
        let packets = decoder.feed(&[0x01, b'h', 0x02, b'e', b'l', 0x03, b'l', b'o', b'!', b' ']);
        assert_eq!(packets, [data(0, b"h"), data(0, b"el"), data(0, b"lo! ")]);
    }

    #[test]
    fn the_port_comes_from_the_header() {
        let mut decoder = ItmDecoder::new();
        // Port 1 is header 0b00001_0_01, port 31 is 0b11111_0_01.
        let packets = decoder.feed(&[0x09, b'a', 0xF9, b'z']);
        assert_eq!(packets, [data(1, b"a"), data(31, b"z")]);
    }

    #[test]
    fn packets_split_across_reads_are_joined() {
        let mut decoder = ItmDecoder::new();
        assert!(decoder.feed(&[0x03, b'a', b'b']).is_empty());
        assert_eq!(decoder.feed(b"cd"), [data(0, b"abcd")]);
    }

    #[test]
    fn sync_and_timestamps_are_skipped() {
        let mut decoder = ItmDecoder::new();
        let mut stream = vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x80];
        // Local timestamp format 1, with three bytes of time.
        stream.extend([0xC0, 0x81, 0x82, 0x03]);
        // Local timestamp format 2, a single byte.
        stream.push(0x30);
        // Global timestamp 1, with two bytes.
        stream.extend([0x94, 0x85, 0x06]);
        stream.extend([0x01, b'x']);
        assert_eq!(decoder.feed(&stream), [data(0, b"x")]);
    }

    #[test]
    fn hardware_packets_are_skipped() {
        let mut decoder = ItmDecoder::new();
        // An exception trace packet from the DWT: discriminator 1, two bytes.
        let packets = decoder.feed(&[0x0E, 0x0F, 0x10, 0x01, b'y']);
        assert_eq!(packets, [data(0, b"y")]);
    }

    #[test]
    fn overflow_is_reported() {
        let mut decoder = ItmDecoder::new();
        assert_eq!(
            decoder.feed(&[0x70, 0x01, b'k']),
            [ItmPacket::Overflow, data(0, b"k")]
        );
    }

    #[test]
    fn extension_packets_are_skipped() {
        let mut decoder = ItmDecoder::new();
        // A stimulus port page extension with one more byte, then data.
        assert_eq!(decoder.feed(&[0x88, 0x01, 0x01, b'p']), [data(0, b"p")]);
    }
}
