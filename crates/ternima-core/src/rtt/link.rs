//! The probe and chip behind an RTT session, behind traits so tests can use pretend ones.

use std::fs;
use std::ops::Range;

use probe_rs::config::MemoryRegion;
use probe_rs::rtt::{Rtt, find_rtt_control_block_in_raw_file};
use probe_rs::{Core, MemoryInterface, Session};
use serde::Serialize;

use super::config::RttConfig;
use crate::probe::{self, Target};

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
    /// Writes into a down channel as much as fits, and says how much that was.
    fn write(&mut self, channel: usize, data: &[u8]) -> Result<usize, LinkProblem>;
    /// Whether the control block is still where it was found. A reset or new
    /// firmware can clear it or move it.
    fn still_there(&mut self) -> Result<bool, LinkProblem>;
    fn reset(&mut self) -> Result<(), LinkProblem>;
    /// Reads the chip's memory while it runs, as the watched variables are read.
    fn read_memory(&mut self, address: u64, buf: &mut [u8]) -> Result<(), LinkProblem>;
    /// Whether the chip can still be reached through the probe, for a
    /// session that only reads variables and has no control block to check.
    fn reachable(&mut self) -> Result<(), LinkProblem>;
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
        }))
    }
}

struct ProbeRsLink {
    session: Session,
    rtt: Option<Rtt>,
    /// Where the control block was found last, looked at first when it is
    /// looked for again: after a reset, the firmware puts it there again.
    found: Option<u64>,
}

impl RttLink for ProbeRsLink {
    fn find(&mut self, config: &RttConfig, quick: bool) -> Result<Attached, LinkProblem> {
        let exact = match (&config.elf, config.address) {
            (_, Some(address)) => Some(address),
            (Some(path), None) => Some(symbol_address(path)?),
            (None, None) => None,
        };
        let mut core = self.session.core(0).map_err(lost)?;
        let mut rtt = match exact {
            Some(address) => Rtt::attach_at(&mut core, address).map_err(from_rtt)?,
            None => find_in_ram(&mut core, self.found, quick)?,
        };
        let up = rtt
            .up_channels()
            .iter()
            .map(|channel| ChannelInfo {
                number: channel.number(),
                name: channel.name().map(String::from),
                size: channel.buffer_size(),
            })
            .collect();
        let down = rtt
            .down_channels()
            .iter()
            .map(|channel| ChannelInfo {
                number: channel.number(),
                name: channel.name().map(String::from),
                size: channel.buffer_size(),
            })
            .collect();
        let address = rtt.ptr();
        self.rtt = Some(rtt);
        self.found = Some(address);
        Ok(Attached { address, up, down })
    }

    fn read(&mut self, channel: usize, buf: &mut [u8]) -> Result<usize, LinkProblem> {
        let rtt = self.rtt.as_mut().ok_or_else(not_found)?;
        let mut core = self.session.core(0).map_err(lost)?;
        let up = rtt.up_channel(channel).ok_or_else(|| {
            LinkProblem::NoControlBlock(format!("The firmware has no up channel {channel}."))
        })?;
        up.read(&mut core, buf).map_err(from_rtt)
    }

    fn write(&mut self, channel: usize, data: &[u8]) -> Result<usize, LinkProblem> {
        let rtt = self.rtt.as_mut().ok_or_else(not_found)?;
        let mut core = self.session.core(0).map_err(lost)?;
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
        let mut core = self.session.core(0).map_err(lost)?;
        let mut id = [0; 16];
        core.read_8(address, &mut id).map_err(lost)?;
        Ok(id == Rtt::RTT_ID)
    }

    fn reset(&mut self) -> Result<(), LinkProblem> {
        self.rtt = None;
        let mut core = self.session.core(0).map_err(lost)?;
        core.reset().map_err(lost)
    }

    fn read_memory(&mut self, address: u64, buf: &mut [u8]) -> Result<(), LinkProblem> {
        let mut core = self.session.core(0).map_err(lost)?;
        core.read_8(address, buf).map_err(lost)
    }

    fn reachable(&mut self) -> Result<(), LinkProblem> {
        let mut core = self.session.core(0).map_err(lost)?;
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
}
