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
import { activeTab, closeTab, otherPane, showTab, splitView, tabs, unsplit, view } from "./tabs.svelte";
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
  /** Opens the dialog for a new TCP session: a socket, Telnet or an RTT server. */
  newTcpSession(): void;
  /** Opens the settings dialog of the tab in front. */
  sessionSettings(): void;
  /** Opens the app's settings. */
  settings(): void;
  /** Saves what the terminal in front shows to a file the user picks. */
  saveOutput(): void;
  /** Shows the log file, or the log folder, in the OS's file manager. */
  openLogFolder(): void;
  /** Picks a file and sends it through the tab in front. */
  sendFile(): void;
  /** Opens the macros dialog. */
  macros(): void;
};

const GITHUB = "https://github.com/nimaltd/ternima";

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
        {
          label: "New TCP session...",
          shortcut: keys("Ctrl+Shift+T", "Cmd+Shift+T"),
          run: app.newTcpSession,
        },
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
        {
          label: "Log this tab to a file",
          shortcut: keys("Ctrl+Shift+L", "Cmd+Shift+L"),
          enabled: hasTab,
          checked: () => activeTab()?.log ?? false,
          run: () => {
            const tab = activeTab();
            tab?.setLog(!tab.log);
          },
        },
        { label: "Open log folder", run: app.openLogFolder },
        {
          label: "Save output as...",
          shortcut: keys("Ctrl+Shift+S", "Cmd+Shift+S"),
          enabled: hasTerminal,
          run: app.saveOutput,
        },
        null,
        { label: "Settings...", shortcut: keys("Ctrl+,", "Cmd+,"), run: app.settings },
        null,
        isMac
          ? { label: "Quit terNIMA", shortcut: "Cmd+Q", run: app.exit }
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
        {
          label: "Find...",
          shortcut: keys("Ctrl+Shift+F", "Cmd+F"),
          enabled: hasTerminal,
          run: () => terminal()?.openSearch(),
        },
        { label: "Find next", shortcut: "F3", enabled: hasTerminal, run: () => terminal()?.findNext() },
        {
          label: "Find previous",
          shortcut: "Shift+F3",
          enabled: hasTerminal,
          run: () => terminal()?.findPrevious(),
        },
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
        {
          label: "Terminal",
          enabled: hasTab,
          checked: () => hasTab() && activeTab()?.view !== "hex",
          run: () => activeTab()?.setView("terminal"),
        },
        {
          label: "Hex",
          shortcut: keys("Ctrl+Shift+H", "Cmd+Shift+H"),
          enabled: hasTab,
          checked: () => activeTab()?.view === "hex",
          run: () => {
            const tab = activeTab();
            tab?.setView(tab.view === "hex" ? "terminal" : "hex");
          },
        },
        { label: "Log", phase: 3 },
        { label: "Plot", phase: 4 },
        null,
        {
          label: "Send box",
          checked: () => settings.send_box,
          run: () => updateSettings({ send_box: !settings.send_box }),
        },
        null,
        {
          label: "Split right",
          shortcut: keys("Ctrl+Shift+E", "Cmd+Shift+E"),
          enabled: hasTabs,
          checked: () => view.split?.direction === "right",
          run: () => splitView("right"),
        },
        {
          label: "Split down",
          shortcut: keys("Ctrl+Shift+D", "Cmd+Shift+D"),
          enabled: hasTabs,
          checked: () => view.split?.direction === "down",
          run: () => splitView("down"),
        },
        { label: "One tab only", enabled: () => view.split !== null, run: unsplit },
        { label: "Other pane", shortcut: "F6", enabled: () => view.split !== null, run: otherPane },
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
        {
          label: "Send file...",
          enabled: () => connected() && activeTab()?.kind !== "swo" && !activeTab()?.sending,
          run: app.sendFile,
        },
        {
          label: "Stop sending the file",
          enabled: () => activeTab()?.sending != null,
          run: () => activeTab()?.cancelSend(),
        },
        { label: "Macros...", run: app.macros },
        { label: "Plotter", phase: 4 },
        null,
        { label: "Calculators", phase: 6 },
      ],
    },
    {
      title: "Help",
      items: [
        { label: "terNIMA on GitHub", run: () => void openUrl(GITHUB) },
        { label: "Report a bug...", run: () => void openUrl(`${GITHUB}/issues/new`) },
        null,
        { label: "About terNIMA", run: app.about },
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

/** The macro a key press sends, counted from 0: Alt+1 to Alt+9, or Cmd+1 to Cmd+9 on macOS. */
export function macroForKey(event: KeyboardEvent): number | null {
  for (let number = 1; number <= 9; number++) {
    if (matches(keys(`Alt+${number}`, `Cmd+${number}`), event)) {
      return number - 1;
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
