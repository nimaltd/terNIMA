// Modbus: requests and responses for the common functions, framed for RTU
// (with a CRC), ASCII (with an LRC) or TCP (with an MBAP header); a slave's
// answers from its tables; and reading messages out of a stream, as a
// master, a slave, or something listening on the bus.
//
// It has no imports, so its tests run in Node as they are.

export type ModbusMode = "rtu" | "ascii" | "tcp";
export type FunctionCode = 1 | 2 | 3 | 4 | 5 | 6 | 15 | 16;

export const FUNCTION_NAMES: Record<FunctionCode, string> = {
  1: "Read coils",
  2: "Read discrete inputs",
  3: "Read holding registers",
  4: "Read input registers",
  5: "Write single coil",
  6: "Write single register",
  15: "Write multiple coils",
  16: "Write multiple registers",
};

export const EXCEPTION_NAMES: Record<number, string> = {
  1: "illegal function",
  2: "illegal data address",
  3: "illegal data value",
  4: "server device failure",
  5: "acknowledge",
  6: "server device busy",
  8: "memory parity error",
  10: "gateway path unavailable",
  11: "gateway target device failed to respond",
};

/** The most coils and registers one request may read, as the standard says. */
export const READ_BITS_MAX = 2000;
export const READ_REGISTERS_MAX = 125;
export const WRITE_BITS_MAX = 1968;
export const WRITE_REGISTERS_MAX = 123;

/** What a master asks. `values` holds what a write writes: 0 or 1 for coils. */
export type Request = { unit: number; fc: FunctionCode; address: number; count: number; values: number[] };

/** What came back: an exception, bits, registers, or what a write wrote. */
export type Response = {
  unit: number;
  fc: number;
  exception?: number;
  bits?: boolean[];
  registers?: number[];
  address?: number;
  value?: number;
  count?: number;
};

const isRead = (fc: number) => fc >= 1 && fc <= 4;
const isBits = (fc: number) => fc === 1 || fc === 2 || fc === 5 || fc === 15;

export function crc16(bytes: ArrayLike<number>): number {
  let crc = 0xffff;
  for (let index = 0; index < bytes.length; index++) {
    crc ^= bytes[index];
    for (let bit = 0; bit < 8; bit++) crc = crc & 1 ? (crc >>> 1) ^ 0xa001 : crc >>> 1;
  }
  return crc;
}

/** ASCII's check: the two's complement of the bytes' sum. */
export function lrc(bytes: ArrayLike<number>): number {
  let sum = 0;
  for (let index = 0; index < bytes.length; index++) sum = (sum + bytes[index]) & 0xff;
  return -sum & 0xff;
}

const word = (value: number) => [(value >> 8) & 0xff, value & 0xff];
const readWord = (bytes: Uint8Array, at: number) => (bytes[at] << 8) | bytes[at + 1];

/** Packs bits into bytes, the first in the low bit of the first byte. */
function packBits(bits: ArrayLike<number | boolean>): number[] {
  const out = new Array<number>(Math.ceil(bits.length / 8)).fill(0);
  for (let index = 0; index < bits.length; index++) if (bits[index]) out[index >> 3] |= 1 << (index & 7);
  return out;
}

function unpackBits(bytes: Uint8Array, from: number, count: number): boolean[] {
  return Array.from({ length: count }, (_, index) => (bytes[from + (index >> 3)] & (1 << (index & 7))) !== 0);
}

/** A request's PDU: the function code and what follows it. */
export function requestPdu(request: Request): Uint8Array {
  const { fc, address, count, values } = request;
  switch (fc) {
    case 5:
      return Uint8Array.from([fc, ...word(address), ...word(values[0] ? 0xff00 : 0)]);
    case 6:
      return Uint8Array.from([fc, ...word(address), ...word(values[0] & 0xffff)]);
    case 15: {
      const bits = packBits(values.slice(0, count));
      return Uint8Array.from([fc, ...word(address), ...word(count), bits.length, ...bits]);
    }
    case 16: {
      const registers = values.slice(0, count).flatMap((value) => word(value & 0xffff));
      return Uint8Array.from([fc, ...word(address), ...word(count), registers.length, ...registers]);
    }
    default:
      return Uint8Array.from([fc, ...word(address), ...word(count)]);
  }
}

/** Reads a request's PDU, as a slave gets it. */
export function parseRequest(unit: number, pdu: Uint8Array): Request | string {
  const fc = pdu[0];
  if (!(fc in FUNCTION_NAMES)) return `function ${fc} is not one terNIMA knows`;
  if (pdu.length < 5) return "too short";
  const address = readWord(pdu, 1);
  const second = readWord(pdu, 3);
  switch (fc) {
    case 5:
      return { unit, fc, address, count: 1, values: [second === 0xff00 ? 1 : 0] };
    case 6:
      return { unit, fc, address, count: 1, values: [second] };
    case 15:
      if (pdu.length < 6 + Math.ceil(second / 8)) return "too short for its coils";
      return { unit, fc, address, count: second, values: unpackBits(pdu, 6, second).map(Number) };
    case 16:
      if (pdu.length < 6 + second * 2) return "too short for its registers";
      return {
        unit,
        fc,
        address,
        count: second,
        values: Array.from({ length: second }, (_, index) => readWord(pdu, 6 + index * 2)),
      };
    default:
      return { unit, fc: fc as FunctionCode, address, count: second, values: [] };
  }
}

/** Reads a response's PDU, as a master gets it. Reads keep as many bits as `count` asks. */
export function parseResponse(unit: number, pdu: Uint8Array, count = Infinity): Response | string {
  const fc = pdu[0];
  if (fc & 0x80) {
    if (pdu.length < 2) return "too short";
    return { unit, fc: fc & 0x7f, exception: pdu[1] };
  }
  if (isRead(fc)) {
    const size = pdu[1];
    if (pdu.length < 2 + size) return "too short for its byte count";
    if (isBits(fc)) return { unit, fc, bits: unpackBits(pdu, 2, Math.min(size * 8, count)) };
    if (size % 2 !== 0) return "an odd byte count for registers";
    return { unit, fc, registers: Array.from({ length: size / 2 }, (_, index) => readWord(pdu, 2 + index * 2)) };
  }
  if (pdu.length < 5) return "too short";
  const address = readWord(pdu, 1);
  const second = readWord(pdu, 3);
  if (fc === 5) return { unit, fc, address, value: second === 0xff00 ? 1 : 0 };
  if (fc === 6) return { unit, fc, address, value: second };
  return { unit, fc, address, count: second };
}

/** A PDU framed to send: RTU with its CRC, ASCII as a line with its LRC, or TCP with its header. */
export function frame(mode: ModbusMode, unit: number, pdu: Uint8Array, transaction = 0): Uint8Array {
  const body = [unit, ...pdu];
  switch (mode) {
    case "ascii": {
      const hex = [...body, lrc(body)].map((byte) => byte.toString(16).toUpperCase().padStart(2, "0")).join("");
      return new TextEncoder().encode(`:${hex}\r\n`);
    }
    case "tcp":
      return Uint8Array.from([...word(transaction), 0, 0, ...word(body.length), ...body]);
    default: {
      const crc = crc16(body);
      return Uint8Array.from([...body, crc & 0xff, crc >> 8]);
    }
  }
}

/** The tables a slave answers from: values from address `start` on. */
export type Tables = {
  start: number;
  coils: boolean[];
  discrete: boolean[];
  holding: number[];
  input: number[];
};

const exception = (fc: number, code: number) => Uint8Array.of(fc | 0x80, code);

/** A slave's answer to a request: values from its tables, which writes change, or an exception. */
export function answer(pdu: Uint8Array, tables: Tables): Uint8Array {
  const request = parseRequest(0, pdu);
  const fc = pdu[0] ?? 0;
  if (typeof request === "string") return exception(fc, fc in FUNCTION_NAMES ? 3 : 1);
  const { address, count, values } = request;
  const table =
    fc === 1 || fc === 5 || fc === 15
      ? tables.coils
      : fc === 2
        ? tables.discrete
        : fc === 4
          ? tables.input
          : tables.holding;
  const limit = isRead(fc) ? (isBits(fc) ? READ_BITS_MAX : READ_REGISTERS_MAX) : isBits(fc) ? WRITE_BITS_MAX : WRITE_REGISTERS_MAX;
  if (count < 1 || count > limit) return exception(fc, 3);
  const from = address - tables.start;
  if (from < 0 || from + count > table.length) return exception(fc, 2);
  switch (fc) {
    case 1:
    case 2: {
      const bits = packBits((table as boolean[]).slice(from, from + count));
      return Uint8Array.from([fc, bits.length, ...bits]);
    }
    case 3:
    case 4: {
      const words = (table as number[]).slice(from, from + count).flatMap(word);
      return Uint8Array.from([fc, words.length, ...words]);
    }
    case 5:
      if (pdu.length < 5 || (readWord(pdu, 3) !== 0xff00 && readWord(pdu, 3) !== 0)) return exception(fc, 3);
      tables.coils[from] = values[0] === 1;
      return pdu.slice(0, 5);
    case 6:
      tables.holding[from] = values[0];
      return pdu.slice(0, 5);
    case 15:
      values.forEach((value, index) => (tables.coils[from + index] = value === 1));
      return pdu.slice(0, 5);
    default:
      values.forEach((value, index) => (tables.holding[from + index] = value));
      return pdu.slice(0, 5);
  }
}

/** A Modbus message read from a stream, whole and checked. */
export type Message = {
  unit: number;
  pdu: Uint8Array;
  /** The message as it came, framing included. */
  raw: Uint8Array;
  /** TCP's transaction number. */
  transaction: number;
  ok: boolean;
  problem: string | null;
};

/** Which messages a reader looks for: a slave reads requests, a master responses, a listener both. */
export type Side = "requests" | "responses" | "either";

/** How long RTU messages can be, from their first bytes; null until enough came. */
function rtuLengths(bytes: Uint8Array, side: Side): number[] | null {
  if (bytes.length < 3) return null;
  const fc = bytes[1];
  const lengths: number[] = [];
  if (side !== "responses") {
    if ((fc >= 1 && fc <= 6) || fc === 15 || fc === 16) {
      if (fc === 15 || fc === 16) {
        if (bytes.length < 7) return null;
        lengths.push(9 + bytes[6]);
      } else {
        lengths.push(8);
      }
    }
  }
  if (side !== "requests") {
    if (fc & 0x80) lengths.push(5);
    else if (isRead(fc)) lengths.push(5 + bytes[2]);
    else if (fc === 5 || fc === 6 || fc === 15 || fc === 16) lengths.push(8);
  }
  return lengths;
}

/** Splits a stream into Modbus messages. */
export class ModbusReader {
  private buffer = new Uint8Array(0);
  private lastAt = 0;
  private readonly mode: ModbusMode;
  private readonly side: Side;
  private readonly gapMs: number;
  /** Why the last RTU bytes made no message, such as a wrong CRC; null after a good one. */
  problem: string | null = null;

  /** A pause longer than `gapMs` ends whatever RTU message was half way. */
  constructor(mode: ModbusMode, side: Side, gapMs = 50) {
    this.mode = mode;
    this.side = side;
    this.gapMs = gapMs;
  }

  feed(bytes: Uint8Array, at = Date.now()): Message[] {
    if (this.mode === "rtu" && this.buffer.length > 0 && at - this.lastAt > this.gapMs) {
      this.buffer = new Uint8Array(0);
    }
    this.lastAt = at;
    const joined = new Uint8Array(this.buffer.length + bytes.length);
    joined.set(this.buffer);
    joined.set(bytes, this.buffer.length);
    this.buffer = joined;
    switch (this.mode) {
      case "ascii":
        return this.lines();
      case "tcp":
        return this.mbap();
      default:
        return this.rtu();
    }
  }

  private rtu(): Message[] {
    const out: Message[] = [];
    let at = 0;
    while (this.buffer.length - at >= 4) {
      const rest = this.buffer.subarray(at);
      const lengths = rtuLengths(rest, this.side);
      if (lengths === null) break;
      if (lengths.length === 0) {
        // Not a function this reader knows: look again from the next byte.
        at++;
        continue;
      }
      const good = this.whole(at);
      if (good > 0) {
        const raw = rest.slice(0, good);
        out.push({ unit: raw[0], pdu: raw.slice(1, good - 2), raw, transaction: 0, ok: true, problem: null });
        this.problem = null;
        at += good;
        continue;
      }
      if (lengths.some((length) => length > rest.length)) {
        // It may still come whole; but noise can look like the start of a long
        // message, so a whole one further on wins.
        let next = at + 1;
        while (next + 4 <= this.buffer.length && this.whole(next) === 0) next++;
        if (next + 4 > this.buffer.length) break;
        this.problem = "noise before a message";
        at = next;
        continue;
      }
      // Long enough for every length it could have, and the CRC fits none:
      // noise, or a message spoilt on the way. Look again from the next byte.
      this.problem = "wrong CRC";
      at++;
    }
    this.buffer = this.buffer.slice(at);
    // Nothing whole in this much is noise.
    if (this.buffer.length > 512) this.buffer = this.buffer.slice(this.buffer.length - 256);
    return out;
  }

  /** The length of a whole RTU message with a right CRC at `at`, or 0. */
  private whole(at: number): number {
    const rest = this.buffer.subarray(at);
    const lengths = rtuLengths(rest, this.side) ?? [];
    return lengths.find((length) => length <= rest.length && crc16(rest.subarray(0, length)) === 0) ?? 0;
  }

  private lines(): Message[] {
    const out: Message[] = [];
    let from = 0;
    for (let at = 0; at < this.buffer.length; at++) {
      if (this.buffer[at] !== 0x0a) continue;
      const line = new TextDecoder().decode(this.buffer.subarray(from, at)).trim();
      const raw = this.buffer.slice(from, at + 1);
      from = at + 1;
      const colon = line.indexOf(":");
      if (colon < 0) continue;
      const hex = line.slice(colon + 1);
      if (hex.length < 6 || hex.length % 2 !== 0 || !/^[0-9a-f]+$/i.test(hex)) {
        out.push({ unit: 0, pdu: new Uint8Array(0), raw, transaction: 0, ok: false, problem: "not hex" });
        continue;
      }
      const bytes = Uint8Array.from(hex.match(/../g)!, (pair) => parseInt(pair, 16));
      const body = bytes.subarray(0, bytes.length - 1);
      const ok = lrc(body) === bytes[bytes.length - 1];
      out.push({
        unit: body[0],
        pdu: body.slice(1),
        raw,
        transaction: 0,
        ok,
        problem: ok ? null : "wrong LRC",
      });
    }
    this.buffer = this.buffer.slice(from);
    return out;
  }

  private mbap(): Message[] {
    const out: Message[] = [];
    let at = 0;
    while (this.buffer.length - at >= 7) {
      const length = readWord(this.buffer, at + 4);
      if (length < 2 || length > 254) {
        at++;
        continue;
      }
      if (this.buffer.length - at < 6 + length) break;
      const raw = this.buffer.slice(at, at + 6 + length);
      out.push({ unit: raw[6], pdu: raw.slice(7), raw, transaction: readWord(raw, 0), ok: true, problem: null });
      at += 6 + length;
    }
    this.buffer = this.buffer.slice(at);
    return out;
  }
}

/** What a request asks, in words: "read holding registers 0 to 9 of unit 1". */
export function describeRequest(request: Request): string {
  const { unit, fc, address, count, values } = request;
  const range = count > 1 ? `${address} to ${address + count - 1}` : `${address}`;
  const name = FUNCTION_NAMES[fc].toLowerCase();
  switch (fc) {
    case 5:
    case 6:
      return `${name} ${address} to ${values[0]}, unit ${unit}`;
    case 15:
    case 16:
      return `${name} ${range} to ${values.join(", ")}, unit ${unit}`;
    default:
      return `${name} ${range}, unit ${unit}`;
  }
}

/** What a response says, in words. */
export function describeResponse(response: Response): string {
  if (response.exception !== undefined) {
    return `exception ${response.exception}: ${EXCEPTION_NAMES[response.exception] ?? "unknown"}`;
  }
  if (response.bits) return response.bits.map(Number).join(" ");
  if (response.registers) return response.registers.join(" ");
  if (response.value !== undefined) return `wrote ${response.value} at ${response.address}`;
  return `wrote ${response.count} from ${response.address}`;
}
