//! The tasks of a firmware's RTOS, read from the chip as it runs: Zephyr's
//! threads and FreeRTOS's tasks. They are found through the kernel's own
//! lists, with its structs laid out as the ELF file's debug information
//! says. Only reading: the firmware is not stopped.

mod debug;
mod freertos;
mod zephyr;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use debug::Field;

/// The most memory reads one look at the tasks makes, so a list gone wrong
/// cannot keep the probe busy.
const READS_MAX: usize = 600;
/// The stacks are looked through at one read of the tasks in this many: the
/// most a stack has used changes seldom, and it takes the most reads, which
/// keep the probe from other work, as from SWO.
const STACKS_EVERY: u64 = 5;

/// The RTOSes whose tasks are read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kernel {
    Zephyr,
    #[serde(rename = "freertos")]
    FreeRtos,
}

/// What a task is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// On the CPU when it was read.
    Running,
    /// Ready to run, waiting for the CPU.
    Ready,
    /// Waiting for something, such as a semaphore, a queue or a message.
    Blocked,
    /// Waiting for time to pass.
    Sleeping,
    /// Stopped until something resumes it.
    Suspended,
    /// Ended, and not yet cleaned up.
    Dead,
}

/// A task, or a thread in Zephyr's words.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Task {
    pub name: String,
    pub state: State,
    /// As the kernel numbers it: in Zephyr lower is more urgent and negative
    /// is cooperative, in FreeRTOS higher is more urgent.
    pub priority: i64,
    /// Its stack's size in bytes, where the kernel keeps it.
    pub stack_size: Option<u64>,
    /// The bytes of its stack in use when it last left the CPU.
    pub stack_used: Option<u64>,
    /// The fewest bytes of its stack ever free: those still holding the fill
    /// the kernel put there.
    pub stack_free: Option<u64>,
    /// Its share of the CPU since the read before, in percent.
    pub cpu: Option<f64>,
    /// Where its control block is, which tells apart tasks of one name.
    pub address: u64,
}

/// The kernel's heap, where it has one of its own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Heap {
    pub size: u64,
    pub free: u64,
    /// The least it has ever had free.
    pub lowest_free: Option<u64>,
}

/// The tasks as they were at one read.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Snapshot {
    pub kernel: Kernel,
    pub tasks: Vec<Task>,
    pub heap: Option<Heap>,
    /// Why there are no tasks: a scheduler not started yet, or a read that failed.
    pub problem: Option<String>,
}

impl Snapshot {
    fn empty(kernel: Kernel, problem: &str) -> Self {
        Self {
            kernel,
            tasks: Vec::new(),
            heap: None,
            problem: Some(problem.into()),
        }
    }
}

/// Where an RTOS keeps its tasks in a firmware, and how its structs are laid out.
#[derive(Debug)]
pub enum Layout {
    Zephyr(zephyr::Layout),
    FreeRtos(freertos::Layout),
}

/// An RTOS found in a firmware.
#[derive(Debug)]
pub struct Found {
    pub layout: Layout,
    /// What the firmware leaves out that would show more: the config
    /// options to turn on, with what each gives.
    pub missing: Vec<String>,
}

impl Layout {
    /// The RTOS in a firmware's ELF file: `None` when it has none of those read.
    pub fn find(elf: &[u8]) -> Result<Option<Found>, String> {
        let structs: Vec<&str> = zephyr::STRUCTS
            .iter()
            .chain(freertos::STRUCTS.iter())
            .copied()
            .collect();
        let debug = debug::read(elf, &structs, &zephyr::INSTANCES)?;
        Ok(zephyr::layout(&debug).or_else(|| freertos::layout(&debug)))
    }

    pub fn kernel(&self) -> Kernel {
        match self {
            Layout::Zephyr(_) => Kernel::Zephyr,
            Layout::FreeRtos(_) => Kernel::FreeRtos,
        }
    }

    /// The tasks now, with each one's run-time counter and its width in bytes.
    fn read(&self, memory: &mut Memory) -> (Snapshot, Vec<Option<(u64, u64)>>) {
        match self {
            Layout::Zephyr(layout) => zephyr::read(layout, memory),
            Layout::FreeRtos(layout) => freertos::read(layout, memory),
        }
    }

    /// Where the kernel keeps the address of the task running on the first
    /// core, and that address's size.
    pub fn running_at(&self) -> (u64, u64) {
        match self {
            Layout::Zephyr(layout) => zephyr::running_at(layout),
            Layout::FreeRtos(layout) => freertos::running_at(layout),
        }
    }
}

/// In a timeline's switches, where the chip was halted from, by the Live
/// view's Pause: no task ran until the next switch. No control block is at 1.
pub const HALTED: u64 = 1;

/// What sampling saw since it last told: the task switches a timeline draws.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Running {
    /// The whole ms since 1970 the times count from: each is that many ms
    /// after it. 0 for samples, timed in ms since 1970 themselves; a trace
    /// counts from when it began, which keeps its times to the nanosecond,
    /// where ms since 1970 keep a quarter of a microsecond.
    pub base: f64,
    /// When each task was first seen running, in ms after `base`, with the
    /// address of its control block, as a task's `address` is. 0 is for a
    /// time when what ran is not known: a read failed, or samples were too
    /// far apart.
    pub switches: Vec<(f64, u64)>,
    /// When the last sample was taken, in ms after `base`: the task last
    /// seen ran until then.
    pub until: f64,
    /// How many samples these come from.
    pub samples: u32,
    /// Whether these are every switch, traced by the chip, not samples.
    pub exact: bool,
}

/// How often what sampling saw is told.
const RUNNING_EVERY: Duration = Duration::from_millis(100);
/// Samples further apart than this leave a gap: what ran between them is not known.
const SAMPLES_APART_MS: f64 = 50.0;
/// How long sampling rests after a read that failed, so a probe that is gone
/// does not keep it spinning.
const SAMPLE_REST: Duration = Duration::from_millis(20);

/// Reads which task runs again and again, as fast as the probe answers, and
/// keeps when that changed. Each sample is one moment: a task that runs for
/// less than the time between two samples can go unseen.
#[derive(Debug)]
pub struct Sampler {
    /// Where the running task's address is, and its size; `None` while not sampling.
    at: Option<(u64, u64)>,
    /// The task seen last, and when.
    last: Option<(u64, f64)>,
    seen: Running,
    told: Instant,
    /// After a read that failed: not before then.
    rest_until: Option<Instant>,
    /// Whether the chip is halted, and the task that ran when it was, to run on after.
    halted: bool,
    resume: Option<u64>,
}

impl Default for Sampler {
    fn default() -> Self {
        Self {
            at: None,
            last: None,
            seen: Running::default(),
            told: Instant::now(),
            rest_until: None,
            halted: false,
            resume: None,
        }
    }
}

impl Sampler {
    /// Samples this kernel's running task from now on: `None` stops it.
    pub fn set(&mut self, layout: Option<&Layout>) {
        let at = layout.map(Layout::running_at);
        if at != self.at {
            *self = Self {
                at,
                halted: self.halted,
                ..Self::default()
            };
        }
    }

    /// The chip was halted, or runs again. Halted, nothing is sampled, and
    /// what ran stays marked halted; run again, it runs on.
    pub fn halt(&mut self, halted: bool) {
        let at = now_ms();
        if halted && !self.halted {
            self.halted = true;
            self.resume = self.last.map(|(task, _)| task).filter(|&task| task != 0);
            if self.at.is_some() {
                self.seen.switches.push((at, HALTED));
                self.seen.until = at;
            }
        } else if !halted && self.halted {
            self.halted = false;
            let task = self.resume.take();
            if self.at.is_some() {
                self.seen.switches.push((at, task.unwrap_or(0)));
                self.seen.until = at;
            }
            // Sampled on from now, with no gap back to before the halt.
            self.last = task.map(|task| (task, at));
        }
    }

    /// How long until the next sample: `None` while not sampling.
    pub fn next_in(&self) -> Option<Duration> {
        self.at?;
        if self.halted {
            return Some(RUNNING_EVERY.saturating_sub(self.told.elapsed()));
        }
        Some(self.rest_until.map_or(Duration::ZERO, |until| {
            until.saturating_duration_since(Instant::now())
        }))
    }

    /// Takes a sample when sampling, and every so often gives what was seen
    /// since it last did. `read` fills a block of memory from an address and
    /// says whether it could.
    pub fn sample(&mut self, mut read: impl FnMut(u64, &mut [u8]) -> bool) -> Option<Running> {
        self.at?;
        // Halted, the chip runs nothing: told now and then, it stays so until now.
        if self.halted {
            if self.told.elapsed() < RUNNING_EVERY {
                return None;
            }
            self.told = Instant::now();
            self.seen.until = now_ms();
            return Some(std::mem::take(&mut self.seen));
        }
        if self.rest_until.is_some_and(|until| Instant::now() < until) {
            return None;
        }
        self.take(&mut read);
        if self.told.elapsed() < RUNNING_EVERY {
            return None;
        }
        self.told = Instant::now();
        Some(std::mem::take(&mut self.seen))
    }

    /// Samples once more, while sampling, without giving what was seen:
    /// between the blocks of a long read, as of the tasks and their stacks,
    /// so that the samples go on and leave no gap.
    pub fn sample_between(&mut self, read: &mut impl FnMut(u64, &mut [u8]) -> bool) {
        let resting = self.rest_until.is_some_and(|until| Instant::now() < until);
        if self.at.is_some() && !self.halted && !resting {
            self.take(read);
        }
    }

    /// Reads which task runs, and keeps it: a read that failed is a time not
    /// known, and a rest before the next.
    fn take(&mut self, read: &mut impl FnMut(u64, &mut [u8]) -> bool) {
        let Some((address, size)) = self.at else {
            return;
        };
        self.rest_until = None;
        let mut bytes = [0; 8];
        let read_right = read(address, &mut bytes[..size.clamp(1, 8) as usize]);
        let task = if read_right {
            u64::from_le_bytes(bytes)
        } else {
            self.rest_until = Some(Instant::now() + SAMPLE_REST);
            0
        };
        self.add(task, now_ms());
    }

    /// Keeps a sample: a switch where another task runs, or where sampling
    /// starts again after a gap.
    fn add(&mut self, task: u64, at: f64) {
        self.seen.samples += 1;
        match self.last {
            Some((_, last_at)) if at - last_at > SAMPLES_APART_MS => {
                self.seen.switches.push((last_at, 0));
                self.seen.switches.push((at, task));
            }
            Some((last, _)) if last == task => {}
            _ => self.seen.switches.push((at, task)),
        }
        self.last = Some((task, at));
        self.seen.until = at;
    }
}

/// Now, in ms since 1970, as the app times what sessions read.
fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64() * 1000.0)
}

/// Reads the tasks every so often, with each one's share of the CPU between reads.
pub struct Reader {
    layout: Option<Arc<Layout>>,
    every: Duration,
    due: Instant,
    /// Each task's run-time counter at the read before, by its address.
    counters: HashMap<u64, u64>,
    /// How many reads there were, for the stacks to be looked through at some.
    reads: u64,
    /// Each stack's bytes never used, by its task's address, as last looked through.
    never_used: HashMap<u64, u64>,
    /// The stacks are looked through at one read in this many.
    stacks_every: u64,
}

impl Default for Reader {
    fn default() -> Self {
        Self {
            layout: None,
            every: Duration::from_secs(1),
            due: Instant::now(),
            counters: HashMap::new(),
            reads: 0,
            never_used: HashMap::new(),
            stacks_every: STACKS_EVERY,
        }
    }
}

impl Reader {
    /// Looks through the stacks at one read in `every`, the longest part of
    /// a read, or else at one in five.
    pub fn stacks_every(&mut self, every: Option<u64>) {
        self.stacks_every = every.unwrap_or(STACKS_EVERY).max(1);
    }

    /// Reads these tasks from now on, every `every`: `None` stops it.
    pub fn set(&mut self, layout: Option<Arc<Layout>>, every: Duration) {
        self.layout = layout;
        self.every = every.max(Duration::from_millis(100));
        self.due = Instant::now();
        self.counters.clear();
        self.reads = 0;
        self.never_used.clear();
    }

    /// How long until they are read next: `None` while nothing is read.
    pub fn next_in(&self) -> Option<Duration> {
        self.layout
            .is_some()
            .then(|| self.due.saturating_duration_since(Instant::now()))
    }

    /// The tasks, when it is their time. `read` fills a block of memory from
    /// an address and says whether it could.
    pub fn read_if_due(
        &mut self,
        mut read: impl FnMut(u64, &mut [u8]) -> bool,
    ) -> Option<Snapshot> {
        let layout = self.layout.clone()?;
        if Instant::now() < self.due {
            return None;
        }
        self.due = (self.due + self.every).max(Instant::now());
        let stacks = self.reads.is_multiple_of(self.stacks_every);
        self.reads += 1;
        let mut memory = Memory {
            read: &mut read,
            left: READS_MAX,
            stacks,
        };
        let (mut snapshot, counters) = layout.read(&mut memory);
        self.share_cpu(&mut snapshot.tasks, &counters);
        for task in &mut snapshot.tasks {
            if stacks {
                match task.stack_free {
                    Some(free) => self.never_used.insert(task.address, free),
                    None => self.never_used.remove(&task.address),
                };
            } else {
                task.stack_free = self.never_used.get(&task.address).copied();
            }
        }
        Some(snapshot)
    }

    /// Each task's share of what all of them ran since the read before.
    fn share_cpu(&mut self, tasks: &mut [Task], counters: &[Option<(u64, u64)>]) {
        let mut ran = Vec::with_capacity(tasks.len());
        let mut now = HashMap::new();
        for (task, counter) in tasks.iter().zip(counters) {
            let Some((value, width)) = *counter else {
                ran.push(None);
                continue;
            };
            now.insert(task.address, value);
            ran.push(self.counters.get(&task.address).map(|&before| {
                // A 32-bit counter wraps around.
                if width < 8 {
                    value.wrapping_sub(before) & ((1u64 << (width * 8)) - 1)
                } else {
                    value.wrapping_sub(before)
                }
            }));
        }
        let total: u64 = ran.iter().flatten().sum();
        if total > 0 {
            for (task, ran) in tasks.iter_mut().zip(&ran) {
                task.cpu = ran.map(|ran| ran as f64 * 100.0 / total as f64);
            }
        }
        self.counters = now;
    }
}

/// The chip's memory as a look at the tasks reads it.
pub(crate) struct Memory<'a> {
    read: &'a mut dyn FnMut(u64, &mut [u8]) -> bool,
    /// How many more reads it may make.
    left: usize,
    /// Whether the stacks are looked through for the kernel's fill, which
    /// takes the most reads.
    stacks: bool,
}

impl Memory<'_> {
    fn bytes(&mut self, address: u64, len: u64) -> Option<Vec<u8>> {
        if self.left == 0 || address == 0 || len == 0 || len > 0x1_0000 {
            return None;
        }
        self.left -= 1;
        let mut block = vec![0; len as usize];
        (self.read)(address, &mut block).then_some(block)
    }

    fn word(&mut self, address: u64) -> Option<u64> {
        self.bytes(address, 4)
            .map(|bytes| u64::from(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])))
    }
}

/// A field's value as an unsigned number, little-endian.
fn unsigned(bytes: &[u8], field: Field) -> Option<u64> {
    let start = usize::try_from(field.offset).ok()?;
    let size = usize::try_from(field.size).ok()?;
    let part = bytes.get(start..start.checked_add(size)?)?;
    if part.is_empty() || size > 8 {
        return None;
    }
    let mut word = [0; 8];
    word[..size].copy_from_slice(part);
    Some(u64::from_le_bytes(word))
}

/// A field's value as a signed number.
fn signed(bytes: &[u8], field: Field) -> Option<i64> {
    let value = unsigned(bytes, field)?;
    let bits = field.size * 8;
    Some(if bits < 64 {
        ((value << (64 - bits)) as i64) >> (64 - bits)
    } else {
        value as i64
    })
}

/// A field's text, up to its first NUL.
fn text(bytes: &[u8], field: Field) -> String {
    let start = field.offset as usize;
    let end = (start + field.size as usize).min(bytes.len());
    let part = bytes.get(start..end).unwrap_or_default();
    let part = part.split(|&byte| byte == 0).next().unwrap_or_default();
    String::from_utf8_lossy(part).trim().to_string()
}

/// The bytes at the bottom of a stack still holding the fill the kernel put
/// there: the fewest ever free. `None` for a stack that was not filled, or
/// not looked through this time.
fn never_used(memory: &mut Memory, start: u64, size: u64, fill: u8) -> Option<u64> {
    if !memory.stacks {
        return None;
    }
    const CHUNK: u64 = 256;
    let mut free = 0;
    while free < size {
        let len = CHUNK.min(size - free);
        let block = memory.bytes(start + free, len)?;
        match block.iter().position(|&byte| byte != fill) {
            Some(at) => {
                free += at as u64;
                break;
            }
            None => free += len,
        }
    }
    (free >= 4).then_some(free)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_read_as_numbers_and_text() {
        let bytes = [0xFE, 0x34, 0x12, b'm', b'a', b'i', b'n', 0, b'x'];
        assert_eq!(unsigned(&bytes, Field { offset: 1, size: 2 }), Some(0x1234));
        assert_eq!(signed(&bytes, Field { offset: 0, size: 1 }), Some(-2));
        assert_eq!(text(&bytes, Field { offset: 3, size: 6 }), "main");
        assert_eq!(unsigned(&bytes, Field { offset: 8, size: 4 }), None);
    }

    #[test]
    fn the_free_part_of_a_stack_is_its_fill_at_the_bottom() {
        let mut ram = vec![0xAAu8; 600];
        ram[300..].fill(0x11);
        let mut read = |address: u64, block: &mut [u8]| {
            let at = (address - 0x2000_0000) as usize;
            block.copy_from_slice(&ram[at..at + block.len()]);
            true
        };
        let mut memory = Memory {
            read: &mut read,
            left: 10,
            stacks: true,
        };
        assert_eq!(never_used(&mut memory, 0x2000_0000, 600, 0xAA), Some(300));
        // Not filled: nothing to tell.
        assert_eq!(never_used(&mut memory, 0x2000_0000 + 300, 200, 0xAA), None);
    }

    fn task(address: u64) -> Task {
        Task {
            name: String::new(),
            state: State::Ready,
            priority: 0,
            stack_size: None,
            stack_used: None,
            stack_free: None,
            cpu: None,
            address,
        }
    }

    #[test]
    fn each_task_has_its_share_of_the_cpu_between_reads() {
        let mut reader = Reader::default();
        let mut tasks = vec![task(1), task(2)];
        reader.share_cpu(&mut tasks, &[Some((100, 4)), Some((1000, 4))]);
        // Nothing to compare with yet.
        assert_eq!(tasks[0].cpu, None);
        // A 32-bit counter that wrapped around still counts.
        reader.share_cpu(&mut tasks, &[Some((400, 4)), Some((1900, 4))]);
        assert_eq!(tasks[0].cpu, Some(25.0));
        assert_eq!(tasks[1].cpu, Some(75.0));
        let mut wrapped = vec![task(1)];
        reader.counters = HashMap::from([(1, u64::from(u32::MAX) - 9)]);
        reader.share_cpu(&mut wrapped, &[Some((10, 4))]);
        assert_eq!(wrapped[0].cpu, Some(100.0));
    }

    #[test]
    fn samples_keep_only_the_switches() {
        let mut sampler = Sampler::default();
        sampler.add(0x2000_0100, 1000.0);
        sampler.add(0x2000_0100, 1000.5);
        sampler.add(0x2000_0200, 1001.0);
        sampler.add(0x2000_0200, 1001.5);
        sampler.add(0x2000_0100, 1002.0);
        assert_eq!(
            sampler.seen.switches,
            [
                (1000.0, 0x2000_0100),
                (1001.0, 0x2000_0200),
                (1002.0, 0x2000_0100)
            ]
        );
        assert_eq!(sampler.seen.until, 1002.0);
        assert_eq!(sampler.seen.samples, 5);
    }

    #[test]
    fn samples_far_apart_leave_a_gap() {
        let mut sampler = Sampler::default();
        sampler.add(0x2000_0100, 1000.0);
        // The probe was busy elsewhere for a while: what ran then is not known.
        sampler.add(0x2000_0100, 1200.0);
        assert_eq!(
            sampler.seen.switches,
            [(1000.0, 0x2000_0100), (1000.0, 0), (1200.0, 0x2000_0100)]
        );
    }

    #[test]
    fn a_halt_stops_sampling_and_the_task_runs_on_after_it() {
        let mut sampler = Sampler {
            at: Some((0x2000_0460, 4)),
            ..Sampler::default()
        };
        sampler.add(0x2000_0208, 1000.0);
        sampler.halt(true);
        // Nothing is read while halted.
        let mut asked = false;
        sampler.sample(|_, _| {
            asked = true;
            true
        });
        assert!(!asked);
        assert!(sampler.next_in().is_some_and(|wait| wait > Duration::ZERO));
        sampler.halt(false);
        let tasks: Vec<u64> = sampler
            .seen
            .switches
            .iter()
            .map(|&(_, task)| task)
            .collect();
        assert_eq!(tasks, [0x2000_0208, HALTED, 0x2000_0208]);
    }

    #[test]
    fn samples_between_the_blocks_of_a_long_read_are_kept_for_later() {
        let mut sampler = Sampler {
            at: Some((0x2000_0460, 4)),
            ..Sampler::default()
        };
        let mut read = |_: u64, block: &mut [u8]| {
            block.copy_from_slice(&0x2000_0208u32.to_le_bytes());
            true
        };
        sampler.sample_between(&mut read);
        sampler.sample_between(&mut read);
        // Kept, and given with the next sample's.
        assert_eq!(sampler.seen.samples, 2);
        let tasks: Vec<u64> = sampler
            .seen
            .switches
            .iter()
            .map(|&(_, task)| task)
            .collect();
        assert_eq!(tasks, [0x2000_0208]);
        // Halted, nothing is read between either.
        sampler.halt(true);
        let mut asked = false;
        sampler.sample_between(&mut |_, _| {
            asked = true;
            true
        });
        assert!(!asked);
        // Nor while not sampling.
        let mut idle = Sampler::default();
        idle.sample_between(&mut |_, _| {
            asked = true;
            true
        });
        assert!(!asked);
    }

    #[test]
    fn a_failed_read_is_a_time_not_known_and_a_rest() {
        let mut sampler = Sampler {
            at: Some((0x2000_0460, 4)),
            ..Sampler::default()
        };
        sampler.sample(|_, block| {
            block.copy_from_slice(&0x2000_0208u32.to_le_bytes());
            true
        });
        sampler.sample(|_, _| false);
        assert_eq!(sampler.seen.switches.len(), 2);
        assert_eq!(sampler.seen.switches[0].1, 0x2000_0208);
        assert_eq!(sampler.seen.switches[1].1, 0);
        // Resting: nothing is read until the rest is over.
        let mut asked = false;
        sampler.sample(|_, _| {
            asked = true;
            true
        });
        assert!(!asked);
        assert!(sampler.next_in().is_some_and(|wait| wait > Duration::ZERO));
    }
}
