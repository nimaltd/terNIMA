// How a tab's state is shown: a colored dot and a few words.

import { channelText, hexAddress, hzText } from "./probe";
import type { Tab } from "./tabs.svelte";

export type Look = "connected" | "waiting" | "problem" | "stopped";

export function look(tab: Tab): Look {
  if (!tab.running) return "stopped";
  if (tab.connected) return "connected";
  return tab.problem ? "problem" : "waiting";
}

export function stateText(tab: Tab): string {
  switch (look(tab)) {
    case "connected":
      if (tab.kind === "rtt") return rttText(tab);
      if (tab.kind === "swo") return swoText(tab);
      return `Connected to ${tab.target}`;
    case "problem":
      return tab.problem ?? "";
    case "waiting":
      return tab.kind === "serial"
        ? `Waiting for ${tab.port.name}: plug it in and ternima connects by itself`
        : "Waiting for the probe: plug it in and ternima connects by itself";
    case "stopped":
      return "Not connected";
  }
}

/** "RTT on STM32H503RB at 0x20000400, channel 0 Terminal" */
function rttText(tab: Tab): string {
  const at = tab.rttAddress === null ? "" : ` at ${hexAddress(tab.rttAddress)}`;
  const channel = tab.channels?.up.find((up) => up.number === tab.rtt.up_channel);
  const which = channel ? channelText(channel) : String(tab.rtt.up_channel);
  return `RTT on ${tab.target}${at}, channel ${which}`;
}

/** "SWO from STM32H503RB at 2 MHz, ITM port 0" */
function swoText(tab: Tab): string {
  const ports = tab.swo.all_ports ? "all ITM ports" : `ITM port ${tab.swo.port}`;
  return `SWO from ${tab.target} at ${hzText(tab.swo.baud)}, ${ports}`;
}

/** "1.2 KB" */
export function bytes(count: number): string {
  if (count < 1024) return `${count} B`;
  if (count < 1024 * 1024) return `${(count / 1024).toFixed(1)} KB`;
  return `${(count / 1024 / 1024).toFixed(1)} MB`;
}

/** "00:12:31" */
export function elapsed(ms: number): string {
  const seconds = Math.floor(ms / 1000);
  const two = (n: number) => String(n).padStart(2, "0");
  return `${two(Math.floor(seconds / 3600))}:${two(Math.floor(seconds / 60) % 60)}:${two(seconds % 60)}`;
}
