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

/** How full its stack has been at most, from 0 to 1: what it uses now when that is all that is known. */
export function stackFill(task: RtosTask): number | null {
  if (task.stack_size === null || task.stack_size <= 0) return null;
  const used = stackPeak(task) ?? task.stack_used;
  return used === null ? null : Math.min(1, Math.max(0, used / task.stack_size));
}

/** A number of bytes, short: 512 B, 1.5 KB. */
export function bytes(count: number): string {
  if (count < 1024) return `${count} B`;
  const kb = count / 1024;
  return `${Number(kb.toFixed(kb < 10 ? 1 : 0))} KB`;
}
