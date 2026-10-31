// The kinds of tab that File > New session and the + button open: one list,
// so both offer the same, in the same words and order.

import type { Protocol } from "./net";
import type { SessionKind } from "./serial";
import { keys } from "./shortcuts";

export type NewTab = {
  kind: SessionKind;
  /** The protocol a TCP tab starts with. */
  protocol?: Protocol;
  label: string;
  /** What it is for, under the label in the + menu. */
  detail: string;
  shortcut?: string;
};

export const NEW_TABS: NewTab[] = [
  {
    kind: "serial",
    label: "Serial port",
    detail: "A UART, USB serial adapter or a board's virtual COM port",
    shortcut: keys("Ctrl+Shift+N", "Cmd+N"),
  },
  {
    kind: "rtt",
    label: "RTT through a debug probe",
    detail: "CMSIS-DAP, J-Link or ST-Link, while the chip runs",
    shortcut: keys("Ctrl+Shift+R", "Cmd+Shift+R"),
  },
  {
    kind: "tcp",
    protocol: "jlink",
    label: "RTT through a debugger's server",
    detail: "J-Link or OpenOCD, while it debugs the chip",
  },
  {
    kind: "swo",
    label: "SWO through a debug probe",
    detail: "ITM output on the chip's trace pin, such as printf",
  },
  {
    kind: "tcp",
    label: "TCP or Telnet",
    detail: "A device, a serial bridge or a shell on the network",
    shortcut: keys("Ctrl+Shift+T", "Cmd+Shift+T"),
  },
  {
    kind: "replay",
    label: "A recording",
    detail: "Play back what a tab recorded, with its timing",
    shortcut: keys("Ctrl+Shift+O", "Cmd+O"),
  },
];
