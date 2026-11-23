// An RTOS's tasks as the Live view shows them: Zephyr's threads and
// FreeRTOS's tasks, read from the chip as it runs.

export type Kernel = "zephyr" | "freertos";
export type TaskState = "running" | "ready" | "blocked" | "sleeping" | "suspended" | "dead";

export type RtosTask = {
  name: string;
  state: TaskState;
  /** As the kernel numbers it: in Zephyr lower is more urgent, in FreeRTOS higher. */
  priority: number;
  /** Its stack's size in bytes, where the kernel keeps it. */
  stack_size: number | null;
  /** The bytes of its stack in use when it last left the CPU. */
  stack_used: number | null;
  /** The fewest bytes of its stack ever free. */
  stack_free: number | null;
  /** Its share of the CPU since the read before, in percent. */
  cpu: number | null;
  /** Where its control block is, which tells apart tasks of one name. */
  address: number;
};

export type RtosSnapshot = {
  kernel: Kernel;
  tasks: RtosTask[];
  heap: { size: number; free: number; lowest_free: number | null } | null;
  /** Why there are no tasks, as before the scheduler starts. */
  problem: string | null;
};

/** The RTOS found in a firmware, with the config options it leaves out. */
export type RtosFound = { kernel: Kernel; missing: string[] };

/** How often the tasks are read, in ms. */
export const TASKS_EVERY_MS = 1000;

export const KERNEL_NAMES: Record<Kernel, string> = { zephyr: "Zephyr", freertos: "FreeRTOS" };

/** What a task is doing, in words, and what it means. */
export const STATES: Record<TaskState, { text: string; title: string }> = {
  running: { text: "Running", title: "On the CPU when it was read" },
  ready: { text: "Ready", title: "Ready to run, waiting for the CPU" },
  blocked: { text: "Blocked", title: "Waiting for something: a semaphore, a queue, a message or a notification" },
  sleeping: { text: "Sleeping", title: "Waiting for time to pass" },
  suspended: { text: "Suspended", title: "Stopped until something resumes it" },
  dead: { text: "Dead", title: "Ended, and not yet cleaned up" },
};

/** The most of its stack a task has used: all but what was never touched. */
export function stackPeak(task: RtosTask): number | null {
  if (task.stack_size !== null && task.stack_free !== null) return task.stack_size - task.stack_free;
  return null;
}

/** How full its stack is now, from 0 to 1, as it last left the CPU, the running one's too. */
export function stackNow(task: RtosTask): number | null {
  if (task.stack_size === null || task.stack_size <= 0 || task.stack_used === null) return null;
  return Math.min(1, Math.max(0, task.stack_used / task.stack_size));
}

/** Its stack's high-water mark: how full it has been at most, from 0 to 1, where the kernel fills the stacks. */
export function stackMost(task: RtosTask): number | null {
  const peak = stackPeak(task);
  if (task.stack_size === null || task.stack_size <= 0 || peak === null) return null;
  return Math.min(1, Math.max(0, peak / task.stack_size));
}

/**
 * What to turn on in the firmware for the most each stack has used: where
 * the stacks have sizes, but none holds the kernel's fill under what was used.
 */
export function stackFillNote(kernel: Kernel, tasks: RtosTask[]): string | null {
  const sized = tasks.filter((task) => task.stack_size !== null);
  if (sized.length === 0 || sized.some((task) => task.stack_free !== null)) return null;
  return kernel === "zephyr"
    ? "CONFIG_INIT_STACKS=y, for the most each stack has used"
    : "INCLUDE_uxTaskGetStackHighWaterMark 1, for the most each stack has used";
}

/** Whether a task is the kernel's idle one: its share of the CPU is the time left free. */
export function isIdle(task: RtosTask): boolean {
  return /^idle/i.test(task.name);
}

/** The CPU's load over the last read: all but the idle tasks' share, in percent; null where that is not known. */
export function cpuLoad(tasks: RtosTask[]): number | null {
  const idle = tasks.filter(isIdle);
  if (idle.length === 0 || idle.some((task) => task.cpu === null)) return null;
  return Math.max(0, 100 - idle.reduce((sum, task) => sum + (task.cpu ?? 0), 0));
}

/** The least and the most a value has been. */
export type Range = { min: number; max: number };

/** A range taking in a value too. */
export function widen(range: Range | null | undefined, value: number): Range {
  return range ? { min: Math.min(range.min, value), max: Math.max(range.max, value) } : { min: value, max: value };
}

/** The least and the most share of the CPU each task has had in a read, by its control block's address, and the load's. */
export type CpuRanges = { tasks: Map<number, Range>; load: Range | null };

/** No shares yet. */
export function noCpuRanges(): CpuRanges {
  return { tasks: new Map(), load: null };
}

/** How many of each task's shares of the CPU are kept to draw: a minute's, as they are read every second. */
export const CPU_HISTORY = 60;

/** Each task's shares of the CPU lately, oldest first, by its control block's address. */
export type CpuHistory = Map<number, number[]>;

/** The history taking in a read's shares too, as a new one: the old stays as it was. */
export function addCpuHistory(history: CpuHistory, tasks: RtosTask[], keep = CPU_HISTORY): CpuHistory {
  const next = new Map(history);
  for (const task of tasks) {
    if (task.cpu === null) continue;
    next.set(task.address, [...(next.get(task.address) ?? []), task.cpu].slice(-keep));
  }
  return next;
}

/**
 * A line through shares of the CPU, as an SVG polyline's points in a box
 * `width` by `height`: the newest at the right edge and a minute back at
 * the left, scaled to the most among them, at least `floor` percent, so a
 * share that changes little stays flat.
 */
export function sparkPoints(shares: number[], width: number, height: number, slots = CPU_HISTORY, floor = 5): string {
  const top = Math.max(floor, ...shares) * 1.25;
  const step = width / (slots - 1);
  const start = (slots - shares.length) * step;
  return shares
    .map((share, index) => `${(start + index * step).toFixed(1)},${(height - 1 - (share / top) * (height - 2)).toFixed(1)}`)
    .join(" ");
}

/** The ranges taking in a read's shares too, as new ones: the old stay as they were. */
export function widenCpu(ranges: CpuRanges, tasks: RtosTask[]): CpuRanges {
  const each = new Map(ranges.tasks);
  for (const task of tasks) {
    if (task.cpu !== null) each.set(task.address, widen(each.get(task.address), task.cpu));
  }
  const load = cpuLoad(tasks);
  return { tasks: each, load: load === null ? ranges.load : widen(ranges.load, load) };
}

/** A number of bytes, short: 512 B, 1.5 KB. */
export function bytes(count: number): string {
  if (count < 1024) return `${count} B`;
  const kb = count / 1024;
  return `${Number(kb.toFixed(kb < 10 ? 1 : 0))} KB`;
}
