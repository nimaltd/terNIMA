<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { save } from "@tauri-apps/plugin-dialog";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import About from "./lib/About.svelte";
  import MenuBar from "./lib/MenuBar.svelte";
  import ProbeDialog from "./lib/ProbeDialog.svelte";
  import SessionDialog from "./lib/SessionDialog.svelte";
  import SettingsDialog from "./lib/SettingsDialog.svelte";
  import StatusBar from "./lib/StatusBar.svelte";
  import TabBar from "./lib/TabBar.svelte";
  import Terminal from "./lib/Terminal.svelte";
  import Toolbar from "./lib/Toolbar.svelte";
  import { buildMenus, isEnabled, itemForKey, menuForAltKey } from "./lib/menus";
  import { defaultLogFolder, loadSettings, settings } from "./lib/settings.svelte";
  import { activeTab, changed, loadWorkspace, showTab, tabs, view, type Tab } from "./lib/tabs.svelte";
  import { activeTerminal } from "./lib/terminal-actions";
  import type { SessionKind } from "./lib/serial";
  import { appScheme } from "./lib/theme.svelte";

  type AppInfo = { version: string; os: string; arch: string };

  let info = $state<AppInfo>();
  let ready = $state(false);
  let aboutOpen = $state(false);
  let settingsOpen = $state(false);
  /** The open session dialog, if any: for a new session, or for a tab's settings. */
  let dialog: { kind: SessionKind; tab: Tab | null } | null = $state(null);
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
    sessionSettings,
    settings: () => (settingsOpen = true),
    saveOutput,
    openLogFolder,
  });

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

  /** Opens the settings of the tab in front, in the dialog of its kind. */
  function sessionSettings() {
    const tab = activeTab();
    if (tab) {
      dialog = { kind: tab.kind, tab };
    }
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
    if (aboutOpen || settingsOpen || dialog) {
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
    const menu = menuForAltKey(menus, event);
    if (menu !== null) {
      event.preventDefault();
      void menuBar.openMenu(menu);
    }
  }

  /** Keys the app uses itself, which the terminal must not send to the device. */
  function isAppKey(event: KeyboardEvent): boolean {
    return itemForKey(menus, event) !== null || menuForAltKey(menus, event) !== null;
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
  <TabBar onnew={(kind) => (dialog = { kind, tab: null })} />

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
            <Toolbar {tab} onsettings={() => (dialog = { kind: tab.kind, tab })} />
          {/if}
          <div class="layer">
            <Terminal {tab} active={i === view.active} {isAppKey} />
          </div>
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
