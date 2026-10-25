import { invoke } from "@tauri-apps/api/core";

export type ThemeChoice = "system" | "dark" | "light";

export type LogFormat = "text" | "raw";

/** How log files are written; each tab turns its own log on or off. */
export type LogSettings = {
  /** New tabs start with their log on. */
  new_tabs: boolean;
  /** Where log files go. Empty means "ternimal logs" in Documents. */
  folder: string;
  format: LogFormat;
  /** The date and time at the start of every line of a text log. */
  timestamps: boolean;
};

/** What comes back when ternimal starts. */
export type StartupSettings = {
  /** Opens the tabs from last time, with their settings and split view. */
  restore_tabs: boolean;
  /** Connects the tabs that were connected when ternimal closed. */
  reconnect: boolean;
  /** Opens the window where it was, at the size it had. */
  restore_window: boolean;
};

/** The same fields as `Settings` in ternimal-core's settings.rs. */
export type Settings = {
  theme: ThemeChoice;
  font_size: number;
  startup: StartupSettings;
  log: LogSettings;
};

/** Terminal text size limits, the same as in settings.rs. */
export const FONT_SIZE_MIN = 8;
export const FONT_SIZE_MAX = 32;
export const FONT_SIZE_DEFAULT = 14;

/** The settings in use. Read them anywhere; change them only with `updateSettings`. */
export const settings: Settings = $state({
  theme: "system",
  font_size: FONT_SIZE_DEFAULT,
  startup: { restore_tabs: true, reconnect: true, restore_window: true },
  log: { new_tabs: false, folder: "", format: "text", timestamps: true },
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
