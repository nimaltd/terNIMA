// Run with npm test: Node runs TypeScript as it is.

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  BusReader,
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

test("bytes that make no message are given as one, when a reader keeps them", () => {
  const bad = rtuResponse.slice();
  bad[4] ^= 1;
  const reader = new ModbusReader("rtu", "either", 50, true);
  const messages = reader.feed(Uint8Array.from([...bad, ...rtuResponse]), 0);
  assert.deepEqual(
    messages.map((message) => [message.ok, message.problem, hex(message.raw)]),
    [
      [false, "wrong CRC", hex(bad)],
      [true, null, hex(rtuResponse)],
    ],
  );
  // A pause ends a message cut short.
  assert.deepEqual(reader.feed(rtuResponse.subarray(0, 4), 100), []);
  const cut = reader.feed(rtuResponse, 200);
  assert.deepEqual(
    cut.map((message) => [message.ok, message.problem]),
    [
      [false, "cut short"],
      [true, null],
    ],
  );
});

const request = frame("rtu", 1, requestPdu(readTen));
const tenRegisters = frame("rtu", 1, Uint8Array.of(3, 20, ...Array.from({ length: 20 }, (_, index) => index)));

test("the bus pairs each response with its request, and says how long it took", () => {
  const bus = new BusReader("rtu");
  const asked = bus.feed(request, 1000, true);
  const answered = bus.feed(tenRegisters, 1042, false);
  assert.equal(asked.length, 1);
  assert.equal(asked[0].kind, "request");
  assert.ok(asked[0].sent);
  assert.deepEqual(asked[0].request, readTen);
  assert.equal(answered.length, 1);
  assert.equal(answered[0].kind, "response");
  assert.equal(answered[0].ms, 42);
  assert.deepEqual(answered[0].request, readTen);
  assert.deepEqual(answered[0].response?.registers, [1, 0x203, 0x405, 0x607, 0x809, 0xa0b, 0xc0d, 0xe0f, 0x1011, 0x1213]);
});

test("the echo of what terNIMA sent, as RS-485 adapters give, is left out", () => {
  const bus = new BusReader("rtu");
  bus.feed(request, 1000, true);
  assert.deepEqual(bus.feed(request, 1003, false), []);
  const answered = bus.feed(tenRegisters, 1040, false);
  assert.equal(answered[0].kind, "response");
  // Known to echo, a single write comes back twice: the echo, then the answer.
  const write = frame("rtu", 1, requestPdu({ unit: 1, fc: 6, address: 5, count: 1, values: [1234] }));
  bus.feed(write, 2000, true);
  assert.deepEqual(bus.feed(write, 2004, false), []);
  assert.equal(bus.feed(write, 2030, false)[0].kind, "response");
});

test("without an echo, the answer to a single write, the same bytes, is its response", () => {
  const bus = new BusReader("rtu");
  const write = frame("rtu", 1, requestPdu({ unit: 1, fc: 6, address: 5, count: 1, values: [1234] }));
  bus.feed(write, 0, true);
  const answered = bus.feed(write, 30, false);
  assert.equal(answered[0].kind, "response");
  assert.equal(answered[0].ms, 30);
});

// The first 8 of these 9 bytes have a right CRC too: the 7th byte is the low byte of the CRC of the first 6.
const tricky = Uint8Array.of(0x02, 0x04, 0x04, 0x01, 0xf3, 0x01, 0x25, 0xf9, 0x00);
const readTwo = frame("rtu", 2, requestPdu({ unit: 2, fc: 4, address: 0, count: 2, values: [] }));

test("a response whose first bytes make a request with a right CRC is read whole, even in pieces", () => {
  for (const sent of [true, false]) {
    const bus = new BusReader("rtu");
    bus.feed(readTwo, 0, sent);
    const frames = [...bus.feed(tricky.subarray(0, 8), 40, false), ...bus.feed(tricky.subarray(8), 42, false)];
    // Its time is when its last byte came.
    assert.deepEqual(
      frames.map((frame) => [frame.kind, frame.ms]),
      [["response", 42]],
    );
    assert.deepEqual(frames[0].response?.registers, [0x01f3, 0x0125]);
  }
});

test("what one side left half way comes before what the other sends after a pause, at its own time", () => {
  const bus = new BusReader("rtu");
  bus.feed(request, 0, true);
  const spoilt = tenRegisters.slice();
  spoilt[spoilt.length - 1] ^= 0x5a;
  assert.deepEqual(bus.feed(spoilt, 50, false), []);
  const frames = bus.feed(request, 400, true);
  assert.deepEqual(
    frames.map((frame) => [frame.kind, frame.at, frame.problem]),
    [
      [null, 50, "wrong CRC"],
      ["request", 400, null],
    ],
  );
  // And once the line is quiet, without anything after it.
  bus.feed(spoilt, 450, false);
  assert.deepEqual(
    bus.pause(600).map((frame) => [frame.at, frame.checked]),
    [[450, false]],
  );
});

test("a pause ends a request that was waited for as a longer response", () => {
  // From address 0x0400, its third byte is the byte count of the response to the first: 9 bytes could be one.
  const readHigh = frame("rtu", 2, requestPdu({ unit: 2, fc: 4, address: 0x400, count: 2, values: [] }));
  const bus = new BusReader("rtu");
  const first = bus.feed(readHigh, 0, false)[0];
  // Nothing answered; the next request comes, and then a pause.
  assert.deepEqual(bus.feed(readHigh, 300, false), []);
  const frames = bus.feed(Uint8Array.of(0x00), 400, false);
  assert.equal(frames[0].kind, "request");
  assert.equal(frames[0].at, 300);
  assert.ok(first.unanswered);
});

test("listening, a request nothing answers is marked, and an exception is a response", () => {
  const bus = new BusReader("rtu");
  const first = bus.feed(request, 0, false)[0];
  const second = bus.feed(request, 500, false)[0];
  assert.ok(first.unanswered);
  assert.equal(second.kind, "request");
  const exception = bus.feed(frame("rtu", 1, Uint8Array.of(0x83, 2)), 520, false)[0];
  assert.equal(exception.kind, "response");
  assert.equal(exception.fc, 3);
  assert.equal(exception.problem, "exception 2: illegal data address");
  assert.equal(exception.ms, 20);
  assert.ok(!second.unanswered);
});

test("a single write and its echo are a request and its response", () => {
  const bus = new BusReader("rtu");
  const write = frame("rtu", 1, requestPdu({ unit: 1, fc: 6, address: 2, count: 1, values: [1234] }));
  const frames = [...bus.feed(write, 0, false), ...bus.feed(write, 15, false), ...bus.feed(write, 500, false)];
  assert.deepEqual(
    frames.map((frame) => frame.kind),
    ["request", "response", "request"],
  );
  assert.deepEqual(frames[1].response, { unit: 1, fc: 6, address: 2, value: 1234 });
});

test("a response whose request came before terNIMA listened is read without it", () => {
  const frames = new BusReader("rtu").feed(rtuResponse, 0, false);
  assert.equal(frames[0].kind, "response");
  assert.equal(frames[0].request, null);
  assert.deepEqual(frames[0].response?.registers, [1, 2]);
});

test("broken bytes on the bus are a frame that is not ok", () => {
  const broken = tenRegisters.slice();
  broken[6] ^= 0x40;
  const frames = new BusReader("rtu").feed(Uint8Array.from([...broken, ...request]), 0, false);
  assert.deepEqual(
    frames.map((frame) => [frame.ok, frame.checked, frame.kind]),
    [
      [false, false, null],
      [true, true, "request"],
    ],
  );
});

test("ASCII on the bus: lines, their LRC, and the response with its request", () => {
  const bus = new BusReader("ascii");
  const line = (text: string) => new TextEncoder().encode(text);
  const asked = bus.feed(line(":01030000000AF2\r\n"), 0, false)[0];
  const answer = frame("ascii", 1, Uint8Array.of(3, 4, 0, 1, 0, 2));
  const answered = bus.feed(answer, 30, false)[0];
  assert.equal(asked.kind, "request");
  assert.equal(hex(asked.bytes), "01 03 00 00 00 0a f2");
  assert.equal(answered.kind, "response");
  assert.deepEqual(answered.response?.registers, [1, 2]);
  const bad = bus.feed(line(":01030000000AF3\r\n"), 60, false)[0];
  assert.ok(!bad.checked);
  assert.equal(bad.problem, "wrong LRC");
});
