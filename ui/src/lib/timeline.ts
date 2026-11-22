// What the timeline at the top of the Live view draws: which task ran when,
// sampled through the probe or traced over SWO, and the interrupts, with the
// watched variables over the same time. It is kept while the tab records,
// and can be saved to a file and opened again later.

import type { Sample } from "./modbus.svelte";
import type { Kernel, RtosSnapshot } from "./rtos";

/** What sampling saw since it last told, as the session sends it. */
export type RtosRunning = {
  /**
   * The whole ms since 1970 the times count from: 0 for samples, timed in ms
   * since 1970 themselves; a trace's keep their nanoseconds counted so.
   */
  base?: number;
  /** When each task was first seen running, in ms after `base`, with its control block's address; 0 where not known. */
  switches: [number, number][];
  /** When the last sample was taken, in ms after `base`: the task last seen ran until then. */
  until: number;
  /** How many samples these come from. */
  samples: number;
  /** Whether these are every switch, traced over SWO, not samples. */
  exact?: boolean;
};

/**
 * An exception an SWO tab traced: entered, left, or returned to, in ms after
 * the trace's base; or, as exception 0, a time when some of the trace was lost.
 */
export type TracedInterrupt = { at: number; number: number; event: "entered" | "exited" | "returned" | "lost" };

/** The core's exceptions by number; 16 and on are the IRQs. */
const EXCEPTIONS: Record<number, string> = {
  1: "Reset",
  2: "NMI",
  3: "HardFault",
  4: "MemManage",
  5: "BusFault",
  6: "UsageFault",
  7: "SecureFault",
  11: "SVCall",
  12: "DebugMon",
  14: "PendSV",
  15: "SysTick",
};

/** An exception's name: PendSV, SysTick, or IRQ 50 for exception 66. */
export function exceptionName(number: number): string {
  return EXCEPTIONS[number] ?? (number >= 16 ? `IRQ ${number - 16}` : `Exception ${number}`);
}

/** When an exception ran: entered to left, oldest first. */
type Runs = { starts: number[]; ends: number[] };

/** In the switches, where the chip was halted from, by Pause: no task ran until the next. */
export const HALTED = 1;

/** The most switches kept: the oldest go first. Hours of a busy RTOS. */
export const SWITCHES_MAX = 2_000_000;

/**
 * Which task ran when: switches, oldest first, each lasting until the next.
 * Its times are in ms after its origin, a whole ms since 1970: counted so,
 * they keep the nanoseconds a trace has, where ms since 1970 keep only a
 * quarter of a microsecond.
 */
export class Timeline {
  /** The whole ms since 1970 the times count from. */
  readonly origin: number;
  /** When each switch was, in ms after the origin. */
  times: number[] = [];
  /** The task that ran from then on: its control block's address; 0 where it is not known. */
  tasks: number[] = [];
  /** Until when the last task is known to have run. */
  until = 0;
  /** How many samples all this comes from. */
  samples = 0;
  /** About how many samples a second were taken lately. */
  rate = 0;
  /** Each task's name, by its address. */
  names = new Map<number, string>();
  /** The tasks in the order of their lanes: the kernel's, then others seen running. */
  order: number[] = [];
  kernel: Kernel | null = null;
  /** Whether the switches lately are every one, traced over SWO, not samples. */
  exact = false;
  /** Each exception's runs, by its number, when they were traced. */
  interrupts = new Map<number, Runs>();
  /** The interrupts' names in the firmware, by exception number: TIM7 for 66. */
  interruptNames = new Map<number, string>();
  /** Exceptions entered and not left yet, with when they were entered. */
  private entered = new Map<number, number>();
  /** How many exceptions there were lately, a second. */
  interruptRate = 0;
  /** Whether what comes next is from a new connection, with a gap before it. */
  private broken = false;

  constructor(origin = Math.floor(Date.now())) {
    this.origin = origin;
  }

  /**
   * The session starts anew, as after the probe was unplugged: what ran
   * between what there is and what comes next is not known.
   */
  restart(): void {
    this.broken = this.times.length > 0;
  }

  /**
   * Adds what sampling saw. Within a session, the session marks where it
   * does not know what ran; a new connection leaves a gap.
   */
  add(running: RtosRunning): void {
    // The base and the origin are whole ms: told apart exactly, before the part of a ms is added.
    const shift = (running.base ?? 0) - this.origin;
    const before = this.times.length > 0 ? this.until : null;
    if (this.broken && running.switches.length > 0) {
      if (before !== null && this.tasks.at(-1) !== 0) this.push(before, 0);
      this.broken = false;
    }
    for (const [at, task] of running.switches) {
      this.push(shift + at, task);
    }
    const until = shift + running.until;
    if (before !== null && until > before && running.samples > 0) {
      this.rate = (running.samples * 1000) / (until - before);
    }
    this.until = before === null ? until : Math.max(before, until);
    this.samples += running.samples;
    this.exact = running.exact === true;
    if (this.times.length > SWITCHES_MAX) {
      const drop = SWITCHES_MAX / 10;
      this.times.splice(0, drop);
      this.tasks.splice(0, drop);
    }
  }

  private push(at: number, task: number): void {
    if (this.tasks.at(-1) === task) return;
    // Times only go on, so they can be searched.
    this.times.push(Math.max(at, this.times.at(-1) ?? at));
    this.tasks.push(task);
    if (task > HALTED && !this.order.includes(task)) {
      this.order.push(task);
    }
  }

  /** Adds traced exceptions, timed in ms after `base`: each run from when it was entered to when it was left. */
  addInterrupts(interrupts: TracedInterrupt[], base = 0): void {
    const shift = base - this.origin;
    for (const { at: when, number, event } of interrupts) {
      // Lost with the trace, when those entered were left is not known; and
      // back in the thread, all were left, though SWO lost when.
      if (event === "lost" || (event === "returned" && number === 0)) {
        this.entered.clear();
        continue;
      }
      if (number === 0) continue;
      const at = shift + when;
      if (event === "entered") {
        this.entered.set(number, at);
      } else if (event === "exited") {
        const start = this.entered.get(number);
        if (start === undefined) continue;
        this.entered.delete(number);
        let runs = this.interrupts.get(number);
        if (!runs) {
          runs = { starts: [], ends: [] };
          this.interrupts.set(number, runs);
        }
        runs.starts.push(start);
        runs.ends.push(at);
        if (runs.starts.length > SWITCHES_MAX / 4) {
          runs.starts.splice(0, SWITCHES_MAX / 40);
          runs.ends.splice(0, SWITCHES_MAX / 40);
        }
      }
    }
    const first = interrupts[0];
    const last = interrupts.at(-1);
    if (first && last && last.at > first.at) {
      this.interruptRate = (interrupts.filter((item) => item.event === "entered").length * 1000) / (last.at - first.at);
    }
  }

  /** The exceptions traced, in their numbers' order. */
  get interruptOrder(): number[] {
    return [...this.interrupts.keys()].sort((a, b) => a - b);
  }

  /** Each run of an exception that overlaps from..to. */
  interruptRuns(number: number, from: number, to: number, each: (start: number, end: number) => void): void {
    const runs = this.interrupts.get(number);
    if (!runs) return;
    // The first run that ends after `from`: runs end in order, as they start.
    let low = 0;
    let high = runs.ends.length;
    while (low < high) {
      const middle = (low + high) >> 1;
      if (runs.ends[middle] < from) low = middle + 1;
      else high = middle;
    }
    for (let index = low; index < runs.starts.length && runs.starts[index] <= to; index++) {
      each(runs.starts[index], runs.ends[index]);
    }
  }

  /** Learns the tasks' names, and their order, from a read of them. */
  learn(snapshot: RtosSnapshot): void {
    this.kernel = snapshot.kernel;
    if (snapshot.tasks.length === 0) return;
    const listed = snapshot.tasks.map((task) => task.address);
    for (const task of snapshot.tasks) {
      this.names.set(task.address, task.name);
    }
    this.order = [...listed, ...this.order.filter((task) => !listed.includes(task))];
  }

  /** A task's name, or its address when it was never read. */
  name(task: number): string {
    return this.names.get(task) ?? `0x${task.toString(16).padStart(8, "0")}`;
  }

  /** An exception's name: the firmware's for an IRQ, as TIM7, or else as the core has it. */
  interruptName(number: number): string {
    return this.interruptNames.get(number) ?? exceptionName(number);
  }

  /** The index of the last switch at or before a time: -1 before the first. */
  indexAt(at: number): number {
    let low = 0;
    let high = this.times.length - 1;
    let found = -1;
    while (low <= high) {
      const middle = (low + high) >> 1;
      if (this.times[middle] <= at) {
        found = middle;
        low = middle + 1;
      } else {
        high = middle - 1;
      }
    }
    return found;
  }

  /** The task running at a time, or null where it is not known, or the chip was halted. */
  taskAt(at: number): number | null {
    const index = this.indexAt(at);
    if (index < 0 || at > this.until) return null;
    const task = this.tasks[index];
    return task > HALTED ? task : null;
  }

  /** Each run that overlaps from..to: the task, and when it started and ended. */
  runs(from: number, to: number, each: (task: number, start: number, end: number) => void): void {
    this.spans(from, to, (task, start, end) => {
      if (task > HALTED) each(task, start, end);
    });
  }

  /** Each time what ran is not known that overlaps from..to, as when the trace was lost: when from, and until when. */
  unknowns(from: number, to: number, each: (start: number, end: number) => void): void {
    this.spans(from, to, (task, start, end) => {
      if (task === 0) each(start, end);
    });
  }

  /** Each time the chip was halted that overlaps from..to: when from, and until when. */
  halts(from: number, to: number, each: (start: number, end: number) => void): void {
    this.spans(from, to, (task, start, end) => {
      if (task === HALTED) each(start, end);
    });
  }

  private spans(from: number, to: number, each: (task: number, start: number, end: number) => void): void {
    for (let index = Math.max(0, this.indexAt(from)); index < this.times.length; index++) {
      const start = this.times[index];
      if (start > to) break;
      const end = index + 1 < this.times.length ? this.times[index + 1] : this.until;
      if (end > from) each(this.tasks[index], start, end);
    }
  }

  /** The first time anything is known, or null when nothing is. */
  get since(): number | null {
    return this.times.length > 0 ? this.times[0] : null;
  }

  clear(): void {
    this.times = [];
    this.tasks = [];
    this.until = 0;
    this.samples = 0;
    this.rate = 0;
    this.interrupts = new Map();
    this.entered = new Map();
    this.interruptRate = 0;
  }
}

/** The index of the first sample at or after a time. */
export function sampleAt(samples: Sample[], at: number): number {
  let low = 0;
  let high = samples.length;
  while (low < high) {
    const middle = (low + high) >> 1;
    if (samples[middle].at < at) {
      low = middle + 1;
    } else {
      high = middle;
    }
  }
  return low;
}

/** What a timeline file holds. */
export type TimelineFile = {
  format: typeof FORMAT;
  /** 2 counts every time from `origin`; 1 had them all in ms since 1970. */
  version: 1 | 2;
  /** The tab it was recorded in. */
  name: string;
  /** The whole ms since 1970 the times count from. */
  origin?: number;
  kernel: Kernel | null;
  names: [number, string][];
  order: number[];
  times: number[];
  tasks: number[];
  until: number;
  samples: number;
  /** Whether the switches were traced, every one, rather than sampled. */
  exact?: boolean;
  /** Each exception traced: its number, and when each of its runs started and ended. */
  interrupts?: [number, number[], number[]][];
  /** The interrupts' names in the firmware, by exception number. */
  interruptNames?: [number, string][];
  /** The watched variables, each with its own times and values. */
  variables: { name: string; times: number[]; values: number[] }[];
};

const FORMAT = "terNIMA timeline";

/** A timeline as a file's text: JSON, with times in ms after its origin. */
export function timelineText(name: string, timeline: Timeline, samples: Sample[], series: string[]): string {
  const variables = series.map((series) => ({ name: series, times: [] as number[], values: [] as number[] }));
  const index = new Map(series.map((name, at) => [name, at]));
  for (const sample of samples) {
    for (const [series, value] of sample.values) {
      const variable = variables[index.get(series) ?? -1];
      if (variable) {
        variable.times.push(sample.at - timeline.origin);
        variable.values.push(value);
      }
    }
  }
  const file: TimelineFile = {
    format: FORMAT,
    version: 2,
    name,
    origin: timeline.origin,
    kernel: timeline.kernel,
    names: [...timeline.names],
    order: timeline.order,
    times: timeline.times,
    tasks: timeline.tasks,
    until: timeline.until,
    samples: timeline.samples,
    exact: timeline.exact,
    interrupts: [...timeline.interrupts].map(([number, runs]) => [number, runs.starts, runs.ends]),
    interruptNames: [...timeline.interruptNames],
    variables,
  };
  return JSON.stringify(file);
}

/** A timeline read back from a file's text, with its variables as samples. Throws for another kind of file. */
export function readTimeline(text: string): { name: string; timeline: Timeline; samples: Sample[]; series: string[] } {
  let file: Partial<TimelineFile>;
  try {
    file = JSON.parse(text);
  } catch {
    throw new Error("This is not a timeline saved by terNIMA.");
  }
  if (file?.format !== FORMAT || !Array.isArray(file.times) || !Array.isArray(file.tasks)) {
    throw new Error("This is not a timeline saved by terNIMA.");
  }
  if (file.times.length !== file.tasks.length) {
    throw new Error("This timeline is broken: its times and tasks do not match.");
  }
  const variables = file.variables ?? [];
  // A file of version 1 has its times in ms since 1970: they count from its first whole ms here.
  const counted = file.version !== 1 && typeof file.origin === "number";
  const origin = counted ? (file.origin as number) : Math.floor(file.times[0] ?? variables[0]?.times[0] ?? 0);
  const shift = counted ? 0 : -origin;
  const moved = (times: number[]) => (shift === 0 ? times : times.map((at) => at + shift));
  const timeline = new Timeline(origin);
  timeline.times = moved(file.times);
  timeline.tasks = file.tasks;
  timeline.until = (file.until ?? file.times.at(-1) ?? 0) + shift;
  timeline.samples = file.samples ?? 0;
  timeline.kernel = file.kernel ?? null;
  timeline.names = new Map(file.names ?? []);
  timeline.order = file.order ?? [...new Set(file.tasks.filter((task) => task > HALTED))];
  timeline.exact = file.exact === true;
  for (const [number, starts, ends] of file.interrupts ?? []) {
    if (Array.isArray(starts) && Array.isArray(ends) && starts.length === ends.length) {
      timeline.interrupts.set(number, { starts: moved(starts), ends: moved(ends) });
    }
  }
  timeline.interruptNames = new Map(file.interruptNames ?? []);
  // The samples keep ms since 1970, as a tab's own.
  const samples: Sample[] = [];
  for (const variable of variables) {
    variable.times.forEach((at, index) => {
      samples.push({ n: 0, at: counted ? origin + at : at, ok: true, values: [[variable.name, variable.values[index]]] });
    });
  }
  samples.sort((a, b) => a.at - b.at);
  samples.forEach((sample, index) => (sample.n = index));
  return { name: file.name ?? "", timeline, samples, series: variables.map((variable) => variable.name) };
}
