// A tab's Modbus master or slave, on the tab's own connection: asking
// devices and keeping what they answer, or answering a master from tables.

import {
  ModbusReader,
  answer,
  describeRequest,
  describeResponse,
  frame,
  parseRequest,
  parseResponse,
  requestPdu,
  type FunctionCode,
  type Message,
  type ModbusMode,
  type Request,
  type Response,
} from "./modbus";

export type RegisterFormat = "unsigned" | "signed" | "hex" | "float" | "float_swapped";

/** One request in the master's list, as workspace.toml keeps it. */
export type ModbusRequestRow = {
  unit: number;
  function: number;
  address: number;
  count: number;
  /** What a write writes, such as "1, 2, 3". */
  values: string;
  /** Asked again and again while polling. */
  poll: boolean;
};

/** How a tab's Modbus view asks or answers, as workspace.toml keeps it. */
export type ModbusSetup = {
  role: "master" | "slave";
  mode: ModbusMode;
  timeout_ms: number;
  poll_ms: number;
  format: RegisterFormat;
  requests: ModbusRequestRow[];
  /** The slave's own unit, and its tables from address `start` on. */
  unit: number;
  start: number;
  holding: number[];
  input: number[];
  coils: boolean[];
  discrete: boolean[];
};

const TABLE = 20;

export const DEFAULT_MODBUS: ModbusSetup = {
  role: "master",
  mode: "rtu",
  timeout_ms: 1000,
  poll_ms: 1000,
  format: "unsigned",
  requests: [{ unit: 1, function: 3, address: 0, count: 10, values: "", poll: true }],
  unit: 1,
  start: 0,
  holding: new Array(TABLE).fill(0),
  input: new Array(TABLE).fill(0),
  coils: new Array(TABLE).fill(false),
  discrete: new Array(TABLE).fill(false),
};

export type LogEntry = {
  at: number;
  /** Sent, came in, or went wrong. */
  kind: "out" | "in" | "problem";
  text: string;
  hex: string;
};

/** What a request in the list got last. */
export type RowResult = { at: number; ms: number; values: number[] | null; text: string | null; problem: string | null };

/** Values read, for the plot, as frames are. */
export type Sample = { n: number; at: number; ok: boolean; values: [string, number][] };

type Outcome = { response?: Response; problem?: string; ms: number };

/** What the tab gives its Modbus. */
export type ModbusHost = {
  send(bytes: Uint8Array): void;
  setup(): ModbusSetup;
  /** Saves the setup: the slave's tables change as a master writes them. */
  changed(): void;
  connected(): boolean;
};

const LOG_MAX = 300;
const SAMPLES_MAX = 50_000;
/** How long a scan waits for each unit. */
const SCAN_TIMEOUT_MS = 150;

const hex = (bytes: Uint8Array) =>
  Array.from(bytes, (byte) => byte.toString(16).toUpperCase().padStart(2, "0")).join(" ");

/** Reads "1, 2, 0x10" into numbers; a problem in plain words when it cannot. */
export function parseValues(text: string): number[] | string {
  const words = text.split(/[\s,;]+/).filter(Boolean);
  const values = words.map((word) => Number(word));
  const bad = words.find((_, index) => !Number.isInteger(values[index]) || values[index] < -32768 || values[index] > 0xffff);
  return bad === undefined ? values.map((value) => value & 0xffff) : `"${bad}" is not a value from -32768 to 65535`;
}

/** A row of the list as a request, or what is wrong with it. */
export function requestOf(row: ModbusRequestRow): Request | string {
  const fc = row.function as FunctionCode;
  if (fc === 5 || fc === 6 || fc === 15 || fc === 16) {
    const values = parseValues(row.values);
    if (typeof values === "string") return values;
    const count = fc === 5 || fc === 6 ? 1 : values.length;
    if (count === 0) return "Write the values to write, such as 1, 2, 3.";
    return { unit: row.unit, fc, address: row.address, count, values: fc === 5 || fc === 15 ? values.map((value) => (value ? 1 : 0)) : values };
  }
  return { unit: row.unit, fc, address: row.address, count: row.count, values: [] };
}

/** Registers shown in a format: a float takes two, so the second shows nothing. */
export function formatRegisters(values: number[], format: RegisterFormat): string[] {
  switch (format) {
    case "signed":
      return values.map((value) => String(value >= 0x8000 ? value - 0x10000 : value));
    case "hex":
      return values.map((value) => `0x${value.toString(16).toUpperCase().padStart(4, "0")}`);
    case "float":
    case "float_swapped": {
      const view = new DataView(new ArrayBuffer(4));
      return values.map((value, index) => {
        if (index % 2 === 1) return "";
        const next = values[index + 1];
        if (next === undefined) return "—";
        const [high, low] = format === "float" ? [value, next] : [next, value];
        view.setUint16(0, high);
        view.setUint16(2, low);
        return String(Number(view.getFloat32(0).toPrecision(7)));
      });
    }
    default:
      return values.map(String);
  }
}

export class ModbusSession {
  log: LogEntry[] = $state([]);
  /** Each request's last result, by its place in the list. */
  results: (RowResult | null)[] = $state([]);
  stats = $state({ sent: 0, answered: 0, timeouts: 0, problems: 0 });
  polling = $state(false);
  /** Whether the slave answers what it is asked. */
  answering = $state(false);
  scan: { unit: number; found: number[]; running: boolean } | null = $state(null);
  /** Values read, for the plot. Plain values, as there can be many: `version` changes with them. */
  readonly samples: Sample[] = [];
  version = $state(0);
  /** The number of the first sample the plot reads: Clear in the Plot view moves it on. */
  plotFrom = $state(0);
  private nextSample = 0;

  private readonly host: ModbusHost;
  private reader: ModbusReader | null = null;
  private readerFor = "";
  private waiting: { unit: number; fc: number; count: number; transaction: number; done: (outcome: Outcome) => void } | null =
    null;
  private transaction = 0;
  private queue: Promise<unknown> = Promise.resolve();
  private pollTimer: ReturnType<typeof setTimeout> | undefined;
  private wake: (() => void) | null = null;

  constructor(host: ModbusHost) {
    this.host = host;
  }

  /** The number the next sample gets, for Clear in the Plot view. */
  get nextNumber(): number {
    return this.nextSample;
  }

  /** Bytes from the device: an answer the master waits for, or requests to the slave. */
  receive(bytes: Uint8Array, at: number): void {
    const setup = this.host.setup();
    const master = setup.role === "master";
    if (master ? !this.waiting : !this.answering) return;
    for (const message of this.readerOf(setup).feed(bytes, at)) {
      if (master) this.onAnswer(message);
      else this.onRequest(message, setup);
    }
  }

  /** A reader for the role and mode; a new one leaves behind what the old one held. */
  private readerOf(setup: ModbusSetup, fresh = false): ModbusReader {
    const key = `${setup.role} ${setup.mode}`;
    if (fresh || !this.reader || key !== this.readerFor) {
      this.reader = new ModbusReader(setup.mode, setup.role === "master" ? "responses" : "requests");
      this.readerFor = key;
    }
    return this.reader;
  }

  private addLog(kind: LogEntry["kind"], text: string, bytes?: Uint8Array): void {
    this.log.push({ at: Date.now(), kind, text, hex: bytes ? hex(bytes) : "" });
    if (this.log.length > LOG_MAX) this.log.splice(0, this.log.length - LOG_MAX);
  }

  clearLog(): void {
    this.log = [];
    this.stats = { sent: 0, answered: 0, timeouts: 0, problems: 0 };
  }

  /** Asks a device, one request at a time, and says what came back. */
  ask(request: Request, timeoutMs?: number): Promise<Outcome> {
    const next = this.queue.then(() => this.askNow(request, timeoutMs));
    this.queue = next.catch(() => {});
    return next;
  }

  private askNow(request: Request, timeoutMs?: number): Promise<Outcome> {
    return new Promise((resolve) => {
      const setup = this.host.setup();
      if (!this.host.connected()) {
        resolve({ problem: "Not connected.", ms: 0 });
        return;
      }
      this.transaction = (this.transaction + 1) & 0xffff;
      const bytes = frame(setup.mode, request.unit, requestPdu(request), this.transaction);
      const reader = this.readerOf(setup, true);
      const sentAt = performance.now();
      const elapsed = () => Math.round(performance.now() - sentAt);
      this.stats.sent++;
      this.addLog("out", describeRequest(request), bytes);
      if (request.unit === 0) {
        // Every unit acts on a broadcast; none answers it.
        this.host.send(bytes);
        resolve({ problem: "sent to every unit, which do not answer", ms: 0 });
        return;
      }
      const wait = timeoutMs ?? setup.timeout_ms;
      const timer = setTimeout(() => {
        this.waiting = null;
        this.stats.timeouts++;
        const problem = reader.problem ? `no answer in ${wait} ms, only ${reader.problem}` : `no answer in ${wait} ms`;
        if (timeoutMs === undefined) this.addLog("problem", problem);
        resolve({ problem, ms: elapsed() });
      }, wait);
      this.waiting = {
        unit: request.unit,
        fc: request.fc,
        count: request.count,
        transaction: this.transaction,
        done: (outcome) => {
          clearTimeout(timer);
          this.waiting = null;
          resolve({ ...outcome, ms: elapsed() });
        },
      };
      this.host.send(bytes);
    });
  }

  private onAnswer(message: Message): void {
    const waiting = this.waiting;
    if (!waiting) return;
    if (!message.ok) {
      this.addLog("problem", message.problem ?? "a broken message", message.raw);
      return;
    }
    const setup = this.host.setup();
    const sameTransaction = setup.mode !== "tcp" || message.transaction === waiting.transaction;
    if (message.unit !== waiting.unit || (message.pdu[0] & 0x7f) !== waiting.fc || !sameTransaction) {
      this.addLog("problem", `an answer to something else, from unit ${message.unit}`, message.raw);
      return;
    }
    const response = parseResponse(message.unit, message.pdu, waiting.count);
    if (typeof response === "string") {
      this.stats.problems++;
      this.addLog("problem", response, message.raw);
      waiting.done({ problem: response, ms: 0 });
      return;
    }
    if (response.exception !== undefined) this.stats.problems++;
    else this.stats.answered++;
    this.addLog(response.exception !== undefined ? "problem" : "in", describeResponse(response), message.raw);
    waiting.done({ response, ms: 0 });
  }

  /** Sends a request of the list, and keeps what came back. */
  async sendRow(index: number): Promise<void> {
    const row = this.host.setup().requests[index];
    if (!row) return;
    const request = requestOf(row);
    if (typeof request === "string") {
      this.results[index] = { at: Date.now(), ms: 0, values: null, text: null, problem: request };
      return;
    }
    const outcome = await this.ask(request);
    const response = outcome.response;
    let values: number[] | null = null;
    let problem = outcome.problem ?? null;
    let text: string | null = null;
    if (response?.exception !== undefined) {
      problem = describeResponse(response);
    } else if (response?.registers || response?.bits) {
      values = response.registers ?? response.bits!.map(Number);
      this.addSample(request, values);
    } else if (response) {
      text = describeResponse(response);
    }
    this.results[index] = { at: Date.now(), ms: outcome.ms, values, text, problem };
  }

  /**
   * Keeps a read's values for the plot, named by their address: hr0, ir5,
   * c3, di1. Registers are read as Show says: signed, or two to a float.
   */
  private addSample(request: Request, values: number[]): void {
    const kind = { 1: "c", 2: "di", 3: "hr", 4: "ir" }[request.fc as 1 | 2 | 3 | 4];
    if (!kind) return;
    const prefix = request.unit === 1 ? kind : `u${request.unit}.${kind}`;
    const format = this.host.setup().format;
    const registers = request.fc === 3 || request.fc === 4;
    const shown = registers && format !== "hex" && format !== "unsigned" ? formatRegisters(values, format).map(Number) : values;
    this.samples.push({
      n: this.nextSample++,
      at: Date.now(),
      ok: true,
      values: shown
        .map((value, index): [string, number] => [`${prefix}${request.address + index}`, value])
        // A float's second register is part of the first.
        .filter((_, index) => !(registers && format.startsWith("float") && index % 2 === 1))
        .filter(([, value]) => Number.isFinite(value)),
    });
    if (this.samples.length > SAMPLES_MAX) this.samples.splice(0, SAMPLES_MAX / 10);
    this.version++;
  }

  /** Asks the requests marked to poll, again and again, `poll_ms` apart. */
  startPolling(): void {
    if (this.polling) return;
    this.polling = true;
    void this.pollLoop();
  }

  stopPolling(): void {
    this.polling = false;
    clearTimeout(this.pollTimer);
    this.wake?.();
  }

  private async pollLoop(): Promise<void> {
    while (this.polling) {
      const rows = this.host.setup().requests;
      const polled = rows.map((row, index) => (row.poll ? index : -1)).filter((index) => index >= 0);
      if (polled.length === 0) {
        this.addLog("problem", "Nothing to poll: tick Poll on a request.");
        this.polling = false;
        return;
      }
      for (const index of polled) {
        if (!this.polling) return;
        await this.sendRow(index);
      }
      await new Promise<void>((done) => {
        this.wake = done;
        this.pollTimer = setTimeout(done, this.host.setup().poll_ms);
      });
      this.wake = null;
    }
  }

  /** Asks every unit from `from` to `to` for one holding register; any answer, an exception too, means a device is there. */
  async startScan(from = 1, to = 247): Promise<void> {
    if (this.scan?.running) return;
    this.scan = { unit: from, found: [], running: true };
    for (let unit = from; unit <= to && this.scan.running; unit++) {
      this.scan.unit = unit;
      const outcome = await this.ask({ unit, fc: 3, address: 0, count: 1, values: [] }, SCAN_TIMEOUT_MS);
      if (outcome.response) this.scan.found.push(unit);
      if (outcome.problem === "Not connected.") break;
    }
    this.scan.running = false;
    this.addLog("problem", this.scan.found.length > 0 ? `Units that answered: ${this.scan.found.join(", ")}` : "No unit answered.");
  }

  stopScan(): void {
    if (this.scan) this.scan.running = false;
  }

  /** Answers a master from the tables, which writes change. */
  private onRequest(message: Message, setup: ModbusSetup): void {
    if (!message.ok) {
      this.addLog("problem", message.problem ?? "a broken message", message.raw);
      return;
    }
    // For another slave on the bus.
    if (message.unit !== setup.unit && message.unit !== 0) return;
    const request = parseRequest(message.unit, message.pdu);
    this.addLog("in", typeof request === "string" ? request : describeRequest(request), message.raw);
    const reply = answer(message.pdu, {
      start: setup.start,
      coils: setup.coils,
      discrete: setup.discrete,
      holding: setup.holding,
      input: setup.input,
    });
    const fc = message.pdu[0];
    if (fc === 5 || fc === 6 || fc === 15 || fc === 16) this.host.changed();
    // A broadcast is acted on, not answered.
    if (message.unit === 0) return;
    const bytes = frame(setup.mode, message.unit, reply, message.transaction);
    this.host.send(bytes);
    const response = parseResponse(message.unit, reply);
    if (typeof response !== "string" && response.exception !== undefined) this.stats.problems++;
    else this.stats.answered++;
    this.addLog("out", typeof response === "string" ? response : describeResponse(response), bytes);
  }

  setAnswering(on: boolean): void {
    this.answering = on;
    this.readerOf(this.host.setup(), true);
  }

  /** Stops polling and scanning, as the tab disconnects. */
  stop(): void {
    this.stopPolling();
    this.stopScan();
  }
}
