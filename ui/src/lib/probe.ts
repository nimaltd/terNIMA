// The debug probe, RTT and SWO types of ternima-core, as the UI sees them,
// and the commands that reach them.

import { invoke } from "@tauri-apps/api/core";

export type ProbeId = {
  /** Four hex digits, such as "0483". */
  vid: string;
  pid: string;
  serial?: string | null;
};

export type ProbeInfo = { name: string; id: ProbeId };

export type RttConfig = {
  probe?: ProbeId | null;
  /** As probe-rs names it, such as "STM32H503RB". */
  chip: string;
  /** The SWD clock in kHz, or 0 for the probe's own default. */
  speed_khz: number;
  under_reset: boolean;
  elf?: string | null;
  /** Hex text such as "0x20000400", or nothing to search RAM. */
  address?: string | null;
  up_channel: number;
  down_channel: number;
  /** Only the watched variables are read, for firmware without RTT. */
  watch_only?: boolean;
};

export type ChannelInfo = { number: number; name: string | null; size: number };

export type RttState =
  | { state: "waiting"; problem: string | null }
  | { state: "connected"; chip: string; address: number }
  /** Attached, reading only the watched variables. */
  | { state: "watching"; chip: string };

export const DEFAULT_RTT: RttConfig = {
  probe: null,
  chip: "",
  speed_khz: 0,
  under_reset: false,
  elf: null,
  address: null,
  up_channel: 0,
  down_channel: 0,
  watch_only: false,
};

/** SWD clock choices in kHz; 0 is the probe's own default. */
export const SPEEDS = [0, 100, 500, 1000, 4000, 8000, 24000];

export function listProbes(): Promise<ProbeInfo[]> {
  return invoke<ProbeInfo[]>("list_probes");
}

export function searchChips(text: string, limit = 40): Promise<string[]> {
  return invoke<string[]>("search_chips", { text, limit });
}

/** What was found on the board. */
export type Detected = {
  chip: string | null;
  /** Every chip it may be: the same silicon often comes in several packages. */
  candidates: string[];
  description: string;
};

/** Finds out which chip is on the board through a probe. Only reads, for STM32. */
export function detectChip(probe: ProbeId, speedKhz: number): Promise<Detected> {
  return invoke<Detected>("detect_chip", { probe, speedKhz });
}

export function sameProbe(a: ProbeId | null | undefined, b: ProbeId | null | undefined): boolean {
  return !!a && !!b && a.vid === b.vid && a.pid === b.pid && (a.serial ?? null) === (b.serial ?? null);
}

/** "0x20000400" */
export function hexAddress(address: number): string {
  return `0x${address.toString(16).padStart(8, "0")}`;
}

/** "4 MHz", "500 kHz", or "probe default" for 0. */
export function speedText(khz: number): string {
  if (khz === 0) return "Probe default";
  return khz >= 1000 ? `${khz / 1000} MHz` : `${khz} kHz`;
}

/** A channel for a list: "0 Terminal". */
export function channelText(channel: ChannelInfo): string {
  return channel.name ? `${channel.number} ${channel.name}` : `${channel.number}`;
}

export type SwoConfig = {
  probe?: ProbeId | null;
  chip: string;
  speed_khz: number;
  under_reset: boolean;
  /** The trace unit's clock in Hz, usually the core clock. */
  clock_hz: number;
  baud: number;
  /** The ITM stimulus port shown, 0 to 31. */
  port: number;
  all_ports: boolean;
  /** The firmware's ELF file, where the Watch view finds its variables. */
  elf?: string | null;
};

export type SwoState =
  | { state: "waiting"; problem: string | null }
  | { state: "connected"; chip: string; baud: number };

export const DEFAULT_SWO: SwoConfig = {
  probe: null,
  chip: "",
  speed_khz: 0,
  under_reset: false,
  clock_hz: 0,
  baud: 2_000_000,
  port: 0,
  all_ports: false,
  elf: null,
};

/** SWO baud rates offered in the list. Any other number can be typed. */
export const SWO_BAUDS = [500_000, 1_000_000, 2_000_000, 4_000_000, 6_000_000, 8_000_000, 12_000_000];

/** "32 MHz", "2 MHz", "500 kHz", "115200 Hz" */
export function hzText(hz: number): string {
  if (hz >= 1_000_000 && hz % 100_000 === 0) return `${hz / 1_000_000} MHz`;
  if (hz >= 1000 && hz % 1000 === 0) return `${hz / 1000} kHz`;
  return `${hz} Hz`;
}

/** What is wrong with an SWO clock and baud rate, as ternima-core checks it. */
export function swoProblem(clockHz: number, baud: number): string | null {
  if (!Number.isInteger(clockHz) || clockHz <= 0) {
    return "Enter the chip's core clock in Hz, such as 64000000: the SWO speed is divided from it.";
  }
  if (!Number.isInteger(baud) || baud <= 0 || baud > clockHz) {
    return `The SWO baud rate must be between 1 and the clock, ${clockHz} Hz.`;
  }
  if (clockHz % baud !== 0) {
    let divider = Math.ceil(clockHz / baud);
    while (clockHz % divider !== 0) divider++;
    return `${hzText(clockHz)} cannot be divided to ${baud} baud exactly. Try ${clockHz / divider} baud.`;
  }
  return null;
}
