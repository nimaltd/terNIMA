// The open tabs, each with its serial session, and saving them to workspace.toml.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { Terminal } from "@xterm/xterm";
import {
  DEFAULT_SERIAL,
  DEFAULT_TERMINAL,
  closeAllSessions,
  getWorkspace,
  setWorkspace,
  summary,
  type ModemLines,
  type PortId,
  type SerialConfig,
  type SessionEvent,
  type SessionSetup,
  type SessionState,
  type TerminalOptions,
} from "./serial";

const encoder = new TextEncoder();
const ENTER = { cr: "\r", lf: "\n", crlf: "\r\n" };

let nextId = 1;

/** One tab: a serial session and its terminal. */
export class Tab {
  readonly id = nextId++;
  name = $state("");
  port: PortId = $state({ name: "" });
  serial: SerialConfig = $state({ ...DEFAULT_SERIAL });
  terminal: TerminalOptions = $state({ ...DEFAULT_TERMINAL });
  /** True while the session runs: the port is open, or ternimal is waiting for it. */
  running = $state(false);
  /** What the session last said, while it runs. */
  status: SessionState | null = $state(null);
  lines: ModemLines | null = $state(null);
  /** Bytes received and sent since the tab opened. */
  rx = $state(0);
  tx = $state(0);
  /** When the port opened, while it is open. */
  connectedAt: number | null = $state(null);

  private term: Terminal | null = null;
  /** What arrived before the terminal was ready. */
  private early: (Uint8Array | string)[] = [];
  /** When the device went away, while waiting for it to come back. */
  private lostAt: number | null = null;

  constructor(setup: SessionSetup) {
    this.name = setup.name;
    this.port = setup.port;
    this.serial = { ...DEFAULT_SERIAL, ...setup.serial };
    this.terminal = { ...DEFAULT_TERMINAL, ...setup.terminal };
  }

  get connected(): boolean {
    return this.status?.state === "connected";
  }

  /** How the tab is saved in workspace.toml. */
  setup(): SessionSetup {
    return $state.snapshot({
      name: this.name,
      connected: this.running,
      port: this.port,
      serial: this.serial,
      terminal: this.terminal,
    });
  }

  /** Called by the tab's terminal once it is on screen. */
  attach(term: Terminal | null): void {
    this.term = term;
    if (term) {
      for (const chunk of this.early) {
        term.write(chunk);
      }
      this.early = [];
    }
  }

  /** Opens the port, or starts waiting for it. */
  start(): void {
    if (this.running) {
      return;
    }
    this.running = true;
    this.status = null;
    this.lostAt = null;

    const data = new Channel<ArrayBuffer | number[]>();
    data.onmessage = (message) => this.receive(new Uint8Array(message));
    const events = new Channel<SessionEvent>();
    events.onmessage = (event) => this.onEvent(event);

    invoke("open_session", {
      id: this.id,
      port: $state.snapshot(this.port),
      config: $state.snapshot(this.serial),
      data,
      events,
    }).catch((error) => this.note(`could not start: ${error}`));
    changed();
  }

  /** Closes the port. */
  stop(): void {
    if (!this.running) {
      return;
    }
    if (this.connected) {
      this.note(`closed at ${clock(Date.now())}`);
    }
    this.running = false;
    this.status = null;
    this.lines = null;
    this.connectedAt = null;
    void invoke("close_session", { id: this.id });
    changed();
  }

  /** Changes how the port is opened; an open port is opened again at once. */
  reconfigure(serial: SerialConfig): void {
    this.serial = serial;
    if (this.running) {
      void invoke("reconfigure_session", { id: this.id, config: $state.snapshot(serial) });
      if (this.connected) {
        this.note(`settings changed to ${summary(serial)}`);
      }
    }
    changed();
  }

  /** Moves the tab to another port. */
  changePort(port: PortId): void {
    const wasRunning = this.running;
    this.stop();
    this.port = port;
    if (wasRunning) {
      this.start();
    }
    changed();
  }

  /** Sends what was typed in the terminal. */
  type(data: string): void {
    if (!this.connected) {
      return;
    }
    const bytes = encoder.encode(data.replace(/\r/g, ENTER[this.terminal.enter]));
    void invoke("write_session", { id: this.id, data: Array.from(bytes) });
    this.tx += bytes.length;
    if (this.terminal.local_echo) {
      this.write(data.replace(/\r/g, "\r\n").replace(/\x7f/g, "\b \b"));
    }
  }

  setDtr(on: boolean): void {
    this.serial.dtr = on;
    void invoke("set_session_lines", { id: this.id, dtr: on });
    changed();
  }

  setRts(on: boolean): void {
    this.serial.rts = on;
    void invoke("set_session_lines", { id: this.id, rts: on });
    changed();
  }

  sendBreak(): void {
    void invoke("break_session", { id: this.id });
  }

  private receive(bytes: Uint8Array): void {
    this.rx += bytes.length;
    this.write(bytes);
  }

  private onEvent(event: SessionEvent): void {
    if (!this.running) {
      return;
    }
    if (event.kind === "lines") {
      this.lines = { cts: event.cts, dsr: event.dsr, dcd: event.dcd, ri: event.ri };
      return;
    }

    const now = Date.now();
    if (event.state === "connected") {
      if (this.lostAt !== null) {
        this.note(`reconnected at ${clock(now)}, after ${duration(now - this.lostAt)}`);
      } else {
        this.note(`connected to ${event.port.name} at ${clock(now)}, ${summary(this.serial)}`);
      }
      this.lostAt = null;
      this.connectedAt = now;
      this.port = event.port;
      this.status = { state: "connected", port: event.port };
      changed();
      return;
    }

    const before = this.status?.state === "waiting" ? this.status.problem : null;
    if (this.connected) {
      this.lostAt = now;
      this.lines = null;
      this.connectedAt = null;
      this.note(`disconnected at ${clock(now)}`);
    } else if (event.problem && event.problem !== before) {
      this.note(this.explain(event.problem));
    } else if (this.status === null && !event.problem) {
      this.note(`waiting for ${this.port.name}`);
    }
    this.status = { state: "waiting", problem: event.problem };
  }

  /** Why the port could not be opened, if that is the case now. */
  get problem(): string | null {
    return this.status?.state === "waiting" && this.status.problem
      ? this.explain(this.status.problem)
      : null;
  }

  /** When the program that has the port is ternimal itself, says which tab has it. */
  private explain(problem: string): string {
    const owner = tabs.find((tab) => tab !== this && tab.connected && tab.port.name === this.port.name);
    return owner ? `${this.port.name} is open in the tab "${owner.name}".` : problem;
  }

  /** Writes a dim marker line, on a line of its own. */
  private note(text: string): void {
    const atLineStart = !this.term || this.term.buffer.active.cursorX === 0;
    this.write(`${atLineStart ? "" : "\r\n"}\x1b[2m---- ${text} ----\x1b[0m\r\n`);
  }

  private write(chunk: Uint8Array | string): void {
    if (this.term) {
      this.term.write(chunk);
    } else {
      this.early.push(chunk);
    }
  }
}

/** The open tabs, and which one is in front. */
export const tabs: Tab[] = $state([]);
export const view = $state({ active: 0 });

let loaded = false;
let saveTimer: ReturnType<typeof setTimeout> | undefined;

export function activeTab(): Tab | undefined {
  return tabs[view.active];
}

/** Opens the tabs saved last time, and connects the ones that were connected. */
export async function loadWorkspace(): Promise<void> {
  // Sessions from before a reload of the page would hold their ports with nobody watching.
  await closeAllSessions();
  const workspace = await getWorkspace();
  for (const setup of workspace.sessions) {
    tabs.push(new Tab(setup));
  }
  view.active = workspace.active;
  loaded = true;
  workspace.sessions.forEach((setup, index) => {
    if (setup.connected) {
      tabs[index].start();
    }
  });
}

export function addTab(setup: SessionSetup): Tab {
  const tab = new Tab(setup);
  tabs.push(tab);
  view.active = tabs.length - 1;
  changed();
  return tab;
}

export function closeTab(tab: Tab): void {
  tab.stop();
  const index = tabs.indexOf(tab);
  if (index < 0) {
    return;
  }
  tabs.splice(index, 1);
  if (view.active >= tabs.length) {
    view.active = Math.max(0, tabs.length - 1);
  }
  changed();
}

export function showTab(index: number): void {
  if (tabs.length > 0) {
    view.active = (index + tabs.length) % tabs.length;
    changed();
  }
}

/** Saves the workspace a moment after the last change. */
export function changed(): void {
  if (!loaded) {
    return;
  }
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    setWorkspace({ active: view.active, sessions: tabs.map((tab) => tab.setup()) }).catch(
      (error) => console.error(error),
    );
  }, 300);
}

/** "14:02:11.204" */
function clock(at: number): string {
  const time = new Date(at);
  const two = (n: number) => String(n).padStart(2, "0");
  const ms = String(time.getMilliseconds()).padStart(3, "0");
  return `${two(time.getHours())}:${two(time.getMinutes())}:${two(time.getSeconds())}.${ms}`;
}

/** "2.7 s", "3 min 5 s" */
export function duration(ms: number): string {
  if (ms < 60_000) {
    return `${(ms / 1000).toFixed(1)} s`;
  }
  const seconds = Math.round(ms / 1000);
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  return hours > 0 ? `${hours} h ${minutes} min` : `${minutes} min ${seconds % 60} s`;
}
