//! The probe and chip behind an RTT session, behind traits so tests can use pretend ones.

use std::fs;

use probe_rs::rtt::{Rtt, ScanRegion, find_rtt_control_block_in_raw_file};
use probe_rs::{MemoryInterface, Session};
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
    /// Finds the control block and the channels in it.
    fn find(&mut self, config: &RttConfig) -> Result<Attached, LinkProblem>;
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
        let session = probe::attach(&Target {
            probe: config.probe.as_ref(),
            chip: &config.chip,
            speed_khz: config.speed_khz,
            under_reset: config.under_reset,
        })?;
        Ok(Box::new(ProbeRsLink { session, rtt: None }))
    }
}

struct ProbeRsLink {
    session: Session,
    rtt: Option<Rtt>,
}

impl RttLink for ProbeRsLink {
    fn find(&mut self, config: &RttConfig) -> Result<Attached, LinkProblem> {
        let region = match (&config.elf, config.address) {
            (_, Some(address)) => ScanRegion::Exact(address),
            (Some(path), None) => ScanRegion::Exact(symbol_address(path)?),
            (None, None) => ScanRegion::Ram,
        };
        let mut core = self.session.core(0).map_err(lost)?;
        let mut rtt = Rtt::attach_region(&mut core, &region).map_err(from_rtt)?;
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
