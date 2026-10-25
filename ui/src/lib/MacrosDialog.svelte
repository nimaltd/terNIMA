<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import Modal from "./Modal.svelte";
  import { settings, updateSettings, type Macro } from "./settings.svelte";
  import { keys, shortcutLabel } from "./shortcuts";

  let { onclose }: { onclose: () => void } = $props();

  // A copy of the macros: nothing changes until Save.
  let macros: Macro[] = $state(settings.macros.map((macro) => ({ ...macro })));
  /** What is wrong with each macro, found on Save. */
  let problems: (string | null)[] = $state([]);

  const first = shortcutLabel(keys("Alt+1", "Cmd+1"));
  const ninth = shortcutLabel(keys("Alt+9", "Cmd+9"));

  function add() {
    macros.push({ name: `Macro ${macros.length + 1}`, text: "", hex: false, enter: true });
  }

  function remove(index: number) {
    macros.splice(index, 1);
    problems.splice(index, 1);
  }

  function move(index: number, step: number) {
    const other = index + step;
    if (other < 0 || other >= macros.length) {
      return;
    }
    [macros[index], macros[other]] = [macros[other], macros[index]];
    [problems[index], problems[other]] = [problems[other], problems[index]];
  }

  /** Checks every macro the way it will be sent, and keeps them if all are right. */
  async function submit() {
    problems = await Promise.all(
      macros.map(async (macro) => {
        if (!macro.name.trim()) {
          return "Give it a name.";
        }
        try {
          await invoke("encode_input", { text: macro.text, hex: macro.hex });
          return null;
        } catch (problem) {
          return String(problem);
        }
      }),
    );
    if (problems.some((problem) => problem !== null)) {
      return;
    }
    updateSettings({ macros: macros.map((macro) => ({ ...macro, name: macro.name.trim() })) });
    onclose();
  }
</script>

<Modal title="Macros" {onclose} onsubmit={submit}>
  <p class="dlg-hint">
    A macro sends its text with one click under the terminal, or with {first} to {ninth} for the first
    nine. Text can hold \r \n \t \e and \xHH for bytes; hex is bytes such as AA 55 01.
  </p>

  <div class="list">
    {#each macros as macro, index (index)}
      <div class="macro">
        <span class="key">{index < 9 ? index + 1 : ""}</span>
        <input class="field name" placeholder="Name" bind:value={macro.name} />
        <select
          class="field mode"
          value={macro.hex ? "hex" : "text"}
          onchange={(event) => {
            macro.hex = event.currentTarget.value === "hex";
            // Hex is usually a whole packet; text is usually a command line.
            macro.enter = !macro.hex;
          }}
        >
          <option value="text">Text</option>
          <option value="hex">Hex</option>
        </select>
        <input
          class="field mono text"
          spellcheck="false"
          placeholder={macro.hex ? "AA 55 01" : "kernel reboot cold"}
          bind:value={macro.text}
        />
        <label class="dlg-check" title="Send the tab's Enter after it">
          <input type="checkbox" bind:checked={macro.enter} /> Enter
        </label>
        <span class="tools">
          <button class="icon" title="Move up" disabled={index === 0} onclick={() => move(index, -1)}>↑</button>
          <button
            class="icon"
            title="Move down"
            disabled={index === macros.length - 1}
            onclick={() => move(index, 1)}
          >
            ↓
          </button>
          <button class="icon" title="Remove" onclick={() => remove(index)}>×</button>
        </span>
      </div>
      {#if problems[index]}
        <p class="dlg-error problem">{problems[index]}</p>
      {/if}
    {:else}
      <p class="dlg-empty">No macros yet. Add one for a command you send often.</p>
    {/each}
  </div>

  <button class="btn add" onclick={add}>Add a macro</button>

  {#snippet buttons()}
    <button class="btn" onclick={onclose}>Cancel</button>
    <button class="btn primary" onclick={() => void submit()}>Save</button>
  {/snippet}
</Modal>

<style>
  .list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-height: 340px;
    margin-top: 12px;
    overflow-y: auto;
  }

  .macro {
    display: grid;
    grid-template-columns: 16px 130px 72px minmax(0, 1fr) auto auto;
    gap: 8px;
    align-items: center;
  }

  .key {
    font-size: 11px;
    color: var(--muted);
    text-align: right;
  }

  .field {
    height: 28px;
    font-size: 13px;
  }

  .tools {
    display: flex;
    gap: 2px;
  }

  .icon {
    width: 24px;
    height: 24px;
    font: inherit;
    color: var(--muted);
    background: none;
    border: 0;
    border-radius: 4px;
    cursor: default;
  }

  .icon:hover:not(:disabled) {
    color: var(--text);
    background: var(--hover);
  }

  .icon:disabled {
    opacity: 0.35;
  }

  .problem {
    margin: 0 0 0 24px;
    font-size: 12px;
  }

  .add {
    margin-top: 12px;
  }
</style>
