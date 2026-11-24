//! The probe and chip behind an RTT session, behind traits so tests can use pretend ones.

use std::fs;
use std::ops::Range;
use std::time::{Duration, Instant};

use probe_rs::config::MemoryRegion;
use probe_rs::rtt::{Rtt, find_rtt_control_block_in_raw_file};
use probe_rs::{Core, MemoryInterface, Session};
use serde::Serialize;

use super::config::RttConfig;
use crate::awake::Awake;
use crate::probe::{self, Target};
use crate::watch::MemoryRead;

/// One RTT channel, as the firmware set it up.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ChannelInfo {
    pub number: usize,
    pub name: Option<String>,
    /// Its buffer size in bytes.
    pub size: usize,
}

/// A control block that was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attached {
    pub address: u64,
    pub up: Vec<ChannelInfo>,
    pub down: Vec<ChannelInfo>,
}

/// Why talking to the chip stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkProblem {
    /// The probe or the chip is gone: start again from the probe.
    Lost(String),
    /// There is no control block, or no longer: search for it again.
    NoControlBlock(String),
}

/// An up channel read now and then through a long read of memory, as of
/// the tasks and their stacks: the firmware's buffer for it fills
/// meanwhile, and once full, what the firmware sends is dropped.
pub struct Drain<'a> {
    pub channel: usize,
    /// What was read, added to.
    pub into: &'a mut Vec<u8>,
    /// When the channel was read last: it is read again `every` after.
    pub read_at: Instant,
    pub every: Duration,
}

/// How much is read of a drained channel at most, where its size is not known.
const DRAIN_ROOM: usize = 16 * 1024;

impl Drain<'_> {
    fn due(&self) -> bool {
        self.read_at.elapsed() >= self.every
    }

    /// Reads the channel with `read`, given room for `room` bytes, and keeps
    /// what it gave. What cannot be read now is left for the next read.
    fn take(
        &mut self,
        room: usize,
        read: impl FnOnce(usize, &mut [u8]) -> Result<usize, LinkProblem>,
    ) {
        let start = self.into.len();
        self.into.resize(start + room, 0);
        let count = read(self.channel, &mut self.into[start..]).unwrap_or(0);
        self.into.truncate(start + count);
        self.read_at = Instant::now();
    }
}

/// Opens probes and attaches to chips.
pub trait RttBackend: Send + Sync + 'static {
    /// Opens the probe and attaches to the chip. Gives `Err(None)` when the
    /// probe is not plugged in, and the reason when it is but did not work.
    fn attach(&self, config: &RttConfig) -> Result<Box<dyn RttLink>, Option<String>>;
}

/// A chip attached through a probe.
pub trait RttLink: Send {
    /// Finds the control block and the channels in it. A `quick` look only
    /// checks where it was found last, once it has been: after a reset the
    /// firmware sets it up there again, and searching all of RAM can take
    /// seconds at a slow SWD clock.
    fn find(&mut self, config: &RttConfig, quick: bool) -> Result<Attached, LinkProblem>;
    /// Reads what an up channel holds, without waiting.
    fn read(&mut self, channel: usize, buf: &mut [u8]) -> Result<usize, LinkProblem>;
    /// Reads what two up channels hold, as `read` does, with the core taken
    /// once, which is quicker: for SystemView's channel beside the terminal's.
    fn read_two(
        &mut self,
        first: (usize, &mut [u8]),
        second: (usize, &mut [u8]),
    ) -> Result<(usize, usize), LinkProblem> {
        Ok((self.read(first.0, first.1)?, self.read(second.0, second.1)?))
    }
    /// Writes into a down channel as much as fits, and says how much that was.
    fn write(&mut self, channel: usize, data: &[u8]) -> Result<usize, LinkProblem>;
    /// Whether the control block is still where it was found. A reset or new
    /// firmware can clear it or move it.
    fn still_there(&mut self) -> Result<bool, LinkProblem>;
    /// Whether the firmware has set the control block up again since it was
    /// found, with other channels. Moments after a reset, it can be found
    /// before the firmware sets it up, with the last run's names and buffers.
    fn changed(&mut self) -> Result<bool, LinkProblem> {
        Ok(false)
    }
    /// Whether a read since the control block was found saw a channel's
    /// buffer moved, and read nothing from it: the firmware has set the block
    /// up again, and it is to be found again at once. Asks nothing of the chip.
    fn moved(&self) -> bool {
        false
    }
    fn reset(&mut self) -> Result<(), LinkProblem>;
    /// Reads the chip's memory while it runs, as the watched variables are read.
    fn read_memory(&mut self, address: u64, buf: &mut [u8]) -> Result<(), LinkProblem>;
    /// Reads the chip's memory many times over with the core taken once,
    /// which is quicker: `work` gets a reader that fills a block from an
    /// address and says whether it could. Between blocks, `drain`'s channel
    /// is read whenever it is due.
    fn read_many(
        &mut self,
        work: &mut dyn FnMut(&mut MemoryRead<'_>),
        mut drain: Option<&mut Drain<'_>>,
    ) {
        work(&mut |address, block| {
            if let Some(drain) = drain.as_deref_mut()
                && drain.due()
            {
                drain.take(DRAIN_ROOM, |channel, buf| self.read(channel, buf));
            }
            self.read_memory(address, block).is_ok()
        });
    }
    /// Whether the chip can still be reached through the probe, for a
    /// session that only reads variables and has no control block to check.
    fn reachable(&mut self) -> Result<(), LinkProblem>;
    /// Halts the core, or has it run again.
    fn halt(&mut self, halt: bool) -> Result<(), LinkProblem> {
        let _ = halt;
        Ok(())
    }
}

/// Real probes, through probe-rs.
pub struct ProbeRs;

impl RttBackend for ProbeRs {
    fn attach(&self, config: &RttConfig) -> Result<Box<dyn RttLink>, Option<String>> {
        let mut session = probe::attach(&Target {
            probe: config.probe.as_ref(),
            chip: &config.chip,
            speed_khz: config.speed_khz,
            under_reset: config.under_reset,
        })?;
        if config.under_reset {
            // Stopped at its first instruction, the firmware would never set up RTT.
            session.resume_all_cores().map_err(|error| {
                Some(format!(
                    "Could not start {} after the reset: {error}",
                    config.chip.trim()
                ))
            })?;
        }
        Ok(Box::new(ProbeRsLink {
            session,
            rtt: None,
            found: None,
            layout: Layout::default(),
            moved: false,
            awake: Awake::new(&config.chip),
        }))
    }
}

struct ProbeRsLink {
    session: Session,
    rtt: Option<Rtt>,
    /// Where the control block was found last, looked at first when it is
    /// looked for again: after a reset, the firmware puts it there again.
    found: Option<u64>,
    /// Its channels as they were when it was found.
    layout: Layout,
    /// A read found a channel's buffer moved since.
    moved: bool,
    /// Keeps a sleeping STM32's RAM readable.
    awake: Awake,
}

/// The chip's core, with an STM32's DMA clock checked first, so that its
/// memory reads right while it sleeps.
fn awake_core<'a>(session: &'a mut Session, awake: &mut Awake) -> Result<Core<'a>, LinkProblem> {
    let mut core = session.core(0).map_err(lost)?;
    awake.check(&mut core);
    Ok(core)
}

impl RttLink for ProbeRsLink {
    fn find(&mut self, config: &RttConfig, quick: bool) -> Result<Attached, LinkProblem> {
        let exact = match (&config.elf, config.address) {
            (_, Some(address)) => Some(address),
            (Some(path), None) => Some(symbol_address(path)?),
            (None, None) => None,
        };
        let mut core = awake_core(&mut self.session, &mut self.awake)?;
        let mut rtt = match exact {
            Some(address) => Rtt::attach_at(&mut core, address).map_err(from_rtt)?,
            None => find_in_ram(&mut core, self.found, quick)?,
        };
        let up = rtt
            .up_channels()
            .iter()
            .map(|channel| ChannelInfo {
                number: channel.number(),
                name: plausible(channel.name()),
                size: channel.buffer_size(),
            })
            .collect();
        let down = rtt
            .down_channels()
            .iter()
            .map(|channel| ChannelInfo {
                number: channel.number(),
                name: plausible(channel.name()),
                size: channel.buffer_size(),
            })
            .collect();
        let address = rtt.ptr();
        let pointer = pointer_width(&core);
        self.layout = layout(&mut core, address, pointer)?;
        self.moved = false;
        self.rtt = Some(rtt);
        self.found = Some(address);
        Ok(Attached { address, up, down })
    }

    fn read(&mut self, channel: usize, buf: &mut [u8]) -> Result<usize, LinkProblem> {
        self.rtt.as_ref().ok_or_else(not_found)?;
        let up = self.layout.up(channel)?;
        let read = {
            let mut core = awake_core(&mut self.session, &mut self.awake)?;
            read_up(&mut core, &up, buf)?
        };
        self.moved |= read.is_none();
        Ok(read.unwrap_or(0))
    }

    fn read_two(
        &mut self,
        (first, first_buf): (usize, &mut [u8]),
        (second, second_buf): (usize, &mut [u8]),
    ) -> Result<(usize, usize), LinkProblem> {
        self.rtt.as_ref().ok_or_else(not_found)?;
        let (first_up, second_up) = (self.layout.up(first)?, self.layout.up(second)?);
        let (first, second) = {
            let mut core = awake_core(&mut self.session, &mut self.awake)?;
            let first = read_up(&mut core, &first_up, first_buf)?;
            (first, read_up(&mut core, &second_up, second_buf)?)
        };
        self.moved |= first.is_none() || second.is_none();
        Ok((first.unwrap_or(0), second.unwrap_or(0)))
    }

    fn write(&mut self, channel: usize, data: &[u8]) -> Result<usize, LinkProblem> {
        let rtt = self.rtt.as_mut().ok_or_else(not_found)?;
        let mut core = awake_core(&mut self.session, &mut self.awake)?;
        let down = rtt.down_channel(channel).ok_or_else(|| {
            LinkProblem::NoControlBlock(format!("The firmware has no down channel {channel}."))
        })?;
        down.write(&mut core, data).map_err(from_rtt)
    }

    fn still_there(&mut self) -> Result<bool, LinkProblem> {
        let Some(rtt) = &self.rtt else {
            return Ok(false);
        };
        let address = rtt.ptr();
        let mut core = awake_core(&mut self.session, &mut self.awake)?;
        let mut id = [0; 16];
        core.read_8(address, &mut id).map_err(lost)?;
        Ok(id == Rtt::RTT_ID)
    }

    fn changed(&mut self) -> Result<bool, LinkProblem> {
        let Some(rtt) = &self.rtt else {
            return Ok(false);
        };
        let address = rtt.ptr();
        let mut core = awake_core(&mut self.session, &mut self.awake)?;
        let pointer = pointer_width(&core);
        Ok(layout(&mut core, address, pointer)?.fixed != self.layout.fixed)
    }

    fn moved(&self) -> bool {
        self.moved
    }

    fn reset(&mut self) -> Result<(), LinkProblem> {
        self.rtt = None;
        let mut core = self.session.core(0).map_err(lost)?;
        core.reset().map_err(lost)?;
        // A reset turns the DMA clock off.
        self.awake.after_reset();
        Ok(())
    }

    fn halt(&mut self, halt: bool) -> Result<(), LinkProblem> {
        let mut core = self.session.core(0).map_err(lost)?;
        if halt {
            core.halt(HALT_WAIT).map(|_| ()).map_err(lost)
        } else {
            core.run().map_err(lost)
        }
    }

    fn read_memory(&mut self, address: u64, buf: &mut [u8]) -> Result<(), LinkProblem> {
        let mut core = awake_core(&mut self.session, &mut self.awake)?;
        // In words where it can: a byte at a time is many times slower.
        core.read(address, buf).map_err(lost)
    }

    fn read_many(
        &mut self,
        work: &mut dyn FnMut(&mut MemoryRead<'_>),
        mut drain: Option<&mut Drain<'_>>,
    ) {
        let up = drain
            .as_deref()
            .filter(|_| self.rtt.is_some())
            .and_then(|drain| self.layout.up(drain.channel).ok());
        let moved = {
            let Ok(mut core) = awake_core(&mut self.session, &mut self.awake) else {
                return work(&mut |_, _| false);
            };
            let mut moved = false;
            work(&mut |address, block| {
                if let (Some(drain), Some(up)) = (drain.as_deref_mut(), up.as_ref())
                    && drain.due()
                {
                    drain.take(up.size as usize, |_, buf| {
                        let read = read_up(&mut core, up, buf)?;
                        moved |= read.is_none();
                        Ok(read.unwrap_or(0))
                    });
                }
                core.read(address, block).is_ok()
            });
            moved
        };
        self.moved |= moved;
    }

    fn reachable(&mut self) -> Result<(), LinkProblem> {
        let mut core = awake_core(&mut self.session, &mut self.awake)?;
        core.status().map(|_| ()).map_err(lost)
    }
}

/// How much RAM is read first while looking for the control block, from
/// the start of each range. Each block after it is twice as big, up to
/// `SCAN_MAX`, so the rest of a big RAM takes few reads.
const SCAN_BLOCK: u64 = 4096;
const SCAN_MAX: u64 = 64 * 1024;

/// Finds the control block in RAM without an ELF file: where it was last
/// time first, then, unless `quick`, RAM a block at a time, stopping at the
/// first one. Firmware most often has it near the start of RAM, so little of
/// RAM is read, where probe-rs's own search reads all of it: seconds on a
/// chip with much RAM, or at a slow SWD clock.
fn find_in_ram(core: &mut Core, last: Option<u64>, quick: bool) -> Result<Rtt, LinkProblem> {
    if let Some(address) = last {
        if let Ok(rtt) = Rtt::attach_at(core, address) {
            return Ok(rtt);
        }
        if quick {
            return not_in_ram(core);
        }
    }
    let ranges: Vec<Range<u64>> = core
        .memory_regions()
        .filter_map(MemoryRegion::as_ram_region)
        .filter(|region| !region.is_alias)
        .map(|region| region.range.clone())
        .collect();
    let mut from = 0;
    while let Some(address) = scan(&ranges, from, |address, block| {
        core.read(address, block).is_ok()
    }) {
        if let Ok(rtt) = Rtt::attach_at(core, address) {
            return Ok(rtt);
        }
        // Something else that reads like the ID: look on after it.
        from = address + 1;
    }
    not_in_ram(core)
}

/// Says there is no control block, or that the probe is gone: one that is
/// gone reads nothing either, and is then attached again.
fn not_in_ram(core: &mut Core) -> Result<Rtt, LinkProblem> {
    core.status().map_err(lost)?;
    Err(from_rtt(probe_rs::rtt::Error::ControlBlockNotFound))
}

/// The address of the first control block ID at or after `from` in these
/// ranges. RAM is read a block at a time, from the start of each range;
/// `read` fills a block from an address and says whether it could. The rest
/// of a range that cannot be read is left out, as probe-rs leaves out a range.
fn scan(
    ranges: &[Range<u64>],
    from: u64,
    mut read: impl FnMut(u64, &mut [u8]) -> bool,
) -> Option<u64> {
    let id = &Rtt::RTT_ID;
    // Each block reaches into the next by an ID's length, so an ID across two is found.
    let reach = id.len() as u64;
    for range in ranges {
        // Word reads are quicker, so blocks start on a word.
        let mut at = range.start.max(from & !3);
        let mut size = SCAN_BLOCK;
        while at < range.end {
            let mut block = vec![0; ((at + size + reach).min(range.end) - at) as usize];
            if !read(at, &mut block) {
                break;
            }
            let found = block
                .windows(id.len())
                .enumerate()
                .map(|(offset, window)| (at + offset as u64, window))
                .find(|&(address, window)| address >= from && window == id);
            if let Some((address, _)) = found {
                return Some(address);
            }
            at += size;
            size = (size * 2).min(SCAN_MAX);
        }
    }
    None
}

/// The longest name a channel is taken to have: the firmware's are short,
/// as Terminal or SysView are.
const NAME_MAX: usize = 32;

/// A channel's name, or none for what cannot be one. Read from a control
/// block the firmware has not set up yet, a name's pointer can point
/// anywhere, as into the text of an assert in the new firmware's flash.
fn plausible(name: Option<&str>) -> Option<String> {
    name.filter(|name| {
        !name.is_empty()
            && name.len() <= NAME_MAX
            && name.bytes().all(|byte| (0x20..0x7f).contains(&byte))
    })
    .map(String::from)
}

/// The chip's memory as a control block is read through it: a core, or a
/// test's RAM.
trait Memory {
    fn load(&mut self, address: u64, buf: &mut [u8]) -> Result<(), LinkProblem>;
    fn store_32(&mut self, address: u64, value: u32) -> Result<(), LinkProblem>;
}

impl Memory for Core<'_> {
    fn load(&mut self, address: u64, buf: &mut [u8]) -> Result<(), LinkProblem> {
        // In words where it can: a byte at a time is many times slower.
        self.read(address, buf).map_err(lost)
    }

    fn store_32(&mut self, address: u64, value: u32) -> Result<(), LinkProblem> {
        self.write_word_32(address, value).map_err(lost)
    }
}

/// How many bytes a pointer takes in the chip's control block.
fn pointer_width(core: &Core) -> usize {
    if core.is_64_bit() { 8 } else { 4 }
}

/// What the firmware writes once in a control block, as it sets it up: how
/// many channels it has, and for each, where its name and its buffer are and
/// how big that is. The offsets that move as data goes are left out.
#[derive(Debug, Default)]
struct Layout {
    /// The numbers of up and down channels, then each channel's name and
    /// buffer pointers and size, as they are in the chip.
    fixed: Vec<u8>,
    /// Each up channel's buffer, by number.
    up: Vec<UpBuffer>,
}

impl Layout {
    /// Up channel `number`'s buffer, as the control block had it.
    fn up(&self, number: usize) -> Result<UpBuffer, LinkProblem> {
        self.up
            .get(number)
            .filter(|up| up.buffer != 0)
            .copied()
            .ok_or_else(|| {
                LinkProblem::NoControlBlock(format!("The firmware has no up channel {number}."))
            })
    }
}

/// An up channel's buffer and its size, as the control block had them, and
/// where they are in it: the write and read offsets follow them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct UpBuffer {
    at: u64,
    /// How many bytes a pointer takes.
    pointer: usize,
    buffer: u64,
    size: u32,
}

/// The little-endian word at `at`.
fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// The little-endian pointer of `width` bytes at `at`.
fn pointer_at(bytes: &[u8], at: usize, width: usize) -> u64 {
    let mut wide = [0; 8];
    wide[..width].copy_from_slice(&bytes[at..at + width]);
    u64::from_le_bytes(wide)
}

fn layout(memory: &mut impl Memory, address: u64, pointer: usize) -> Result<Layout, LinkProblem> {
    // The ID, then the numbers of up and down channels.
    let mut header = [0; 24];
    memory.load(address, &mut header)?;
    let count = |at: usize| (word(&header, at) as i32).clamp(0, 255) as usize;
    let (up, down) = (count(16), count(20));
    // A name's and a buffer's pointer, then the size, write and read offsets and flags.
    let each = 2 * pointer + 16;
    let mut channels = vec![0; (up + down) * each];
    memory.load(address + 24, &mut channels)?;
    let mut fixed = header[16..].to_vec();
    for channel in channels.chunks(each) {
        fixed.extend_from_slice(&channel[..2 * pointer + 4]);
    }
    let up = channels
        .chunks(each)
        .take(up)
        .enumerate()
        .map(|(number, channel)| UpBuffer {
            at: address + 24 + (number * each + pointer) as u64,
            pointer,
            buffer: pointer_at(channel, pointer, pointer),
            size: word(channel, 2 * pointer),
        })
        .collect();
    Ok(Layout { fixed, up })
}

/// Reads what an up channel holds, without waiting, and moves its read
/// offset past it, as probe-rs does; `None` when the channel's buffer is not
/// the one found, and nothing is read. Moments after a reset, the control
/// block can be found before the firmware sets it up, with the last run's
/// buffers. Once it is set up, what the firmware writes is in its new buffer:
/// read from the old one, as probe-rs reads, it would be lost. So the
/// buffer's pointer and size are read with the offsets, in one read.
fn read_up(
    memory: &mut impl Memory,
    up: &UpBuffer,
    buf: &mut [u8],
) -> Result<Option<usize>, LinkProblem> {
    let width = up.pointer;
    let mut fields = [0; 20];
    let fields = &mut fields[..width + 12];
    memory.load(up.at, fields)?;
    if (pointer_at(fields, 0, width), word(fields, width)) != (up.buffer, up.size) {
        return Ok(None);
    }
    let (size, write, mut read) = (up.size, word(fields, width + 4), word(fields, width + 8));
    if write >= size || read >= size {
        return Err(LinkProblem::NoControlBlock(format!(
            "An up channel's offsets are past its buffer of {size} bytes: write {write}, read {read}."
        )));
    }
    let mut total = 0;
    // To the end of the buffer, then on from its start where the data goes round.
    while read != write && total < buf.len() {
        let end = if read > write { size } else { write };
        let count = ((end - read) as usize).min(buf.len() - total);
        memory.load(up.buffer + u64::from(read), &mut buf[total..total + count])?;
        total += count;
        read += count as u32;
        if read == size {
            read = 0;
        }
    }
    if total > 0 {
        memory.store_32(up.at + width as u64 + 8, read)?;
    }
    Ok(Some(total))
}

/// The address of the `_SEGGER_RTT` symbol in an ELF file.
fn symbol_address(path: &str) -> Result<u64, LinkProblem> {
    let data = fs::read(path)
        .map_err(|error| LinkProblem::NoControlBlock(format!("Could not read {path}: {error}")))?;
    match find_rtt_control_block_in_raw_file(&data) {
        Ok(Some(address)) => Ok(address),
        Ok(None) => Err(LinkProblem::NoControlBlock(format!(
            "{path} has no _SEGGER_RTT symbol: is RTT in that firmware?"
        ))),
        Err(error) => Err(LinkProblem::NoControlBlock(format!(
            "{path} is not an ELF file: {error}"
        ))),
    }
}

fn not_found() -> LinkProblem {
    LinkProblem::NoControlBlock("The control block has not been found yet.".into())
}

/// The longest a core takes to halt.
const HALT_WAIT: Duration = Duration::from_millis(500);

fn lost(error: probe_rs::Error) -> LinkProblem {
    LinkProblem::Lost(error.to_string())
}

fn from_rtt(error: probe_rs::rtt::Error) -> LinkProblem {
    match error {
        probe_rs::rtt::Error::Probe(error) => LinkProblem::Lost(error.to_string()),
        probe_rs::rtt::Error::ControlBlockNotFound => LinkProblem::NoControlBlock(
            "No RTT control block in the chip's RAM yet. Is RTT on in the firmware, and has it started?"
                .into(),
        ),
        other => LinkProblem::NoControlBlock(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAM: u64 = 0x2000_0000;

    #[test]
    fn a_name_is_kept_only_if_it_can_be_one() {
        assert_eq!(plausible(Some("Terminal")).as_deref(), Some("Terminal"));
        assert_eq!(
            plausible(Some("JScope_T4F4")).as_deref(),
            Some("JScope_T4F4")
        );
        // What an old pointer found in new firmware's flash.
        assert_eq!(
            plausible(Some(
                "0 && ((p_xfer_desc->type == NRFX_TWIM_XFER_TXRX) || (p_xfer_desc->type == NRFX_TWIM_XFER_TXTX)))"
            )),
            None
        );
        assert_eq!(plausible(Some("Ter\u{1}minal")), None);
        assert_eq!(plausible(Some("Termin\u{e4}l")), None);
        assert_eq!(plausible(Some("")), None);
        assert_eq!(plausible(None), None);
    }

    /// RAM from `RAM`, as a control block is read through it.
    struct Ram(Vec<u8>);

    impl Memory for Ram {
        fn load(&mut self, address: u64, buf: &mut [u8]) -> Result<(), LinkProblem> {
            let at = (address - RAM) as usize;
            buf.copy_from_slice(&self.0[at..at + buf.len()]);
            Ok(())
        }

        fn store_32(&mut self, address: u64, value: u32) -> Result<(), LinkProblem> {
            let at = (address - RAM) as usize;
            self.0[at..at + 4].copy_from_slice(&value.to_le_bytes());
            Ok(())
        }
    }

    impl Ram {
        fn put(&mut self, address: u64, words: &[u32]) {
            for (index, &word) in words.iter().enumerate() {
                self.store_32(address + 4 * index as u64, word).unwrap();
            }
        }
    }

    /// A control block at `RAM` as 32-bit firmware sets it up, with an up
    /// and a down channel. The up channel's 16 bytes, "a" to "p", are at
    /// `RAM + 0x100`, written to `write` and read to `read`.
    fn control_block(write: u32, read: u32) -> Ram {
        let mut ram = Ram(vec![0; 0x200]);
        ram.0[..16].copy_from_slice(&Rtt::RTT_ID);
        ram.put(RAM + 16, &[1, 1]);
        // Each channel's name, buffer, size, write and read offsets and flags.
        let buffer = RAM as u32 + 0x100;
        ram.put(RAM + 24, &[0x0800_1000, buffer, 16, write, read, 0]);
        ram.put(RAM + 48, &[0x0800_1000, buffer + 0x80, 16, 0, 0, 0]);
        for (index, byte) in ram.0[0x100..0x110].iter_mut().enumerate() {
            *byte = b'a' + index as u8;
        }
        ram
    }

    /// The up channel's read offset.
    const READ_OFFSET: usize = 24 + 16;

    #[test]
    fn an_up_channel_is_read_to_the_end_of_its_buffer_and_on_from_its_start() {
        let mut ram = control_block(3, 12);
        let up = layout(&mut ram, RAM, 4).unwrap().up(0).unwrap();
        assert_eq!(
            up,
            UpBuffer {
                at: RAM + 28,
                pointer: 4,
                buffer: RAM + 0x100,
                size: 16
            }
        );
        let mut buf = [0; 64];
        assert_eq!(read_up(&mut ram, &up, &mut buf), Ok(Some(7)));
        assert_eq!(&buf[..7], b"mnopabc");
        // Read past it, so nothing is left.
        assert_eq!(word(&ram.0, READ_OFFSET), 3);
        assert_eq!(read_up(&mut ram, &up, &mut buf), Ok(Some(0)));
    }

    #[test]
    fn an_up_channel_is_read_as_far_as_there_is_room() {
        let mut ram = control_block(3, 12);
        let up = layout(&mut ram, RAM, 4).unwrap().up(0).unwrap();
        let mut buf = [0; 5];
        assert_eq!(read_up(&mut ram, &up, &mut buf), Ok(Some(5)));
        assert_eq!(&buf, b"mnopa");
        assert_eq!(word(&ram.0, READ_OFFSET), 1);
    }

    #[test]
    fn a_channel_whose_buffer_moved_is_not_read() {
        // Found before the firmware set it up: the last run's buffer.
        let mut ram = control_block(0, 0);
        let up = layout(&mut ram, RAM, 4).unwrap().up(0).unwrap();
        // The firmware sets it up with another buffer, and writes 5 bytes there.
        ram.put(RAM + 28, &[RAM as u32 + 0x140, 16, 5, 0]);
        let mut buf = [0; 64];
        assert_eq!(read_up(&mut ram, &up, &mut buf), Ok(None));
        // Left for the read once it is found again.
        assert_eq!(word(&ram.0, READ_OFFSET), 0);
        let again = layout(&mut ram, RAM, 4).unwrap().up(0).unwrap();
        assert_eq!(read_up(&mut ram, &again, &mut buf), Ok(Some(5)));
    }

    #[test]
    fn offsets_past_the_buffer_are_no_control_block() {
        let mut ram = control_block(16, 0);
        let up = layout(&mut ram, RAM, 4).unwrap().up(0).unwrap();
        assert!(matches!(
            read_up(&mut ram, &up, &mut [0; 8]),
            Err(LinkProblem::NoControlBlock(_))
        ));
    }

    #[test]
    fn the_layout_changes_with_a_name_or_a_buffer_but_not_as_data_goes() {
        let mut ram = control_block(3, 12);
        let found = layout(&mut ram, RAM, 4).unwrap().fixed;
        // The up channel's write and read offsets.
        ram.put(RAM + 36, &[9, 2]);
        assert_eq!(layout(&mut ram, RAM, 4).unwrap().fixed, found);
        // The down channel's name.
        ram.put(RAM + 48, &[0x0800_2000]);
        assert_ne!(layout(&mut ram, RAM, 4).unwrap().fixed, found);
    }

    #[test]
    fn a_channel_without_a_buffer_is_not_there() {
        let mut ram = control_block(0, 0);
        ram.put(RAM + 28, &[0]);
        let found = layout(&mut ram, RAM, 4).unwrap();
        assert!(found.up(0).is_err());
        assert!(found.up(1).is_err());
    }

    /// 32 KiB of RAM at 0x2000_0000, with control block IDs at these offsets.
    fn ram(ids: &[usize]) -> Vec<u8> {
        let mut ram = vec![0xa5; 0x8000];
        for &at in ids {
            ram[at..at + Rtt::RTT_ID.len()].copy_from_slice(&Rtt::RTT_ID);
        }
        ram
    }

    /// Reads `ram`, adding up how many bytes were read.
    fn reader<'a>(ram: &'a [u8], read: &'a mut usize) -> impl FnMut(u64, &mut [u8]) -> bool + 'a {
        move |address, block| {
            let at = (address - RAM) as usize;
            block.copy_from_slice(&ram[at..at + block.len()]);
            *read += block.len();
            true
        }
    }

    /// The RAM of an STM32H503: two ranges of 16 KiB.
    const H503: [Range<u64>; 2] = [RAM..RAM + 0x4000, RAM + 0x4000..RAM + 0x8000];

    #[test]
    fn a_control_block_near_the_start_is_found_without_reading_all_of_ram() {
        let ram = ram(&[0xc00]);
        let mut read = 0;
        assert_eq!(scan(&H503, 0, reader(&ram, &mut read)), Some(RAM + 0xc00));
        assert!(read <= 4096 + 16, "{read} bytes read");
    }

    #[test]
    fn an_id_across_two_blocks_is_found() {
        // Blocks of 4, 8 and 4 KiB in each range: the last one ends with the range.
        for at in [0xff8, 0x2ff8, 0x3ff0, 0x4ff8, 0x6ff8] {
            let ram = ram(&[at]);
            assert_eq!(scan(&H503, 0, reader(&ram, &mut 0)), Some(RAM + at as u64));
        }
    }

    #[test]
    fn the_search_goes_on_after_one_and_into_the_next_range() {
        let ram = ram(&[0x100, 0x5000]);
        assert_eq!(scan(&H503, 0, reader(&ram, &mut 0)), Some(RAM + 0x100));
        assert_eq!(
            scan(&H503, RAM + 0x101, reader(&ram, &mut 0)),
            Some(RAM + 0x5000)
        );
        assert_eq!(scan(&H503, RAM + 0x5001, reader(&ram, &mut 0)), None);
    }

    #[test]
    fn a_range_that_cannot_be_read_is_left_out() {
        let ram = ram(&[0x1000, 0x6000]);
        let mut read = 0;
        let mut all = reader(&ram, &mut read);
        let found = scan(&H503, 0, |address, block| {
            address >= RAM + 0x4000 && all(address, block)
        });
        assert_eq!(found, Some(RAM + 0x6000));
    }

    #[test]
    fn ram_without_a_control_block_is_read_once() {
        let ram = ram(&[]);
        let mut read = 0;
        assert_eq!(scan(&H503, 0, reader(&ram, &mut read)), None);
        // Each block reaches 16 bytes into the next, but not past its range.
        assert_eq!(read, 0x8000 + 4 * 16);
    }

    #[test]
    fn a_big_ram_takes_few_reads() {
        let ram = vec![0; 1024 * 1024];
        let all = RAM..RAM + ram.len() as u64;
        let mut reads = 0;
        let found = scan(std::slice::from_ref(&all), 0, |address, block| {
            reads += 1;
            let at = (address - RAM) as usize;
            block.copy_from_slice(&ram[at..at + block.len()]);
            true
        });
        assert_eq!(found, None);
        // 4, 8, 16 and 32 KiB, then 64 KiB at a time.
        assert_eq!(reads, 4 + (1024 - 60) / 64 + 1);
    }

    /// A chip whose channel always has more, telling what was read in turn.
    #[derive(Default)]
    struct Busy {
        reads: Vec<&'static str>,
    }

    impl RttLink for Busy {
        fn find(&mut self, _config: &RttConfig, _quick: bool) -> Result<Attached, LinkProblem> {
            unreachable!()
        }

        fn read(&mut self, channel: usize, buf: &mut [u8]) -> Result<usize, LinkProblem> {
            assert_eq!(channel, 1);
            self.reads.push("channel");
            buf[..3].copy_from_slice(b"abc");
            Ok(3)
        }

        fn write(&mut self, _channel: usize, _data: &[u8]) -> Result<usize, LinkProblem> {
            Ok(0)
        }

        fn still_there(&mut self) -> Result<bool, LinkProblem> {
            Ok(true)
        }

        fn reset(&mut self) -> Result<(), LinkProblem> {
            Ok(())
        }

        fn read_memory(&mut self, _address: u64, buf: &mut [u8]) -> Result<(), LinkProblem> {
            self.reads.push("memory");
            buf.fill(0);
            Ok(())
        }

        fn reachable(&mut self) -> Result<(), LinkProblem> {
            Ok(())
        }
    }

    fn read_blocks(link: &mut Busy, drain: Option<&mut Drain<'_>>) {
        link.read_many(
            &mut |read| {
                for address in 0..3 {
                    assert!(read(address, &mut [0; 4]));
                }
            },
            drain,
        );
    }

    #[test]
    fn a_long_read_of_memory_reads_the_drained_channel_between_its_blocks() {
        let mut link = Busy::default();
        let mut drained = Vec::new();
        let before = Instant::now();
        let mut drain = Drain {
            channel: 1,
            into: &mut drained,
            read_at: before,
            every: Duration::ZERO,
        };
        read_blocks(&mut link, Some(&mut drain));
        assert!(drain.read_at > before);
        assert_eq!(drained, b"abcabcabc");
        assert_eq!(
            link.reads,
            [
                "channel", "memory", "channel", "memory", "channel", "memory"
            ]
        );
    }

    #[test]
    fn a_channel_read_lately_is_not_read_again_yet() {
        let mut link = Busy::default();
        let mut drained = Vec::new();
        let mut drain = Drain {
            channel: 1,
            into: &mut drained,
            read_at: Instant::now(),
            every: Duration::from_secs(3600),
        };
        read_blocks(&mut link, Some(&mut drain));
        read_blocks(&mut link, None);
        assert!(drained.is_empty());
        assert_eq!(link.reads, ["memory"; 6]);
    }
}
