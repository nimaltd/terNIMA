import { invoke } from "@tauri-apps/api/core";

export type ThemeChoice = "system" | "dark" | "light";

export type LogFormat = "text" | "raw";

/** How log files are written; each tab turns its own log on or off. */
export type LogSettings = {
  /** New tabs start with their log on. */
  new_tabs: boolean;
  /** Where log files go. Empty means "ternima logs" in Documents. */
  folder: string;
  format: LogFormat;
  /** The date and time at the start of every line of a text log. */
  timestamps: boolean;
};

/** What comes back when ternima starts. */
export type StartupSettings = {
  /** Opens the tabs from last time, with their settings and split view. */
  restore_tabs: boolean;
  /** Connects the tabs that were connected when ternima closed. */
  reconnect: boolean;
  /** Opens the window where it was, at the size it had. */
  restore_window: boolean;
};

/** What the mouse does in the terminal and the hex view. */
export type MouseSettings = {
  /** Selecting text copies it, as in PuTTY. */
  select_copies: boolean;
  /** A right-click pastes. */
  right_click_pastes: boolean;
};

/** Looking on GitHub for a newer terNIMA. */
export type UpdateSettings = {
  /** Looks when terNIMA starts. */
  check: boolean;
  /** A version not to tell about again, such as "1.0.2"; empty for none. */
  skipped: string;
};

/** A command kept for one click, or Alt with its number. */
export type Macro = {
  name: string;
  /** Text with escapes such as \r, or hex digits. */
  text: string;
  hex: boolean;
  /** Sends the tab's Enter after the text. */
  enter: boolean;
};

/** Macros sent one after another with a click, a wait after each: a number of times, or until it is stopped. */
export type Sequence = {
  name: string;
  /** The names of the macros it sends, in order. */
  steps: string[];
  /** How long it waits after each macro, in ms. */
  delay_ms: number;
  /** How many times it goes through its macros; 0 goes on until it is stopped. */
  repeat: number;
};

export type HighlightColor = "red" | "orange" | "yellow" | "green" | "blue" | "purple";

export const HIGHLIGHT_COLORS: HighlightColor[] = ["red", "orange", "yellow", "green", "blue", "purple"];

/** Lines with this text are marked in a color in the Lines view. */
export type Highlight = {
  text: string;
  /** Whether `text` is a regular expression. */
  regex: boolean;
  /** Whether upper and lower case must match. */
  case: boolean;
  color: HighlightColor;
};

/** The same fields as `Settings` in ternima-core's settings.rs. */
export type Settings = {
  theme: ThemeChoice;
  font_size: number;
  /** Shows a line under each terminal to type a whole command and send it at once. */
  send_box: boolean;
  startup: StartupSettings;
  log: LogSettings;
  mouse: MouseSettings;
  updates: UpdateSettings;
  macros: Macro[];
  /** Macros sent one after another. */
  sequences: Sequence[];
  /** The Lines view's highlight rules; the first that matches a line marks it. */
  highlights: Highlight[];
};

/** Terminal text size limits, the same as in settings.rs. */
export const FONT_SIZE_MIN = 8;
export const FONT_SIZE_MAX = 32;
export const FONT_SIZE_DEFAULT = 14;

/** The settings in use. Read them anywhere; change them only with `updateSettings`. */
export const settings: Settings = $state({
  theme: "system",
  font_size: FONT_SIZE_DEFAULT,
  send_box: true,
  startup: { restore_tabs: true, reconnect: true, restore_window: true },
  log: { new_tabs: false, folder: "", format: "text", timestamps: true },
  mouse: { select_copies: true, right_click_pastes: true },
  updates: { check: true, skipped: "" },
  macros: [],
  sequences: [],
  highlights: [],
});

/** Reads the settings from settings.toml, through the Rust side. */
export async function loadSettings(): Promise<void> {
  Object.assign(settings, await invoke<Settings>("get_settings"));
}

/** Changes settings at once, and saves them to settings.toml. */
export function updateSettings(change: Partial<Settings>): void {
  Object.assign(settings, change);
  invoke<Settings>("set_settings", { settings: $state.snapshot(settings) })
    .then((kept) => Object.assign(settings, kept))
    .catch((error) => console.error(error));
}

/** The folder logs go to when the settings leave it empty. */
export function defaultLogFolder(): Promise<string> {
  return invoke<string>("default_log_folder");
}
