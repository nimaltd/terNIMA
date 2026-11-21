// Run with npm test: Node runs TypeScript as it is.

import assert from "node:assert/strict";
import { test } from "node:test";

import type { RtosSnapshot } from "./rtos.ts";
import { exceptionName, HALTED, readTimeline, sampleAt, Timeline, timelineText } from "./timeline.ts";

const BUSY = 0x2000_0208;
const IDLE = 0x2000_02d8;

function snapshot(): RtosSnapshot {
  const task = (name: string, address: number) => ({
    name,
    state: "ready" as const,
    priority: 7,
    stack_size: null,
    stack_used: null,
    stack_free: null,
    cpu: null,
    address,
  });
  return { kernel: "zephyr", tasks: [task("idle", IDLE), task("busy", BUSY)], heap: null, problem: null };
}

test("switches are kept, each run lasting until the next", () => {
  const timeline = new Timeline(0);
  timeline.add({ switches: [[1000, IDLE], [1004, BUSY]], until: 1010, samples: 25 });
  timeline.add({ switches: [[1012, IDLE]], until: 1020, samples: 25 });
  const runs: [number, number, number][] = [];
  timeline.runs(1000, 1020, (task, start, end) => runs.push([task, start, end]));
  assert.deepEqual(runs, [
    [IDLE, 1000, 1004],
    [BUSY, 1004, 1012],
    [IDLE, 1012, 1020],
  ]);
  assert.equal(timeline.taskAt(1005), BUSY);
  assert.equal(timeline.taskAt(999), null);
  assert.equal(timeline.taskAt(1021), null);
  assert.equal(timeline.samples, 50);
  // 25 samples in the 10 ms after the first batch.
  assert.equal(timeline.rate, 2500);
});

test("a pause leaves a gap where nothing is known", () => {
  const timeline = new Timeline(0);
  timeline.add({ switches: [[1000, BUSY]], until: 1010, samples: 10 });
  timeline.add({ switches: [[5000, BUSY]], until: 5010, samples: 10 });
  assert.deepEqual(timeline.times, [1000, 1010, 5000]);
  assert.deepEqual(timeline.tasks, [BUSY, 0, BUSY]);
  assert.equal(timeline.taskAt(3000), null);
  const runs: number[] = [];
  timeline.runs(0, 6000, (_, start) => runs.push(start));
  assert.deepEqual(runs, [1000, 5000]);
});

test("lanes follow the kernel's order, and know the names", () => {
  const timeline = new Timeline(0);
  timeline.add({ switches: [[1000, BUSY], [1001, 0x2000_0300]], until: 1002, samples: 2 });
  timeline.learn(snapshot());
  assert.deepEqual(timeline.order, [IDLE, BUSY, 0x2000_0300]);
  assert.equal(timeline.name(BUSY), "busy");
  assert.equal(timeline.name(0x2000_0300), "0x20000300");
});

test("a timeline comes back from its file as it was", () => {
  const timeline = new Timeline(0);
  timeline.learn(snapshot());
  timeline.add({ switches: [[1000, IDLE], [1004, BUSY]], until: 1010, samples: 20 });
  const samples = [
    { n: 0, at: 1001, ok: true, values: [["spins", 5]] as [string, number][] },
    { n: 1, at: 1006, ok: true, values: [["spins", 9]] as [string, number][] },
  ];
  const back = readTimeline(timelineText("Board", timeline, samples, ["spins"]));
  assert.equal(back.name, "Board");
  assert.deepEqual(back.timeline.times, timeline.times);
  assert.deepEqual(back.timeline.tasks, timeline.tasks);
  assert.equal(back.timeline.until, 1010);
  assert.equal(back.timeline.name(BUSY), "busy");
  assert.equal(back.timeline.kernel, "zephyr");
  assert.deepEqual(back.series, ["spins"]);
  assert.deepEqual(
    back.samples.map((sample) => [sample.at, sample.values[0][1]]),
    [
      [1001, 5],
      [1006, 9],
    ],
  );
  assert.throws(() => readTimeline('{"format":"something else"}'), /not a timeline/);
});

test("samples are found by time", () => {
  const samples = [1000, 1010, 1020].map((at, n) => ({ n, at, ok: true, values: [] }));
  assert.equal(sampleAt(samples, 0), 0);
  assert.equal(sampleAt(samples, 1010), 1);
  assert.equal(sampleAt(samples, 1015), 2);
  assert.equal(sampleAt(samples, 2000), 3);
});

test("traced interrupts run from entered to left, and come back from a file", () => {
  const timeline = new Timeline(0);
  timeline.addInterrupts([
    { at: 1000, number: 15, event: "entered" },
    { at: 1000.002, number: 15, event: "exited" },
    { at: 1000.002, number: 0, event: "returned" },
    { at: 1003, number: 66, event: "entered" },
    // Left before it was ever seen entered: nothing to draw.
    { at: 1004, number: 14, event: "exited" },
    { at: 1003.01, number: 66, event: "exited" },
  ]);
  assert.deepEqual(timeline.interruptOrder, [15, 66]);
  const runs: [number, number][] = [];
  timeline.interruptRuns(66, 1002, 1010, (start, end) => runs.push([start, end]));
  assert.deepEqual(runs, [[1003, 1003.01]]);
  const none: number[] = [];
  timeline.interruptRuns(15, 1001, 1010, (start) => none.push(start));
  assert.deepEqual(none, []);
  timeline.add({ switches: [[1000, BUSY]], until: 1010, samples: 3, exact: true });
  assert.equal(timeline.exact, true);
  const back = readTimeline(timelineText("Board", timeline, [], [])).timeline;
  assert.equal(back.exact, true);
  assert.deepEqual(back.interruptOrder, [15, 66]);
  const again: number[] = [];
  back.interruptRuns(66, 0, 2000, (start) => again.push(start));
  assert.deepEqual(again, [1003]);
});

test("exceptions are named as the core has them", () => {
  assert.equal(exceptionName(15), "SysTick");
  assert.equal(exceptionName(14), "PendSV");
  assert.equal(exceptionName(66), "IRQ 50");
  assert.equal(exceptionName(9), "Exception 9");
});

test("a trace's times keep their nanoseconds, counted from the origin", () => {
  const origin = 1_763_600_000_000;
  const timeline = new Timeline(origin);
  // One clock cycle at 240 MHz.
  const cycle = 1 / 240_000;
  // In ms since 1970, a cycle is lost: they keep only a quarter of a microsecond.
  assert.equal(origin + 123.5 + cycle, origin + 123.5);
  timeline.add({ base: origin + 123, switches: [[0.5, BUSY], [0.5 + cycle, IDLE]], until: 0.6, samples: 2, exact: true });
  assert.equal(timeline.times[0], 123.5);
  assert.ok(Math.abs(timeline.times[1] - timeline.times[0] - cycle) < 1e-12);
  assert.ok(Math.abs(timeline.until - 123.6) < 1e-9);
  timeline.addInterrupts(
    [
      { at: 0.55, number: 66, event: "entered" },
      { at: 0.55 + 30 * cycle, number: 66, event: "exited" },
    ],
    origin + 123,
  );
  const runs: [number, number][] = [];
  timeline.interruptRuns(66, 123, 124, (start, end) => runs.push([start, end]));
  assert.equal(runs.length, 1);
  assert.ok(Math.abs(runs[0][1] - runs[0][0] - 30 * cycle) < 1e-12);
  // Samples come timed in ms since 1970, with no base.
  timeline.add({ switches: [[origin + 124, BUSY]], until: origin + 125, samples: 1 });
  assert.equal(timeline.times.at(-1), 124);
  assert.equal(timeline.until, 125);
});

test("a file of version 1, timed in ms since 1970, counts from its first whole ms", () => {
  const text = JSON.stringify({
    format: "terNIMA timeline",
    version: 1,
    name: "Old",
    kernel: "zephyr",
    names: [[BUSY, "busy"]],
    order: [BUSY],
    times: [1_763_600_000_100.5, 1_763_600_000_104],
    tasks: [BUSY, IDLE],
    until: 1_763_600_000_110,
    samples: 3,
    interrupts: [[66, [1_763_600_000_101], [1_763_600_000_101.5]]],
    variables: [{ name: "spins", times: [1_763_600_000_102], values: [7] }],
  });
  const back = readTimeline(text);
  assert.equal(back.timeline.origin, 1_763_600_000_100);
  assert.deepEqual(back.timeline.times, [0.5, 4]);
  assert.equal(back.timeline.until, 10);
  const runs: [number, number][] = [];
  back.timeline.interruptRuns(66, 0, 20, (start, end) => runs.push([start, end]));
  assert.deepEqual(runs, [[1, 1.5]]);
  assert.equal(back.samples[0].at, 1_763_600_000_102);
});

test("interrupts take the firmware's names, kept in the file", () => {
  const timeline = new Timeline(1_763_600_000_000);
  timeline.interruptNames = new Map([[66, "TIM7"]]);
  assert.equal(timeline.interruptName(66), "TIM7");
  assert.equal(timeline.interruptName(67), "IRQ 51");
  assert.equal(timeline.interruptName(14), "PendSV");
  const samples = [{ n: 0, at: 1_763_600_000_250.25, ok: true, values: [["spins", 3]] as [string, number][] }];
  const back = readTimeline(timelineText("Board", timeline, samples, ["spins"]));
  assert.equal(back.timeline.origin, 1_763_600_000_000);
  assert.equal(back.timeline.interruptName(66), "TIM7");
  assert.equal(back.samples[0].at, 1_763_600_000_250.25);
});

test("an interrupt entered before the trace was lost is not drawn on", () => {
  const timeline = new Timeline(0);
  timeline.addInterrupts([
    { at: 1000, number: 14, event: "entered" },
    { at: 1000.5, number: 0, event: "lost" },
    // Its exit after: of another run, entered while the trace was lost.
    { at: 1020, number: 14, event: "exited" },
    { at: 1021, number: 14, event: "entered" },
    { at: 1021.002, number: 14, event: "exited" },
  ]);
  const runs: [number, number][] = [];
  timeline.interruptRuns(14, 0, 2000, (start, end) => runs.push([start, end]));
  assert.deepEqual(runs, [[1021, 1021.002]]);
  // Back in the thread, all that were entered were left: an exit SWO lost is not taken from a later run.
  timeline.addInterrupts([
    { at: 1030, number: 66, event: "entered" },
    { at: 1030.001, number: 0, event: "returned" },
    { at: 1040, number: 66, event: "exited" },
  ]);
  assert.equal(timeline.interrupts.has(66), false);
});

test("a halt is no task's run, and is drawn apart", () => {
  const timeline = new Timeline(0);
  timeline.add({ switches: [[1000, BUSY], [1004, HALTED], [1010, BUSY]], until: 1012, samples: 3 });
  assert.deepEqual(timeline.order, [BUSY]);
  const runs: [number, number, number][] = [];
  timeline.runs(0, 2000, (task, start, end) => runs.push([task, start, end]));
  assert.deepEqual(runs, [
    [BUSY, 1000, 1004],
    [BUSY, 1010, 1012],
  ]);
  const halts: [number, number][] = [];
  timeline.halts(0, 2000, (start, end) => halts.push([start, end]));
  assert.deepEqual(halts, [[1004, 1010]]);
  assert.equal(timeline.taskAt(1005), null);
  assert.equal(timeline.taskAt(1011), BUSY);
});

test("what is not known is told apart from what ran", () => {
  const timeline = new Timeline(0);
  timeline.add({ switches: [[1000, BUSY], [1002, 0], [1005, IDLE]], until: 1006, samples: 3 });
  const unknown: [number, number][] = [];
  timeline.unknowns(0, 2000, (start, end) => unknown.push([start, end]));
  assert.deepEqual(unknown, [[1002, 1005]]);
  assert.equal(timeline.taskAt(1003), null);
});
