<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import Modal from "./Modal.svelte";
  import {
    FONT_SIZE_MAX,
    FONT_SIZE_MIN,
    defaultLogFolder,
    settings,
    updateSettings,
    type LogSettings,
    type StartupSettings,
    type ThemeChoice,
  } from "./settings.svelte";

  let { onclose }: { onclose: () => void } = $props();

  // A copy of the settings: nothing changes until Save.
  let theme: ThemeChoice = $state(settings.theme);
  let fontSize = $state(settings.font_size);
  let startup: StartupSettings = $state({ ...settings.startup });
  let log: LogSettings = $state({ ...settings.log });
  let defaultFolder = $state("");
  let error = $state("");

  onMount(() => {
    defaultLogFolder().then((folder) => (defaultFolder = folder));
  });

  async function browse() {
    const folder = await open({
      title: "The folder for log files",
      directory: true,
      multiple: false,
      defaultPath: log.folder || defaultFolder || undefined,
    });
    if (typeof folder === "string") {
      log.folder = folder;
    }
  }

  function submit() {
    if (!Number.isInteger(fontSize) || fontSize < FONT_SIZE_MIN || fontSize > FONT_SIZE_MAX) {
      error = `The text size must be a whole number from ${FONT_SIZE_MIN} to ${FONT_SIZE_MAX}.`;
      return;
    }
    updateSettings({ theme, font_size: fontSize, startup: { ...startup }, log: { ...log, folder: log.folder.trim() } });
    onclose();
  }
</script>

<Modal title="Settings" {onclose} onsubmit={submit}>
  <h2 class="dlg-section">Appearance</h2>
  <div class="dlg-grid">
    <label class="dlg-label">
      Theme
      <select class="field" bind:value={theme}>
        <option value="system">Follow the OS</option>
        <option value="dark">Dark</option>
        <option value="light">Light</option>
      </select>
    </label>
    <label class="dlg-label">
      Text size
      <input class="field mono" type="number" min={FONT_SIZE_MIN} max={FONT_SIZE_MAX} bind:value={fontSize} />
    </label>
  </div>

  <h2 class="dlg-section">Startup</h2>
  <p class="dlg-hint">What ternima brings back from last time when it starts.</p>
  <div class="dlg-checks startup">
    <label class="dlg-check">
      <input type="checkbox" bind:checked={startup.restore_tabs} /> Reopen the tabs, with their settings and split view
    </label>
    <label class="dlg-check under" class:dlg-muted={!startup.restore_tabs}>
      <input type="checkbox" bind:checked={startup.reconnect} disabled={!startup.restore_tabs} /> Connect the tabs that
      were connected
    </label>
    <label class="dlg-check">
      <input type="checkbox" bind:checked={startup.restore_window} /> Open the window where it was, at its size
    </label>
  </div>

  <h2 class="dlg-section">Logging</h2>
  <p class="dlg-hint">
    Each tab turns its own log on or off with the Log button, or File > Log this tab. Every time a log
    starts, it gets a new file named after the tab and the time.
  </p>
  <div class="dlg-checks">
    <label class="dlg-check">
      <input type="checkbox" bind:checked={log.new_tabs} /> Log new tabs to a file
    </label>
    <label class="dlg-check" title="Only for text logs">
      <input type="checkbox" bind:checked={log.timestamps} disabled={log.format === "raw"} /> Date and time on every
      line
    </label>
  </div>

  <label class="dlg-label folder">
    Folder
    <span class="file">
      <input class="field mono" placeholder={defaultFolder} bind:value={log.folder} />
      <button class="btn" onclick={browse}>Browse...</button>
    </span>
  </label>

  <div class="dlg-grid format">
    <label class="dlg-label">
      Format
      <select class="field" bind:value={log.format}>
        <option value="text">Text, colors removed</option>
        <option value="raw">Raw bytes, as they came</option>
      </select>
    </label>
  </div>

  {#if error}
    <p class="dlg-error">{error}</p>
  {/if}

  {#snippet buttons()}
    <button class="btn" onclick={onclose}>Cancel</button>
    <button class="btn primary" onclick={submit}>Save</button>
  {/snippet}
</Modal>

<style>
  .startup {
    flex-direction: column;
  }

  /* Connecting the tabs again needs them reopened first. */
  .under {
    padding-left: 24px;
  }

  .folder {
    margin-top: 12px;
  }

  .file {
    display: flex;
    gap: 8px;
  }

  .file .field {
    flex: 1;
    min-width: 0;
  }

  .format {
    margin-top: 12px;
  }
</style>
