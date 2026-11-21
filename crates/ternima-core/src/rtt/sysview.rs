//! SEGGER SystemView, as firmware sends it over RTT: each task switch and
//! interrupt, timed by the chip. Zephyr sends it with
//! `CONFIG_SEGGER_SYSTEMVIEW`, on a channel named SysView, once told START
//! on it: first a sync, ten zeros, then what the firmware is and its tasks,
//! then the events as they happen. Each packet is its event's number, what
//! goes with it, and how many ticks of the firmware's clock passed since the
//! one before.

use crate::swo::{ExceptionEvent, ItmPacket};

/// The channel it comes on, the one it is told on having the same name.
pub const CHANNEL: &str = "SysView";
/// The commands told: a byte each.
pub const START: u8 = 1;
pub const STOP: u8 = 2;

/// The longest packet of its own length taken: more is SystemView read wrong.
const PACKET_MAX: u32 = 16 * 1024;
/// The sync: this many zeros.
const SYNC: usize = 10;

/// What a packet tells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// What the firmware is: its clock's ticks a second.
    Init { clock_hz: u32 },
    /// One of its tasks, by its control block's address, and its name.
    Task { address: u64, name: String },
    /// A task runs from now on, by its control block's address.
    Running(u64),
    /// The idle task runs.
    Idle,
    /// An exception was entered, by its number: 15 is SysTick, 16 and on the IRQs.
    Entered(u16),
    /// The exception entered last was left.
    Exited,
    /// The firmware dropped this many packets, its RTT buffer full.
    Overflow(u32),
    /// Anything else, which only moves the time on.
    Other,
}

/// Why a packet could not be read.
enum Short {
    /// It is not all in yet.
    More,
    /// It cannot be SystemView: the stream was read wrong.
    Wrong,
}

/// Reads a packet's parts in turn.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, Short> {
        let byte = *self.bytes.get(self.at).ok_or(Short::More)?;
        self.at += 1;
        Ok(byte)
    }

    /// A number as SystemView writes them: seven bits a byte, the lowest first.
    fn number(&mut self) -> Result<u32, Short> {
        let mut value: u32 = 0;
        for shift in (0..35).step_by(7) {
            let byte = self.byte()?;
            value |= u32::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(Short::Wrong)
    }

    fn numbers(&mut self, count: usize) -> Result<(), Short> {
        for _ in 0..count {
            self.number()?;
        }
        Ok(())
    }

    /// A string: its length in a byte, or 255 and two bytes, the high one
    /// first; then that many bytes.
    fn string(&mut self) -> Result<String, Short> {
        let mut length = usize::from(self.byte()?);
        if length == 255 {
            length = usize::from(self.byte()?) << 8 | usize::from(self.byte()?);
        }
        let bytes = self.take(length)?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }

    fn take(&mut self, count: usize) -> Result<&[u8], Short> {
        let end = self.at.checked_add(count).ok_or(Short::Wrong)?;
        let bytes = self.bytes.get(self.at..end).ok_or(Short::More)?;
        self.at = end;
        Ok(bytes)
    }
}

/// Turns SystemView's bytes into its events.
#[derive(Debug, Default)]
pub struct Decoder {
    /// Bytes not taken yet: a packet that is not all in.
    pending: Vec<u8>,
    synced: bool,
    /// Zeros in a row, while the sync is looked for.
    zeros: usize,
    /// Where task IDs count from, and how far they are shifted.
    base: u64,
    shift: u32,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a sync was found, from which on the packets are read.
    pub fn synced(&self) -> bool {
        self.synced
    }

    /// Looks for a sync again: the firmware sends one when it starts.
    pub fn resync(&mut self) {
        self.pending.clear();
        self.synced = false;
        self.zeros = 0;
    }

    /// Takes in bytes read, and gives the packets they complete: each event
    /// with the ticks of the firmware's clock since the packet before.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<(Event, u32)> {
        self.pending.extend_from_slice(bytes);
        let mut events = Vec::new();
        let mut at = 0;
        while at < self.pending.len() {
            if !self.synced {
                let byte = self.pending[at];
                at += 1;
                self.zeros = if byte == 0 { self.zeros + 1 } else { 0 };
                if self.zeros >= SYNC {
                    self.synced = true;
                    self.zeros = 0;
                }
                continue;
            }
            let pending = std::mem::take(&mut self.pending);
            let packet = self.packet(&pending[at..]);
            self.pending = pending;
            match packet {
                Ok((event, delta, used)) => {
                    at += used;
                    events.push((event, delta));
                }
                Err(Short::More) => break,
                // Read wrong: what follows is taken from the next sync on.
                Err(Short::Wrong) => self.synced = false,
            }
        }
        self.pending.drain(..at);
        events
    }

    /// A task's control block's address, from its ID as sent.
    fn address(&self, id: u32) -> u64 {
        self.base + (u64::from(id) << self.shift)
    }

    /// The packet at the start of `bytes`: its event, the ticks since the
    /// packet before, and how many bytes it took.
    fn packet(&mut self, bytes: &[u8]) -> Result<(Event, u32, usize), Short> {
        let mut read = Reader { bytes, at: 0 };
        let id = read.byte()?;
        let event = match id {
            // Nothing goes with these.
            0 | 10 | 11 | 20 => Event::Other,
            3 | 18 => Event::Exited,
            5 => Event::Other,
            17 => Event::Idle,
            1 => Event::Overflow(read.number()?),
            2 => Event::Entered(u16::try_from(read.number()?).map_err(|_| Short::Wrong)?),
            4 => Event::Running(self.address(read.number()?)),
            6 | 8 | 12 | 15 | 16 | 19 => {
                read.number()?;
                Event::Other
            }
            7 | 13 | 23 => {
                read.numbers(2)?;
                Event::Other
            }
            9 => {
                let address = self.address(read.number()?);
                read.number()?;
                let name = read.string()?;
                Event::Task { address, name }
            }
            14 => {
                read.string()?;
                Event::Other
            }
            21 => {
                read.numbers(4)?;
                Event::Other
            }
            22 => {
                read.numbers(2)?;
                read.string()?;
                Event::Other
            }
            // Of its own length: the number goes on, then the length.
            _ => {
                read.at = 0;
                let id = read.number()?;
                let length = read.number()?;
                if length > PACKET_MAX {
                    return Err(Short::Wrong);
                }
                let payload = read.take(length as usize)?;
                if id == 24 {
                    // What the firmware is: its clock, the CPU's, where task
                    // IDs count from, and how far they are shifted.
                    let mut init = Reader {
                        bytes: payload,
                        at: 0,
                    };
                    let clock_hz = init.number().map_err(|_| Short::Wrong)?;
                    init.number().map_err(|_| Short::Wrong)?;
                    self.base = u64::from(init.number().map_err(|_| Short::Wrong)?);
                    self.shift = init.number().map_err(|_| Short::Wrong)?.min(31);
                    Event::Init { clock_hz }
                } else {
                    Event::Other
                }
            }
        };
        let delta = read.number()?;
        Ok((event, delta, read.at))
    }
}

/// Turns SystemView's events into what a tracer takes from SWO: a task
/// switched to as a write of the running task's address, the exceptions as
/// the exception trace has them, and the ticks after each.
#[derive(Debug, Default)]
pub struct ToTrace {
    /// The exceptions entered and not left, the last on top.
    entered: Vec<u16>,
    /// The idle task's control block, which SystemView names no other way.
    pub idle: Option<u64>,
}

impl ToTrace {
    pub fn packets(&mut self, event: &Event, delta: u32, out: &mut Vec<ItmPacket>) {
        let switch = |task: u64| ItmPacket::Value {
            comparator: 0,
            write: true,
            value: task as u32,
        };
        match *event {
            Event::Running(task) => out.push(switch(task)),
            Event::Idle => out.extend(self.idle.map(switch)),
            Event::Entered(number) => {
                self.entered.push(number);
                out.push(ItmPacket::Exception {
                    number,
                    event: ExceptionEvent::Entered,
                });
            }
            Event::Exited => {
                if let Some(number) = self.entered.pop() {
                    out.push(ItmPacket::Exception {
                        number,
                        event: ExceptionEvent::Exited,
                    });
                    // Back in the thread.
                    if self.entered.is_empty() {
                        out.push(ItmPacket::Exception {
                            number: 0,
                            event: ExceptionEvent::Returned,
                        });
                    }
                }
            }
            Event::Overflow(_) => {
                self.entered.clear();
                out.push(ItmPacket::Overflow);
            }
            Event::Init { .. } | Event::Task { .. } | Event::Other => {}
        }
        out.push(ItmPacket::Time(delta));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As an STM32H503 running Zephyr sent it, once started: the sync, the
    /// start, what it is, its tasks, and the first events.
    const ZEPHYR: &str = "00 00 00 00 00 00 00 00 00 00 0a bd d9 9a 8b 0f 18 0e 80 b8 b8 72 80 b8 b8 72 80 80 80 80 02 00 \
        eb 03 0e 0f 4e 3d 5a 65 70 68 79 72 53 79 73 56 69 65 77 a8 04 0e 19 44 3d 6e 75 63 6c 65 6f 5f \
        68 35 30 33 72 62 20 73 74 6d 33 32 20 61 72 6d e9 04 0e 08 4f 3d 5a 65 70 68 79 72 85 04 0e 0d \
        43 3d 73 74 6d 33 32 68 35 30 33 78 78 d5 03 0d d1 de 9b 8b 0f 00 81 04 09 f8 2d 06 06 77 61 69 \
        74 65 72 bf 05 15 f8 2d a8 c8 80 80 02 80 08 00 83 03 09 f8 2a 03 04 70 6f 6e 67 83 05 15 f8 2a \
        a8 bc 80 80 02 80 06 00 f7 02 09 b8 2c 04 04 70 69 6e 67 83 05 15 b8 2c a8 c2 80 80 02 80 06 00 \
        f7 02 1b 01 00 8d 03 04 b8 2c 85 04 1c 01 29 c5 02 2a 0a cc aa 80 80 02 ff ff ff ff 0f b5 03 1c \
        02 2a 00 f3 02 02 42 85 03 03 d6 02";

    fn bytes(text: &str) -> Vec<u8> {
        text.split_whitespace()
            .map(|byte| u8::from_str_radix(byte, 16).unwrap())
            .collect()
    }

    fn interesting(events: &[(Event, u32)]) -> Vec<Event> {
        events
            .iter()
            .map(|(event, _)| event.clone())
            .filter(|event| *event != Event::Other)
            .collect()
    }

    #[test]
    fn zephyr_s_start_reads_as_sent() {
        let mut decoder = Decoder::new();
        let events = decoder.feed(&bytes(ZEPHYR));
        assert_eq!(
            interesting(&events),
            [
                Event::Init {
                    clock_hz: 240_000_000
                },
                Event::Task {
                    address: 0x2000_16F8,
                    name: "waiter".into()
                },
                Event::Task {
                    address: 0x2000_1578,
                    name: "pong".into()
                },
                Event::Task {
                    address: 0x2000_1638,
                    name: "ping".into()
                },
                Event::Running(0x2000_1638),
                Event::Entered(66),
                Event::Exited,
            ]
        );
        // The ticks after TIM7 was entered, until it was left.
        assert_eq!(events.last().unwrap().1, 0x56 | 2 << 7);
        assert!(decoder.pending.is_empty());
    }

    #[test]
    fn bytes_come_in_any_pieces() {
        let all = bytes(ZEPHYR);
        let mut whole = Decoder::new();
        let expected = whole.feed(&all);
        let mut decoder = Decoder::new();
        let mut events = Vec::new();
        for piece in all.chunks(7) {
            events.extend(decoder.feed(piece));
        }
        assert_eq!(events, expected);
    }

    #[test]
    fn nothing_is_read_before_the_sync() {
        let mut stream = bytes("04 b8 2c 85 04 02 42 85 03");
        stream.extend(bytes(ZEPHYR));
        let mut decoder = Decoder::new();
        let events = decoder.feed(&stream);
        assert_eq!(
            events.first().map(|(event, _)| event.clone()),
            Some(Event::Other)
        );
        assert!(
            events
                .iter()
                .any(|(event, _)| *event == Event::Running(0x2000_1638))
        );
    }

    #[test]
    fn events_turn_into_what_a_tracer_takes() {
        let mut to_trace = ToTrace {
            idle: Some(0x2000_0100),
            ..ToTrace::default()
        };
        let mut packets = Vec::new();
        for (event, delta) in [
            (Event::Running(0x2000_1638), 10),
            (Event::Entered(66), 5),
            (Event::Exited, 7),
            (Event::Idle, 3),
            (Event::Overflow(4), 2),
        ] {
            to_trace.packets(&event, delta, &mut packets);
        }
        assert_eq!(
            packets,
            [
                ItmPacket::Value {
                    comparator: 0,
                    write: true,
                    value: 0x2000_1638
                },
                ItmPacket::Time(10),
                ItmPacket::Exception {
                    number: 66,
                    event: ExceptionEvent::Entered
                },
                ItmPacket::Time(5),
                ItmPacket::Exception {
                    number: 66,
                    event: ExceptionEvent::Exited
                },
                ItmPacket::Exception {
                    number: 0,
                    event: ExceptionEvent::Returned
                },
                ItmPacket::Time(7),
                ItmPacket::Value {
                    comparator: 0,
                    write: true,
                    value: 0x2000_0100
                },
                ItmPacket::Time(3),
                ItmPacket::Overflow,
                ItmPacket::Time(2),
            ]
        );
    }
}
