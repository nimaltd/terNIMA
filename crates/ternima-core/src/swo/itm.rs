//! Decoding the ITM packets that come out of the SWO pin.
//!
//! The stream mixes what the firmware writes to the stimulus ports with
//! timestamps, sync and overflow packets, and hardware packets from the DWT.
//! Kept are the stimulus ports' data, the local timestamps, and the DWT's
//! exception trace and data values, which the timeline is drawn from; the
//! rest is skipped. Packets can be split across reads, so the decoder keeps
//! its place between calls.

/// What the decoder found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItmPacket {
    /// Bytes the firmware wrote to a stimulus port.
    Data { port: u8, bytes: Vec<u8> },
    /// The chip's trace FIFO overflowed, so some packets were lost.
    Overflow,
    /// A local timestamp: the ITM's clock ticks since the one before. It
    /// tells the time of the packets that came since then.
    Time(u32),
    /// From the DWT's exception trace: an exception entered, left, or
    /// returned to. Number 0 is no exception: the thread.
    Exception { number: u16, event: ExceptionEvent },
    /// A value written to, or read from, the address a DWT comparator watches.
    Value {
        comparator: u8,
        write: bool,
        value: u32,
    },
}

/// What happened to an exception.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExceptionEvent {
    /// It started.
    Entered,
    /// It ended, or was left for another.
    Exited,
    /// It goes on again, after one it was left for.
    Returned,
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
    /// Collecting a local timestamp's bytes, 7 bits of time in each.
    Time { value: u32, shift: u8 },
}

/// Turns raw SWO bytes into ITM packets.
#[derive(Clone, Debug, Default)]
pub struct ItmDecoder {
    state: State,
    /// Zero bytes in a row: five and more, then 0x80, are a sync packet,
    /// which puts a decoder that lost its place, as when a byte was lost on
    /// the way, back in step.
    zeros: u8,
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
        if byte == 0 {
            self.zeros = self.zeros.saturating_add(1);
        } else {
            let synced = byte == 0x80 && self.zeros >= 5;
            self.zeros = 0;
            if synced {
                self.state = State::Header;
                return None;
            }
        }
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
                let bytes = &bytes[..size as usize];
                if software {
                    return Some(ItmPacket::Data {
                        port,
                        bytes: bytes.to_vec(),
                    });
                }
                hardware(port, bytes)
            }
            State::Continuation => {
                if byte & 0x80 == 0 {
                    self.state = State::Header;
                }
                None
            }
            State::Time { value, shift } => {
                let value = value | (u32::from(byte & 0x7F) << shift);
                if byte & 0x80 != 0 && shift < 21 {
                    self.state = State::Time {
                        value,
                        shift: shift + 7,
                    };
                    return None;
                }
                self.state = State::Header;
                Some(ItmPacket::Time(value))
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
            // Local timestamp: format 1 (top bits 11) has the time in the
            // bytes after it; format 2 has a time of 1 to 6 in its bits 6 to 4.
            byte if byte & 0x0F == 0 => {
                if byte & 0xC0 == 0xC0 {
                    self.state = State::Time { value: 0, shift: 0 };
                    None
                } else if byte & 0x80 == 0 {
                    Some(ItmPacket::Time(u32::from(byte >> 4)))
                } else {
                    None
                }
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

/// What a hardware packet from the DWT says, of what the timeline uses.
fn hardware(discriminator: u8, bytes: &[u8]) -> Option<ItmPacket> {
    match discriminator {
        1 if bytes.len() == 2 => {
            let number = u16::from(bytes[0]) | (u16::from(bytes[1] & 0x01) << 8);
            let event = match (bytes[1] >> 4) & 0x03 {
                1 => ExceptionEvent::Entered,
                2 => ExceptionEvent::Exited,
                3 => ExceptionEvent::Returned,
                _ => return None,
            };
            Some(ItmPacket::Exception { number, event })
        }
        // Data values: 0b10 then the comparator's two bits, then 1 for a write.
        16..=23 => {
            let mut word = [0; 4];
            word[..bytes.len()].copy_from_slice(bytes);
            Some(ItmPacket::Value {
                comparator: (discriminator >> 1) & 0x03,
                write: discriminator & 1 == 1,
                value: u32::from_le_bytes(word),
            })
        }
        // Event counters, PC samples, and data trace PCs and addresses.
        _ => None,
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
    fn local_timestamps_are_read_and_the_rest_skipped() {
        let mut decoder = ItmDecoder::new();
        let mut stream = vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x80];
        // Local timestamp format 1, with three bytes of time: 1 + 2 << 7 + 3 << 14.
        stream.extend([0xC0, 0x81, 0x82, 0x03]);
        // Local timestamp format 2, a single byte: 3.
        stream.push(0x30);
        // Global timestamp 1, with two bytes.
        stream.extend([0x94, 0x85, 0x06]);
        stream.extend([0x01, b'x']);
        assert_eq!(
            decoder.feed(&stream),
            [ItmPacket::Time(49_409), ItmPacket::Time(3), data(0, b"x")]
        );
    }

    #[test]
    fn exceptions_are_read() {
        let mut decoder = ItmDecoder::new();
        // Discriminator 1, two bytes: SysTick (15) entered, left, and the thread returned to.
        let packets = decoder.feed(&[0x0E, 0x0F, 0x10, 0x0E, 0x0F, 0x20, 0x0E, 0x00, 0x30]);
        assert_eq!(
            packets,
            [
                ItmPacket::Exception {
                    number: 15,
                    event: ExceptionEvent::Entered
                },
                ItmPacket::Exception {
                    number: 15,
                    event: ExceptionEvent::Exited
                },
                ItmPacket::Exception {
                    number: 0,
                    event: ExceptionEvent::Returned
                },
            ]
        );
        // IRQ 50 is exception 66: its ninth bit is in the second byte.
        assert_eq!(
            decoder.feed(&[0x0E, 0x42, 0x10]),
            [ItmPacket::Exception {
                number: 66,
                event: ExceptionEvent::Entered
            }]
        );
        assert_eq!(
            decoder.feed(&[0x0E, 0x10, 0x11]),
            [ItmPacket::Exception {
                number: 272,
                event: ExceptionEvent::Entered
            }]
        );
    }

    #[test]
    fn data_values_are_read() {
        let mut decoder = ItmDecoder::new();
        // Discriminator 17: comparator 0, a write; four bytes.
        let packets = decoder.feed(&[0x8F, 0x08, 0x02, 0x00, 0x20, 0x01, b'v']);
        assert_eq!(
            packets,
            [
                ItmPacket::Value {
                    comparator: 0,
                    write: true,
                    value: 0x2000_0208
                },
                data(0, b"v")
            ]
        );
        // Discriminator 18: comparator 1, a read; one byte.
        assert_eq!(
            decoder.feed(&[0x95, 0x7F]),
            [ItmPacket::Value {
                comparator: 1,
                write: false,
                value: 0x7F
            }]
        );
    }

    #[test]
    fn other_hardware_packets_are_skipped() {
        let mut decoder = ItmDecoder::new();
        // A periodic PC sample: discriminator 2, four bytes.
        let packets = decoder.feed(&[0x17, 0x01, 0x02, 0x03, 0x08, 0x01, b'y']);
        assert_eq!(packets, [data(0, b"y")]);
    }

    #[test]
    fn a_sync_packet_puts_a_decoder_out_of_step_back() {
        let mut decoder = ItmDecoder::new();
        // A format 1 timestamp's header, the bytes after it lost on the way,
        // so what follows is read as its time; then a sync packet.
        let mut stream = vec![0xC0, 0x8E, 0x8F];
        stream.extend([0x00, 0x00, 0x00, 0x00, 0x00, 0x80]);
        stream.extend([0x01, b's']);
        let packets = decoder.feed(&stream);
        // What came before the sync may be wrong; after it, all is read right.
        assert_eq!(packets.last(), Some(&data(0, b"s")));
        assert_eq!(decoder.state, State::Header);
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
