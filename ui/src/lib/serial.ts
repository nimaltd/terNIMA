// The serial and workspace types of ternima-core, as the UI sees them, and
// the commands that reach them.

import { invoke } from "@tauri-apps/api/core";
import type { NetState, TcpConfig } from "./net";
import type { ChannelInfo, RttConfig, RttState, SwoConfig, SwoState } from "./probe";

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
};

export type LineEnding = "cr" | "lf" | "crlf";

export type TerminalOptions = {
  enter: LineEnding;
  local_echo: boolean;
  lf_is_newline: boolean;
  /** Starts a new line when data comes after a pause this long, in ms; 0 never does. */
  pause_ms: number;
};

export type SessionKind = "serial" | "rtt" | "swo" | "tcp";

/** What a tab shows: the terminal, or every byte in hex. */
export type View = "terminal" | "hex";

/** How the hex view breaks its lines, besides the tab's pause. It only changes what is shown. */
export type HexView = {
  /** Bytes on a full line: 8, 16 or 32. */
  width: number;
  /** A byte that ends its line, such as 0x0A for LF, or null for none. */
  end_byte?: number | null;
};

export const DEFAULT_HEX_VIEW: HexView = { width: 16, end_byte: null };

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
  terminal: TerminalOptions;
  view?: View;
  hex_view?: HexView;
  /** The lines sent from the send box, oldest first. */
  history?: SentLine[];
};

export type SplitDirection = "right" | "down";

/** Two tabs on screen at once: first (left or top) and second. */
export type Split = {
  first: number;
  second: number;
  direction: SplitDirection;
  /** The first pane's share of the room, in percent. */
  ratio: number;
};

export type Workspace = { active: number; split?: Split | null; sessions: SessionSetup[] };

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
