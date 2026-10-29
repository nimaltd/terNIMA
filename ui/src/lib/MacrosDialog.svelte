<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import Modal from "./Modal.svelte";
  import { settings, updateSettings, type Macro } from "./settings.svelte";
  import { keys, shortcutLabel } from "./shortcuts";

  let { onclose }: { onclose: () => void } = $props();

  /** A sequence as the dialog changes it: its steps are the dialog's own macros, so a renamed one stays in. */
  type SequenceDraft = { name: string; steps: Macro[]; delay_ms: number; repeat: number };

  // A copy of the macros and the sequences: nothing changes until Save.
  let macros: Macro[] = $state(settings.macros.map((macro) => ({ ...macro })));
  let sequences: SequenceDraft[] = $state(
    settings.sequences.map((sequence) => ({
      ...sequence,
      steps: sequence.steps.flatMap((name) => macros.find((macro) => macro.name === name) ?? []),
    })),
  );
  /** What is wrong with each macro and each sequence, found on Save. */
  let problems: (string | null)[] = $state([]);
  let sequenceProblems: (string | null)[] = $state([]);

  const first = shortcutLabel(keys("Alt+1", "Cmd+1"));
  const ninth = shortcutLabel(keys("Alt+9", "Cmd+9"));

  function add() {
    macros.push({ name: `Macro ${macros.length + 1}`, text: "", hex: false, enter: true });
  }

  function remove(index: number) {
    const [gone] = macros.splice(index, 1);
    problems.splice(index, 1);
    // The sequences that send it go on without it.
    for (const sequence of sequences) {
      sequence.steps = sequence.steps.filter((step) => step !== gone);
    }
  }

  function move(index: number, step: number) {
    const other = index + step;
    if (other < 0 || other >= macros.length) {
      return;
    }
    [macros[index], macros[other]] = [macros[other], macros[index]];
    [problems[index], problems[other]] = [problems[other], problems[index]];
  }

  function addSequence() {
    sequences.push({ name: `Sequence ${sequences.length + 1}`, steps: [], delay_ms: 500, repeat: 1 });
  }

  function removeSequence(index: number) {
    sequences.splice(index, 1);
    sequenceProblems.splice(index, 1);
  }

  /** Checks every macro the way it will be sent, and every sequence, and keeps them if all are right. */
  async function submit() {
    // Sequences find their macros by name, so each has its own.
    const names = macros.map((macro) => macro.name.trim());
    problems = await Promise.all(
      macros.map(async (macro, index) => {
        if (!names[index]) {
          return "Give it a name.";
        }
        if (names.indexOf(names[index]) !== index) {
          return "Another macro has this name.";
        }
        try {
          await invoke("encode_input", { text: macro.text, hex: macro.hex });
          return null;
        } catch (problem) {
          return String(problem);
        }
      }),
    );
    const sequenceNames = sequences.map((sequence) => sequence.name.trim());
    sequenceProblems = sequences.map((sequence, index) => {
      if (!sequenceNames[index]) {
        return "Give it a name.";
      }
      if (sequenceNames.indexOf(sequenceNames[index]) !== index) {
        return "Another sequence has this name.";
      }
      if (sequence.steps.length === 0) {
        return "Add the macros it sends.";
      }
      if (!Number.isInteger(sequence.delay_ms) || sequence.delay_ms < 0) {
        return "The wait is a whole number of ms, 0 or more.";
      }
      if (!Number.isInteger(sequence.repeat) || sequence.repeat < 0) {
        return "The times are a whole number, or 0 to go on until you stop it.";
      }
      return null;
    });
    if ([...problems, ...sequenceProblems].some((problem) => problem !== null)) {
      return;
    }
    updateSettings({
      macros: macros.map((macro, index) => ({ ...macro, name: names[index] })),
      sequences: sequences.map((sequence, index) => ({
        name: sequenceNames[index],
        steps: sequence.steps.map((step) => step.name.trim()),
        delay_ms: sequence.delay_ms,
        repeat: sequence.repeat,
      })),
    });
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

  <h2 class="dlg-section">Sequences</h2>
  <p class="dlg-hint">
    A sequence sends macros one after another, with a wait after each: a number of times, or with 0, until you
    stop it. Its button under the terminal runs it, and stops it.
  </p>

  <div class="list sequences">
    {#each sequences as sequence, index (index)}
      <div class="sequence">
        <div class="head">
          <input class="field name" placeholder="Name" bind:value={sequence.name} />
          <label class="inline" title="How long it waits after each macro">
            Wait
            <input class="field mono number" type="number" min="0" step="100" bind:value={sequence.delay_ms} />
            ms
          </label>
          <label class="inline" title="0 goes on until you stop it">
            <input class="field mono number" type="number" min="0" bind:value={sequence.repeat} />
            times
          </label>
          <button class="icon" title="Remove" onclick={() => removeSequence(index)}>×</button>
        </div>
        <div class="steps">
          {#each sequence.steps as step, at (at)}
            <span class="step">
              {step.name}
              <button class="icon tiny" title="Take it out" onclick={() => sequence.steps.splice(at, 1)}>×</button>
            </span>
          {/each}
          <select
            class="field add-step"
            aria-label="Add a macro to the sequence"
            onchange={(event) => {
              const macro = macros[Number(event.currentTarget.value)];
              if (macro) {
                sequence.steps.push(macro);
              }
              event.currentTarget.selectedIndex = 0;
            }}
          >
            <option value="" selected disabled>+ Macro</option>
            {#each macros as macro, at (at)}
              <option value={String(at)}>{macro.name}</option>
            {/each}
          </select>
        </div>
      </div>
      {#if sequenceProblems[index]}
        <p class="dlg-error problem">{sequenceProblems[index]}</p>
      {/if}
    {:else}
      <p class="dlg-empty">No sequences yet. Add one to send macros one after another, again and again.</p>
    {/each}
  </div>

  <button class="btn add" disabled={macros.length === 0} onclick={addSequence}>Add a sequence</button>

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

  .list.sequences {
    max-height: 260px;
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

  .icon.tiny {
    width: 18px;
    height: 18px;
    font-size: 12px;
  }

  .problem {
    margin: 0 0 0 24px;
    font-size: 12px;
  }

  .add {
    margin-top: 12px;
  }

  .sequence {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  .head {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto auto auto;
    gap: 12px;
    align-items: center;
  }

  .inline {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--muted);
    white-space: nowrap;
  }

  .number {
    width: 80px;
  }

  .steps {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
  }

  .step {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    height: 24px;
    padding: 0 2px 0 9px;
    font-size: 12px;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 12px;
  }

  .field.add-step {
    height: 24px;
    padding: 0 4px;
    font-size: 12px;
  }
</style>
