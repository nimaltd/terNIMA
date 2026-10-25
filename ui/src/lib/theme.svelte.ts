import type { ITheme } from "@xterm/xterm";
import { settings } from "./settings.svelte";

export type Scheme = "dark" | "light";

// The terminal's background matches --bg in app.css, so the pane has no seam.
export const terminalThemes: Record<Scheme, ITheme> = {
  dark: {
    background: "#0f1419",
    foreground: "#d6dde6",
    cursor: "#2dd4bf",
    cursorAccent: "#0f1419",
    selectionBackground: "#2dd4bf44",
    black: "#1c232c",
    red: "#f07178",
    green: "#a6da95",
    yellow: "#f5c56b",
    blue: "#6cb6ff",
    magenta: "#d197f5",
    cyan: "#5eead4",
    white: "#c9d1d9",
    brightBlack: "#5c6773",
    brightRed: "#ff8f8f",
    brightGreen: "#b9f27c",
    brightYellow: "#ffd580",
    brightBlue: "#8ccaff",
    brightMagenta: "#e2b0ff",
    brightCyan: "#99f6e4",
    brightWhite: "#ffffff",
  },
  light: {
    background: "#fbfcfd",
    foreground: "#1d2733",
    cursor: "#0f9488",
    cursorAccent: "#fbfcfd",
    selectionBackground: "#0f948833",
    black: "#1d2733",
    red: "#c62828",
    green: "#2e7d32",
    yellow: "#a66b00",
    blue: "#1565c0",
    magenta: "#8e24aa",
    cyan: "#00838f",
    // Light backgrounds make "white" text vanish, so it is a grey here.
    white: "#6b7785",
    brightBlack: "#8a96a3",
    brightRed: "#e53935",
    brightGreen: "#43a047",
    brightYellow: "#c98a00",
    brightBlue: "#1e88e5",
    brightMagenta: "#ab47bc",
    brightCyan: "#0097a7",
    brightWhite: "#3b4652",
  },
};

/** Search match colors: every match, and the one the search is on. */
export const searchColors: Record<Scheme, { match: string; current: string }> = {
  dark: { match: "#4d3d0f", current: "#9a6b00" },
  light: { match: "#fff1b8", current: "#ffc53d" },
};

const lightQuery = window.matchMedia("(prefers-color-scheme: light)");
const osScheme = (): Scheme => (lightQuery.matches ? "light" : "dark");

// The OS's scheme, kept up to date when the OS switches between dark and light.
const os = $state({ scheme: osScheme() });
lightQuery.addEventListener("change", () => (os.scheme = osScheme()));

/** The color scheme in use: the one picked in the View menu, or else the OS's. */
export function appScheme(): Scheme {
  return settings.theme === "system" ? os.scheme : settings.theme;
}
