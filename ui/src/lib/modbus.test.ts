// Run with npm test: Node runs TypeScript as it is.

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  ModbusReader,
  answer,
  crc16,
  describeRequest,
  describeResponse,
  frame,
  lrc,
  parseRequest,
  parseResponse,
  requestPdu,
  type Request,
  type Tables,
} from "./modbus.ts";

const hex = (bytes: Uint8Array) => Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join(" ");
const readTen: Request = { unit: 1, fc: 3, address: 0, count: 10, values: [] };

test("checks and frames match the published examples", () => {
  assert.equal(crc16(new TextEncoder().encode("123456789")), 0x4b37);
  assert.equal(lrc([0x01, 0x03, 0x00, 0x00, 0x00, 0x0a]), 0xf2);
  const pdu = requestPdu(readTen);
  assert.equal(hex(frame("rtu", 1, pdu)), "01 03 00 00 00 0a c5 cd");
  assert.equal(new TextDecoder().decode(frame("ascii", 1, pdu)), ":01030000000AF2\r\n");
  assert.equal(hex(frame("tcp", 1, pdu, 1)), "00 01 00 00 00 06 01 03 00 00 00 0a");
});

test("writes are built as the standard says", () => {
  const coil = requestPdu({ unit: 1, fc: 5, address: 0xac, count: 1, values: [1] });
  assert.equal(hex(coil), "05 00 ac ff 00");
  const coils = requestPdu({ unit: 1, fc: 15, address: 19, count: 10, values: [1, 0, 1, 1, 0, 0, 1, 1, 1, 0] });
  assert.equal(hex(coils), "0f 00 13 00 0a 02 cd 01");
  const registers = requestPdu({ unit: 1, fc: 16, address: 1, count: 2, values: [0x000a, 0x0102] });
  assert.equal(hex(registers), "10 00 01 00 02 04 00 0a 01 02");
  assert.deepEqual(parseRequest(1, coils), { unit: 1, fc: 15, address: 19, count: 10, values: [1, 0, 1, 1, 0, 0, 1, 1, 1, 0] });
});

function tables(): Tables {
  return {
    start: 100,
    coils: [false, true, false, false],
    discrete: [true, false, true],
    holding: [1, 2, 3, 4, 5],
    input: [500, 600],
  };
}

test("a slave answers reads from its tables", () => {
  const table = tables();
  const read = answer(requestPdu({ unit: 1, fc: 3, address: 101, count: 3, values: [] }), table);
  assert.equal(hex(read), "03 06 00 02 00 03 00 04");
  assert.deepEqual(parseResponse(1, read), { unit: 1, fc: 3, registers: [2, 3, 4] });
  const bits = answer(requestPdu({ unit: 1, fc: 2, address: 100, count: 3, values: [] }), table);
  assert.deepEqual(parseResponse(1, bits, 3), { unit: 1, fc: 2, bits: [true, false, true] });
  const input = answer(requestPdu({ unit: 1, fc: 4, address: 101, count: 1, values: [] }), table);
  assert.deepEqual(parseResponse(1, input), { unit: 1, fc: 4, registers: [600] });
});

test("writes change a slave's tables and are echoed", () => {
  const table = tables();
  const single = requestPdu({ unit: 1, fc: 6, address: 104, count: 1, values: [0xbeef] });
  assert.deepEqual(answer(single, table), single);
  assert.equal(table.holding[4], 0xbeef);
  answer(requestPdu({ unit: 1, fc: 16, address: 100, count: 2, values: [7, 8] }), table);
  assert.deepEqual(table.holding.slice(0, 2), [7, 8]);
  answer(requestPdu({ unit: 1, fc: 15, address: 100, count: 4, values: [1, 1, 0, 1] }), table);
  assert.deepEqual(table.coils, [true, true, false, true]);
  const echo = parseResponse(1, answer(requestPdu({ unit: 1, fc: 5, address: 102, count: 1, values: [1] }), table));
  assert.deepEqual(echo, { unit: 1, fc: 5, address: 102, value: 1 });
  assert.equal(table.coils[2], true);
});

test("a slave answers what it cannot do with the right exception", () => {
  const table = tables();
  const outside = parseResponse(1, answer(requestPdu({ unit: 1, fc: 3, address: 99, count: 2, values: [] }), table));
  assert.deepEqual(outside, { unit: 1, fc: 3, exception: 2 });
  const tooMany = parseResponse(1, answer(requestPdu({ unit: 1, fc: 3, address: 100, count: 126, values: [] }), table));
  assert.equal(tooMany && typeof tooMany !== "string" && tooMany.exception, 3);
  const unknown = parseResponse(1, answer(Uint8Array.of(7), table));
  assert.deepEqual(unknown, { unit: 1, fc: 7, exception: 1 });
  assert.equal(describeResponse({ unit: 1, fc: 3, exception: 2 }), "exception 2: illegal data address");
});

const rtuResponse = frame("rtu", 1, Uint8Array.of(3, 4, 0, 1, 0, 2));

test("a master's reader finds a response that comes in pieces, after noise", () => {
  const reader = new ModbusReader("rtu", "responses");
  assert.deepEqual(reader.feed(Uint8Array.of(0x00, 0x01), 0), []);
  const messages = reader.feed(rtuResponse.subarray(1), 10);
  assert.equal(messages.length, 1);
  assert.ok(messages[0].ok);
  assert.deepEqual(parseResponse(messages[0].unit, messages[0].pdu), { unit: 1, fc: 3, registers: [1, 2] });
});

test("a response with a wrong CRC is no message, and the reader says why", () => {
  const bad = rtuResponse.slice();
  bad[4] ^= 1;
  const reader = new ModbusReader("rtu", "responses");
  assert.deepEqual(reader.feed(bad, 0), []);
  assert.equal(reader.problem, "wrong CRC");
  assert.equal(reader.feed(rtuResponse, 10).length, 1);
  assert.equal(reader.problem, null);
});

test("a pause ends a message that was half way", () => {
  const reader = new ModbusReader("rtu", "responses", 50);
  reader.feed(rtuResponse.subarray(0, 4), 0);
  const messages = reader.feed(rtuResponse, 100);
  assert.equal(messages.length, 1);
  assert.ok(messages[0].ok);
});

test("a slave's reader skips noise and a bad CRC, and a listener tells requests and responses apart", () => {
  const request = frame("rtu", 1, requestPdu(readTen));
  const broken = request.slice();
  broken[3] ^= 1;
  const slave = new ModbusReader("rtu", "requests");
  const found = slave.feed(Uint8Array.from([0xff, ...broken, ...request]), 0);
  assert.equal(found.length, 1);
  assert.ok(found[0].ok);
  assert.equal(describeRequest(parseRequest(found[0].unit, found[0].pdu) as Request), "read holding registers 0 to 9, unit 1");
  const listener = new ModbusReader("rtu", "either");
  const both = listener.feed(Uint8Array.from([...request, ...rtuResponse]), 0);
  assert.deepEqual(
    both.map((message) => hex(message.raw)),
    [hex(request), hex(rtuResponse)],
  );
});

test("ASCII lines are read and their LRC checked", () => {
  const reader = new ModbusReader("ascii", "either");
  const good = new TextEncoder().encode(":01030000000AF2\r\n");
  const bad = new TextEncoder().encode(":01030000000AF3\r\n");
  const messages = reader.feed(Uint8Array.from([...good, ...bad]), 0);
  assert.equal(messages.length, 2);
  assert.ok(messages[0].ok);
  assert.equal(hex(messages[0].pdu), "03 00 00 00 0a");
  assert.equal(messages[1].problem, "wrong LRC");
});

test("TCP messages are split by their header, with their transaction", () => {
  const one = frame("tcp", 1, requestPdu(readTen), 7);
  const two = frame("tcp", 2, Uint8Array.of(3, 2, 0, 5), 8);
  const messages = new ModbusReader("tcp", "either").feed(Uint8Array.from([...one, ...two]), 0);
  assert.deepEqual(
    messages.map((message) => [message.transaction, message.unit, hex(message.pdu)]),
    [
      [7, 1, "03 00 00 00 0a"],
      [8, 2, "03 02 00 05"],
    ],
  );
});
