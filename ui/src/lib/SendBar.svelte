<script lang="ts">
  import { settings, type Macro, type Sequence } from "./settings.svelte";
  import { keys, shortcutLabel } from "./shortcuts";
  import type { Tab } from "./tabs.svelte";
  import { activeTerminal } from "./terminal-actions";

  let { tab, onmacros }: { tab: Tab; onmacros: () => void } = $props();

  const ENTER_NAMES = { cr: "CR", lf: "LF", crlf: "CR LF" };

  let input: HTMLInputElement;
  let error = $state("");
  /** Where ↑ and ↓ are in the history; null while typing a new line. */
  let position: number | null = null;
  /** The line being typed, and its mode, kept while ↑ shows older ones. */
  let typed = { text: "", hex: false };

  async function send() {
    const { text, hex, enter } = tab.draft;
    if (!text && !enter) {
      return;
    }
    tab.setScrollLock(false);
    try {
      await tab.sendLine(text, hex, enter);
      if (text) {
        tab.remember(text, hex);
      }
      tab.draft.text = "";
      position = null;
      error = "";
    } catch (problem) {
      error = String(problem);
    }
  }

  async function sendMacro(macro: Macro) {
    tab.setScrollLock(false);
    try {
      await tab.sendLine(macro.text, macro.hex, macro.enter);
      error = "";
    } catch (problem) {
      error = `${macro.name}: ${problem}`;
    }
  }

  /** Shows an older line (-1) or a newer one (1) from the history. */
  function browse(step: number) {
    const history = tab.history;
    if (history.length === 0) {
      return;
    }
    if (position === null) {
      typed = { text: tab.draft.text, hex: tab.draft.hex };
      position = history.length;
    }
    const next = Math.min(history.length, Math.max(0, position + step));
    if (next === position) {
      return;
    }
    position = next;
    if (position === history.length) {
      tab.draft.text = typed.text;
      tab.draft.hex = typed.hex;
    } else {
      tab.draft.text = history[position].text;
      tab.draft.hex = history[position].hex ?? false;
    }
    error = "";
    // The cursor goes to the end, as in a shell.
    requestAnimationFrame(() => input.setSelectionRange(input.value.length, input.value.length));
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Enter") {
      event.preventDefault();
      event.stopPropagation();
      void send();
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      browse(-1);
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      browse(1);
    } else if (event.key === "Escape") {
      event.preventDefault();
      activeTerminal()?.focus();
    }
  }

  function setHex(hex: boolean) {
    tab.draft.hex = hex;
    // Hex is usually a whole packet; text is usually a command line.
    tab.draft.enter = !hex;
    error = "";
    input.focus();
  }

  /** "Alt+1" on Windows and Linux, "⌘1" on macOS, for the first nine macros. */
  function macroKey(index: number): string | null {
    return index < 9 ? shortcutLabel(keys(`Alt+${index + 1}`, `Cmd+${index + 1}`)) : null;
  }

  function sequenceTitle(sequence: Sequence): string {
    const times = sequence.repeat === 0 ? "until stopped" : sequence.repeat === 1 ? "once" : `${sequence.repeat} times`;
    return `Sends ${sequence.steps.join(", ")}, ${sequence.delay_ms} ms apart, ${times}. Click again to stop.`;
  }

  /** Where a running sequence is: "3 of 10", or "3" when it goes on until stopped. */
  function progress(running: { round: number; rounds: number }): string {
    return running.rounds === 0 ? String(running.round) : `${running.round} of ${running.rounds}`;
  }

  function macroTitle(macro: Macro, index: number): string {
    const enter = macro.enter ? ` and ${ENTER_NAMES[tab.terminal.enter]}` : "";
    const key = macroKey(index);
    return `Sends ${macro.hex ? "hex " : ""}${macro.text}${enter}${key ? ` (${key})` : ""}`;
  }
</script>

<div class="sendbar">
  <div class="row">
    <select
      class="field mode"
      title="Text, with escapes such as \r, or hex bytes"
      value={tab.draft.hex ? "hex" : "text"}
      onchange={(event) => setHex(event.currentTarget.value === "hex")}
    >
      <option value="text">Text</option>
      <option value="hex">Hex</option>
    </select>
    <input
      class="field mono line"
      class:bad={error !== ""}
      spellcheck="false"
      placeholder={tab.draft.hex
        ? "Hex bytes, such as AA 55 01"
        : "A command; \\r \\n \\t \\e \\xHH for bytes, \\\\ for a backslash"}
      bind:this={input}
      bind:value={tab.draft.text}
      oninput={() => {
        error = "";
        position = null;
      }}
      {onkeydown}
    />
    <label class="check" title={`Send ${ENTER_NAMES[tab.terminal.enter]} after it`}>
      <input type="checkbox" bind:checked={tab.draft.enter} /> Enter
    </label>
    <button class="btn small" disabled={!tab.connected} onclick={() => void send()}>Send</button>
    <button class="btn small" title="Add or change macros" onclick={onmacros}>Macros...</button>
  </div>

  {#if error}
    <p class="error">{error}</p>
  {/if}

  {#if settings.macros.length > 0}
    <div class="row macros">
      {#each settings.macros as macro, index (index)}
        <button
          class="btn small macro"
          disabled={!tab.connected}
          title={macroTitle(macro, index)}
          onclick={() => void sendMacro(macro)}
        >
          {#if index < 9}
            <span class="key">{index + 1}</span>
          {/if}
          {macro.name}
        </button>
      {/each}
      {#each settings.sequences as sequence, index (index)}
        {@const running = tab.sequence?.name === sequence.name ? tab.sequence : null}
        <button
          class="btn small macro sequence"
          class:running={running !== null}
          disabled={!tab.connected || (tab.sequence !== null && running === null)}
          title={running ? "Stop it" : sequenceTitle(sequence)}
          onclick={() => (running ? tab.stopSequence() : void tab.runSequence(sequence))}
        >
          <span class="key">{running ? "■" : "▶"}</span>
          {sequence.name}
          {#if running}
            <span class="key">{progress(running)}</span>
          {/if}
        </button>
      {/each}
    </div>
  {/if}
</div>

<style>
  .sendbar {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 6px 10px;
    background: var(--bg);
    border-top: 1px solid var(--border);
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .mode {
    height: 26px;
    padding: 0 4px;
  }

  .line {
    flex: 1;
    min-width: 60px;
    height: 26px;
    font-size: 13px;
  }

  .line.bad {
    border-color: var(--bad);
  }

  .check {
    display: flex;
    align-items: center;
    gap: 5px;
    color: var(--muted);
    white-space: nowrap;
  }

  .check:has(input:checked) {
    color: var(--text);
  }

  .small {
    height: 26px;
    padding: 0 10px;
    font-size: 12px;
    white-space: nowrap;
  }

  .error {
    margin: 0;
    font-size: 12px;
    color: var(--bad);
  }

  .macros {
    flex-wrap: wrap;
    gap: 6px;
  }

  .macro {
    gap: 6px;
  }

  .sequence.running {
    border-color: var(--accent);
    background: var(--accent-soft);
  }

  .key {
    font-size: 10px;
    color: var(--muted);
  }
</style>
