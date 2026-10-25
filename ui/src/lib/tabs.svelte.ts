// The open tabs, each with its session (serial, RTT or SWO) and its log, and
// saving them to workspace.toml.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { Terminal } from "@xterm/xterm";
import {
  DEFAULT_RTT,
  DEFAULT_SWO,
  channelText,
  hexAddress,
  hzText,
  type ChannelInfo,
  type RttConfig,
  type SwoConfig,
} from "./probe";
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
  type SessionKind,
  type SessionSetup,
  type Split,
  type SplitDirection,
  type TerminalOptions,
} from "./serial";
import { settings } from "./settings.svelte";

/** Whether a log is being written, and where; or why it could not start. */
type LogStatus = { path: string | null; problem: string | null };

/** How a session's log file is written, for the Rust side. */
type LogSetup = { folder: string; name: string; format: string; timestamps: boolean };

const encoder = new TextEncoder();
const ENTER = { cr: "\r", lf: "\n", crlf: "\r\n" };

let nextId = 1;

/** Where a session is: waiting (and why, if something is wrong), or connected. */
export type Status = { state: "waiting"; problem: string | null } | { state: "connected" };

/** One tab: a session and its terminal. */
export class Tab {
  readonly id = nextId++;
  readonly kind: SessionKind;
  name = $state("");
  port: PortId = $state({ name: "" });
  serial: SerialConfig = $state({ ...DEFAULT_SERIAL });
  rtt: RttConfig = $state({ ...DEFAULT_RTT });
  swo: SwoConfig = $state({ ...DEFAULT_SWO });
  terminal: TerminalOptions = $state({ ...DEFAULT_TERMINAL });
  /** True while the session runs: connected, or ternimal is waiting for the device. */
  running = $state(false);
  /** What the session last said, while it runs. */
  status: Status | null = $state(null);
  /** What the tab is connected to, for people: COM6, or STM32H503RB. */
  target = $state("");
  lines: ModemLines | null = $state(null);
  /** The RTT control block's address and channels, once found. */
  rttAddress: number | null = $state(null);
  channels: { up: ChannelInfo[]; down: ChannelInfo[] } | null = $state(null);
  /** Bytes received and sent since the tab opened. */
  rx = $state(0);
  tx = $state(0);
  /** When the connection started, while connected. */
  connectedAt: number | null = $state(null);
  /** Whether what the tab receives is written to a log file. */
  log = $state(false);
  /** The log file being written, while the session runs with its log on. */
  logPath: string | null = $state(null);

  private term: Terminal | null = null;
  /** What arrived before the terminal was ready. */
  private early: (Uint8Array | string)[] = [];
  /** When the device went away, while waiting for it to come back. */
  private lostAt: number | null = null;
  /** Set between a target reset and finding the control block again. */
  private resetting = false;
  /** Set once the "SWO only receives" hint has been shown. */
  private hinted = false;

  constructor(setup: SessionSetup) {
    this.kind = setup.kind ?? "serial";
    this.name = setup.name;
    this.port = setup.port ?? { name: "" };
    this.serial = { ...DEFAULT_SERIAL, ...setup.serial };
    this.rtt = { ...DEFAULT_RTT, ...setup.rtt };
    this.swo = { ...DEFAULT_SWO, ...setup.swo };
    this.terminal = { ...DEFAULT_TERMINAL, ...setup.terminal };
    this.log = setup.log ?? false;
  }

  get connected(): boolean {
    return this.status?.state === "connected";
  }

  /** Why the session cannot connect, if that is the case now. */
  get problem(): string | null {
    return this.status?.state === "waiting" && this.status.problem
      ? this.explain(this.status.problem)
      : null;
  }

  /** How the tab is saved in workspace.toml. */
  setup(): SessionSetup {
    const common = {
      name: this.name,
      kind: this.kind,
      connected: this.running,
      log: this.log,
      terminal: this.terminal,
    };
    switch (this.kind) {
      case "rtt":
        return $state.snapshot({ ...common, rtt: this.rtt });
      case "swo":
        return $state.snapshot({ ...common, swo: this.swo });
      default:
        return $state.snapshot({ ...common, port: this.port, serial: this.serial });
    }
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

  /** Connects, or starts waiting for the device. */
  start(): void {
    if (this.running) {
      return;
    }
    this.running = true;
    this.status = null;
    this.lostAt = null;
    this.resetting = false;

    const data = new Channel<ArrayBuffer | number[]>();
    data.onmessage = (message) => this.receive(new Uint8Array(message));
    const events = new Channel<SessionEvent>();
    events.onmessage = (event) => this.onEvent(event);

    // The log starts with the session, so not a byte is missed.
    const common = { id: this.id, log: this.logSetup(), data, events };
    const started =
      this.kind === "rtt"
        ? invoke<LogStatus>("open_rtt_session", { ...common, config: $state.snapshot(this.rtt) })
        : this.kind === "swo"
          ? invoke<LogStatus>("open_swo_session", { ...common, config: $state.snapshot(this.swo) })
          : invoke<LogStatus>("open_session", {
              ...common,
              port: $state.snapshot(this.port),
              config: $state.snapshot(this.serial),
            });
    started
      .then((status) => this.onLogStatus(status))
      .catch((error) => this.note(`could not start: ${error}`));
    changed();
  }

  /** Turns the tab's log on or off. A running session starts a new file at once. */
  setLog(on: boolean): void {
    this.log = on;
    changed();
    if (!this.running) {
      return;
    }
    if (!on && this.logPath) {
      this.note(`log closed: ${this.logPath}`);
    }
    this.logPath = null;
    invoke<LogStatus>("set_log", { id: this.id, log: this.logSetup() })
      .then((status) => this.onLogStatus(status))
      .catch((error) => this.note(`could not start the log: ${error}`));
  }

  /** How the log is written, from the settings, or null while the log is off. */
  private logSetup(): LogSetup | null {
    if (!this.log) {
      return null;
    }
    const { folder, format, timestamps } = settings.log;
    return { folder, name: this.name, format, timestamps };
  }

  private onLogStatus(status: LogStatus): void {
    this.logPath = status.path;
    if (status.path) {
      this.note(`logging to ${status.path}`);
    } else if (status.problem) {
      this.log = false;
      this.note(status.problem);
      changed();
    }
  }

  /** Disconnects. */
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
    this.logPath = null;
    void invoke("close_session", { id: this.id });
    changed();
  }

  /** Changes how a serial port is opened; an open port is opened again at once. */
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

  /** Changes an RTT session's settings; a running session attaches again at once. */
  reconfigureRtt(rtt: RttConfig): void {
    const channelBefore = this.rtt.up_channel;
    this.rtt = rtt;
    if (this.running) {
      void invoke("reconfigure_rtt_session", { id: this.id, config: $state.snapshot(rtt) });
      if (this.connected && rtt.up_channel !== channelBefore) {
        const channel = this.channels?.up.find((up) => up.number === rtt.up_channel);
        this.note(`now showing channel ${channel ? channelText(channel) : rtt.up_channel}`);
      }
    }
    changed();
  }

  /** Changes an SWO session's settings; a running session attaches again at once. */
  reconfigureSwo(swo: SwoConfig): void {
    this.swo = swo;
    if (this.running) {
      void invoke("reconfigure_swo_session", { id: this.id, config: $state.snapshot(swo) });
    }
    changed();
  }

  /** Moves the tab to another serial port. */
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
    if (this.kind === "swo") {
      if (!this.hinted) {
        this.hinted = true;
        this.note("SWO only comes from the chip: what you type goes nowhere. Use RTT to type to it.");
      }
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

  /** Resets the chip of an RTT or SWO session. */
  resetTarget(): void {
    if (!this.connected) {
      return;
    }
    // RTT looks for its control block again after a reset; SWO just goes on.
    this.resetting = this.kind === "rtt";
    this.note(`target reset at ${clock(Date.now())}`);
    void invoke("reset_target", { id: this.id });
  }

  private receive(bytes: Uint8Array): void {
    this.rx += bytes.length;
    this.write(bytes);
  }

  private onEvent(event: SessionEvent): void {
    if (!this.running) {
      return;
    }
    switch (event.kind) {
      case "lines":
        this.lines = { cts: event.cts, dsr: event.dsr, dcd: event.dcd, ri: event.ri };
        return;
      case "channels":
        this.channels = { up: event.up, down: event.down };
        return;
      case "state":
        if (event.state === "connected") {
          this.port = event.port;
          this.onConnected(event.port.name, `${event.port.name}, ${summary(this.serial)}`);
          changed();
        } else {
          this.onWaiting(event.problem);
        }
        return;
      case "rtt_state":
        if (event.state === "connected") {
          this.rttAddress = event.address;
          this.onConnected(event.chip, `${event.chip}, RTT at ${hexAddress(event.address)}`);
        } else {
          this.onWaiting(event.problem);
        }
        return;
      case "swo_state":
        if (event.state === "connected") {
          this.onConnected(event.chip, `${event.chip}, SWO at ${hzText(event.baud)}`);
        } else {
          this.onWaiting(event.problem);
        }
        return;
      case "log_stopped":
        this.log = false;
        this.logPath = null;
        this.note(event.problem);
        changed();
        return;
      case "overflow":
        this.note(`SWO overflow: the chip dropped trace data ${event.count} times. A higher baud rate helps.`);
        return;
    }
  }

  private onConnected(target: string, description: string): void {
    const now = Date.now();
    if (this.resetting) {
      this.note(this.kind === "rtt" ? "found RTT again after the reset" : "SWO on again after the reset");
    } else if (this.lostAt !== null) {
      this.note(`reconnected at ${clock(now)}, after ${duration(now - this.lostAt)}`);
    } else {
      this.note(`connected to ${description} at ${clock(now)}`);
    }
    this.resetting = false;
    this.lostAt = null;
    this.connectedAt = now;
    this.target = target;
    this.status = { state: "connected" };
  }

  private onWaiting(problem: string | null): void {
    const now = Date.now();
    const before = this.status?.state === "waiting" ? this.status.problem : null;
    if (this.connected) {
      this.lostAt = now;
      this.lines = null;
      this.connectedAt = null;
      if (!this.resetting) {
        this.note(`disconnected at ${clock(now)}`);
      }
    } else if (problem && problem !== before) {
      this.note(this.explain(problem));
    } else if (this.status === null && !problem) {
      this.note(this.kind === "serial" ? `waiting for ${this.port.name}` : "waiting for the probe");
    }
    this.status = { state: "waiting", problem };
  }

  /** When the program that has the port is ternimal itself, says which tab has it. */
  private explain(problem: string): string {
    if (this.kind !== "serial") {
      return problem;
    }
    const owner = tabs.find(
      (tab) => tab !== this && tab.kind === "serial" && tab.connected && tab.port.name === this.port.name,
    );
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

/**
 * What is on screen. `active` is the tab with the keyboard. With a split
 * view, two tabs are on screen, `first` (left or top) and `second`, and
 * `active` is one of them.
 */
export const view: { active: number; split: Split | null } = $state({ active: 0, split: null });

let loaded = false;
let saveTimer: ReturnType<typeof setTimeout> | undefined;

export function activeTab(): Tab | undefined {
  return tabs[view.active];
}

/**
 * Opens the tabs saved last time, and connects the ones that were connected,
 * as far as the Startup settings say. The settings must be loaded first.
 */
export async function loadWorkspace(): Promise<void> {
  // Sessions from before a reload of the page would hold their ports with nobody watching.
  await closeAllSessions();
  const workspace = await getWorkspace();
  const { restore_tabs, reconnect } = settings.startup;
  if (restore_tabs) {
    for (const setup of workspace.sessions) {
      tabs.push(new Tab(setup));
    }
    view.active = workspace.active;
    view.split = workspace.split ?? null;
  }
  loaded = true;
  if (restore_tabs && reconnect) {
    workspace.sessions.forEach((setup, index) => {
      if (setup.connected) {
        tabs[index].start();
      }
    });
  }
}

export function addTab(setup: SessionSetup): Tab {
  const tab = new Tab(setup);
  tabs.push(tab);
  showTab(tabs.length - 1);
  return tab;
}

export function closeTab(tab: Tab): void {
  tab.stop();
  const index = tabs.indexOf(tab);
  if (index < 0) {
    return;
  }
  // A split view ends when one of its tabs closes; the other stays in front.
  if (view.split && (index === view.split.first || index === view.split.second)) {
    view.active = index === view.split.first ? view.split.second : view.split.first;
    view.split = null;
  }
  tabs.splice(index, 1);
  const after = (position: number) => (position > index ? position - 1 : position);
  view.active = Math.min(after(view.active), Math.max(0, tabs.length - 1));
  if (view.split) {
    view.split.first = after(view.split.first);
    view.split.second = after(view.split.second);
  }
  changed();
}

/**
 * Brings a tab in front. With a split view, a tab on screen gets the
 * keyboard, and another one takes the place of the pane that has it.
 */
export function showTab(index: number): void {
  if (tabs.length === 0) {
    return;
  }
  const wanted = (index + tabs.length) % tabs.length;
  const split = view.split;
  if (split && wanted !== split.first && wanted !== split.second) {
    if (view.active === split.first) {
      split.first = wanted;
    } else {
      split.second = wanted;
    }
  }
  view.active = wanted;
  changed();
}

/**
 * Shows a second tab next to the one in front: the next tab along. The panes
 * keep the order of the tab bar, and the tab in front keeps the keyboard.
 */
export function splitView(direction: SplitDirection): void {
  if (tabs.length < 2) {
    return;
  }
  if (view.split) {
    view.split.direction = direction;
  } else {
    const other = (view.active + 1) % tabs.length;
    view.split = {
      first: Math.min(view.active, other),
      second: Math.max(view.active, other),
      direction,
      ratio: 50,
    };
  }
  changed();
}

/** Back to one tab on screen: the one with the keyboard. */
export function unsplit(): void {
  view.split = null;
  changed();
}

/** Gives the keyboard to the other pane of a split view. */
export function otherPane(): void {
  const split = view.split;
  if (split) {
    view.active = view.active === split.first ? split.second : split.first;
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
    setWorkspace({
      active: view.active,
      split: $state.snapshot(view.split),
      sessions: tabs.map((tab) => tab.setup()),
    }).catch((error) => console.error(error));
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
