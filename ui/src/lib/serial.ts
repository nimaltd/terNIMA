// The serial and workspace types of ternimal-core, as the UI sees them, and
// the commands that reach them.

import { invoke } from "@tauri-apps/api/core";

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
};

export type SessionSetup = {
  name: string;
  connected: boolean;
  port: PortId;
  serial: SerialConfig;
  terminal: TerminalOptions;
};

export type Workspace = { active: number; sessions: SessionSetup[] };

export type SessionState =
  | { state: "waiting"; problem: string | null }
  | { state: "connected"; port: PortId };

export type ModemLines = { cts: boolean; dsr: boolean; dcd: boolean; ri: boolean };

export type SessionEvent =
  | ({ kind: "state" } & SessionState)
  | ({ kind: "lines" } & ModemLines);

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
};

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
