<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { open, save } from "@tauri-apps/plugin-dialog";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import About from "./lib/About.svelte";
  import CommandPalette from "./lib/CommandPalette.svelte";
  import HexBar from "./lib/HexBar.svelte";
  import HighlightsDialog from "./lib/HighlightsDialog.svelte";
  import LinesView from "./lib/LinesView.svelte";
  import PlotView from "./lib/PlotView.svelte";
  import ModbusView from "./lib/ModbusView.svelte";
  import MacrosDialog from "./lib/MacrosDialog.svelte";
  import MenuBar from "./lib/MenuBar.svelte";
  import ProbeDialog from "./lib/ProbeDialog.svelte";
  import SendBar from "./lib/SendBar.svelte";
  import SessionDialog from "./lib/SessionDialog.svelte";
  import SettingsDialog from "./lib/SettingsDialog.svelte";
  import StatusBar from "./lib/StatusBar.svelte";
  import TabBar from "./lib/TabBar.svelte";
  import TcpDialog from "./lib/TcpDialog.svelte";
  import Terminal from "./lib/Terminal.svelte";
  import Toolbar from "./lib/Toolbar.svelte";
  import { buildMenus, isEnabled, itemForKey, macroForKey, menuCommands, menuForAltKey } from "./lib/menus";
  import type { Command } from "./lib/palette";
  import { keys } from "./lib/shortcuts";
  import { defaultLogFolder, loadSettings, settings } from "./lib/settings.svelte";
  import { activeTab, addTab, changed, loadWorkspace, recordingName, showTab, tabs, view, type Tab } from "./lib/tabs.svelte";
  import { activeTerminal } from "./lib/terminal-actions";
  import type { Protocol } from "./lib/net";
  import { DEFAULT_TERMINAL, type SessionKind } from "./lib/serial";
  import { appScheme } from "./lib/theme.svelte";

  type AppInfo = { version: string; os: string; arch: string };

  let info = $state<AppInfo>();
  let ready = $state(false);
  let aboutOpen = $state(false);
  let settingsOpen = $state(false);
  let macrosOpen = $state(false);
  let highlightsOpen = $state(false);
  /** The command palette's commands while it is open. */
  let palette: Command[] | null = $state(null);
  /**
   * The open session dialog, if any: for a new session, or for a tab's
   * settings. A new TCP session can start with a protocol picked.
   */
  let dialog: { kind: SessionKind; tab: Tab | null; protocol?: Protocol } | null = $state(null);
  let menuBar: MenuBar;
  let pane: HTMLElement;
  /** Whether the line between the two panes of a split view is being dragged. */
  let dragging = $state(false);

  const menus = buildMenus({
    about: () => (aboutOpen = true),
    exit: () => void getCurrentWindow().close(),
    newSession: () => (dialog = { kind: "serial", tab: null }),
    newRttSession: () => (dialog = { kind: "rtt", tab: null }),
    newSwoSession: () => (dialog = { kind: "swo", tab: null }),
    newTcpSession: () => (dialog = { kind: "tcp", tab: null }),
    sessionSettings,
    settings: () => (settingsOpen = true),
    saveOutput,
    openLogFolder,
    sendFile,
    macros: () => (macrosOpen = true),
    highlights: () => (highlightsOpen = true),
    commandPalette: openPalette,
    playRecording: () => void playRecording(null),
  });

  /** Opens the command palette with what can be done now: the menus, the tabs and the macros. */
  function openPalette() {
    const commands = menuCommands(menus);
    tabs.forEach((tab, index) =>
      commands.push({
        label: `Go to ${tab.name}`,
        group: "Tab",
        checked: index === view.active,
        run: () => showTab(index),
      }),
    );
    const tab = activeTab();
    if (tab?.connected && tab.takesInput) {
      settings.macros.forEach((macro, index) =>
        commands.push({
          label: `Send ${macro.name}`,
          group: "Macro",
          shortcut: index < 9 ? keys(`Alt+${index + 1}`, `Cmd+${index + 1}`) : undefined,
          run: () => void tab.sendMacro(macro),
        }),
      );
    }
    palette = commands;
  }

  /** Picks a file and sends it through the tab in front. */
  async function sendFile() {
    const tab = activeTab();
    if (!tab) {
      return;
    }
    const path = await open({ title: "Send a file", multiple: false, directory: false });
    if (typeof path === "string") {
      await tab.sendFile(path);
    }
    focusTerminal();
  }

  /** Saves what the terminal in front shows, scrollback and all, to a file the user picks. */
  async function saveOutput() {
    const terminal = activeTerminal();
    if (!terminal) {
      return;
    }
    const name = activeTab()?.name ?? "ternima";
    const path = await save({
      title: "Save output as",
      // The tab's name, without the characters file names cannot have.
      defaultPath: `${name.replace(/[<>:"/\\|?*]/g, "_")}.txt`,
      filters: [
        { name: "Text", extensions: ["txt", "log"] },
        { name: "All files", extensions: ["*"] },
      ],
    });
    if (path) {
      await invoke("save_text", { path, text: terminal.text() }).catch((error) => console.error(error));
    }
    focusTerminal();
  }

  /** Shows the tab's log file in its folder, or the log folder when there is no log. */
  async function openLogFolder() {
    const path = activeTab()?.logPath ?? (settings.log.folder || (await defaultLogFolder()));
    await revealItemInDir(path).catch((error) => console.error(error));
  }

  /** Opens the settings of the tab in front, in the dialog of its kind; a replay tab picks another recording. */
  function sessionSettings() {
    const tab = activeTab();
    if (tab) {
      openSettings(tab);
    }
  }

  function openSettings(tab: Tab) {
    if (tab.kind === "replay") {
      void playRecording(tab);
    } else {
      dialog = { kind: tab.kind, tab };
    }
  }

  /** Picks a recording, from the log folder where they are made, and plays it: in `tab`, or in a new tab. */
  async function playRecording(tab: Tab | null) {
    const path = await open({
      title: "Play a recording",
      multiple: false,
      directory: false,
      defaultPath: settings.log.folder || (await defaultLogFolder()),
      filters: [
        { name: "terNIMA recordings", extensions: ["trec"] },
        { name: "All files", extensions: ["*"] },
      ],
    });
    if (typeof path !== "string") {
      focusTerminal();
      return;
    }
    if (tab) {
      tab.openFile(path);
    } else {
      addTab({
        name: recordingName(path),
        kind: "replay",
        connected: false,
        replay: { path, speed: 1 },
        terminal: { ...DEFAULT_TERMINAL },
      }).play();
    }
    focusTerminal();
  }

  onMount(() => {
    invoke<AppInfo>("app_info").then((result) => (info = result));
    // The terminals wait for the settings, so they open at the saved text size,
    // and for the workspace, so the tabs from last time come back. The
    // workspace waits for the settings, which say how much of it comes back.
    loadSettings()
      .catch((error) => console.error(error))
      .then(() => loadWorkspace())
      .catch((error) => console.error(error))
      .finally(() => (ready = true));
  });

  // The page and the window frame follow the theme.
  $effect(() => {
    document.documentElement.dataset.theme = appScheme();
  });
  $effect(() => {
    const theme = settings.theme;
    void getCurrentWindow().setTheme(theme === "system" ? null : theme);
  });

  function onkeydown(event: KeyboardEvent) {
    if (aboutOpen || settingsOpen || macrosOpen || highlightsOpen || palette || dialog) {
      return;
    }
    const item = itemForKey(menus, event);
    if (item) {
      event.preventDefault();
      if (isEnabled(item)) {
        item.run?.();
      }
      return;
    }
    const macro = macroForKey(event);
    if (macro !== null) {
      event.preventDefault();
      const chosen = settings.macros[macro];
      if (chosen) {
        void activeTab()?.sendMacro(chosen);
      }
      return;
    }
    const menu = menuForAltKey(menus, event);
    if (menu !== null) {
      event.preventDefault();
      void menuBar.openMenu(menu);
    }
  }

  /** Keys the app uses itself, which the terminal must not send to the device. */
  function isAppKey(event: KeyboardEvent): boolean {
    return (
      itemForKey(menus, event) !== null ||
      macroForKey(event) !== null ||
      menuForAltKey(menus, event) !== null
    );
  }

  function focusTerminal() {
    activeTerminal()?.focus();
  }

  function closeDialog() {
    dialog = null;
    focusTerminal();
  }

  /** Whether a tab is on screen: the one in front, or the other half of a split view. */
  function onScreen(index: number): boolean {
    const split = view.split;
    return index === view.active || (split !== null && (index === split.first || index === split.second));
  }

  /** Where a tab's pane sits in a split view, as CSS; all the room without one. */
  function place(index: number): string {
    const split = view.split;
    if (!split) {
      return "inset: 0";
    }
    const [start, end] = split.direction === "right" ? ["left", "right"] : ["top", "bottom"];
    const across = split.direction === "right" ? "top: 0; bottom: 0" : "left: 0; right: 0";
    return index === split.first
      ? `${across}; ${start}: 0; ${end}: ${100 - split.ratio}%`
      : `${across}; ${start}: ${split.ratio}%; ${end}: 0`;
  }

  /** A click in the other pane of a split view gives it the keyboard. */
  function focusPane(index: number) {
    if (index !== view.active) {
      showTab(index);
    }
  }

  function startDrag(event: PointerEvent) {
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    dragging = true;
  }

  function drag(event: PointerEvent) {
    const split = view.split;
    if (!dragging || !split) {
      return;
    }
    const room = pane.getBoundingClientRect();
    const share =
      split.direction === "right"
        ? (event.clientX - room.left) / room.width
        : (event.clientY - room.top) / room.height;
    // The same limits as workspace.rs keeps, so both panes stay usable.
    split.ratio = Math.min(85, Math.max(15, Math.round(share * 100)));
  }

  function endDrag() {
    if (dragging) {
      dragging = false;
      changed();
      focusTerminal();
    }
  }
</script>

<svelte:window {onkeydown} />

<div class="app">
  <MenuBar bind:this={menuBar} {menus} onclose={focusTerminal} />
  <TabBar onnew={(kind, protocol) => (kind === "replay" ? void playRecording(null) : (dialog = { kind, tab: null, protocol }))} />

  <main class="pane" class:dragging bind:this={pane}>
    {#if ready}
      {#if tabs.length === 0}
        <div class="slot" style="inset: 0">
          <div class="layer">
            <Terminal active={true} {isAppKey} />
          </div>
        </div>
      {/if}
      <!-- Every tab keeps its terminal, so its scrollback stays while another tab is in front. -->
      {#each tabs as tab, i (tab.id)}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="slot"
          class:hidden={!onScreen(i)}
          class:focused={view.split !== null && i === view.active}
          class:second={view.split !== null && i === view.split.second}
          class:down={view.split?.direction === "down"}
          style={place(i)}
          onmousedown={() => focusPane(i)}
        >
          {#if onScreen(i)}
            <Toolbar {tab} onsettings={() => openSettings(tab)} />
          {/if}
          <div class="layer">
            <!-- The terminal stays while another view is shown, so nothing it showed is lost. -->
            <div class="view" class:hidden={tab.view !== "terminal"}>
              <Terminal {tab} active={i === view.active && tab.view === "terminal"} {isAppKey} />
            </div>
            {#if tab.view === "lines" && onScreen(i)}
              <div class="view">
                <LinesView {tab} active={i === view.active} onhighlights={() => (highlightsOpen = true)} />
              </div>
            {/if}
            {#if tab.view === "plot" && onScreen(i)}
              <div class="view">
                <PlotView {tab} active={i === view.active} />
              </div>
            {/if}
            {#if tab.view === "modbus" && onScreen(i)}
              <div class="view">
                <ModbusView {tab} active={i === view.active} />
              </div>
            {/if}
            {#if tab.view === "hex" && onScreen(i)}
              <div class="view hexview">
                <HexBar {tab} />
                <div class="hexterm">
                  <Terminal {tab} hex active={i === view.active} {isAppKey} />
                </div>
              </div>
            {/if}
          </div>
          {#if onScreen(i) && settings.send_box && tab.takesInput && tab.view !== "modbus"}
            <SendBar {tab} onmacros={() => (macrosOpen = true)} />
          {/if}
        </div>
      {/each}
      {#if view.split}
        <!-- The line between the panes: drag it to share the room, double-click for halves. -->
        <div
          class="divider {view.split.direction}"
          role="separator"
          aria-orientation={view.split.direction === "right" ? "vertical" : "horizontal"}
          aria-valuenow={view.split.ratio}
          title="Drag to resize, double-click for halves"
          style={view.split.direction === "right" ? `left: ${view.split.ratio}%` : `top: ${view.split.ratio}%`}
          onpointerdown={startDrag}
          onpointermove={drag}
          onpointerup={endDrag}
          onpointercancel={endDrag}
          ondblclick={() => {
            if (view.split) {
              view.split.ratio = 50;
              changed();
            }
          }}
        ></div>
      {/if}
    {/if}
  </main>

  <StatusBar tab={activeTab()} {info} />
</div>

{#if dialog?.kind === "rtt" || dialog?.kind === "swo"}
  <ProbeDialog kind={dialog.kind} tab={dialog.tab} onclose={closeDialog} />
{:else if dialog?.kind === "tcp"}
  <TcpDialog tab={dialog.tab} protocol={dialog.protocol} onclose={closeDialog} />
{:else if dialog}
  <SessionDialog tab={dialog.tab} onclose={closeDialog} />
{/if}

{#if settingsOpen}
  <SettingsDialog
    onclose={() => {
      settingsOpen = false;
      focusTerminal();
    }}
  />
{/if}

{#if palette}
  <CommandPalette
    commands={palette}
    onclose={() => {
      palette = null;
      focusTerminal();
    }}
  />
{/if}

{#if highlightsOpen}
  <HighlightsDialog
    onclose={() => {
      highlightsOpen = false;
      focusTerminal();
    }}
  />
{/if}

{#if macrosOpen}
  <MacrosDialog
    onclose={() => {
      macrosOpen = false;
      focusTerminal();
    }}
  />
{/if}

{#if aboutOpen && info}
  <About
    version={info.version}
    onclose={() => {
      aboutOpen = false;
      focusTerminal();
    }}
  />
{/if}

<style>
  .app {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  .pane {
    position: relative;
    flex: 1;
    min-height: 0;
    background: var(--bg);
  }

  /* One tab's place on screen: its toolbar over its terminal. */
  .slot {
    position: absolute;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .slot.hidden {
    display: none;
  }

  /* In a split view, a line between the panes, and a mark on the one with the keyboard. */
  .slot.second {
    border-left: 1px solid var(--border);
  }

  .slot.second.down {
    border-left: 0;
    border-top: 1px solid var(--border);
  }

  .slot.focused::before {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    z-index: 1;
    height: 2px;
    background: var(--accent);
    content: "";
  }

  .layer {
    position: relative;
    flex: 1;
    min-height: 0;
    padding: 8px 4px 4px 12px;
  }

  .view {
    height: 100%;
  }

  .view.hidden {
    display: none;
  }

  .hexview {
    display: flex;
    flex-direction: column;
  }

  .hexterm {
    flex: 1;
    min-height: 0;
  }

  .divider {
    position: absolute;
    z-index: 2;
    touch-action: none;
  }

  .divider.right {
    top: 0;
    bottom: 0;
    width: 7px;
    margin-left: -3px;
    cursor: col-resize;
  }

  .divider.down {
    left: 0;
    right: 0;
    height: 7px;
    margin-top: -3px;
    cursor: row-resize;
  }

  .divider:hover,
  .dragging .divider {
    background: var(--accent-soft);
  }

  /* While the line is dragged, the terminals must not select text under the pointer. */
  .pane.dragging {
    user-select: none;
  }

  .pane.dragging .slot {
    pointer-events: none;
  }
</style>
