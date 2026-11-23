//! FreeRTOS's tasks, through the kernel's lists in tasks.c: the ready list
//! of each priority, the delayed lists, and those of tasks suspended or
//! deleted. What list a task is on, and whether it waits on an event, says
//! what it is doing.

use std::collections::HashMap;

use super::debug::{Debug, Field};
use super::{Found, Heap, Kernel, Layout as Any, Memory, Snapshot, State, Task};
use super::{never_used, text, unsigned};

pub(super) const STRUCTS: [&str; 6] = [
    "TCB_t",
    "tskTaskControlBlock",
    "List_t",
    "xLIST",
    "ListItem_t",
    "xLIST_ITEM",
];

/// What FreeRTOS fills the stacks with (tskSTACK_FILL_BYTE).
const FILL: u8 = 0xA5;
/// The most tasks followed on one list.
const TASKS_MAX: usize = 64;
/// ucNotifyState while a task waits for a notification.
const WAITING_NOTIFICATION: u64 = 1;

/// What the tasks on a list are doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lists {
    Ready,
    Delayed,
    Suspended,
    Deleted,
}

/// Where FreeRTOS keeps its tasks in a firmware.
#[derive(Debug)]
pub struct Layout {
    /// `pxCurrentTCB`, or `pxCurrentTCBs` and how many cores it has.
    current: (u64, u64),
    /// Each list's address and what its tasks are doing, the most urgent ready first.
    lists: Vec<(u64, Lists)>,
    /// In a List_t: its end marker, and the first item after it.
    list_end: u64,
    list_first: u64,
    /// In a ListItem_t: the next item and the TCB it belongs to.
    item_next: u64,
    item_owner: u64,
    tcb: Tcb,
    /// The stacks of tasks made with xTaskCreateStatic: arrays in the firmware, by address.
    static_stacks: HashMap<u64, u64>,
    heap: Option<HeapLayout>,
}

/// The parts of a TCB_t read.
#[derive(Debug)]
struct Tcb {
    size: u64,
    top: Field,
    stack: Field,
    end: Option<Field>,
    priority: Field,
    name: Field,
    /// The list the task waits on, if it waits on an event.
    event: Option<Field>,
    notify: Option<Field>,
    runtime: Option<Field>,
}

/// heap_4's: `ucHeap`, and how much of it is free.
#[derive(Debug)]
struct HeapLayout {
    size: u64,
    free: u64,
    lowest: Option<u64>,
}

pub(super) fn layout(debug: &Debug) -> Option<Found> {
    let tcb = debug
        .structs
        .get("TCB_t")
        .or_else(|| debug.structs.get("tskTaskControlBlock"))?;
    let list = debug
        .structs
        .get("List_t")
        .or_else(|| debug.structs.get("xLIST"))?;
    let item = debug
        .structs
        .get("ListItem_t")
        .or_else(|| debug.structs.get("xLIST_ITEM"))?;
    let current = match debug.data.get("pxCurrentTCB") {
        Some(&(address, _)) => (address, 1),
        None => {
            let &(address, size) = debug.data.get("pxCurrentTCBs")?;
            (address, (size / 4).max(1))
        }
    };
    let &(ready, ready_size) = debug.data.get("pxReadyTasksLists")?;
    if list.size == 0 {
        return None;
    }
    let mut lists: Vec<(u64, Lists)> = (0..ready_size / list.size)
        .rev()
        .map(|priority| (ready + priority * list.size, Lists::Ready))
        .collect();
    for (name, what) in [
        ("xPendingReadyList", Lists::Ready),
        ("xDelayedTaskList1", Lists::Delayed),
        ("xDelayedTaskList2", Lists::Delayed),
        ("xSuspendedTaskList", Lists::Suspended),
        ("xTasksWaitingTermination", Lists::Deleted),
    ] {
        if let Some(&(address, _)) = debug.data.get(name) {
            lists.push((address, what));
        }
    }
    let event = tcb
        .field("xEventListItem")
        .zip(item.field("pvContainer"))
        .map(|(event, container)| Field {
            offset: event.offset + container.offset,
            size: container.size,
        });
    let layout = Layout {
        current,
        lists,
        list_end: list.field("xListEnd")?.offset,
        list_first: list.field("xListEnd.pxNext")?.offset,
        item_next: item.field("pxNext")?.offset,
        item_owner: item.field("pvOwner")?.offset,
        tcb: Tcb {
            size: tcb.size,
            top: tcb.field("pxTopOfStack")?,
            stack: tcb.field("pxStack")?,
            end: tcb.field("pxEndOfStack"),
            priority: tcb.field("uxPriority")?,
            name: tcb.field("pcTaskName")?,
            event,
            notify: tcb.field("ucNotifyState").map(|field| Field {
                offset: field.offset,
                size: 1,
            }),
            runtime: tcb.field("ulRunTimeCounter"),
        },
        static_stacks: debug
            .data
            .values()
            .filter(|&&(_, size)| size > 0)
            .copied()
            .collect(),
        heap: match (
            debug.data.get("ucHeap"),
            debug.data.get("xFreeBytesRemaining"),
        ) {
            (Some(&(_, size)), Some(&(free, _))) => Some(HeapLayout {
                size,
                free,
                lowest: debug
                    .data
                    .get("xMinimumEverFreeBytesRemaining")
                    .map(|&(address, _)| address),
            }),
            _ => None,
        },
    };
    let mut missing = Vec::new();
    if layout.tcb.runtime.is_none() {
        missing
            .push("configGENERATE_RUN_TIME_STATS 1, for each task's share of the CPU".to_string());
    }
    if layout.tcb.end.is_none() {
        missing.push(
            "configRECORD_STACK_HIGH_ADDRESS 1, for the size of the stacks of tasks made with xTaskCreate"
                .to_string(),
        );
    }
    Some(Found {
        layout: Any::FreeRtos(layout),
        missing,
    })
}

/// Where `pxCurrentTCB` is, or the first core's in `pxCurrentTCBs`, and its size.
pub(super) fn running_at(layout: &Layout) -> (u64, u64) {
    (layout.current.0, 4)
}

pub(super) fn read(layout: &Layout, memory: &mut Memory) -> (Snapshot, Vec<Option<(u64, u64)>>) {
    let (current, cores) = layout.current;
    let running: Vec<u64> = (0..cores)
        .filter_map(|core| memory.word(current + core * 4))
        .filter(|&tcb| tcb != 0)
        .collect();
    if running.is_empty() {
        return (
            Snapshot::empty(Kernel::FreeRtos, "The scheduler has not started yet."),
            Vec::new(),
        );
    }

    // Each task once, on the first list it is found on.
    let mut found: Vec<(u64, Lists)> = Vec::new();
    for &(list, what) in &layout.lists {
        let end = list + layout.list_end;
        let Some(mut item) = memory.word(list + layout.list_first) else {
            continue;
        };
        let mut steps = 0;
        while item != end && item != 0 && steps < TASKS_MAX {
            steps += 1;
            let (Some(owner), Some(next)) = (
                memory.word(item + layout.item_owner),
                memory.word(item + layout.item_next),
            ) else {
                break;
            };
            if owner != 0 && !found.iter().any(|&(tcb, _)| tcb == owner) {
                found.push((owner, what));
            }
            item = next;
        }
    }

    let tcb = &layout.tcb;
    let mut tasks = Vec::new();
    let mut counters = Vec::new();
    for (address, what) in found {
        let Some(bytes) = memory.bytes(address, tcb.size) else {
            continue;
        };
        let waits = tcb
            .event
            .and_then(|field| unsigned(&bytes, field))
            .is_some_and(|list| list != 0)
            || tcb
                .notify
                .and_then(|field| unsigned(&bytes, field))
                .is_some_and(|state| state == WAITING_NOTIFICATION);
        let state = match what {
            _ if running.contains(&address) => State::Running,
            Lists::Ready => State::Ready,
            Lists::Delayed | Lists::Suspended if waits => State::Blocked,
            Lists::Delayed => State::Sleeping,
            Lists::Suspended => State::Suspended,
            Lists::Deleted => State::Dead,
        };
        let stack = unsigned(&bytes, tcb.stack).unwrap_or(0);
        let top = unsigned(&bytes, tcb.top).unwrap_or(0);
        // The stack grows down: pxEndOfStack is its highest word.
        let size = tcb
            .end
            .and_then(|field| unsigned(&bytes, field))
            .filter(|&end| end >= stack && stack != 0)
            .map(|end| end + 4 - stack)
            .or_else(|| layout.static_stacks.get(&stack).copied());
        // As it last left the CPU, the running one's too: its own is in the core.
        let stack_used = size
            .filter(|&size| top >= stack && top <= stack + size)
            .map(|size| stack + size - top);
        // What was never used lies under the lowest the stack pointer went.
        let bound = size.or_else(|| (top > stack).then(|| top - stack));
        let stack_free = bound.and_then(|bound| never_used(memory, stack, bound, FILL));
        counters.push(
            tcb.runtime
                .and_then(|field| unsigned(&bytes, field).map(|value| (value, field.size))),
        );
        tasks.push(Task {
            name: text(&bytes, tcb.name),
            state,
            priority: unsigned(&bytes, tcb.priority).unwrap_or(0) as i64,
            stack_size: size,
            stack_used,
            stack_free,
            cpu: None,
            address,
        });
    }

    let heap = layout.heap.as_ref().and_then(|heap| {
        Some(Heap {
            size: heap.size,
            free: memory.word(heap.free)?,
            lowest_free: heap.lowest.and_then(|address| memory.word(address)),
        })
    });
    let snapshot = Snapshot {
        kernel: Kernel::FreeRtos,
        tasks,
        heap,
        problem: None,
    };
    (snapshot, counters)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A firmware's RAM, from 0x2000_0000.
    struct Ram(Vec<u8>);

    impl Ram {
        fn word(&mut self, address: u64, value: u32) {
            let at = (address - 0x2000_0000) as usize;
            self.0[at..at + 4].copy_from_slice(&value.to_le_bytes());
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

    // A List_t of 20 bytes: uxNumberOfItems, pxIndex, then xListEnd (value,
    // next, previous). A ListItem_t: value, next, previous, owner, container.
    // A TCB_t of 72 bytes: pxTopOfStack 0, xStateListItem 4, xEventListItem 24
    // (its container at 40), uxPriority 44, pxStack 48, pcTaskName 52 (16),
    // ulRunTimeCounter 68.
    const READY: u64 = 0x2000_0000;
    const DELAYED: u64 = 0x2000_0040;
    const SUSPENDED: u64 = 0x2000_0060;
    const CURRENT: u64 = 0x2000_0080;

    fn layout() -> Layout {
        Layout {
            current: (CURRENT, 1),
            lists: vec![
                (READY + 20, Lists::Ready),
                (READY, Lists::Ready),
                (DELAYED, Lists::Delayed),
                (SUSPENDED, Lists::Suspended),
            ],
            list_end: 8,
            list_first: 12,
            item_next: 4,
            item_owner: 12,
            tcb: Tcb {
                size: 72,
                top: field(0, 4),
                stack: field(48, 4),
                end: None,
                priority: field(44, 4),
                name: field(52, 16),
                event: Some(field(40, 4)),
                notify: None,
                runtime: Some(field(68, 4)),
            },
            static_stacks: HashMap::from([(0x2000_1000, 0x400)]),
            heap: Some(HeapLayout {
                size: 0x4000,
                free: 0x2000_0090,
                lowest: Some(0x2000_0094),
            }),
        }
    }

    /// Puts a task on a list of its own: the list's end points to its item and back.
    #[allow(clippy::too_many_arguments)]
    fn task(
        ram: &mut Ram,
        list: u64,
        tcb: u64,
        name: &str,
        priority: u32,
        stack: u32,
        top: u32,
        event: u32,
    ) {
        let item = tcb + 4;
        ram.word(list, 1);
        ram.word(list + 12, item as u32);
        ram.word(list + 16, item as u32);
        ram.word(item + 4, (list + 8) as u32);
        ram.word(item + 12, tcb as u32);
        ram.word(tcb, top);
        ram.word(tcb + 40, event);
        ram.word(tcb + 44, priority);
        ram.word(tcb + 48, stack);
        let at = (tcb + 52 - 0x2000_0000) as usize;
        ram.0[at..at + name.len()].copy_from_slice(name.as_bytes());
    }

    fn firmware() -> Ram {
        let mut ram = Ram(vec![0; 0x2000]);
        // Every list empty: its end marker points to itself.
        for list in [READY, READY + 20, DELAYED, SUSPENDED] {
            ram.word(list + 12, (list + 8) as u32);
        }
        task(
            &mut ram,
            READY + 20,
            0x2000_0200,
            "Tmr Svc",
            1,
            0x2000_1800,
            0x2000_1900,
            0,
        );
        task(
            &mut ram,
            DELAYED,
            0x2000_0300,
            "blink",
            2,
            0x2000_1000,
            0x2000_1300,
            0,
        );
        task(
            &mut ram,
            SUSPENDED,
            0x2000_0400,
            "shell",
            1,
            0x2000_1400,
            0x2000_1700,
            0x2000_0600,
        );
        ram.word(CURRENT, 0x2000_0200);
        ram.word(0x2000_0090, 0x1000);
        ram.word(0x2000_0094, 0x0800);
        // blink's static stack has gone 0x100 deep at most.
        let at = 0x1000;
        ram.0[at..at + 0x300].fill(FILL);
        ram
    }

    #[test]
    fn tasks_are_read_from_the_lists_with_what_they_are_doing() {
        let ram = firmware();
        let mut fetch = |address, block: &mut [u8]| ram.read(address, block);
        let mut memory = Memory {
            read: &mut fetch,
            left: 200,
            stacks: true,
        };
        let (snapshot, counters) = read(&layout(), &mut memory);
        let summary: Vec<_> = snapshot
            .tasks
            .iter()
            .map(|task| (task.name.as_str(), task.state, task.priority))
            .collect();
        assert_eq!(
            summary,
            [
                ("Tmr Svc", State::Running, 1),
                ("blink", State::Sleeping, 2),
                // Suspended while it waits on a queue: blocked on it.
                ("shell", State::Blocked, 1),
            ]
        );
        let blink = &snapshot.tasks[1];
        // A static stack: its size is its array's.
        assert_eq!(blink.stack_size, Some(0x400));
        assert_eq!(blink.stack_used, Some(0x100));
        assert_eq!(blink.stack_free, Some(0x300));
        assert_eq!(
            snapshot.heap,
            Some(Heap {
                size: 0x4000,
                free: 0x1000,
                lowest_free: Some(0x800),
            })
        );
        assert_eq!(counters, [Some((0, 4)); 3]);
    }

    #[test]
    fn before_the_scheduler_starts_there_are_no_tasks() {
        let mut ram = firmware();
        ram.word(CURRENT, 0);
        let mut fetch = |address, block: &mut [u8]| ram.read(address, block);
        let mut memory = Memory {
            read: &mut fetch,
            left: 200,
            stacks: true,
        };
        let (snapshot, _) = read(&layout(), &mut memory);
        assert!(snapshot.tasks.is_empty());
        assert_eq!(
            snapshot.problem.as_deref(),
            Some("The scheduler has not started yet.")
        );
    }
}
