<script lang="ts">
  import { untrack } from "svelte";
  import type { Tab } from "./tabs.svelte";
  import { activeTerminal } from "./terminal-actions";

  let { tab }: { tab: Tab } = $props();

  const WIDTHS = [8, 16, 32];

  const endOf = (byte: number | null | undefined) =>
    byte === null || byte === undefined ? "" : byte.toString(16).toUpperCase().padStart(2, "0");
  let endText = $state(untrack(() => endOf(tab.hexView.end_byte)));
  let endBad = $state(false);

  /** Takes the end byte as typed: one or two hex digits, or empty for none. */
  function setEnd() {
    const text = endText.trim().replace(/^0x/i, "");
    if (text === "") {
      endBad = false;
      tab.setHexView({ ...tab.hexView, end_byte: null });
      return;
    }
    if (!/^[0-9a-f]{1,2}$/i.test(text)) {
      endBad = true;
      return;
    }
    endBad = false;
    endText = text.toUpperCase().padStart(2, "0");
    tab.setHexView({ ...tab.hexView, end_byte: parseInt(text, 16) });
  }
</script>

<div class="hexbar">
  <label class="option">
    Bytes a line
    <select
      class="field"
      value={String(tab.hexView.width)}
      onchange={(event) => tab.setHexView({ ...tab.hexView, width: Number(event.currentTarget.value) })}
    >
      {#each WIDTHS as width (width)}
        <option value={String(width)}>{width}</option>
      {/each}
    </select>
  </label>

  <label
    class="option"
    title="A byte that ends its line, in hex: 0A for LF, 0D for CR, or a frame's end byte. Empty for none."
  >
    New line after the byte
    <input
      class="field mono end"
      class:bad={endBad}
      placeholder="none"
      maxlength="4"
      spellcheck="false"
      bind:value={endText}
      onchange={setEnd}
      onkeydown={(event) => {
        if (event.key === "Enter") {
          setEnd();
          activeTerminal()?.focus();
        }
      }}
    />
  </label>

  <span class="note">A new line also starts after the pause set in the toolbar. Only the layout changes.</span>
</div>

<style>
  .hexbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 16px;
    padding-bottom: 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .option {
    display: flex;
    align-items: center;
    gap: 6px;
    white-space: nowrap;
  }

  .option select {
    height: 24px;
    padding: 0 4px;
  }

  .end {
    width: 52px;
    height: 24px;
    font-size: 12px;
    text-align: center;
  }

  .end.bad {
    border-color: var(--bad);
  }

  .note {
    opacity: 0.8;
  }
</style>
