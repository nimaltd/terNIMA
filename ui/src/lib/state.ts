// How a tab's state is shown: a colored dot and a few words.

import { address, protocolText } from "./net";
import { channelText, hexAddress, hzText } from "./probe";
import { sshTarget } from "./ssh";
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
      if (tab.kind === "tcp") return `Connected to ${tab.target}, ${protocolText(tab.tcp)}`;
      if (tab.kind === "ssh") return `Connected to ${tab.target}, SSH`;
      if (tab.kind === "replay") return replayText(tab);
      return `Connected to ${tab.target}`;
    case "problem":
      return tab.problem ?? "";
    case "waiting":
      return waitingText(tab);
    case "stopped":
      return tab.kind === "replay" ? "Not open" : "Not connected";
  }
}

/** What the tab waits for, and what brings it. */
function waitingText(tab: Tab): string {
  switch (tab.kind) {
    case "serial":
      return `Waiting for ${tab.port.name}: plug it in and terNIMA connects by itself`;
    case "tcp":
      return tcpWaitingText(tab);
    case "ssh":
      return tab.question
        ? `Waiting for you to answer, to connect to ${sshTarget(tab.ssh)}`
        : `Waiting for ${address(tab.ssh)} to answer: terNIMA connects by itself`;
    case "replay":
      return "Opening the recording";
    default:
      return "Waiting for the probe: plug it in and terNIMA connects by itself";
  }
}

function tcpWaitingText(tab: Tab): string {
  const at = address(tab.tcp);
  switch (tab.tcp.protocol) {
    case "jlink":
      return `Waiting for J-Link's RTT server at ${at}: connect J-Link's software to the chip`;
    case "openocd":
      return `Waiting for OpenOCD's RTT server at ${at}: start it with rtt server start`;
    default:
      return `Waiting for ${at} to answer: terNIMA connects by itself`;
  }
}

/** "RTT on STM32H503RB at 0x20000400, channel 0 Terminal", or "Watching variables on STM32H503RB" */
function rttText(tab: Tab): string {
  if (tab.watchOnly) return `Watching variables on ${tab.target} through the probe`;
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

/** "Playing Nucleo.trec at 2×, 0:12.3 of 1:00.0" */
function replayText(tab: Tab): string {
  const play = tab.playback;
  if (!play) return `Playing ${tab.target}`;
  const speed = play.speed === 1 ? "" : ` at ${play.speed}×`;
  const where = `${playTime(play.position)} of ${playTime(play.length)}`;
  if (play.playing) return `Playing ${tab.target}${speed}, ${where}`;
  if (play.position >= play.length) return `Played ${tab.target} to the end, ${playTime(play.length)}`;
  return `Paused ${tab.target} at ${where}`;
}

/** A time in a recording: "1:02.5". */
export function playTime(seconds: number): string {
  const tenths = Math.max(0, Math.round(seconds * 10));
  const minutes = Math.floor(tenths / 600);
  const rest = ((tenths % 600) / 10).toFixed(1).padStart(4, "0");
  return `${minutes}:${rest}`;
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
