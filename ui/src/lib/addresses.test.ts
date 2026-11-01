// Run with npm test: Node runs TypeScript as it is.

import assert from "node:assert/strict";
import { test } from "node:test";
import { codeAddresses, placeText } from "./addresses.ts";

test("a Zephyr fatal error's addresses are found, its colors taken out first", () => {
  assert.deepEqual(
    codeAddresses("\x1b[1;31m[00:00:05.124,000] <err> os: Faulting instruction address (r15/pc): 0x08002b14\x1b[0m"),
    ["0x08002b14"],
  );
  assert.deepEqual(codeAddresses("E: r12/ip:  0x00000000 r14/lr:  0x08001a2f"), ["0x00000000", "0x08001a2f"]);
});

test("an ESP32 backtrace gives every address of its pairs", () => {
  assert.deepEqual(codeAddresses("Backtrace: 0x400d1234:0x3ffb1234 0x400d5678:0x3ffb1250"), [
    "0x400d1234",
    "0x3ffb1234",
    "0x400d5678",
    "0x3ffb1250",
  ]);
});

test("digits without 0x count only on a line that speaks of registers or a fault", () => {
  assert.deepEqual(codeAddresses("PC = 08002B14"), ["08002B14"]);
  assert.deepEqual(codeAddresses("HardFault at 08002B14, LR 08001A2F"), ["08002B14", "08001A2F"]);
  assert.deepEqual(codeAddresses("serial number 08002B14"), []);
  assert.deepEqual(codeAddresses("temperature 21.5, uptime 12345678"), []);
});

test("a line says each address once, and a very long one stops at a limit", () => {
  assert.deepEqual(codeAddresses("pc 0x08000100 again 0x08000100"), ["0x08000100"]);
  const many = Array.from({ length: 40 }, (_, index) => `0x${(index + 0x1000).toString(16)}`).join(" ");
  assert.equal(codeAddresses(many).length, 16);
});

test("text around an address does not make one", () => {
  assert.deepEqual(codeAddresses("no addresses here, 0xZZ, x0800"), []);
  assert.deepEqual(codeAddresses(""), []);
});

test("a place is written with its file and line when it has them", () => {
  assert.equal(
    placeText("0x08002b14", { function: "sensor_read", file: "src/sensor.c", line: 42 }),
    "↳ 0x08002b14  src/sensor.c:42 in sensor_read()",
  );
  assert.equal(placeText("0x08002b14", { function: "sensor_read", file: null, line: null }), "↳ 0x08002b14  in sensor_read()");
  assert.equal(
    placeText("0x08002b14", { function: "sensor_read", file: "src/sensor.c", line: null }),
    "↳ 0x08002b14  src/sensor.c in sensor_read()",
  );
});
