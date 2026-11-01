// The serial and workspace types of ternima-core, as the UI sees them, and
// the commands that reach them.

import { invoke } from "@tauri-apps/api/core";
import type { ModbusSetup } from "./modbus.svelte";
import type { NetState, TcpConfig } from "./net";
import type { ChannelInfo, RttConfig, RttState, SwoConfig, SwoState } from "./probe";
import type { SshConfig, SshQuestion, SshState } from "./ssh";

export type UsbId = {
  /** Four hex digits, such as "0483". */
  vid: string;
  pid: string;
  serial?: string | null;
  interface?: number | null;
  location?: string | null;
};

export type PortInfo = {
  name: string;
  usb: UsbId | null;
  manufacturer: string | null;
  product: string | null;
};

/** What a session remembers about its port, to find it again. */
export type PortId = { name: string; usb?: UsbId | null };

export type Parity = "none" | "odd" | "even";
export type FlowControl = "none" | "rtscts" | "xonxoff";

export type SerialConfig = {
  baud: number;
  data_bits: number;
  parity: Parity;
  stop_bits: number;
  flow: FlowControl;
  dtr: boolean;
  rts: boolean;
  /** The firmware's ELF file, to show where code addresses in the output are. */
  elf?: string | null;
};

export type LineEnding = "cr" | "lf" | "crlf";

export type TerminalOptions = {
  enter: LineEnding;
  local_echo: boolean;
  lf_is_newline: boolean;
  /** Starts a new line when data comes after a pause this long, in ms; 0 never does. */
  pause_ms: number;
};

/** A tab's session: a device, or a recording played back. */
export type SessionKind = "serial" | "rtt" | "swo" | "tcp" | "ssh" | "replay";

/**
 * What a tab shows: the terminal, every byte in hex, every line in a list,
 * the numbers in a chart, or Modbus on a serial tab.
 */
export type View = "terminal" | "hex" | "lines" | "plot" | "modbus" | "watch";

/** A Zephyr log level. */
export type Level = "err" | "wrn" | "inf" | "dbg";

/** A received line, with a Zephyr log line read into its parts. */
export type LogLine = {
  /** The whole line, without colors and control characters. */
  text: string;
  /** The time as the log wrote it, such as "00:00:01.234,567". */
  time: string | null;
  level: Level | null;
  module: string | null;
  /** What follows the module; the whole text for a line that is not a log line. */
  message: string;
};

/** What the Lines view leaves out. */
export type LinesView = {
  hidden_levels: Level[];
  hidden_modules: string[];
  /** Shows the lines that are not log lines, such as the shell's. */
  others: boolean;
};

export const DEFAULT_LINES_VIEW: LinesView = { hidden_levels: [], hidden_modules: [], others: true };

/** What the Plot view draws: values over time, one against the others, how often each comes, or their frequencies. */
export type Chart = "time" | "xy" | "histogram" | "fft";

/** A scope's trigger on the time chart: the time around the last crossing of a level, the crossing in the middle. */
export type Trigger = {
  on: boolean;
  /** The series it watches; empty for the first. */
  series: string;
  level: number;
  edge: "rising" | "falling" | "both";
  /** Stops at the first crossing, until it is armed again. */
  single: boolean;
};

/** How the Plot view reads and shows numbers. */
export type PlotView = {
  /** "auto" reads JSON, named values and plain numbers; "regex" uses `regex`. */
  /** "modbus" plots the values read on a serial tab's Modbus line; "watch" the variables an RTT tab watches. */
  mode: "auto" | "regex" | "modbus" | "watch";
  regex: string;
  /** The time shown while it follows the newest values, in ms; 0 shows them all. */
  window_ms: number;
  /** The series left out of the chart. */
  hidden: string[];
  /** The series on the time chart's right axis, with a scale of their own. */
  right: string[];
  chart: Chart;
  /** The series along the XY chart's x axis; empty for the first. */
  x_series: string;
  /** The histogram's bins; 0 picks them from the values. */
  bins: number;
  /** The series the histogram counts; empty for the first. */
  hist_series: string;
  fft_window: "hann" | "hamming" | "blackman" | "rect";
  /** The FFT in dB rather than as amplitudes. */
  fft_db: boolean;
  /** Samples a second for the FFT; 0 measures it from when the lines came. */
  fft_rate: number;
  trigger: Trigger;
  /** Keeps the help over the chart while values come too; it shows on its own while there are none. */
  help: boolean;
  /** Marks each sample of the time chart with a dot, where there is room for it. */
  dots: boolean;
};

export const DEFAULT_TRIGGER: Trigger = { on: false, series: "", level: 0, edge: "rising", single: false };

export const DEFAULT_PLOT_VIEW: PlotView = {
  mode: "auto",
  regex: "",
  window_ms: 30_000,
  hidden: [],
  right: [],
  chart: "time",
  x_series: "",
  bins: 0,
  hist_series: "",
  fft_window: "hann",
  fft_db: true,
  fft_rate: 0,
  trigger: DEFAULT_TRIGGER,
  help: false,
  dots: true,
};

/** How the hex view breaks its lines, besides the tab's pause. It only changes what is shown. */
export type HexView = {
  /** Bytes on a full line: 8, 16 or 32. */
  width: number;
  /** A byte that ends its line, such as 0x0A for LF, or null for none. */
  end_byte?: number | null;
};

export const DEFAULT_HEX_VIEW: HexView = { width: 16, end_byte: null };

/** The recording a replay tab plays, and how fast. */
export type ReplaySetup = { path: string; speed: number };

export const DEFAULT_REPLAY: ReplaySetup = { path: "", speed: 1 };

/** Where a recording being played is, in seconds. */
export type PlayState = { position: number; length: number; playing: boolean; speed: number };

/** A line sent from the send box, kept so ↑ brings it back. */
export type SentLine = { text: string; hex?: boolean };

/** One tab as workspace.toml keeps it. Only the settings of its kind are there. */
export type SessionSetup = {
  name: string;
  kind: SessionKind;
  connected: boolean;
  /** Whether what it receives is written to a log file. */
  log?: boolean;
  port?: PortId;
  serial?: SerialConfig;
  rtt?: RttConfig;
  swo?: SwoConfig;
  tcp?: TcpConfig;
  ssh?: SshConfig;
  replay?: ReplaySetup;
  terminal: TerminalOptions;
  view?: View;
  hex_view?: HexView;
  lines_view?: LinesView;
  plot_view?: PlotView;
  modbus?: ModbusSetup;
  /** The lines sent from the send box, oldest first. */
  history?: SentLine[];
  watch?: WatchSetup;
};

/** The variables an RTT tab's Watch view reads, and how often. */
export type WatchSetup = {
  /** Their names, as the ELF file has them: counter, sensor.temp. */
  names: string[];
  /** How often they are read, in ms. */
  every_ms: number;
  /** They are shown beside the terminal too. */
  beside?: boolean;
};

export const DEFAULT_WATCH: WatchSetup = { names: [], every_ms: 50, beside: false };

export type VariableKind = "bool" | "u8" | "i8" | "u16" | "i16" | "u32" | "i32" | "u64" | "i64" | "f32" | "f64";

/** A variable in a firmware's ELF file that the Watch view can read. */
export type Variable = { name: string; address: number; kind: VariableKind; type_name: string };

/** Every variable in an ELF file that can be watched, by name. Rejects with why it cannot be read. */
export function elfVariables(path: string): Promise<Variable[]> {
  return invoke<Variable[]>("elf_variables", { path });
}

export type SplitDirection = "right" | "down";

/** Two tabs on screen at once: first (left or top) and second. */
export type Split = {
  first: number;
  second: number;
  direction: SplitDirection;
  /** The first pane's share of the room, in percent. */
  ratio: number;
};

export type Workspace = {
  active: number;
  split?: Split | null;
  sessions: SessionSetup[];
  /** The tabs closed lately, newest first, as they were. */
  closed?: SessionSetup[];
};

export type SessionState =
  | { state: "waiting"; problem: string | null }
  | { state: "connected"; port: PortId };

export type ModemLines = { cts: boolean; dsr: boolean; dcd: boolean; ri: boolean };

/** What a session tells the UI, apart from the data it receives. */
export type SessionEvent =
  | ({ kind: "state" } & SessionState)
  | ({ kind: "lines" } & ModemLines)
  | ({ kind: "rtt_state" } & RttState)
  | { kind: "channels"; up: ChannelInfo[]; down: ChannelInfo[] }
  | ({ kind: "swo_state" } & SwoState)
  | ({ kind: "tcp_state" } & NetState)
  | ({ kind: "ssh_state" } & SshState)
  | ({ kind: "ssh_ask" } & SshQuestion)
  | { kind: "text_lines"; lines: LogLine[]; at: number }
  /** The watched variables' values, in their order, with when they were read. */
  | { kind: "watch"; values: (number | null)[]; at: number }
  /** Why the watched variables cannot be read now, or null once they can. */
  | { kind: "watch_problem"; problem: string | null }
  | {
      kind: "replayed";
      pieces: { sent: boolean; bytes: number[]; at: number }[];
      lines: LogLine[];
      /** When each line came, in ms since 1970. */
      times: number[];
    }
  | { kind: "restarted" }
  | ({ kind: "replay_state" } & PlayState)
  | { kind: "recording_stopped"; problem: string }
  | { kind: "overflow"; count: number }
  | { kind: "log_stopped"; problem: string };

export const DEFAULT_SERIAL: SerialConfig = {
  baud: 115200,
  data_bits: 8,
  parity: "none",
  stop_bits: 1,
  flow: "none",
  dtr: true,
  rts: true,
};

export const DEFAULT_TERMINAL: TerminalOptions = {
  enter: "cr",
  local_echo: false,
  lf_is_newline: true,
  pause_ms: 0,
};

/**
 * The pauses offered for starting a new line, in ms; 0 is never. Received
 * bytes reach the UI in batches about every 16 ms, plus up to 10 ms of read
 * wait, so in a steady stream batches come up to about 26 ms apart: a
 * shorter pause would break lines in the middle of it.
 */
export const PAUSES = [0, 50, 100, 200, 500, 1000, 2000];

/** "never", "50 ms", "1 s" */
export function pauseText(ms: number): string {
  if (ms === 0) return "never";
  return ms >= 1000 ? `${ms / 1000} s` : `${ms} ms`;
}

/** Baud rates offered in lists. Any other number can be typed. */
export const BAUD_RATES = [
  1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600, 1000000, 2000000,
  3000000,
];

export function listPorts(): Promise<PortInfo[]> {
  return invoke<PortInfo[]>("list_ports");
}

export function getWorkspace(): Promise<Workspace> {
  return invoke<Workspace>("get_workspace");
}

export function setWorkspace(workspace: Workspace): Promise<void> {
  return invoke("set_workspace", { workspace });
}

/** The usual short form, such as "115200 8N1". */
export function summary(config: SerialConfig): string {
  const parity = { none: "N", odd: "O", even: "E" }[config.parity];
  return `${config.baud} ${config.data_bits}${parity}${config.stop_bits}`;
}

/** A port's name for people: its product, else its maker, else its kind. */
export function describe(port: PortInfo): string {
  return product(port) ?? port.manufacturer ?? (port.usb ? "USB serial port" : "Serial port");
}

/** The product name without the maker in front, which some drivers repeat there. */
export function product(port: PortInfo): string | null {
  const { product, manufacturer } = port;
  if (product && manufacturer && product.startsWith(`${manufacturer} `)) {
    return product.slice(manufacturer.length + 1);
  }
  return product;
}

export function closeAllSessions(): Promise<void> {
  return invoke("close_all_sessions");
}

/** "0483:3754", for USB ports. */
export function usbIds(usb: UsbId | null | undefined): string {
  return usb ? `${usb.vid}:${usb.pid}` : "";
}
