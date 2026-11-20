//! Variables read from the chip as it runs, through whatever reaches its
//! memory: a probe an RTT or SWO session has open, or a debugger's server.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::elf::Kind;
use crate::rtos;

/// Watched variables this close to one another are read in one go.
const BLOCK_GAP: u64 = 32;
/// The most bytes read in one go.
const BLOCK_MAX: u64 = 1024;
/// How long the running task is sampled in one go, before the session does
/// its other work: taking the probe's core costs more than a sample.
const SAMPLING_SLICE: Duration = Duration::from_millis(5);

/// Fills a block of the chip's memory from an address, and says whether it could.
pub type MemoryRead<'a> = dyn FnMut(u64, &mut [u8]) -> bool + 'a;

/// A variable read from the chip as it runs, where its firmware keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Watched {
    pub address: u64,
    pub kind: Kind,
}

/// The variables a session reads as the chip runs, how often, and when
/// next; and its RTOS's tasks, read beside them, and which one runs, sampled.
pub struct Watcher {
    watch: Vec<Option<Watched>>,
    every: Duration,
    due: Instant,
    tasks: rtos::Reader,
    running: rtos::Sampler,
}

impl Default for Watcher {
    fn default() -> Self {
        Self {
            watch: Vec::new(),
            every: Duration::from_millis(50),
            due: Instant::now(),
            tasks: rtos::Reader::default(),
            running: rtos::Sampler::default(),
        }
    }
}

impl Watcher {
    /// Reads these from now on, every `every`; `None` stands for one not in
    /// the firmware. An empty list stops it.
    pub fn set(&mut self, watch: Vec<Option<Watched>>, every: Duration) {
        self.watch = watch;
        self.every = every.max(Duration::from_millis(1));
        self.due = Instant::now();
    }

    /// Reads the tasks of the RTOS laid out so, every `every`, and with
    /// `sample` which one runs, again and again: `None` stops both.
    pub fn set_tasks(&mut self, layout: Option<Arc<rtos::Layout>>, every: Duration, sample: bool) {
        self.running.set(layout.as_deref().filter(|_| sample));
        self.tasks.set(layout, every);
    }

    /// The chip was halted, or runs again: sampling which task runs stops
    /// while it is, the task marked halted.
    pub fn halt(&mut self, halted: bool) {
        self.running.halt(halted);
    }

    /// Looks through the tasks' stacks at one read of them in `every`, or
    /// else at one in five.
    pub fn stacks_every(&mut self, every: Option<u64>) {
        self.tasks.stacks_every(every);
    }

    /// Whether the running task is sampled: then there is no time to rest
    /// between reads.
    pub fn sampling(&self) -> bool {
        self.running.next_in() == Some(Duration::ZERO)
    }

    /// How long until the variables or the tasks are read next, or the
    /// running task sampled: `None` while none is.
    pub fn next_in(&self) -> Option<Duration> {
        let variables =
            (!self.watch.is_empty()).then(|| self.due.saturating_duration_since(Instant::now()));
        [variables, self.tasks.next_in(), self.running.next_in()]
            .into_iter()
            .flatten()
            .min()
    }

    /// Whether something is to be read now.
    pub fn due(&self) -> bool {
        self.next_in() == Some(Duration::ZERO)
    }

    /// Samples which task runs, while it is sampled, again and again for a
    /// moment; every so often, what was seen since the last time.
    pub fn running_if_due(
        &mut self,
        mut read: impl FnMut(u64, &mut [u8]) -> bool,
    ) -> Option<rtos::Running> {
        let until = Instant::now() + SAMPLING_SLICE;
        loop {
            let seen = self.running.sample(&mut read);
            if seen.is_some() || !self.sampling() || Instant::now() >= until {
                return seen;
            }
        }
    }

    /// The RTOS's tasks, when it is their time.
    pub fn tasks_if_due(
        &mut self,
        read: impl FnMut(u64, &mut [u8]) -> bool,
    ) -> Option<rtos::Snapshot> {
        self.tasks.read_if_due(read)
    }

    /// Their values, in their order, when it is their time. `read` fills a
    /// block of memory from an address and says whether it could.
    pub fn read_if_due(
        &mut self,
        read: impl FnMut(u64, &mut [u8]) -> bool,
    ) -> Option<Vec<Option<f64>>> {
        if self.watch.is_empty() || Instant::now() < self.due {
            return None;
        }
        // On time, but without catching up after a stall.
        self.due = (self.due + self.every).max(Instant::now());
        Some(read_blocks(&self.watch, read))
    }
}

/// Reads variables near one another in one block. A variable whose block
/// cannot be read, as one whose firmware changed, has no value.
fn read_blocks(
    watch: &[Option<Watched>],
    mut read: impl FnMut(u64, &mut [u8]) -> bool,
) -> Vec<Option<f64>> {
    let mut values = vec![None; watch.len()];
    let mut order: Vec<(usize, Watched)> = watch
        .iter()
        .enumerate()
        .filter_map(|(index, watched)| watched.map(|watched| (index, watched)))
        .collect();
    order.sort_by_key(|(_, watched)| watched.address);

    let mut first = 0;
    while first < order.len() {
        let start = order[first].1.address;
        let mut end = start + order[first].1.kind.size() as u64;
        let mut last = first;
        while let Some(&(_, next)) = order.get(last + 1) {
            let next_end = next.address + next.kind.size() as u64;
            if next.address > end + BLOCK_GAP || next_end - start > BLOCK_MAX {
                break;
            }
            end = end.max(next_end);
            last += 1;
        }
        let mut block = vec![0; (end - start) as usize];
        if read(start, &mut block) {
            for &(index, watched) in &order[first..=last] {
                let at = (watched.address - start) as usize;
                values[index] = Some(watched.kind.decode(&block[at..at + watched.kind.size()]));
            }
        }
        first = last + 1;
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watched(address: u64, kind: Kind) -> Option<Watched> {
        Some(Watched { address, kind })
    }

    #[test]
    fn near_ones_are_read_in_one_block_and_far_ones_apart() {
        let mut ram = vec![0u8; 0x500];
        ram[0..4].copy_from_slice(&1000u32.to_le_bytes());
        ram[4..8].copy_from_slice(&21.5f32.to_le_bytes());
        ram[0x400..0x402].copy_from_slice(&(-2i16).to_le_bytes());
        let mut reads = Vec::new();
        let values = read_blocks(
            &[
                watched(0x2000_0400, Kind::I16),
                watched(0x2000_0000, Kind::U32),
                None,
                watched(0x2000_0004, Kind::F32),
            ],
            |address, block| {
                reads.push((address, block.len()));
                let at = (address - 0x2000_0000) as usize;
                block.copy_from_slice(&ram[at..at + block.len()]);
                true
            },
        );
        assert_eq!(values, [Some(-2.0), Some(1000.0), None, Some(21.5)]);
        assert_eq!(reads, [(0x2000_0000, 8), (0x2000_0400, 2)]);
    }

    #[test]
    fn a_block_that_cannot_be_read_has_no_values() {
        let values = read_blocks(&[watched(0x3000_0000, Kind::U32)], |_, _| false);
        assert_eq!(values, [None]);
    }

    #[test]
    fn they_are_read_when_it_is_their_time() {
        let mut watcher = Watcher::default();
        assert_eq!(watcher.next_in(), None);
        assert!(watcher.read_if_due(|_, _| true).is_none());

        watcher.set(
            vec![watched(0x2000_0000, Kind::U8)],
            Duration::from_secs(60),
        );
        assert!(watcher.read_if_due(|_, _| true).is_some());
        // Not again until a minute is up.
        assert!(watcher.read_if_due(|_, _| true).is_none());
        assert!(
            watcher
                .next_in()
                .is_some_and(|left| left > Duration::from_secs(50))
        );
    }
}
