<script lang="ts">
  import { look, stateText } from "./state";
  import { closeTab, showTab, tabs, view } from "./tabs.svelte";

  let { onnew }: { onnew: () => void } = $props();
</script>

<header class="tabs" role="tablist">
  {#if tabs.length === 0}
    <div class="tab active" role="tab" aria-selected="true">
      <span class="dot stopped"></span>
      Welcome
    </div>
  {/if}

  {#each tabs as tab, i (tab.id)}
    <div
      class="tab"
      class:active={i === view.active}
      role="tab"
      tabindex="-1"
      aria-selected={i === view.active}
      title={`${tab.name}: ${stateText(tab)}`}
      onmousedown={() => showTab(i)}
    >
      <span class="dot {look(tab)}"></span>
      <span class="name">{tab.name}</span>
      <button
        class="close"
        aria-label={`Close ${tab.name}`}
        title="Close tab"
        onmousedown={(event) => {
          event.stopPropagation();
          closeTab(tab);
        }}
      >
        ×
      </button>
    </div>
  {/each}

  <button class="add" aria-label="New serial session" title="New serial session" onclick={onnew}>
    +
  </button>
</header>

<style>
  .tabs {
    display: flex;
    align-items: stretch;
    height: 36px;
    overflow-x: auto;
    background: var(--panel);
    border-bottom: 1px solid var(--border);
    scrollbar-width: none;
  }

  .tab {
    display: flex;
    flex-shrink: 0;
    align-items: center;
    gap: 8px;
    max-width: 240px;
    padding: 0 8px 0 14px;
    color: var(--muted);
    border-right: 1px solid var(--border);
  }

  .tab:hover {
    color: var(--text);
  }

  .tab.active {
    color: var(--text);
    background: var(--bg);
    box-shadow: inset 0 2px 0 var(--accent);
    /* The active tab covers the bar's bottom border, so it joins the pane. */
    margin-bottom: -1px;
  }

  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .dot {
    flex-shrink: 0;
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }

  .dot.connected {
    background: var(--ok);
  }

  .dot.waiting {
    background: var(--warn);
    animation: pulse 1.4s ease-in-out infinite;
  }

  .dot.problem {
    background: var(--bad);
  }

  .dot.stopped {
    border: 1.5px solid var(--muted);
  }

  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }

  button {
    font: inherit;
    color: var(--muted);
    background: none;
    border: 0;
    cursor: default;
  }

  .close {
    width: 20px;
    height: 20px;
    font-size: 15px;
    line-height: 1;
    border-radius: 4px;
    visibility: hidden;
  }

  .tab:hover .close,
  .tab.active .close {
    visibility: visible;
  }

  .close:hover {
    color: var(--text);
    background: var(--hover);
  }

  .add {
    flex-shrink: 0;
    width: 36px;
    font-size: 18px;
  }

  .add:hover {
    color: var(--text);
    background: var(--hover);
  }
</style>
