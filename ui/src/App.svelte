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
  import { activeTab, loadWorkspace, tabs, view, type Tab } from "./lib/tabs.svelte";
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
    const name = activeTab()?.name ?? "ternimal";
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
    // and for the workspace, so the tabs from last time come back.
    Promise.all([loadSettings(), loadWorkspace()])
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
</script>

<svelte:window {onkeydown} />

<div class="app">
  <MenuBar bind:this={menuBar} {menus} onclose={focusTerminal} />
  <TabBar onnew={(kind) => (dialog = { kind, tab: null })} />

  {#if activeTab()}
    <Toolbar tab={activeTab()!} onsettings={sessionSettings} />
  {/if}

  <main class="pane">
    {#if ready}
      {#if tabs.length === 0}
        <div class="layer">
          <Terminal active={true} {isAppKey} />
        </div>
      {/if}
      <!-- Every tab keeps its terminal, so its scrollback stays while another tab is in front. -->
      {#each tabs as tab, i (tab.id)}
        <div class="layer" class:hidden={i !== view.active}>
          <Terminal {tab} active={i === view.active} {isAppKey} />
        </div>
      {/each}
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

  .layer {
    position: absolute;
    inset: 0;
    padding: 8px 4px 4px 12px;
  }

  .layer.hidden {
    display: none;
  }
</style>
