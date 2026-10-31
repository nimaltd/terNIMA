//! Variables read from the chip as it runs, through whatever reaches its
//! memory: a probe an RTT or SWO session has open, or a debugger's server.

use std::time::{Duration, Instant};

use crate::elf::Kind;

/// Watched variables this close to one another are read in one go.
const BLOCK_GAP: u64 = 32;
/// The most bytes read in one go.
const BLOCK_MAX: u64 = 1024;

/// A variable read from the chip as it runs, where its firmware keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Watched {
    pub address: u64,
    pub kind: Kind,
}

/// The variables a session reads as the chip runs, how often, and when next.
pub struct Watcher {
    watch: Vec<Option<Watched>>,
    every: Duration,
    due: Instant,
}

impl Default for Watcher {
    fn default() -> Self {
        Self {
            watch: Vec::new(),
            every: Duration::from_millis(50),
            due: Instant::now(),
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

    /// How long until they are read next: `None` while nothing is watched.
    pub fn next_in(&self) -> Option<Duration> {
        (!self.watch.is_empty()).then(|| self.due.saturating_duration_since(Instant::now()))
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
