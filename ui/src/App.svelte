<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import About from "./lib/About.svelte";
  import MenuBar from "./lib/MenuBar.svelte";
  import Terminal from "./lib/Terminal.svelte";
  import { buildMenus, isEnabled, itemForKey, menuForAltKey } from "./lib/menus";
  import { loadSettings, settings } from "./lib/settings.svelte";
  import { activeTerminal } from "./lib/terminal-actions";
  import { appScheme } from "./lib/theme.svelte";

  type AppInfo = { version: string; os: string; arch: string };

  let info = $state<AppInfo>();
  let ready = $state(false);
  let aboutOpen = $state(false);
  let menuBar: MenuBar;

  const menus = buildMenus({
    about: () => (aboutOpen = true),
    exit: () => void getCurrentWindow().close(),
  });

  onMount(() => {
    invoke<AppInfo>("app_info").then((result) => (info = result));
    // The terminal waits for the settings, so it opens at the saved text size.
    loadSettings()
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
    if (aboutOpen) {
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
</script>

<svelte:window {onkeydown} />

<div class="app">
  <MenuBar bind:this={menuBar} {menus} onclose={focusTerminal} />

  <header class="tabs">
    <div class="tab active">
      <span class="state" title="Not connected"></span>
      Welcome
    </div>
  </header>

  <main class="pane">
    {#if ready}
      <Terminal {isAppKey} />
    {/if}
  </main>

  <footer class="status">
    <span>Not connected</span>
    <span class="spacer"></span>
    {#if info}
      <span>ternimal v{info.version}</span>
      <span class="divider"></span>
      <span>{info.os} {info.arch}</span>
    {/if}
  </footer>
</div>

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
    display: grid;
    grid-template-rows: auto auto 1fr auto;
    height: 100%;
  }

  .tabs {
    display: flex;
    align-items: stretch;
    height: 36px;
    background: var(--panel);
    border-bottom: 1px solid var(--border);
  }

  .tab {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 16px;
    color: var(--muted);
    border-right: 1px solid var(--border);
  }

  .tab.active {
    color: var(--text);
    background: var(--bg);
    box-shadow: inset 0 2px 0 var(--accent);
    /* The active tab covers the bar's bottom border, so it joins the pane. */
    margin-bottom: -1px;
  }

  .state {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    border: 1.5px solid var(--muted);
  }

  .pane {
    min-height: 0;
    padding: 8px 4px 4px 12px;
    background: var(--bg);
  }

  .status {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 24px;
    padding: 0 12px;
    font-size: 12px;
    color: var(--muted);
    background: var(--panel);
    border-top: 1px solid var(--border);
  }

  .spacer {
    flex: 1;
  }

  .divider {
    width: 1px;
    height: 12px;
    background: var(--border);
  }
</style>
