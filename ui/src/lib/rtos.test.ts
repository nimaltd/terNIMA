// Run with npm test: Node runs TypeScript as it is.

import assert from "node:assert/strict";
import { test } from "node:test";

import {
  addCpuHistory,
  bytes,
  cpuLoad,
  isIdle,
  noCpuRanges,
  type RtosTask,
  stackFillNote,
  stackMost,
  stackNow,
  sparkPoints,
  stackPeak,
  widenCpu,
} from "./rtos.ts";

function task(size: number | null, used: number | null, free: number | null): RtosTask {
  return {
    name: "blink",
    state: "sleeping",
    priority: 7,
    stack_size: size,
    stack_used: used,
    stack_free: free,
    cpu: null,
    address: 0x2000_0200,
  };
}

test("a stack's peak is all but what was never touched", () => {
  assert.equal(stackPeak(task(1024, 300, 600)), 424);
  assert.equal(stackPeak(task(1024, 300, null)), null);
  assert.equal(stackPeak(task(null, null, 600)), null);
});

test("a stack is as full as it is now, and its high-water mark as its peak", () => {
  assert.equal(stackNow(task(1000, 300, 600)), 0.3);
  assert.equal(stackMost(task(1000, 300, 600)), 0.4);
  // The kernel does not fill the stacks: no mark.
  assert.equal(stackMost(task(1000, 300, null)), null);
  // Where its stack pointer is not known: only the mark.
  assert.equal(stackNow(task(1000, null, 600)), null);
  assert.equal(stackMost(task(1000, null, 600)), 0.4);
  assert.equal(stackNow(task(null, 300, 600)), null);
});

test("where no stack holds the kernel's fill, the note says what fills them", () => {
  assert.equal(stackFillNote("zephyr", [task(1024, 300, null), task(512, 100, null)]), "CONFIG_INIT_STACKS=y, for the most each stack has used");
  assert.match(stackFillNote("freertos", [task(1024, 300, null)]) ?? "", /^INCLUDE_uxTaskGetStackHighWaterMark 1/);
  // One that holds it: they are filled, and that one went deep.
  assert.equal(stackFillNote("zephyr", [task(1024, 300, null), task(512, 100, 200)]), null);
  // No sizes: nothing to fill.
  assert.equal(stackFillNote("zephyr", [task(null, 300, null)]), null);
});

function shared(name: string, address: number, cpu: number | null): RtosTask {
  return { ...task(1024, 300, 600), name, address, cpu };
}

test("the load is all but the idle task's share, where that is known", () => {
  assert.ok(isIdle(shared("idle", 1, 0)) && isIdle(shared("IDLE0", 1, 0)) && !isIdle(shared("main", 1, 0)));
  assert.equal(cpuLoad([shared("main", 1, 3), shared("idle", 2, 97)]), 3);
  assert.equal(cpuLoad([shared("main", 1, 3), shared("idle", 2, null)]), null);
  assert.equal(cpuLoad([shared("main", 1, 3)]), null);
});

test("a minute of each task's shares of the CPU is kept, the newest last", () => {
  let history = new Map<number, number[]>();
  for (let second = 0; second < 70; second++) {
    history = addCpuHistory(history, [shared("main", 1, second), shared("idle", 2, null)]);
  }
  assert.equal(history.get(1)?.length, 60);
  assert.equal(history.get(1)?.[0], 10);
  assert.equal(history.get(1)?.at(-1), 69);
  // A share not known is not drawn as 0.
  assert.equal(history.has(2), false);
});

test("the line through the shares ends at the right edge, scaled to the most of them", () => {
  // Two shares of a minute's 60 slots, in a box 590 wide: 10 a slot.
  assert.equal(sparkPoints([0, 8], 590, 18), "580.0,17.0 590.0,4.2");
  // A task that never ran lies along the bottom.
  assert.equal(sparkPoints([0, 0], 590, 18), "580.0,17.0 590.0,17.0");
});

test("each task's least and most share of the CPU take in each read", () => {
  let ranges = noCpuRanges();
  ranges = widenCpu(ranges, [shared("main", 1, 3), shared("idle", 2, 97)]);
  const first = ranges;
  ranges = widenCpu(ranges, [shared("main", 1, 12.5), shared("idle", 2, 87.5)]);
  // A share not known yet, as at the first read, leaves the ranges as they were.
  ranges = widenCpu(ranges, [shared("main", 1, 0.5), shared("idle", 2, null)]);
  assert.deepEqual(ranges.tasks.get(1), { min: 0.5, max: 12.5 });
  assert.deepEqual(ranges.tasks.get(2), { min: 87.5, max: 97 });
  assert.deepEqual(ranges.load, { min: 3, max: 12.5 });
  // What was frozen on Pause stays as it was.
  assert.deepEqual(first.tasks.get(1), { min: 3, max: 3 });
});

test("bytes are written short", () => {
  assert.equal(bytes(512), "512 B");
  assert.equal(bytes(1536), "1.5 KB");
  assert.equal(bytes(20480), "20 KB");
});
