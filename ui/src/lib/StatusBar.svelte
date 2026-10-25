<script lang="ts">
  import { onMount } from "svelte";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { bytes, elapsed, look, stateText } from "./state";
  import type { Tab } from "./tabs.svelte";

  /** The file name at the end of a path, with \ or / between folders. */
  function fileName(path: string): string {
    return path.split(/[\\/]/).pop() ?? path;
  }

  type AppInfo = { version: string; os: string; arch: string };

  let { tab, info }: { tab: Tab | undefined; info: AppInfo | undefined } = $props();

  // A clock for the time connected, ticking once a second.
  let now = $state(Date.now());
  onMount(() => {
    const timer = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(timer);
  });
</script>

<footer class="status">
  {#if tab}
    <span class="state {look(tab)}" title={stateText(tab)}>{stateText(tab)}</span>
    {#if tab.connectedAt !== null}
      <span class="mono">{elapsed(now - tab.connectedAt)}</span>
    {/if}
    <span class="divider"></span>
    <span class="number" title="Bytes received">RX {bytes(tab.rx)}</span>
    <span class="number" title="Bytes sent">TX {bytes(tab.tx)}</span>
    {#if tab.sending}
      {@const sending = tab.sending}
      <span class="divider"></span>
      <span class="sending" title={`${bytes(sending.sent)} of ${bytes(sending.total)}`}>
        Sending {sending.name}
        {sending.total > 0 ? `${Math.floor((100 * sending.sent) / sending.total)}%` : ""}
      </span>
      <button class="stop" title="Stop sending the file" onclick={() => tab.cancelSend()}>Stop</button>
    {/if}
    {#if tab.logPath}
      <span class="divider"></span>
      <button
        class="log"
        title={`${tab.logPath}\nClick to show it in its folder`}
        onclick={() => revealItemInDir(tab.logPath!)}
      >
        Logging to {fileName(tab.logPath)}
      </button>
    {/if}
  {:else}
    <span>Not connected</span>
  {/if}
  <span class="spacer"></span>
  {#if info}
    <span>terNIMA v{info.version}</span>
    <span class="divider"></span>
    <span>{info.os} {info.arch}</span>
  {/if}
</footer>

<style>
  .status {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 24px;
    padding: 0 12px;
    overflow: hidden;
    font-size: 12px;
    color: var(--muted);
    white-space: nowrap;
    background: var(--panel);
    border-top: 1px solid var(--border);
  }

  .state {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* Byte counts keep their width as they grow, so nothing after them moves. */
  .number {
    min-width: 10ch;
    font-variant-numeric: tabular-nums;
  }

  .state.connected {
    color: var(--ok);
  }

  .state.waiting {
    color: var(--warn);
  }

  .state.problem {
    color: var(--bad);
  }

  .log {
    max-width: 320px;
    overflow: hidden;
    padding: 0;
    font: inherit;
    color: var(--bad);
    text-overflow: ellipsis;
    white-space: nowrap;
    background: none;
    border: 0;
    cursor: default;
  }

  .log:hover {
    text-decoration: underline;
  }

  .sending {
    color: var(--text);
  }

  .stop {
    padding: 0 6px;
    font: inherit;
    color: var(--muted);
    background: none;
    border: 1px solid var(--border);
    border-radius: 4px;
    cursor: default;
  }

  .stop:hover {
    color: var(--text);
    background: var(--hover);
  }

  .spacer {
    flex: 1;
  }

  .divider {
    flex-shrink: 0;
    width: 1px;
    height: 12px;
    background: var(--border);
  }
</style>
