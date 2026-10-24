// What is in the menu bar. Items for later phases of the roadmap are listed
// already, greyed out with their phase, so the menus show where the app is going.

import { openUrl } from "@tauri-apps/plugin-opener";
import {
  FONT_SIZE_DEFAULT,
  FONT_SIZE_MAX,
  FONT_SIZE_MIN,
  settings,
  updateSettings,
  type ThemeChoice,
} from "./settings.svelte";
import { isMac, keys, matches } from "./shortcuts";
import { activeTab, closeTab, showTab, tabs, view } from "./tabs.svelte";
import { activeTerminal } from "./terminal-actions";

export type Item = {
  label: string;
  shortcut?: string;
  /** The roadmap phase that brings this item. Until then it is greyed out. */
  phase?: number;
  enabled?: () => boolean;
  checked?: () => boolean;
  run?: () => void;
};

/** A menu's items, with `null` for a separator line. */
export type Menu = { title: string; items: (Item | null)[] };

/** What the menus ask of the app around them. */
export type AppActions = {
  about(): void;
  exit(): void;
  /** Opens the dialog for a new serial session. */
  newSession(): void;
  /** Opens the dialog for a new RTT session. */
  newRttSession(): void;
  /** Opens the dialog for a new SWO session. */
  newSwoSession(): void;
  /** Opens the settings dialog of the tab in front. */
  sessionSettings(): void;
};

const GITHUB = "https://github.com/nimaltd/ternimal";

export function buildMenus(app: AppActions): Menu[] {
  const terminal = activeTerminal;
  const hasTerminal = () => terminal() !== null;

  const theme = (label: string, choice: ThemeChoice): Item => ({
    label,
    checked: () => settings.theme === choice,
    run: () => updateSettings({ theme: choice }),
  });
  const textSize = (size: number) =>
    updateSettings({ font_size: Math.min(FONT_SIZE_MAX, Math.max(FONT_SIZE_MIN, size)) });

  const hasTab = () => activeTab() !== undefined;
  const running = () => activeTab()?.running ?? false;
  const connected = () => activeTab()?.connected ?? false;
  const hasTabs = () => tabs.length > 1;
  const isSerial = () => activeTab()?.kind === "serial";
  const onProbe = () => activeTab()?.kind === "rtt" || activeTab()?.kind === "swo";

  return [
    {
      title: "File",
      items: [
        {
          label: "New serial session...",
          shortcut: keys("Ctrl+Shift+N", "Cmd+N"),
          run: app.newSession,
        },
        {
          label: "New RTT session...",
          shortcut: keys("Ctrl+Shift+R", "Cmd+Shift+R"),
          run: app.newRttSession,
        },
        { label: "New SWO session...", run: app.newSwoSession },
        null,
        {
          label: "Close tab",
          shortcut: keys("Ctrl+Shift+W", "Cmd+W"),
          enabled: hasTab,
          run: () => {
            const tab = activeTab();
            if (tab) closeTab(tab);
          },
        },
        null,
        { label: "Save output as...", phase: 1 },
        null,
        isMac
          ? { label: "Quit ternimal", shortcut: "Cmd+Q", run: app.exit }
          : { label: "Exit", run: app.exit },
      ],
    },
    {
      title: "Edit",
      items: [
        {
          label: "Copy",
          shortcut: keys("Ctrl+Shift+C", "Cmd+C"),
          enabled: () => terminal()?.hasSelection() ?? false,
          run: () => void terminal()?.copy(),
        },
        {
          label: "Paste",
          shortcut: keys("Ctrl+Shift+V", "Cmd+V"),
          enabled: hasTerminal,
          run: () => void terminal()?.paste(),
        },
        {
          label: "Select all",
          shortcut: keys("Ctrl+Shift+A", "Cmd+A"),
          enabled: hasTerminal,
          run: () => terminal()?.selectAll(),
        },
        null,
        { label: "Find...", shortcut: keys("Ctrl+Shift+F", "Cmd+F"), phase: 3 },
        {
          label: "Clear screen",
          shortcut: keys("Ctrl+Shift+K", "Cmd+K"),
          enabled: hasTerminal,
          run: () => terminal()?.clear(),
        },
      ],
    },
    {
      title: "View",
      items: [
        { label: "Terminal", checked: () => true, run: () => {} },
        { label: "Log", phase: 3 },
        { label: "Hex", phase: 3 },
        { label: "Plot", phase: 4 },
        null,
        {
          label: "Next tab",
          shortcut: "Ctrl+Tab",
          enabled: hasTabs,
          run: () => showTab(view.active + 1),
        },
        {
          label: "Previous tab",
          shortcut: "Ctrl+Shift+Tab",
          enabled: hasTabs,
          run: () => showTab(view.active - 1),
        },
        null,
        {
          label: "Bigger text",
          shortcut: keys("Ctrl+=", "Cmd+="),
          run: () => textSize(settings.font_size + 1),
        },
        {
          label: "Smaller text",
          shortcut: keys("Ctrl+-", "Cmd+-"),
          run: () => textSize(settings.font_size - 1),
        },
        {
          label: "Normal text size",
          shortcut: keys("Ctrl+0", "Cmd+0"),
          run: () => textSize(FONT_SIZE_DEFAULT),
        },
        null,
        theme("System theme", "system"),
        theme("Dark theme", "dark"),
        theme("Light theme", "light"),
      ],
    },
    {
      title: "Connection",
      items: [
        {
          label: "Connect",
          enabled: () => hasTab() && !running(),
          run: () => activeTab()?.start(),
        },
        {
          label: "Disconnect",
          enabled: running,
          run: () => activeTab()?.stop(),
        },
        null,
        {
          label: "Session settings...",
          enabled: hasTab,
          run: app.sessionSettings,
        },
        null,
        {
          label: "DTR",
          checked: () => activeTab()?.serial.dtr ?? false,
          enabled: isSerial,
          run: () => {
            const tab = activeTab();
            tab?.setDtr(!tab.serial.dtr);
          },
        },
        {
          label: "RTS",
          checked: () => activeTab()?.serial.rts ?? false,
          enabled: () => isSerial() && activeTab()?.serial.flow !== "rtscts",
          run: () => {
            const tab = activeTab();
            tab?.setRts(!tab.serial.rts);
          },
        },
        {
          label: "Send break",
          enabled: () => isSerial() && connected(),
          run: () => activeTab()?.sendBreak(),
        },
        null,
        {
          label: "Reset target",
          enabled: () => onProbe() && connected(),
          run: () => activeTab()?.resetTarget(),
        },
      ],
    },
    {
      title: "Tools",
      items: [
        { label: "Send file...", phase: 3 },
        { label: "Macros...", phase: 3 },
        { label: "Plotter", phase: 4 },
        null,
        { label: "Flash firmware...", phase: 5 },
        { label: "Calculators", phase: 6 },
      ],
    },
    {
      title: "Help",
      items: [
        { label: "ternimal on GitHub", run: () => void openUrl(GITHUB) },
        { label: "Report a bug...", run: () => void openUrl(`${GITHUB}/issues/new`) },
        null,
        { label: "About ternimal", run: app.about },
      ],
    },
  ];
}

/** Whether an item can be used right now. */
export function isEnabled(item: Item): boolean {
  return item.phase === undefined && item.run !== undefined && (item.enabled?.() ?? true);
}

/**
 * The item whose shortcut this key press is, if any. Its key never reaches the
 * device, even while the item is greyed out, so a shortcut always means one thing.
 */
export function itemForKey(menus: Menu[], event: KeyboardEvent): Item | null {
  for (const menu of menus) {
    for (const item of menu.items) {
      if (item?.shortcut && matches(item.shortcut, event)) {
        return item;
      }
    }
  }
  return null;
}

/** Alt+F opens File, Alt+E opens Edit and so on; not on macOS, where Option types characters. */
export function menuForAltKey(menus: Menu[], event: KeyboardEvent): number | null {
  if (isMac || !event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) {
    return null;
  }
  const index = menus.findIndex((menu) => event.code === `Key${menu.title[0]}`);
  return index >= 0 ? index : null;
}
