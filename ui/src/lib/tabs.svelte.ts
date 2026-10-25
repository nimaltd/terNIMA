// The open tabs, each with its session (serial, RTT, SWO or TCP) and its log,
// and saving them to workspace.toml.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { Terminal } from "@xterm/xterm";
import { HexDump } from "./hexdump";
import { DEFAULT_TCP, address, protocolText, type TcpConfig } from "./net";
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
  DEFAULT_HEX_VIEW,
  DEFAULT_LINES_VIEW,
  DEFAULT_PLOT_VIEW,
  DEFAULT_REPLAY,
  DEFAULT_SERIAL,
  DEFAULT_TERMINAL,
  closeAllSessions,
  getWorkspace,
  setWorkspace,
  summary,
  type HexView,
  type LinesView,
  type PlotView,
  type PlayState,
  type ReplaySetup,
  type LogLine,
  type ModemLines,
  type PortId,
  type SerialConfig,
  type SentLine,
  type SessionEvent,
  type SessionKind,
  type SessionSetup,
  type Split,
  type SplitDirection,
  type TerminalOptions,
  type View,
} from "./serial";
import { settings, type Macro } from "./settings.svelte";
import { bytes as byteText } from "./state";

/** Whether a log is being written, and where; or why it could not start. */
type LogStatus = { path: string | null; problem: string | null };

/** How a session's log file is written, for the Rust side. */
type LogSetup = { folder: string; name: string; format: string; timestamps: boolean };

/** What the Rust side learns of a recording it opens. */
type ReplayInfo = { name: string; length: number };

/** What a recording being played is asked to do. */
type ReplayCommand =
  | { kind: "play" }
  | { kind: "pause" }
  | { kind: "speed"; speed: number }
  | { kind: "seek"; position: number };

/** How sending a file goes, from the Rust side. */
type FileProgress = { sent: number; total: number; done: boolean; cancelled: boolean; problem: string | null };

const encoder = new TextEncoder();
const ENTER = { cr: "\r", lf: "\n", crlf: "\r\n" };

/** The most sent lines a tab keeps, the same as HISTORY_MAX in workspace.rs. */
const HISTORY_MAX = 100;
/** About how many recent bytes a tab keeps, to fill its hex view when it opens. */
const RECENT_MAX = 256 * 1024;
/** How fast a file goes into RTT: the firmware empties its small buffer when it gets to it. */
const RTT_FILE_RATE = 1000;
/** The most lines a tab keeps for its Lines view; a tenth of them go when there are more. */
const LINES_MAX = 100_000;
/** The most time, in ms, that lines read at once are spread over. */
const SPREAD_MS = 200;

/** A line as the Lines view keeps it: numbered from the tab's first, with when it came. */
export type StoredLine = LogLine & { n: number; at: number };

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
  tcp: TcpConfig = $state({ ...DEFAULT_TCP });
  replay: ReplaySetup = $state({ ...DEFAULT_REPLAY });
  terminal: TerminalOptions = $state({ ...DEFAULT_TERMINAL });
  /** True while the session runs: connected, or ternima is waiting for the device. */
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
  /** The recording being written, while the tab records. */
  recordPath: string | null = $state(null);
  /** Where the recording a replay tab plays is, once it is open. */
  playback: PlayState | null = $state(null);
  /** Set to play a recording as soon as it is open. */
  private playWhenOpen = false;
  /** What the tab shows: the terminal, every byte in hex, every line, or a chart of the numbers. */
  view: View = $state("terminal");
  /** How the hex view breaks its lines. */
  hexView: HexView = $state({ ...DEFAULT_HEX_VIEW });
  /** What the Lines view leaves out. */
  linesView: LinesView = $state({ ...DEFAULT_LINES_VIEW });
  /** How the Plot view reads and shows numbers. */
  plotView: PlotView = $state({ ...DEFAULT_PLOT_VIEW });
  /** The number of the first line the plot reads: Clear in the Plot view moves it on. */
  plotFrom = $state(0);
  /**
   * The lines received lately, oldest first, for the Lines view, with how
   * many of each level and module there are. Plain values rather than
   * $state, as there can be many: `linesVersion` changes when they do.
   */
  readonly textLines: StoredLine[] = [];
  readonly levelCounts = new Map<string, number>();
  readonly moduleCounts = new Map<string, number>();
  /** The numbers of the lines marked with a bookmark. */
  readonly bookmarks = new Set<number>();
  linesVersion = $state(0);
  private nextLine = 0;
  /** When the last lines were read, in ms since 1970. */
  private linesAt = 0;
  /** The lines sent from the send box, oldest first. */
  history: SentLine[] = $state([]);
  /** What is in the send box, kept while the tab is out of sight. */
  draft = $state({ text: "", hex: false, enter: true });
  /** The file being sent, while one is. */
  sending: { name: string; sent: number; total: number } | null = $state(null);

  private term: Terminal | null = null;
  private hexTerm: Terminal | null = null;
  private hexDump = new HexDump(DEFAULT_HEX_VIEW, 0);
  /** When the last bytes came, and whether anything was sent since, for the pause's new line. */
  private lastReceived = 0;
  private sentSince = false;
  private hexTimer: ReturnType<typeof setTimeout> | undefined;
  /** What was received and sent lately, oldest first, to fill the hex view when it opens. */
  private recent: { tx: boolean; at: number; bytes: Uint8Array }[] = [];
  private recentBytes = 0;
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
    this.tcp = { ...DEFAULT_TCP, ...setup.tcp };
    this.replay = { ...DEFAULT_REPLAY, ...setup.replay };
    this.terminal = { ...DEFAULT_TERMINAL, ...setup.terminal };
    this.log = setup.log ?? false;
    this.view = setup.view ?? "terminal";
    this.hexView = { ...DEFAULT_HEX_VIEW, ...setup.hex_view };
    this.linesView = { ...DEFAULT_LINES_VIEW, ...setup.lines_view };
    this.plotView = {
      ...DEFAULT_PLOT_VIEW,
      ...setup.plot_view,
      trigger: { ...DEFAULT_PLOT_VIEW.trigger, ...setup.plot_view?.trigger },
    };
    this.history = setup.history ?? [];
  }

  get connected(): boolean {
    return this.status?.state === "connected";
  }

  /** Whether the tab can send: SWO only receives, and a recording only plays back. */
  get takesInput(): boolean {
    return this.kind !== "swo" && this.kind !== "replay";
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
      view: this.view,
      hex_view: this.hexView,
      lines_view: this.linesView,
      plot_view: this.plotView,
      history: this.history,
    };
    switch (this.kind) {
      case "rtt":
        return $state.snapshot({ ...common, rtt: this.rtt });
      case "swo":
        return $state.snapshot({ ...common, swo: this.swo });
      case "tcp":
        return $state.snapshot({ ...common, tcp: this.tcp });
      case "replay":
        return $state.snapshot({ ...common, replay: this.replay });
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

  /** Called by the tab's hex view when it opens, which shows what came lately, and with null when it closes. */
  attachHex(term: Terminal | null): void {
    clearTimeout(this.hexTimer);
    this.hexTerm = term;
    if (term) {
      this.hexDump = new HexDump(this.hexView, this.terminal.pause_ms);
      let text = "";
      for (const chunk of this.recent) {
        text += this.hexDump.feed(chunk.tx, chunk.at, chunk.bytes);
      }
      term.write(text + this.hexDump.finish());
    }
  }

  /** Shows the terminal or the hex view. */
  setView(view: View): void {
    this.view = view;
    changed();
  }

  /** Changes how the hex view breaks its lines, and lays out what it shows again. */
  setHexView(hexView: HexView): void {
    this.hexView = hexView;
    this.layOutHex();
    changed();
  }

  /** Changes what the Lines view leaves out. */
  setLinesView(linesView: LinesView): void {
    this.linesView = linesView;
    changed();
  }

  /** Changes how the Plot view reads and shows numbers. */
  setPlotView(plotView: PlotView): void {
    this.plotView = plotView;
    changed();
  }

  /** Starts the plot again from the next line, leaving the lines kept as they are. */
  clearPlot(): void {
    this.plotFrom = this.nextLine;
  }

  /** Marks a line with a bookmark, or takes the bookmark off. */
  toggleBookmark(n: number): void {
    if (!this.bookmarks.delete(n)) {
      this.bookmarks.add(n);
    }
    this.linesVersion++;
  }

  /** Forgets the lines kept for the Lines view, as Clear does for the terminal. */
  clearLines(): void {
    this.textLines.length = 0;
    this.levelCounts.clear();
    this.moduleCounts.clear();
    this.bookmarks.clear();
    this.linesVersion++;
  }

  /**
   * Keeps lines for the Lines view and the plot, with when they were read;
   * the oldest tenth goes when there are too many. Lines read at once came
   * one after another since the read before, so they are spread over that
   * time, up to SPREAD_MS: a fast stream then draws as a curve, not steps.
   */
  private addLines(lines: LogLine[], at: number, times?: number[]): void {
    const step = this.linesAt > 0 ? Math.min(Math.max(0, at - this.linesAt), SPREAD_MS) / lines.length : 0;
    this.linesAt = at;
    lines.forEach((line, index) => {
      // A recording knows when each line came.
      const when = times?.[index] ?? at - (lines.length - 1 - index) * step;
      this.textLines.push({ ...line, n: this.nextLine++, at: when });
      this.count(line, 1);
    });
    if (this.textLines.length > LINES_MAX) {
      const gone = this.textLines.splice(0, this.textLines.length - LINES_MAX + LINES_MAX / 10);
      for (const line of gone) {
        this.count(line, -1);
        this.bookmarks.delete(line.n);
      }
    }
    this.linesVersion++;
  }

  private count(line: LogLine, step: number): void {
    const level = line.level ?? "other";
    this.levelCounts.set(level, (this.levelCounts.get(level) ?? 0) + step);
    if (line.module) {
      const left = (this.moduleCounts.get(line.module) ?? 0) + step;
      if (left > 0) {
        this.moduleCounts.set(line.module, left);
      } else {
        this.moduleCounts.delete(line.module);
      }
    }
  }

  /** Sets the pause that starts a new line, in both views; 0 for never. */
  setPause(ms: number): void {
    this.terminal.pause_ms = ms;
    this.layOutHex();
    changed();
  }

  /** Writes the hex view again from what came lately, with the settings as they are now. */
  private layOutHex(): void {
    const term = this.hexTerm;
    if (term) {
      // A full reset as an escape, so it comes after what is still waiting to be drawn.
      term.write("\x1bc");
      this.attachHex(term);
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
    if (this.kind === "replay") {
      this.openRecording(events);
      changed();
      return;
    }

    // The log starts with the session, so not a byte is missed.
    const common = { id: this.id, log: this.logSetup(), data, events };
    let started: Promise<LogStatus>;
    switch (this.kind) {
      case "rtt":
        started = invoke("open_rtt_session", { ...common, config: $state.snapshot(this.rtt) });
        break;
      case "swo":
        started = invoke("open_swo_session", { ...common, config: $state.snapshot(this.swo) });
        break;
      case "tcp":
        started = invoke("open_tcp_session", { ...common, config: $state.snapshot(this.tcp) });
        break;
      default:
        started = invoke("open_session", {
          ...common,
          port: $state.snapshot(this.port),
          config: $state.snapshot(this.serial),
        });
    }
    started
      .then((status) => this.onLogStatus(status))
      .catch((error) => this.note(`could not start: ${error}`));
    changed();
  }

  /** Opens the tab's recording, paused at its start, or plays it at once if asked to. */
  private openRecording(events: Channel<SessionEvent>): void {
    const { path, speed } = this.replay;
    invoke<ReplayInfo>("open_replay_session", { id: this.id, path, speed, events })
      .then((info) => {
        if (!this.running) {
          return;
        }
        this.target = fileName(path);
        this.connectedAt = Date.now();
        this.status = { state: "connected" };
        this.note(`playing ${this.target}: ${duration(info.length * 1000)} recorded from ${info.name || "a tab"}`);
        if (this.playWhenOpen) {
          this.playWhenOpen = false;
          this.control({ kind: "play" });
        }
      })
      .catch((error) => {
        this.note(String(error));
        this.stop();
      });
  }

  /** Plays the recording of a replay tab, opening it first if it is not open. */
  play(): void {
    if (this.kind !== "replay") {
      return;
    }
    if (!this.running) {
      this.playWhenOpen = true;
      this.start();
      return;
    }
    this.control({ kind: "play" });
  }

  pause(): void {
    this.control({ kind: "pause" });
  }

  /** Goes to a point in the recording, in seconds: back means from the start again. */
  seek(position: number): void {
    this.control({ kind: "seek", position });
  }

  /** Plays the recording this many times as fast as it was recorded. */
  setSpeed(speed: number): void {
    this.replay.speed = speed;
    this.control({ kind: "speed", speed });
    changed();
  }

  /** Plays another recording in this replay tab, from its start. */
  openFile(path: string): void {
    this.stop();
    this.replay.path = path;
    this.name = recordingName(path);
    this.clearViews();
    this.play();
    changed();
  }

  private control(command: ReplayCommand): void {
    if (this.running) {
      void invoke("control_replay", { id: this.id, command });
    }
  }

  /** Clears what the tab shows, for a recording that starts again. */
  private clearViews(): void {
    this.write("\x1bc");
    this.recent = [];
    this.recentBytes = 0;
    this.layOutHex();
    this.clearLines();
    this.rx = 0;
    this.tx = 0;
  }

  /** Starts or stops recording what the tab receives and sends, with its timing, to play back later. */
  setRecording(on: boolean): void {
    if (!this.running || this.kind === "replay") {
      return;
    }
    const before = this.recordPath;
    const folder = on ? settings.log.folder : null;
    invoke<LogStatus>("set_recording", { id: this.id, folder, name: this.name })
      .then((status) => {
        this.recordPath = status.path;
        if (status.path) {
          this.note(`recording to ${status.path}`);
        } else if (status.problem) {
          this.note(status.problem);
        } else if (before) {
          this.note(`recording saved: ${before}`);
        }
      })
      .catch((error) => this.note(`could not record: ${error}`));
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
    if (this.recordPath) {
      this.note(`recording saved: ${this.recordPath}`);
    }
    this.running = false;
    this.status = null;
    this.lines = null;
    this.connectedAt = null;
    this.logPath = null;
    this.recordPath = null;
    this.playback = null;
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

  /** Changes where a TCP session connects; a running session connects again at once. */
  reconfigureTcp(tcp: TcpConfig): void {
    this.tcp = tcp;
    if (this.running) {
      void invoke("reconfigure_tcp_session", { id: this.id, config: $state.snapshot(tcp) });
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
    if (!this.takesInput) {
      if (!this.hinted) {
        this.hinted = true;
        this.note(
          this.kind === "swo"
            ? "SWO only comes from the chip: what you type goes nowhere. Use RTT to type to it."
            : "A recording only plays back: what you type goes nowhere.",
        );
      }
      return;
    }
    this.sendBytes(encoder.encode(data.replace(/\r/g, ENTER[this.terminal.enter])));
    if (this.terminal.local_echo) {
      this.write(data.replace(/\r/g, "\r\n").replace(/\x7f/g, "\b \b"));
    }
  }

  /**
   * Sends a line from the send box or a macro: hex digits, or text with
   * escapes such as \r, then the tab's Enter if `enter` is set. Rejects with
   * the reason when it cannot.
   */
  async sendLine(text: string, hex: boolean, enter: boolean): Promise<void> {
    if (this.kind === "swo") {
      throw "SWO only comes from the chip. Use RTT to send to it.";
    }
    if (this.kind === "replay") {
      throw "A recording only plays back.";
    }
    if (!this.connected) {
      throw "Not connected.";
    }
    const body = new Uint8Array(await invoke<number[]>("encode_input", { text, hex }));
    const end = enter ? encoder.encode(ENTER[this.terminal.enter]) : new Uint8Array();
    const bytes = new Uint8Array(body.length + end.length);
    bytes.set(body);
    bytes.set(end, body.length);
    this.sendBytes(bytes);
    if (this.terminal.local_echo && !hex) {
      this.write(body);
      if (enter) {
        this.write("\r\n");
      }
    }
  }

  /** Sends a macro from its key, and says in the terminal why when it cannot. */
  async sendMacro(macro: Macro): Promise<void> {
    try {
      await this.sendLine(macro.text, macro.hex, macro.enter);
    } catch (problem) {
      this.note(`${macro.name}: ${problem}`);
    }
  }

  /** Keeps a line sent from the send box, so ↑ brings it back. */
  remember(text: string, hex: boolean): void {
    const last = this.history.at(-1);
    if (last && last.text === text && (last.hex ?? false) === hex) {
      return;
    }
    this.history.push(hex ? { text, hex } : { text });
    if (this.history.length > HISTORY_MAX) {
      this.history.splice(0, this.history.length - HISTORY_MAX);
    }
    changed();
  }

  /** Sends bytes to the device, and shows them in the hex view. */
  private sendBytes(bytes: Uint8Array): void {
    if (bytes.length === 0) {
      return;
    }
    void invoke("write_session", { id: this.id, data: Array.from(bytes) });
    this.tx += bytes.length;
    this.sentSince = true;
    this.keepForHex(true, bytes);
  }

  /** Sends a file, paced for the link, with its progress in `sending`. */
  async sendFile(path: string): Promise<void> {
    if (!this.connected || this.sending || !this.takesInput) {
      return;
    }
    const name = path.split(/[\\/]/).pop() ?? path;
    const started = Date.now();
    this.sending = { name, sent: 0, total: 0 };
    this.note(`sending ${name}`);
    const progress = new Channel<FileProgress>();
    progress.onmessage = (report) => this.onFileProgress(name, started, report);
    try {
      await invoke("send_file", { id: this.id, path, bytesPerSecond: this.fileRate(), progress });
    } catch (error) {
      this.sending = null;
      this.note(String(error));
    }
  }

  /** Stops sending a file. */
  cancelSend(): void {
    if (this.sending) {
      void invoke("cancel_send", { id: this.id });
    }
  }

  /** How many bytes a second the link carries, for pacing a file; 0 for as fast as it takes them. */
  private fileRate(): number {
    switch (this.kind) {
      case "serial": {
        const { baud, data_bits, parity, stop_bits } = this.serial;
        // A start bit, the data bits, the parity bit if any, and the stop bits.
        const bits = 1 + data_bits + (parity === "none" ? 0 : 1) + stop_bits;
        return Math.max(1, Math.floor(baud / bits));
      }
      case "rtt":
        return RTT_FILE_RATE;
      default:
        return 0;
    }
  }

  private onFileProgress(name: string, started: number, report: FileProgress): void {
    if (!this.sending) {
      return;
    }
    this.tx += report.sent - this.sending.sent;
    this.sending = { name, sent: report.sent, total: report.total };
    if (!report.done) {
      return;
    }
    this.sending = null;
    if (report.problem) {
      this.note(report.problem);
    } else if (report.cancelled) {
      this.note(`stopped sending ${name} after ${byteText(report.sent)} of ${byteText(report.total)}`);
    } else {
      this.note(`sent ${name}, ${byteText(report.total)} in ${duration(Date.now() - started)}`);
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
    // After a pause, what comes starts on a new line, but not an echo of what
    // was just typed or sent: an interactive shell keeps its lines whole.
    const now = Date.now();
    const pause = this.terminal.pause_ms;
    if (
      pause > 0 &&
      now - this.lastReceived >= pause &&
      !this.sentSince &&
      this.term &&
      this.term.buffer.active.cursorX !== 0
    ) {
      this.write("\r\n");
    }
    this.lastReceived = now;
    this.sentSince = false;
    this.write(bytes);
    this.keepForHex(false, bytes);
  }

  /** Keeps bytes for the hex view, and shows them there if it is open. */
  private keepForHex(tx: boolean, bytes: Uint8Array): void {
    const at = Date.now();
    this.recent.push({ tx, at, bytes });
    this.recentBytes += bytes.length;
    // The oldest half goes at once, now and then, rather than a chunk at a time.
    if (this.recentBytes > 2 * RECENT_MAX) {
      let drop = 0;
      while (this.recentBytes > RECENT_MAX && drop < this.recent.length - 1) {
        this.recentBytes -= this.recent[drop].bytes.length;
        drop++;
      }
      this.recent.splice(0, drop);
    }
    if (this.hexTerm) {
      this.hexTerm.write(this.hexDump.feed(tx, at, bytes));
      // A pause ends the line, so the next packet starts on its own.
      clearTimeout(this.hexTimer);
      const pause = this.terminal.pause_ms;
      if (pause > 0) {
        this.hexTimer = setTimeout(() => this.hexTerm?.write(this.hexDump.finish()), pause);
      }
    }
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
      case "text_lines":
        this.addLines(event.lines, event.at);
        return;
      case "replayed":
        for (const piece of event.pieces) {
          const bytes = new Uint8Array(piece.bytes);
          if (piece.sent) {
            this.tx += bytes.length;
            this.sentSince = true;
            this.keepForHex(true, bytes);
          } else {
            this.receive(bytes);
          }
        }
        if (event.lines.length > 0) {
          this.addLines(event.lines, event.times.at(-1) ?? Date.now(), event.times);
        }
        return;
      case "restarted":
        this.clearViews();
        return;
      case "replay_state":
        this.playback = { position: event.position, length: event.length, playing: event.playing, speed: event.speed };
        return;
      case "recording_stopped":
        this.recordPath = null;
        this.note(event.problem);
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
      case "tcp_state":
        if (event.state === "connected") {
          this.onConnected(event.address, `${event.address}, ${protocolText(this.tcp)}`);
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
      // What is sent while waiting goes nowhere.
      this.cancelSend();
    } else if (problem && problem !== before) {
      this.note(this.explain(problem));
    } else if (this.status === null && !problem) {
      this.note(`waiting for ${this.awaited()}`);
    }
    this.status = { state: "waiting", problem };
  }

  /** What the tab waits for while it is not connected: "COM6", "localhost:19021". */
  private awaited(): string {
    switch (this.kind) {
      case "serial":
        return this.port.name;
      case "tcp":
        return address(this.tcp);
      case "replay":
        return fileName(this.replay.path);
      default:
        return "the probe";
    }
  }

  /** When the program that has the port is ternima itself, says which tab has it. */
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
/** The last part of a path: "Nucleo 2026-10-25 14-02-11.trec". */
export function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

/** A recording's file name without its ending, as the name of the tab that plays it. */
export function recordingName(path: string): string {
  return fileName(path).replace(/\.trec$/i, "");
}

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
