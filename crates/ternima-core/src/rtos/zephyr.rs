//! Zephyr's threads: the kernel's list of them (CONFIG_THREAD_MONITOR), or
//! without it the k_thread structs the firmware defines, as those
//! K_THREAD_DEFINE makes, its main thread and its idle threads.

use super::debug::{Debug, Field};
use super::{Found, Kernel, Layout as Any, Memory, Snapshot, State, Task};
use super::{never_used, signed, text, unsigned};

pub(super) const STRUCTS: [&str; 2] = ["k_thread", "z_kernel"];
pub(super) const INSTANCES: [&str; 1] = ["k_thread"];

/// What CONFIG_INIT_STACKS fills the stacks with.
const FILL: u8 = 0xAA;
/// The most threads followed on the kernel's list.
const THREADS_MAX: usize = 64;

// The bits of a thread's state, as kernel_structs.h has them.
const DUMMY: u64 = 1 << 0;
const PENDING: u64 = 1 << 1;
const SLEEPING: u64 = 1 << 2;
const DEAD: u64 = 1 << 3;
const SUSPENDED: u64 = 1 << 4;
const ABORTING: u64 = 1 << 5;

/// Where Zephyr keeps its threads in a firmware.
#[derive(Debug)]
pub struct Layout {
    /// `_kernel`'s address.
    kernel: u64,
    /// In `_kernel`: the thread on the first CPU.
    current: Field,
    /// In `_kernel`: the first thread on the kernel's list.
    threads: Option<Field>,
    /// The threads the firmware defines, by their variables' names.
    known: Vec<(String, u64)>,
    thread: Thread,
}

/// The parts of a `struct k_thread` read.
#[derive(Debug)]
struct Thread {
    size: u64,
    state: Field,
    priority: Field,
    next: Option<Field>,
    name: Option<Field>,
    stack_start: Option<Field>,
    stack_size: Option<Field>,
    /// Its stack pointer, saved when it last left the CPU.
    sp: Option<Field>,
    /// The cycles it has run (CONFIG_SCHED_THREAD_USAGE).
    usage: Option<Field>,
}

pub(super) fn layout(debug: &Debug) -> Option<Found> {
    let &(kernel, _) = debug.data.get("_kernel")?;
    let kernel_struct = debug.structs.get("z_kernel")?;
    let thread = debug.structs.get("k_thread")?;
    let field = |path: &str| thread.field(path);
    let layout = Layout {
        kernel,
        current: kernel_struct.field("cpus[0].current")?,
        threads: kernel_struct.field("threads"),
        known: debug.instances.get("k_thread").cloned().unwrap_or_default(),
        thread: Thread {
            size: thread.size,
            state: field("base.thread_state")?,
            priority: field("base.prio")?,
            next: field("next_thread"),
            name: field("name"),
            stack_start: field("stack_info.start"),
            stack_size: field("stack_info.size"),
            sp: field("callee_saved.psp").or_else(|| field("callee_saved.sp")),
            usage: field("base.usage.total"),
        },
    };
    let mut missing = Vec::new();
    if layout.threads.is_none() || layout.thread.next.is_none() {
        missing.push("CONFIG_THREAD_MONITOR=y, for the threads made as it runs".to_string());
    }
    if layout.thread.name.is_none() {
        missing.push("CONFIG_THREAD_NAME=y, for their names".to_string());
    }
    if layout.thread.stack_start.is_none() || layout.thread.stack_size.is_none() {
        missing.push("CONFIG_THREAD_STACK_INFO=y, for their stacks".to_string());
    }
    if layout.thread.usage.is_none() {
        missing.push("CONFIG_THREAD_RUNTIME_STATS=y, for their share of the CPU".to_string());
    }
    Some(Found {
        layout: Any::Zephyr(layout),
        missing,
    })
}

/// Where `_kernel` keeps the thread on the first CPU, and its size.
pub(super) fn running_at(layout: &Layout) -> (u64, u64) {
    (layout.kernel + layout.current.offset, layout.current.size)
}

pub(super) fn read(layout: &Layout, memory: &mut Memory) -> (Snapshot, Vec<Option<(u64, u64)>>) {
    let fields = [Some(layout.current), layout.threads];
    let header = fields
        .iter()
        .flatten()
        .map(|field| field.offset + field.size)
        .max()
        .unwrap_or(0);
    let Some(kernel) = memory.bytes(layout.kernel, header) else {
        return (
            Snapshot::empty(Kernel::Zephyr, "The kernel could not be read."),
            Vec::new(),
        );
    };
    let current = unsigned(&kernel, layout.current).unwrap_or(0);
    if current == 0 {
        return (
            Snapshot::empty(Kernel::Zephyr, "The kernel has not started yet."),
            Vec::new(),
        );
    }

    let thread = &layout.thread;
    // Each thread read once, whole: the next one's address is in it.
    let mut threads: Vec<(u64, Vec<u8>)> = Vec::new();
    match (layout.threads, thread.next) {
        (Some(first), Some(next)) => {
            let mut at = unsigned(&kernel, first).unwrap_or(0);
            while at != 0
                && threads.len() < THREADS_MAX
                && !threads.iter().any(|&(address, _)| address == at)
            {
                let Some(bytes) = memory.bytes(at, thread.size) else {
                    break;
                };
                let following = unsigned(&bytes, next).unwrap_or(0);
                threads.push((at, bytes));
                at = following;
            }
        }
        _ => {
            for &(_, address) in &layout.known {
                if let Some(bytes) = memory.bytes(address, thread.size) {
                    threads.push((address, bytes));
                }
            }
        }
    }

    let mut tasks = Vec::new();
    let mut counters = Vec::new();
    for (address, bytes) in threads {
        let bits = unsigned(&bytes, thread.state).unwrap_or(0);
        // A dummy thread stands in for none, as when the kernel starts.
        if bits & DUMMY != 0 {
            continue;
        }
        let state = if address == current {
            State::Running
        } else if bits & (DEAD | ABORTING) != 0 {
            State::Dead
        } else if bits & SUSPENDED != 0 {
            State::Suspended
        } else if bits & PENDING != 0 {
            State::Blocked
        } else if bits & SLEEPING != 0 {
            State::Sleeping
        } else {
            State::Ready
        };
        let mut name = thread
            .name
            .map(|field| text(&bytes, field))
            .unwrap_or_default();
        if name.is_empty() {
            name = known_name(layout, address);
        }
        let start = thread.stack_start.and_then(|field| unsigned(&bytes, field));
        let size = thread.stack_size.and_then(|field| unsigned(&bytes, field));
        let (stack_used, stack_free) = match (start, size) {
            (Some(start), Some(size)) if size > 0 => {
                // The stack grows down from its top; the running thread's saved one is old.
                let used = thread
                    .sp
                    .and_then(|field| unsigned(&bytes, field))
                    .filter(|&sp| state != State::Running && sp >= start && sp <= start + size)
                    .map(|sp| start + size - sp);
                (used, never_used(memory, start, size, FILL))
            }
            _ => (None, None),
        };
        counters.push(
            thread
                .usage
                .and_then(|field| unsigned(&bytes, field).map(|value| (value, field.size))),
        );
        tasks.push(Task {
            name,
            state,
            priority: signed(&bytes, thread.priority).unwrap_or(0),
            stack_size: size.filter(|&size| size > 0),
            stack_used,
            stack_free,
            cpu: None,
            address,
        });
    }
    let snapshot = Snapshot {
        kernel: Kernel::Zephyr,
        tasks,
        heap: None,
        problem: None,
    };
    (snapshot, counters)
}

/// A thread without a name of its own, named after its variable: `blink`
/// for K_THREAD_DEFINE(blink, ...), `main` and `idle` for the kernel's own.
fn known_name(layout: &Layout, address: u64) -> String {
    let Some((name, _)) = layout.known.iter().find(|&&(_, at)| at == address) else {
        return format!("thread at {address:#010x}");
    };
    if name == "z_main_thread" {
        return "main".into();
    }
    if name.starts_with("z_idle_threads") {
        return "idle".into();
    }
    name.strip_prefix("_k_thread_obj_")
        .unwrap_or(name)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A firmware's RAM, from 0x2000_0000.
    struct Ram(Vec<u8>);

    impl Ram {
        fn put(&mut self, address: u64, bytes: &[u8]) {
            let at = (address - 0x2000_0000) as usize;
            self.0[at..at + bytes.len()].copy_from_slice(bytes);
        }

        fn word(&mut self, address: u64, value: u32) {
            self.put(address, &value.to_le_bytes());
        }

        fn read(&self, address: u64, block: &mut [u8]) -> bool {
            let Some(at) = address.checked_sub(0x2000_0000) else {
                return false;
            };
            let at = at as usize;
            match self.0.get(at..at + block.len()) {
                Some(bytes) => {
                    block.copy_from_slice(bytes);
                    true
                }
                None => false,
            }
        }
    }

    fn field(offset: u64, size: u64) -> Field {
        Field { offset, size }
    }

    /// A k_thread: state at 0, prio at 2, next at 8, name at 12 (16 bytes),
    /// stack start and size at 28 and 32, psp at 36, usage at 40.
    fn layout(threads: bool) -> Layout {
        Layout {
            kernel: 0x2000_0000,
            current: field(8, 4),
            threads: threads.then_some(field(24, 4)),
            known: vec![
                ("z_main_thread".into(), 0x2000_0100),
                ("_k_thread_obj_blink".into(), 0x2000_0200),
            ],
            thread: Thread {
                size: 48,
                state: field(0, 1),
                priority: field(2, 1),
                next: Some(field(8, 4)),
                name: Some(field(12, 16)),
                stack_start: Some(field(28, 4)),
                stack_size: Some(field(32, 4)),
                sp: Some(field(36, 4)),
                usage: Some(field(40, 8)),
            },
        }
    }

    fn thread(
        ram: &mut Ram,
        at: u64,
        state: u8,
        prio: i8,
        next: u32,
        name: &str,
        stack: (u32, u32, u32),
    ) {
        ram.put(at, &[state, 0, prio as u8]);
        ram.word(at + 8, next);
        ram.put(at + 12, name.as_bytes());
        ram.word(at + 28, stack.0);
        ram.word(at + 32, stack.1);
        ram.word(at + 36, stack.2);
    }

    fn firmware() -> Ram {
        let mut ram = Ram(vec![0; 0x2000]);
        // main runs; blink sleeps; idle is ready.
        ram.word(0x2000_0000 + 8, 0x2000_0100);
        ram.word(0x2000_0000 + 24, 0x2000_0100);
        thread(
            &mut ram,
            0x2000_0100,
            0x80,
            0,
            0x2000_0200,
            "main",
            (0x2000_1000, 0x400, 0),
        );
        thread(
            &mut ram,
            0x2000_0200,
            1 << 2,
            7,
            0x2000_0300,
            "blink",
            (0x2000_1400, 0x200, 0x2000_1580),
        );
        thread(
            &mut ram,
            0x2000_0300,
            0x80,
            15,
            0,
            "idle",
            (0x2000_1600, 0x100, 0x2000_16C0),
        );
        // CONFIG_INIT_STACKS: blink's stack has gone 0x100 deep at most.
        ram.put(0x2000_1400, &[FILL; 0x100]);
        ram
    }

    #[test]
    fn threads_are_read_from_the_kernels_list() {
        let ram = firmware();
        let mut fetch = |address, block: &mut [u8]| ram.read(address, block);
        let mut memory = Memory {
            read: &mut fetch,
            left: 100,
            stacks: true,
        };
        let (snapshot, counters) = read_threads(&layout(true), &mut memory);
        let names: Vec<_> = snapshot
            .tasks
            .iter()
            .map(|task| task.name.as_str())
            .collect();
        assert_eq!(names, ["main", "blink", "idle"]);
        let blink = &snapshot.tasks[1];
        assert_eq!(blink.state, State::Sleeping);
        assert_eq!(blink.priority, 7);
        assert_eq!(blink.stack_size, Some(0x200));
        assert_eq!(blink.stack_used, Some(0x80));
        assert_eq!(blink.stack_free, Some(0x100));
        assert_eq!(snapshot.tasks[0].state, State::Running);
        // The running thread's saved stack pointer is old: not told.
        assert_eq!(snapshot.tasks[0].stack_used, None);
        assert_eq!(snapshot.tasks[2].state, State::Ready);
        assert_eq!(counters.len(), 3);
    }

    fn read_threads(layout: &Layout, memory: &mut Memory) -> (Snapshot, Vec<Option<(u64, u64)>>) {
        read(layout, memory)
    }

    #[test]
    fn without_the_list_the_threads_the_firmware_defines_are_read() {
        let mut ram = firmware();
        // No names either: they come from the variables.
        for at in [0x2000_0100u64, 0x2000_0200] {
            ram.put(at + 12, &[0; 16]);
        }
        let mut fetch = |address, block: &mut [u8]| ram.read(address, block);
        let mut memory = Memory {
            read: &mut fetch,
            left: 100,
            stacks: true,
        };
        let (snapshot, _) = read_threads(&layout(false), &mut memory);
        let names: Vec<_> = snapshot
            .tasks
            .iter()
            .map(|task| task.name.as_str())
            .collect();
        assert_eq!(names, ["main", "blink"]);
    }

    #[test]
    fn before_the_kernel_starts_there_are_no_threads() {
        let mut ram = firmware();
        ram.word(0x2000_0000 + 8, 0);
        let mut fetch = |address, block: &mut [u8]| ram.read(address, block);
        let mut memory = Memory {
            read: &mut fetch,
            left: 100,
            stacks: true,
        };
        let (snapshot, _) = read_threads(&layout(true), &mut memory);
        assert!(snapshot.tasks.is_empty());
        assert_eq!(
            snapshot.problem.as_deref(),
            Some("The kernel has not started yet.")
        );
    }
}
