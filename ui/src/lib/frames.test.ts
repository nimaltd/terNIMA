// Run with npm test: Node runs TypeScript as it is.

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  DEFAULT_FRAME_SPEC,
  FrameReader,
  checkOf,
  decodeCobs,
  decodeSlip,
  encodeCobs,
  nmea,
  parseHex,
  readField,
  type FrameSpec,
} from "./frames.ts";
import { frame as modbusFrame, requestPdu } from "./modbus.ts";

const text = (value: string) => new TextEncoder().encode(value);

test("checks match their published check values for 123456789", () => {
  const digits = text("123456789");
  assert.equal(checkOf("sum8", digits), 0xdd);
  assert.equal(checkOf("xor8", digits), 0x31);
  assert.equal(checkOf("crc8", digits), 0xf4);
  assert.equal(checkOf("crc16_modbus", digits), 0x4b37);
  assert.equal(checkOf("crc16_ccitt", digits), 0x29b1);
  assert.equal(checkOf("crc16_xmodem", digits), 0x31c3);
  assert.equal(checkOf("crc32", digits), 0xcbf43926);
});

test("hex reads with or without spaces and 0x", () => {
  assert.deepEqual(parseHex("AA 55"), Uint8Array.of(0xaa, 0x55));
  assert.deepEqual(parseHex("0xaa,0x55"), Uint8Array.of(0xaa, 0x55));
  assert.deepEqual(parseHex(""), new Uint8Array(0));
  assert.equal(parseHex("A"), null);
  assert.equal(parseHex("zz"), null);
});

test("fields read every type, both ways, scaled", () => {
  const bytes = Uint8Array.of(0xff, 0x34, 0x12, 0x00, 0x00, 0x80, 0x3f);
  assert.equal(readField(bytes, { name: "a", offset: 0, type: "u8", endian: "le", scale: 1 }), 255);
  assert.equal(readField(bytes, { name: "a", offset: 0, type: "i8", endian: "le", scale: 1 }), -1);
  assert.equal(readField(bytes, { name: "a", offset: 1, type: "u16", endian: "le", scale: 1 }), 0x1234);
  assert.equal(readField(bytes, { name: "a", offset: 1, type: "u16", endian: "be", scale: 1 }), 0x3412);
  assert.equal(readField(bytes, { name: "a", offset: 1, type: "u16", endian: "le", scale: 0.5 }), 0x1234 / 2);
  assert.equal(readField(bytes, { name: "a", offset: 3, type: "f32", endian: "le", scale: 1 }), 1);
  assert.equal(readField(bytes, { name: "a", offset: 5, type: "u32", endian: "le", scale: 1 }), null);
});

/** AA 55, a length of the bytes after it, temp i16 /100, hum u16 /10, then a Modbus CRC. */
const layout: FrameSpec = {
  ...DEFAULT_FRAME_SPEC,
  decoder: "layout",
  start: "AA 55",
  length: { offset: 2, size: 1, endian: "le", counts: "after", adjust: 0 },
  fields: [
    { name: "temp", offset: 3, type: "i16", endian: "le", scale: 0.01 },
    { name: "hum", offset: 5, type: "u16", endian: "le", scale: 0.1 },
  ],
  check: { kind: "crc16_modbus", from: 0, endian: "le" },
};

function frameOf(temp: number, hum: number): Uint8Array {
  const body = [0xaa, 0x55, 6, temp & 0xff, (temp >> 8) & 0xff, hum & 0xff, hum >> 8];
  const crc = checkOf("crc16_modbus", Uint8Array.from(body));
  return Uint8Array.from([...body, crc & 0xff, crc >> 8]);
}

test("a layout finds its frames among noise, in pieces, and reads them", () => {
  const reader = new FrameReader(layout);
  const stream = Uint8Array.from([0x01, 0xaa, ...frameOf(2150, 405), ...frameOf(-125, 999)]);
  const frames = [...reader.feed(stream.subarray(0, 7)), ...reader.feed(stream.subarray(7))];
  assert.equal(frames.length, 2);
  assert.ok(frames.every((frame) => frame.ok));
  assert.equal(frames[0].values[0][0], "temp");
  assert.ok(Math.abs(frames[0].values[0][1] - 21.5) < 1e-9);
  assert.ok(Math.abs(frames[0].values[1][1] - 40.5) < 1e-9);
  assert.ok(Math.abs(frames[1].values[0][1] + 1.25) < 1e-9);
  assert.equal(reader.skipped, 2);
});

test("a bad check is reported, and the next good frame still found", () => {
  const reader = new FrameReader(layout);
  const bad = frameOf(100, 200);
  bad[4] ^= 0xff;
  const frames = reader.feed(Uint8Array.from([...bad, ...frameOf(300, 400)]));
  assert.equal(frames[0].ok, false);
  assert.match(frames[0].problem ?? "", /check is 0x[0-9A-F]{4}, should be 0x[0-9A-F]{4}/);
  const good = frames.filter((frame) => frame.ok);
  assert.equal(good.length, 1);
  assert.ok(Math.abs(good[0].values[0][1] - 3) < 1e-9);
});

test("fixed size frames without a start or a check", () => {
  const reader = new FrameReader({
    ...DEFAULT_FRAME_SPEC,
    decoder: "layout",
    size: 2,
    fields: [{ name: "n", offset: 0, type: "u16", endian: "be", scale: 1 }],
  });
  const frames = reader.feed(Uint8Array.of(0, 1, 0, 2, 0));
  assert.deepEqual(
    frames.map((frame) => frame.values[0][1]),
    [1, 2],
  );
});

test("COBS comes back as it went, zeros and long runs included", () => {
  assert.deepEqual(encodeCobs(Uint8Array.of(0x11, 0x22, 0x00, 0x33)), Uint8Array.of(0x03, 0x11, 0x22, 0x02, 0x33));
  assert.deepEqual(encodeCobs(Uint8Array.of(0)), Uint8Array.of(1, 1));
  for (const size of [0, 1, 253, 254, 255, 600]) {
    const data = Uint8Array.from({ length: size }, (_, index) => (index * 7) % 5);
    assert.deepEqual(decodeCobs(encodeCobs(data)), data, `size ${size}`);
  }
  assert.equal(typeof decodeCobs(Uint8Array.of(5, 1, 2)), "string");
});

test("COBS frames end at 0, and a check is read from what they carried", () => {
  const spec: FrameSpec = {
    ...DEFAULT_FRAME_SPEC,
    decoder: "cobs",
    fields: [{ name: "v", offset: 0, type: "u16", endian: "le", scale: 1 }],
    check: { kind: "crc8", from: 0, endian: "le" },
  };
  const payload = Uint8Array.of(0x00, 0x01);
  const framed = Uint8Array.from([...encodeCobs(Uint8Array.from([...payload, checkOf("crc8", payload)])), 0]);
  const frames = new FrameReader(spec).feed(Uint8Array.from([...framed, ...framed]));
  assert.equal(frames.length, 2);
  assert.ok(frames[0].ok, frames[0].problem ?? "");
  assert.deepEqual(frames[0].values, [["v", 256]]);
});

test("SLIP frames between C0, with their escapes undone", () => {
  assert.deepEqual(decodeSlip(Uint8Array.of(1, 0xdb, 0xdc, 0xdb, 0xdd, 2)), Uint8Array.of(1, 0xc0, 0xdb, 2));
  assert.equal(typeof decodeSlip(Uint8Array.of(0xdb, 0x01)), "string");
  const reader = new FrameReader({ ...DEFAULT_FRAME_SPEC, decoder: "slip" });
  const frames = reader.feed(Uint8Array.of(0xc0, 1, 2, 0xc0, 0xc0, 3, 0xdb, 0xdc, 0xc0));
  assert.deepEqual(
    frames.map((frame) => Array.from(frame.bytes)),
    [[1, 2], [3, 0xc0]],
  );
});

test("NMEA sentences are checked and their positions read in degrees", () => {
  const gga = nmea(text("$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47\r"));
  assert.ok(gga?.ok, gga?.problem ?? "");
  const values = Object.fromEntries(gga!.values);
  assert.ok(Math.abs(values.lat - (48 + 7.038 / 60)) < 1e-9);
  assert.ok(Math.abs(values.lon - (11 + 31 / 60)) < 1e-9);
  assert.equal(values.sats, 8);
  assert.equal(values.alt, 545.4);
  const bad = nmea(text("$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*48"));
  assert.equal(bad?.ok, false);
  assert.equal(nmea(text("hello")), null);
  // South and west, with its checksum worked out here.
  const body = "GPRMC,123519,A,4807.038,S,01131.000,W,022.4,084.4,230394,003.1,W";
  let sum = 0;
  for (const char of body) sum ^= char.charCodeAt(0);
  const sentence = `$${body}*${sum.toString(16).toUpperCase().padStart(2, "0")}`;
  const reader = new FrameReader({ ...DEFAULT_FRAME_SPEC, decoder: "nmea" });
  const frames = reader.feed(text(`noise\r\n${sentence}\r\n`));
  assert.equal(frames.length, 1);
  const rmc = Object.fromEntries(frames[0].values);
  assert.ok(rmc.lat < 0 && rmc.lon < 0);
  assert.equal(rmc.knots, 22.4);
});

test("Modbus off a bus: requests, answers named by address, and exceptions", () => {
  const reader = new FrameReader({ ...DEFAULT_FRAME_SPEC, decoder: "modbus_rtu" });
  const read = modbusFrame("rtu", 1, requestPdu({ unit: 1, fc: 3, address: 10, count: 2, values: [] }));
  const answer = modbusFrame("rtu", 1, Uint8Array.of(3, 4, 0, 7, 1, 0));
  const failed = modbusFrame("rtu", 2, Uint8Array.of(0x83, 2));
  const frames = reader.feed(Uint8Array.from([...read, ...answer, ...failed]), 0);
  assert.equal(frames.length, 3);
  assert.equal(frames[0].note, "→ read holding registers 10 to 11, unit 1");
  assert.deepEqual(frames[1].values, [
    ["hr10", 7],
    ["hr11", 256],
  ]);
  assert.equal(frames[2].ok, false);
  assert.equal(frames[2].problem, "unit 2: exception 2: illegal data address");
  const ascii = new FrameReader({ ...DEFAULT_FRAME_SPEC, decoder: "modbus_ascii" });
  const line = ascii.feed(text(":01030000000AF2\r\n"), 0);
  assert.equal(line[0].note, "→ read holding registers 0 to 9, unit 1");
});
