<script lang="ts">
  import { NEW_TABS } from "./new-tab";

  import type { Protocol } from "./net";
  import type { SessionKind } from "./serial";

  // The kinds of session a new tab can be, the same as File > New session: the
  // menu of the + button, and of the welcome tab's New session button. Where it
  // shows, and when it closes, is up to them.
  let { onpick }: { onpick: (kind: SessionKind, protocol?: Protocol, live?: boolean) => void } = $props();
</script>

<div class="menu" role="menu">
  {#each NEW_TABS as { kind, protocol, live, label, detail } (label)}
    <button role="menuitem" onclick={() => onpick(kind, protocol, live)}>
      <strong>{label}</strong>
      <span>{detail}</span>
    </button>
  {/each}
</div>

<style>
  .menu {
    display: flex;
    flex-direction: column;
    width: 290px;
    padding: 4px;
    background: var(--menu-bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: var(--shadow);
  }

  button {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    padding: 7px 10px;
    font: inherit;
    color: var(--text);
    text-align: left;
    background: none;
    border: 0;
    border-radius: 6px;
    cursor: default;
  }

  button:hover {
    background: var(--accent-soft);
  }

  strong {
    font-weight: 600;
  }

  span {
    font-size: 12px;
    color: var(--muted);
  }
</style>
