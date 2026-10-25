// Splitting received bytes into frames, checking them and reading their
// fields:
//
//   layout   frames of your own: start bytes, a length or a fixed size,
//            numeric fields at offsets, and a check at the end
//   cobs     frames ended by a 0 byte, COBS encoded
//   slip     frames between C0 bytes, SLIP escaped
//   nmea     $... sentences from GPS receivers, with their checksum
//   modbus   Modbus RTU or ASCII read off a bus: requests, and the answers
//            to them with their values named by address
//
// For COBS and SLIP, fields and the check are read from the decoded frame.
// It imports only modbus.ts, so its tests run in Node as they are.

import {
  ModbusReader,
  describeRequest,
  describeResponse,
  parseRequest,
  parseResponse,
  type Message,
  type Request,
  type Response,
} from "./modbus.ts";

export type FieldType = "u8" | "i8" | "u16" | "i16" | "u32" | "i32" | "f32" | "f64";
export type Endian = "le" | "be";
export type CheckKind = "none" | "sum8" | "xor8" | "crc8" | "crc16_modbus" | "crc16_ccitt" | "crc16_xmodem" | "crc32";
export type Decoder = "off" | "layout" | "cobs" | "slip" | "nmea" | "modbus_rtu" | "modbus_ascii";

/** A number in a frame, `scale` times what its bytes hold. */
export type Field = { name: string; offset: number; type: FieldType; endian: Endian; scale: number };

/** Where a frame's length is: a 1 or 2 byte count of the bytes after it, or of the whole frame, plus `adjust`. */
export type LengthField = { offset: number; size: 1 | 2; endian: Endian; counts: "after" | "frame"; adjust: number };

/** A check at the end of each frame, over its bytes from `from` up to the check. */
export type Check = { kind: CheckKind; from: number; endian: Endian };

export type FrameSpec = {
  decoder: Decoder;
  /** The bytes every frame starts with, in hex such as "AA 55". Layout only. */
  start: string;
  /** Where the length is; null for frames of `size` bytes. Layout only. */
  length: LengthField | null;
  size: number;
  fields: Field[];
  check: Check;
};

export const DEFAULT_FRAME_SPEC: FrameSpec = {
  decoder: "off",
  start: "",
  length: null,
  size: 8,
  fields: [],
  check: { kind: "none", from: 0, endian: "le" },
};

export type Frame = {
  /** The frame as decoded: a layout frame whole, or what COBS or SLIP carried, or an NMEA sentence. */
  bytes: Uint8Array;
  /** Whether its check and fields were right. */
  ok: boolean;
  /** What is wrong with it, in plain words. */
  problem: string | null;
  values: [name: string, value: number][];
  /** What it says, in words, for Modbus. */
  note?: string;
};

/** The longest frame looked for; a longer length is taken as noise. */
export const FRAME_MAX = 4096;

export const FIELD_SIZES: Record<FieldType, number> = { u8: 1, i8: 1, u16: 2, i16: 2, u32: 4, i32: 4, f32: 4, f64: 8 };
export const CHECK_SIZES: Record<CheckKind, number> = {
  none: 0,
  sum8: 1,
  xor8: 1,
  crc8: 1,
  crc16_modbus: 2,
  crc16_ccitt: 2,
  crc16_xmodem: 2,
  crc32: 4,
};

/** Reads hex such as "AA 55" or "aa55" into bytes; null when it is not hex. */
export function parseHex(text: string): Uint8Array | null {
  const digits = text.replace(/0x/gi, "").replace(/[\s,]+/g, "");
  if (digits.length % 2 !== 0 || !/^[0-9a-f]*$/i.test(digits)) return null;
  return Uint8Array.from(digits.match(/../g) ?? [], (pair) => parseInt(pair, 16));
}

export function hex(bytes: Uint8Array, max = Infinity): string {
  const shown = Array.from(bytes.subarray(0, Math.min(bytes.length, max)), (byte) =>
    byte.toString(16).padStart(2, "0").toUpperCase(),
  ).join(" ");
  return bytes.length > max ? `${shown} …` : shown;
}

/** The check of some bytes, as a number. */
export function checkOf(kind: CheckKind, bytes: Uint8Array): number {
  let value = 0;
  switch (kind) {
    case "sum8":
      for (const byte of bytes) value = (value + byte) & 0xff;
      return value;
    case "xor8":
      for (const byte of bytes) value ^= byte;
      return value;
    case "crc8":
      for (const byte of bytes) {
        value ^= byte;
        for (let bit = 0; bit < 8; bit++) value = value & 0x80 ? ((value << 1) ^ 0x07) & 0xff : (value << 1) & 0xff;
      }
      return value;
    case "crc16_modbus":
      value = 0xffff;
      for (const byte of bytes) {
        value ^= byte;
        for (let bit = 0; bit < 8; bit++) value = value & 1 ? (value >>> 1) ^ 0xa001 : value >>> 1;
      }
      return value;
    case "crc16_ccitt":
    case "crc16_xmodem":
      value = kind === "crc16_ccitt" ? 0xffff : 0;
      for (const byte of bytes) {
        value ^= byte << 8;
        for (let bit = 0; bit < 8; bit++) value = value & 0x8000 ? ((value << 1) ^ 0x1021) & 0xffff : (value << 1) & 0xffff;
      }
      return value;
    case "crc32":
      value = 0xffffffff;
      for (const byte of bytes) {
        value ^= byte;
        for (let bit = 0; bit < 8; bit++) value = value & 1 ? (value >>> 1) ^ 0xedb88320 : value >>> 1;
      }
      return (value ^ 0xffffffff) >>> 0;
    default:
      return 0;
  }
}

function readUnsigned(bytes: Uint8Array, offset: number, size: number, endian: Endian): number {
  let value = 0;
  for (let at = 0; at < size; at++) {
    const byte = bytes[offset + (endian === "le" ? size - 1 - at : at)];
    value = value * 256 + byte;
  }
  return value;
}

/** A field's value, or null when the frame is too short for it. */
export function readField(bytes: Uint8Array, field: Field): number | null {
  const size = FIELD_SIZES[field.type];
  if (field.offset < 0 || field.offset + size > bytes.length) return null;
  const view = new DataView(bytes.buffer, bytes.byteOffset + field.offset, size);
  const le = field.endian === "le";
  let value: number;
  switch (field.type) {
    case "u8":
      value = view.getUint8(0);
      break;
    case "i8":
      value = view.getInt8(0);
      break;
    case "u16":
      value = view.getUint16(0, le);
      break;
    case "i16":
      value = view.getInt16(0, le);
      break;
    case "u32":
      value = view.getUint32(0, le);
      break;
    case "i32":
      value = view.getInt32(0, le);
      break;
    case "f32":
      value = view.getFloat32(0, le);
      break;
    default:
      value = view.getFloat64(0, le);
  }
  return value * (field.scale || 1);
}

const hex2 = (value: number, size: number) => `0x${value.toString(16).toUpperCase().padStart(size * 2, "0")}`;

/** Checks a frame and reads its fields. */
function examine(bytes: Uint8Array, spec: FrameSpec): Frame {
  const problems: string[] = [];
  const size = CHECK_SIZES[spec.check.kind];
  if (size > 0) {
    const end = bytes.length - size;
    if (end < spec.check.from) {
      problems.push("too short for its check");
    } else {
      const found = readUnsigned(bytes, end, size, spec.check.endian);
      const wanted = checkOf(spec.check.kind, bytes.subarray(spec.check.from, end));
      if (found !== wanted) problems.push(`check is ${hex2(found, size)}, should be ${hex2(wanted, size)}`);
    }
  }
  const values: [string, number][] = [];
  for (const field of spec.fields) {
    const value = readField(bytes, field);
    if (value === null) problems.push(`too short for ${field.name}`);
    else values.push([field.name, value]);
  }
  return { bytes, ok: problems.length === 0, problem: problems.length > 0 ? problems.join("; ") : null, values };
}

/** Splits a stream of bytes into frames, as they come. */
export class FrameReader {
  private buffer = new Uint8Array(0);
  private readonly spec: FrameSpec;
  private readonly start: Uint8Array;
  private readonly modbus: ModbusListener | null;
  /** Bytes dropped between frames: noise, or frames that never ended. */
  skipped = 0;

  constructor(spec: FrameSpec) {
    this.spec = spec;
    this.start = parseHex(spec.start) ?? new Uint8Array(0);
    this.modbus =
      spec.decoder === "modbus_rtu"
        ? new ModbusListener("rtu")
        : spec.decoder === "modbus_ascii"
          ? new ModbusListener("ascii")
          : null;
  }

  /** Reads frames from bytes that came at `at`, in ms; Modbus RTU takes a pause as the end of a message. */
  feed(bytes: Uint8Array, at = Date.now()): Frame[] {
    if (this.spec.decoder === "off" || bytes.length === 0) return [];
    if (this.modbus) return this.modbus.feed(bytes, at);
    const joined = new Uint8Array(this.buffer.length + bytes.length);
    joined.set(this.buffer);
    joined.set(bytes, this.buffer.length);
    this.buffer = joined;
    switch (this.spec.decoder) {
      case "cobs":
        return this.delimited(0x00, decodeCobs);
      case "slip":
        return this.delimited(0xc0, decodeSlip);
      case "nmea":
        return this.sentences();
      default:
        return this.layout();
    }
  }

  /** Keeps the bytes from `at` on for later. */
  private keep(at: number) {
    this.buffer = this.buffer.slice(at);
  }

  private layout(): Frame[] {
    const out: Frame[] = [];
    const { length, size } = this.spec;
    const buffer = this.buffer;
    let at = 0;
    while (at < buffer.length) {
      const found = findStart(buffer, this.start, at);
      if (found < 0) {
        // The end may hold the first bytes of a start.
        const left = Math.max(at, buffer.length - (this.start.length - 1));
        this.skipped += left - at;
        at = left;
        break;
      }
      this.skipped += found - at;
      at = found;
      let total = size;
      if (length) {
        if (buffer.length - at < length.offset + length.size) break;
        const count = readUnsigned(buffer, at + length.offset, length.size, length.endian);
        total = (length.counts === "after" ? length.offset + length.size : 0) + count + length.adjust;
      }
      const smallest = Math.max(this.start.length, 1, length ? length.offset + length.size : 0);
      if (total < smallest || total > FRAME_MAX) {
        // Not a frame after all: look again from the next byte.
        at++;
        this.skipped++;
        continue;
      }
      if (buffer.length - at < total) break;
      const frame = examine(buffer.slice(at, at + total), this.spec);
      if (frame.ok || this.start.length > 0) out.push(frame);
      if (frame.ok) {
        at += total;
      } else {
        // A bad frame may have been noise that looked like a start.
        at++;
        if (this.start.length === 0) this.skipped++;
      }
    }
    this.keep(at);
    if (this.buffer.length > FRAME_MAX * 2) {
      this.skipped += this.buffer.length - FRAME_MAX;
      this.keep(this.buffer.length - FRAME_MAX);
    }
    return out;
  }

  private delimited(end: number, decode: (bytes: Uint8Array) => Uint8Array | string): Frame[] {
    const out: Frame[] = [];
    let from = 0;
    for (let at = 0; at < this.buffer.length; at++) {
      if (this.buffer[at] !== end) continue;
      const raw = this.buffer.subarray(from, at);
      from = at + 1;
      // SLIP often starts a frame with END too: an empty frame is no frame.
      if (raw.length === 0) continue;
      const decoded = decode(raw);
      if (typeof decoded === "string") {
        out.push({ bytes: raw.slice(), ok: false, problem: decoded, values: [] });
      } else {
        out.push(examine(decoded, this.spec));
      }
    }
    this.keep(from);
    if (this.buffer.length > FRAME_MAX) {
      this.skipped += this.buffer.length;
      this.buffer = new Uint8Array(0);
    }
    return out;
  }

  private sentences(): Frame[] {
    const out: Frame[] = [];
    let from = 0;
    for (let at = 0; at < this.buffer.length; at++) {
      if (this.buffer[at] !== 0x0a) continue;
      const line = this.buffer.subarray(from, at);
      from = at + 1;
      const frame = nmea(line);
      if (frame) out.push(frame);
    }
    this.keep(from);
    if (this.buffer.length > FRAME_MAX) {
      this.buffer = new Uint8Array(0);
    }
    return out;
  }
}

/** Reads Modbus off a bus: requests, and the answers to them, named by what was asked. */
class ModbusListener {
  private readonly reader: ModbusReader;
  /** What each unit was asked last, to name the values of its answer. */
  private readonly asked = new Map<number, Request>();

  constructor(mode: "rtu" | "ascii") {
    this.reader = new ModbusReader(mode, "either");
  }

  feed(bytes: Uint8Array, at: number): Frame[] {
    return this.reader.feed(bytes, at).map((message) => this.frameOf(message));
  }

  private frameOf(message: Message): Frame {
    const frame: Frame = { bytes: message.raw, ok: false, problem: message.problem, values: [] };
    if (!message.ok) return frame;
    const { unit, pdu } = message;
    const asked = this.asked.get(unit);
    if (pdu[0] & 0x80 || answers(pdu, asked)) {
      this.asked.delete(unit);
      const response = parseResponse(unit, pdu, asked?.count);
      if (typeof response === "string") return { ...frame, problem: response };
      if (response.exception !== undefined) {
        return { ...frame, problem: `unit ${unit}: ${describeResponse(response)}`, note: `← unit ${unit}` };
      }
      const values = asked ? namedValues(asked, response) : [];
      const note = response.bits || response.registers ? `← unit ${unit}` : `← unit ${unit} ${describeResponse(response)}`;
      return { ...frame, ok: true, problem: null, values, note };
    }
    const request = parseRequest(unit, pdu);
    if (typeof request === "string") return { ...frame, problem: request };
    this.asked.set(unit, request);
    return { ...frame, ok: true, problem: null, note: `→ ${describeRequest(request)}` };
  }
}

/** Whether a PDU is the answer to what was asked, rather than a new request. */
function answers(pdu: Uint8Array, asked: Request | undefined): boolean {
  if (!asked || asked.fc !== pdu[0]) return false;
  const fc = asked.fc;
  if (fc <= 4) {
    // A read's answer is its byte count and the bytes; registers come in pairs.
    return pdu.length === 2 + pdu[1] && (fc <= 2 || pdu[1] % 2 === 0);
  }
  // A single write is echoed; a multiple write is answered with its address and count.
  return fc === 5 || fc === 6 || pdu.length === 5;
}

/** A read's values, named by their address: hr0, ir5, c3, di1; with the unit, but for unit 1. */
function namedValues(asked: Request, response: Response): [string, number][] {
  const kind = { 1: "c", 2: "di", 3: "hr", 4: "ir" }[asked.fc as 1 | 2 | 3 | 4];
  if (!kind) return [];
  const prefix = asked.unit === 1 ? kind : `u${asked.unit}.${kind}`;
  const values = response.registers ?? response.bits?.map(Number) ?? [];
  return values.map((value, index) => [`${prefix}${asked.address + index}`, value]);
}

function findStart(buffer: Uint8Array, start: Uint8Array, from: number): number {
  if (start.length === 0) return from < buffer.length ? from : -1;
  outer: for (let at = from; at + start.length <= buffer.length; at++) {
    for (let index = 0; index < start.length; index++) {
      if (buffer[at + index] !== start[index]) continue outer;
    }
    return at;
  }
  return -1;
}

/** Undoes COBS: each code byte says where the next 0 was. */
export function decodeCobs(bytes: Uint8Array): Uint8Array | string {
  const out: number[] = [];
  let at = 0;
  while (at < bytes.length) {
    const code = bytes[at];
    if (code === 0) return "a 0 inside a COBS frame";
    if (at + code > bytes.length) return "a COBS frame that ends too soon";
    for (let index = 1; index < code; index++) out.push(bytes[at + index]);
    at += code;
    if (code < 0xff && at < bytes.length) out.push(0);
  }
  return Uint8Array.from(out);
}

export function encodeCobs(bytes: Uint8Array): Uint8Array {
  const out: number[] = [0];
  let code = 0;
  let codeAt = 0;
  const close = () => {
    out[codeAt] = code + 1;
    codeAt = out.length;
    out.push(0);
    code = 0;
  };
  for (const byte of bytes) {
    if (byte === 0) {
      close();
      continue;
    }
    out.push(byte);
    code++;
    if (code === 0xfe) close();
  }
  out[codeAt] = code + 1;
  return Uint8Array.from(out);
}

/** Undoes SLIP's escapes: DB DC for C0, DB DD for DB. */
export function decodeSlip(bytes: Uint8Array): Uint8Array | string {
  const out: number[] = [];
  for (let at = 0; at < bytes.length; at++) {
    if (bytes[at] !== 0xdb) {
      out.push(bytes[at]);
      continue;
    }
    const next = bytes[++at];
    if (next === 0xdc) out.push(0xc0);
    else if (next === 0xdd) out.push(0xdb);
    else return "a SLIP escape not followed by DC or DD";
  }
  return Uint8Array.from(out);
}

/** An NMEA sentence such as $GPGGA,...*47, checked, with its numbers; null for other lines. */
export function nmea(line: Uint8Array): Frame | null {
  let text = new TextDecoder().decode(line).trim();
  const begin = text.search(/[$!]/);
  if (begin < 0) return null;
  text = text.slice(begin);
  const bytes = new TextEncoder().encode(text);
  const star = text.lastIndexOf("*");
  const body = star >= 0 ? text.slice(1, star) : text.slice(1);
  let problem: string | null = null;
  if (star < 0) {
    problem = "no checksum";
  } else {
    let sum = 0;
    for (let index = 0; index < body.length; index++) sum ^= body.charCodeAt(index);
    const found = parseInt(text.slice(star + 1, star + 3), 16);
    if (found !== sum) problem = `checksum is ${text.slice(star + 1, star + 3)}, should be ${hex2(sum, 1).slice(2)}`;
  }
  const parts = body.split(",");
  return { bytes, ok: problem === null, problem, values: problem === null ? sentenceValues(parts) : [] };
}

/** Degrees from NMEA's ddmm.mmmm and its N, S, E or W. */
function degrees(value: string, side: string): number | null {
  if (!value) return null;
  const number = Number(value);
  if (!Number.isFinite(number)) return null;
  const whole = Math.floor(number / 100);
  const result = whole + (number - whole * 100) / 60;
  return side === "S" || side === "W" ? -result : result;
}

function sentenceValues(parts: string[]): [string, number][] {
  const type = parts[0].slice(-3);
  const out: [string, number][] = [];
  const add = (name: string, value: number | null) => {
    if (value !== null && Number.isFinite(value)) out.push([name, value]);
  };
  const number = (index: number) => (parts[index] ? Number(parts[index]) : null);
  switch (type) {
    case "GGA":
      add("lat", degrees(parts[2], parts[3]));
      add("lon", degrees(parts[4], parts[5]));
      add("fix", number(6));
      add("sats", number(7));
      add("hdop", number(8));
      add("alt", number(9));
      break;
    case "RMC":
      add("lat", degrees(parts[3], parts[4]));
      add("lon", degrees(parts[5], parts[6]));
      add("knots", number(7));
      add("course", number(8));
      break;
    case "VTG":
      add("course", number(1));
      add("knots", number(5));
      add("kmh", number(7));
      break;
    default:
      parts.slice(1).forEach((part, index) => {
        if (part !== "" && Number.isFinite(Number(part))) out.push([`${type}.${index + 1}`, Number(part)]);
      });
  }
  return out;
}
