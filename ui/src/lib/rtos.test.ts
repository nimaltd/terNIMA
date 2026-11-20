// Run with npm test: Node runs TypeScript as it is.

import assert from "node:assert/strict";
import { test } from "node:test";

import { bytes, type RtosTask, stackFill, stackPeak } from "./rtos.ts";

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

test("how full a stack has been is its peak, or else its use now", () => {
  assert.equal(stackFill(task(1000, 300, 600)), 0.4);
  assert.equal(stackFill(task(1000, 300, null)), 0.3);
  assert.equal(stackFill(task(null, 300, 600)), null);
});

test("bytes are written short", () => {
  assert.equal(bytes(512), "512 B");
  assert.equal(bytes(1536), "1.5 KB");
  assert.equal(bytes(20480), "20 KB");
});
