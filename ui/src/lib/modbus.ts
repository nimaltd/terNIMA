// Modbus on a serial line: requests and responses for the common functions,
// framed for RTU (with a CRC) or ASCII (with an LRC); a slave's answers from
// its tables; reading messages out of a stream, as a master or a slave; and
// every frame on the line, each response with its request.
//
// It has no imports, so its tests run in Node as they are.

export type ModbusMode = "rtu" | "ascii";
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

/** A PDU framed to send: RTU with its CRC, or ASCII as a line with its LRC. */
export function frame(mode: ModbusMode, unit: number, pdu: Uint8Array): Uint8Array {
  const body = [unit, ...pdu];
  if (mode === "ascii") {
    const hex = [...body, lrc(body)].map((byte) => byte.toString(16).toUpperCase().padStart(2, "0")).join("");
    return new TextEncoder().encode(`:${hex}\r\n`);
  }
  const crc = crc16(body);
  return Uint8Array.from([...body, crc & 0xff, crc >> 8]);
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

/** A Modbus message read from a stream, whole and checked; or bytes that make none, when a reader keeps them. */
export type Message = {
  unit: number;
  pdu: Uint8Array;
  /** The message as it came, framing included. */
  raw: Uint8Array;
  /** Its bytes: the unit, the PDU and the check. ASCII's hex is read into bytes. */
  bytes: Uint8Array;
  ok: boolean;
  problem: string | null;
  /** When its last bytes came, in ms. */
  at: number;
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

/** How long the RTU response to a request is: a read's byte count and values, or a write's echo. */
export function responseLength(request: Request): number {
  switch (request.fc) {
    case 1:
    case 2:
      return 5 + Math.ceil(request.count / 8);
    case 3:
    case 4:
      return 5 + request.count * 2;
    default:
      return 8;
  }
}

/** The most bytes that make no message kept together, before they are given as one. */
const JUNK_MAX = 256;

/** Splits a stream into Modbus messages. */
export class ModbusReader {
  private buffer = new Uint8Array(0);
  private lastAt = 0;
  /** When the bytes being read came. */
  private now = 0;
  private readonly mode: ModbusMode;
  private readonly side: Side;
  private readonly gapMs: number;
  private readonly keepBad: boolean;
  /** RTU bytes that made no message, while they are kept, and why the first of them made none. */
  private junk: number[] = [];
  private junkProblem = "";
  /** Why the last RTU bytes made no message, such as a wrong CRC; null after a good one. */
  problem: string | null = null;
  /**
   * The length the next RTU message should have, such as that of the
   * response to a request. A message of that length wins over a shorter one
   * whose CRC happens to fit too, and is waited for until a pause.
   */
  expect: number | null = null;
  /** Called with each message as it is found, in order; what it gives back is what `expect` becomes. */
  onMessage: ((message: Message) => number | null) | null = null;

  /**
   * A pause longer than `gapMs` ends whatever RTU message was half way. With
   * `keepBad`, RTU bytes that make no message are given too, as a message
   * that is not ok, rather than left out.
   */
  constructor(mode: ModbusMode, side: Side, gapMs = 50, keepBad = false) {
    this.mode = mode;
    this.side = side;
    this.gapMs = gapMs;
    this.keepBad = keepBad;
  }

  feed(bytes: Uint8Array, at = Date.now()): Message[] {
    const out = this.pause(at);
    this.lastAt = at;
    this.now = at;
    const joined = new Uint8Array(this.buffer.length + bytes.length);
    joined.set(this.buffer);
    joined.set(bytes, this.buffer.length);
    this.buffer = joined;
    if (this.mode === "ascii") this.lines(out);
    else this.rtu(out, false);
    return out;
  }

  /**
   * Ends what was half way, once no byte has come for longer than a pause:
   * a whole message in it is given, at the time its last byte came, and the
   * rest was cut short. RTU only: ASCII lines end themselves.
   */
  pause(at: number): Message[] {
    const out: Message[] = [];
    if (this.mode !== "rtu" || at - this.lastAt <= this.gapMs) return out;
    this.now = this.lastAt;
    if (this.buffer.length > 0) {
      this.rtu(out, true);
      this.skip(this.buffer, "cut short");
      this.buffer = new Uint8Array(0);
    }
    this.giveJunk(out);
    return out;
  }

  /** Gives a message found: in the list, and to `onMessage`. */
  private give(out: Message[], message: Omit<Message, "at">): void {
    const found = { ...message, at: this.now };
    out.push(found);
    if (this.onMessage) this.expect = this.onMessage(found);
  }

  /** Keeps bytes that make no message, to give them as one. */
  private skip(bytes: Uint8Array, why: string): void {
    if (!this.keepBad) return;
    if (this.junk.length === 0) this.junkProblem = why;
    for (const byte of bytes) this.junk.push(byte);
  }

  /** Gives the bytes kept that made no message, as one message that is not ok. */
  private giveJunk(out: Message[]): void {
    if (this.junk.length === 0) return;
    const raw = Uint8Array.from(this.junk);
    this.junk = [];
    this.give(out, { unit: raw[0], pdu: new Uint8Array(0), raw, bytes: raw, ok: false, problem: this.junkProblem });
  }

  /** Reads the RTU messages kept; `ended` after a pause, when no more of them will come. */
  private rtu(out: Message[], ended: boolean): void {
    let at = 0;
    while (this.buffer.length - at >= 4) {
      const rest = this.buffer.subarray(at);
      const lengths = rtuLengths(rest, this.side);
      if (lengths === null) break;
      if (lengths.length === 0) {
        // Not a function this reader knows: look again from the next byte.
        this.skip(rest.subarray(0, 1), "not a function terNIMA knows");
        at++;
        continue;
      }
      const good = this.whole(at, ended);
      if (good > 0) {
        this.giveJunk(out);
        const raw = rest.slice(0, good);
        this.give(out, { unit: raw[0], pdu: raw.slice(1, good - 2), raw, bytes: raw, ok: true, problem: null });
        this.problem = null;
        at += good;
        continue;
      }
      if (lengths.some((length) => length > rest.length)) {
        // It may still come whole; but noise can look like the start of a long
        // message, so a whole one further on wins.
        let next = at + 1;
        while (next + 4 <= this.buffer.length && this.whole(next, true) === 0) next++;
        if (next + 4 > this.buffer.length) break;
        this.problem = "noise before a message";
        this.skip(this.buffer.subarray(at, next), this.problem);
        at = next;
        continue;
      }
      // Long enough for every length it could have, and the CRC fits none:
      // noise, or a message spoilt on the way. Look again from the next byte.
      this.problem = "wrong CRC";
      this.skip(rest.subarray(0, 1), this.problem);
      at++;
    }
    this.buffer = this.buffer.slice(at);
    // Nothing whole in this much is noise.
    if (this.buffer.length > 512) {
      this.skip(this.buffer.subarray(0, this.buffer.length - 256), "noise");
      this.buffer = this.buffer.slice(this.buffer.length - 256);
    }
    if (this.junk.length > JUNK_MAX) this.giveJunk(out);
  }

  /**
   * The length of a whole RTU message with a right CRC at `at`, or 0. The
   * length expected comes first: a response's first bytes can make a shorter
   * request whose CRC fits, one time in 256, so until the pause that ends it,
   * the rest of the expected one is waited for.
   */
  private whole(at: number, ended: boolean): number {
    const rest = this.buffer.subarray(at);
    const lengths = rtuLengths(rest, this.side) ?? [];
    const fits = (length: number) => length <= rest.length && crc16(rest.subarray(0, length)) === 0;
    if (this.expect !== null && lengths.includes(this.expect)) {
      if (fits(this.expect)) return this.expect;
      if (!ended && this.expect > rest.length) return 0;
    }
    return lengths.find(fits) ?? 0;
  }

  private lines(out: Message[]): void {
    let from = 0;
    for (let at = 0; at < this.buffer.length; at++) {
      if (this.buffer[at] !== 0x0a) continue;
      const line = new TextDecoder().decode(this.buffer.subarray(from, at)).trim();
      const raw = this.buffer.slice(from, at + 1);
      from = at + 1;
      const colon = line.indexOf(":");
      if (colon < 0) {
        if (this.keepBad && line !== "") {
          this.give(out, { unit: 0, pdu: new Uint8Array(0), raw, bytes: new Uint8Array(0), ok: false, problem: "not a Modbus ASCII line" });
        }
        continue;
      }
      const hex = line.slice(colon + 1);
      if (hex.length < 6 || hex.length % 2 !== 0 || !/^[0-9a-f]+$/i.test(hex)) {
        this.give(out, { unit: 0, pdu: new Uint8Array(0), raw, bytes: new Uint8Array(0), ok: false, problem: "not hex" });
        continue;
      }
      const bytes = Uint8Array.from(hex.match(/../g)!, (pair) => parseInt(pair, 16));
      const body = bytes.subarray(0, bytes.length - 1);
      const ok = lrc(body) === bytes[bytes.length - 1];
      this.give(out, { unit: body[0], pdu: body.slice(1), raw, bytes, ok, problem: ok ? null : "wrong LRC" });
    }
    this.buffer = this.buffer.slice(from);
  }
}

/** A frame on the line, as the Traffic view lists it. */
export type BusFrame = {
  /** When it came or went, in ms since 1970. */
  at: number;
  /** Whether terNIMA sent it, rather than received it. */
  sent: boolean;
  /** As it was on the line. */
  raw: Uint8Array;
  /** Its bytes: the unit, the PDU and the check; ASCII's hex read into bytes. */
  bytes: Uint8Array;
  /** Whether its CRC or LRC is right. */
  checked: boolean;
  /** Whether it is whole, right, and a request or a response terNIMA can read. */
  ok: boolean;
  /** What is wrong with it, or the exception a slave answered. */
  problem: string | null;
  /** Null for a frame that is not ok. */
  kind: "request" | "response" | null;
  /** The unit and the function, without the exception bit; null for a frame that is not ok. */
  unit: number | null;
  fc: number | null;
  /** What was asked: the request itself, or the one a response answers, if it was on the line. */
  request: Request | null;
  response: Response | null;
  /** How long a response came after its request, in ms. */
  ms: number | null;
  /** A request that no response answered before the next request. */
  unanswered: boolean;
};

/** How long after terNIMA sent a frame the same bytes coming back are its echo, in ms, as RS-485 adapters do. */
const ECHO_MS = 100;

/** Whether a PDU has the shape of a request: a read or a single write, or a multiple write with its byte count. */
function requestShaped(pdu: Uint8Array): boolean {
  const fc = pdu[0];
  if (fc === 15 || fc === 16) return pdu.length >= 6 && pdu.length === 6 + pdu[5];
  return fc >= 1 && fc <= 6 && pdu.length === 5;
}

/** Whether a PDU is the answer to what was asked, rather than a new request. */
export function answers(pdu: Uint8Array, asked: Request): boolean {
  if ((pdu[0] & 0x7f) !== asked.fc) return false;
  if (pdu[0] & 0x80) return pdu.length === 2;
  const fc = asked.fc;
  if (fc <= 4) {
    // A read's answer is its byte count and the bytes; registers come in pairs.
    return pdu.length === 2 + pdu[1] && (fc <= 2 || pdu[1] % 2 === 0);
  }
  // A single write is echoed; a multiple write is answered with its address and count.
  return pdu.length === 5;
}

const same = (a: Uint8Array, b: Uint8Array) => a.length === b.length && a.every((byte, index) => byte === b[index]);

/**
 * Reads every frame on a line, what terNIMA sends and what it receives,
 * whoever is master: each one checked, and each response with the request
 * it answers and how long it took.
 */
export class BusReader {
  private readonly received: ModbusReader;
  private readonly sentReader: ModbusReader;
  /** The last request, while no response has answered it. */
  private asked: BusFrame | null = null;
  /** What terNIMA sent last, to know its echo. */
  private lastSent: Message | null = null;
  /** Whether what terNIMA sends comes back to it, as some RS-485 adapters do: learnt from a read coming back. */
  private echoes = false;
  /** The frames found while bytes are fed. */
  private found: BusFrame[] = [];
  /** Whether terNIMA answers as a slave: what it receives are then requests, never the responses to them. */
  answering = false;

  constructor(mode: ModbusMode, gapMs = 50) {
    this.received = new ModbusReader(mode, "either", gapMs, true);
    this.sentReader = new ModbusReader(mode, "either", gapMs, true);
    this.received.onMessage = (message) => this.take(message, false);
    this.sentReader.onMessage = (message) => this.take(message, true);
  }

  feed(bytes: Uint8Array, at: number, sent: boolean): BusFrame[] {
    this.found = [];
    // What the other side left half way is over if it has been quiet since: it came first.
    (sent ? this.received : this.sentReader).pause(at);
    const reader = sent ? this.sentReader : this.received;
    reader.expect = this.expected(sent);
    reader.feed(bytes, at);
    return this.found;
  }

  /** Ends what either side left half way, once the line has been quiet for a pause. */
  pause(at: number): BusFrame[] {
    this.found = [];
    this.received.pause(at);
    this.sentReader.pause(at);
    return this.found;
  }

  /** The length the next frame on one side should have: the response to a request that waits for one. */
  private expected(sent: boolean): number | null {
    const asked = this.asked;
    if (!asked?.request || asked.request.unit === 0) return null;
    // A response comes from the side that did not ask; or, listening to a bus, with the request.
    const comes = sent ? !asked.sent : asked.sent || !this.answering;
    return comes ? responseLength(asked.request) : null;
  }

  private take(message: Message, sent: boolean): number | null {
    if (sent) {
      this.lastSent = message;
    } else if (this.isEcho(message)) {
      return this.expected(sent);
    }
    this.found.push(this.frameOf(message, sent));
    return this.expected(sent);
  }

  private isEcho(message: Message): boolean {
    const last = this.lastSent;
    if (!message.ok || !last || message.at - last.at > ECHO_MS || !same(message.raw, last.raw)) return false;
    // A single write is answered with the same bytes: those are an echo only from an adapter known to echo.
    const fc = message.pdu[0];
    if (fc === 5 || fc === 6) {
      if (!this.echoes) return false;
    } else {
      this.echoes = true;
    }
    this.lastSent = null;
    return true;
  }

  private frameOf(message: Message, sent: boolean): BusFrame {
    const at = message.at;
    const frame: BusFrame = {
      at,
      sent,
      raw: message.raw,
      bytes: message.bytes,
      checked: message.ok,
      ok: false,
      problem: message.problem,
      kind: null,
      unit: null,
      fc: null,
      request: null,
      response: null,
      ms: null,
      unanswered: false,
    };
    if (!message.ok) return frame;
    const { unit, pdu } = message;
    const fc = pdu[0] & 0x7f;
    const asked = this.asked;
    // A response answers the last request to its unit, from the other side:
    // terNIMA does not answer what it asks itself.
    if (asked?.request && asked.request.unit === unit && !(asked.sent && sent) && answers(pdu, asked.request)) {
      this.asked = null;
      return this.response({ ...frame, unit, fc }, pdu, asked.request, at - asked.at);
    }
    if (requestShaped(pdu)) {
      const request = parseRequest(unit, pdu);
      if (typeof request !== "string") {
        if (asked?.request && asked.request.unit !== 0) asked.unanswered = true;
        const out: BusFrame = { ...frame, ok: true, kind: "request", unit, fc, request };
        this.asked = out;
        return out;
      }
    }
    // A response to a request that was not on the line, such as one before terNIMA listened.
    return this.response({ ...frame, unit, fc }, pdu, null, null);
  }

  private response(frame: BusFrame, pdu: Uint8Array, request: Request | null, ms: number | null): BusFrame {
    const response = parseResponse(frame.unit!, pdu, request?.count);
    if (typeof response === "string") return { ...frame, problem: response };
    const problem = response.exception !== undefined ? describeResponse(response) : null;
    return { ...frame, ok: true, problem, kind: "response", request, response, ms };
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
