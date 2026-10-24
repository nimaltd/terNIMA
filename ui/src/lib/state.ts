// How a tab's state is shown: a colored dot and a few words.

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
      return `Connected to ${tab.port.name}`;
    case "problem":
      return tab.problem ?? "";
    case "waiting":
      return `Waiting for ${tab.port.name}: plug it in and ternimal connects by itself`;
    case "stopped":
      return "Not connected";
  }
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
