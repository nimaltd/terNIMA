<script lang="ts">
  import { look, stateText } from "./state";
  import { closeTab, showTab, tabs, view } from "./tabs.svelte";

  import type { Protocol } from "./net";
  import type { SessionKind } from "./serial";

  /** Opens the dialog for a new session of a kind; a TCP one can start with a protocol. */
  let { onnew }: { onnew: (kind: SessionKind, protocol?: Protocol) => void } = $props();

  let bar: HTMLDivElement;
  let add: HTMLButtonElement;
  /** Where the choice under the + button shows, while it is open. */
  let choice: { left: number } | null = $state(null);

  function toggle() {
    if (choice) {
      choice = null;
    } else {
      choice = { left: add.getBoundingClientRect().left - bar.getBoundingClientRect().left };
    }
  }

  function pick(kind: SessionKind, protocol?: Protocol) {
    choice = null;
    onnew(kind, protocol);
  }

  function onWindowMouseDown(event: MouseEvent) {
    if (choice && !bar.contains(event.target as Node)) {
      choice = null;
    }
  }
</script>

<svelte:window
  onmousedown={onWindowMouseDown}
  onkeydown={(event) => {
    if (choice && event.key === "Escape") {
      choice = null;
    }
  }}
/>

<div class="bar" bind:this={bar}>
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
        class:shown={view.split !== null &&
          i !== view.active &&
          (i === view.split.first || i === view.split.second)}
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

    <button
      class="add"
      aria-label="New session"
      aria-haspopup="menu"
      aria-expanded={choice !== null}
      title="New session"
      bind:this={add}
      onclick={toggle}
    >
      +
    </button>
  </header>

  {#if choice}
    <div class="choice" role="menu" style:left={`${choice.left}px`}>
      <button role="menuitem" onclick={() => pick("serial")}>
        <strong>Serial port</strong>
        <span>A UART, USB serial adapter or a board's virtual COM port</span>
      </button>
      <button role="menuitem" onclick={() => pick("rtt")}>
        <strong>RTT through a debug probe</strong>
        <span>CMSIS-DAP, J-Link or ST-Link, while the chip runs</span>
      </button>
      <button role="menuitem" onclick={() => pick("tcp", "jlink")}>
        <strong>RTT through J-Link or OpenOCD</strong>
        <span>From the RTT server they open, while a debugger has the probe</span>
      </button>
      <button role="menuitem" onclick={() => pick("swo")}>
        <strong>SWO through a debug probe</strong>
        <span>ITM output on the chip's trace pin, such as printf</span>
      </button>
      <button role="menuitem" onclick={() => pick("tcp")}>
        <strong>TCP or Telnet</strong>
        <span>A device, a serial bridge or a shell on the network</span>
      </button>
      <button role="menuitem" onclick={() => pick("replay")}>
        <strong>A recording</strong>
        <span>Play back what a tab recorded, with its timing</span>
      </button>
    </div>
  {/if}
</div>

<style>
  .bar {
    position: relative;
  }

  .choice {
    position: absolute;
    top: calc(100% + 2px);
    z-index: 10;
    display: flex;
    flex-direction: column;
    width: 290px;
    padding: 4px;
    background: var(--menu-bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: var(--shadow);
  }

  .choice button {
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

  .choice button:hover {
    background: var(--accent-soft);
  }

  .choice strong {
    font-weight: 600;
  }

  .choice span {
    font-size: 12px;
    color: var(--muted);
  }

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

  /* The other tab of a split view: on screen, without the keyboard. */
  .tab.shown {
    color: var(--text);
    background: var(--bg);
    box-shadow: inset 0 2px 0 var(--muted);
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
