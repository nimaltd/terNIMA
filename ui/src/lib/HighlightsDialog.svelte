<script lang="ts">
  import Modal from "./Modal.svelte";
  import { HIGHLIGHT_COLORS, settings, updateSettings, type Highlight } from "./settings.svelte";

  let { onclose }: { onclose: () => void } = $props();

  // A copy of the rules: nothing changes until Save.
  let rules: Highlight[] = $state(settings.highlights.map((rule) => ({ ...rule })));
  /** What is wrong with each rule, found on Save. */
  let problems: (string | null)[] = $state([]);

  function add() {
    rules.push({ text: "", regex: false, case: false, color: HIGHLIGHT_COLORS[rules.length % HIGHLIGHT_COLORS.length] });
  }

  function remove(index: number) {
    rules.splice(index, 1);
    problems.splice(index, 1);
  }

  function move(index: number, step: number) {
    const other = index + step;
    if (other < 0 || other >= rules.length) {
      return;
    }
    [rules[index], rules[other]] = [rules[other], rules[index]];
    [problems[index], problems[other]] = [problems[other], problems[index]];
  }

  function submit() {
    problems = rules.map((rule) => {
      if (!rule.text) {
        return "Enter the text to look for.";
      }
      if (rule.regex) {
        try {
          new RegExp(rule.text);
        } catch (error) {
          return `Not a regular expression: ${(error as Error).message}`;
        }
      }
      return null;
    });
    if (problems.some((problem) => problem !== null)) {
      return;
    }
    updateSettings({ highlights: rules.map((rule) => ({ ...rule })) });
    onclose();
  }
</script>

<Modal title="Highlight rules" {onclose} onsubmit={submit}>
  <p class="dlg-hint">
    In the Zephyr log view, a line with the text gets the rule's color. The first rule that matches
    wins, so put the most important ones first.
  </p>

  <div class="list">
    {#each rules as rule, index (index)}
      <div class="rule">
        <span class="swatch {rule.color}"></span>
        <input
          class="field mono text"
          spellcheck="false"
          placeholder={rule.regex ? "timeout|fail(ed)?" : "error"}
          bind:value={rule.text}
        />
        <label class="dlg-check" title="The text is a regular expression">
          <input type="checkbox" bind:checked={rule.regex} /> Regex
        </label>
        <label class="dlg-check" title="Upper and lower case must match">
          <input type="checkbox" bind:checked={rule.case} /> Case
        </label>
        <select class="field color" bind:value={rule.color}>
          {#each HIGHLIGHT_COLORS as color (color)}
            <option value={color}>{color[0].toUpperCase() + color.slice(1)}</option>
          {/each}
        </select>
        <span class="tools">
          <button class="icon" title="Move up" disabled={index === 0} onclick={() => move(index, -1)}>↑</button>
          <button class="icon" title="Move down" disabled={index === rules.length - 1} onclick={() => move(index, 1)}>
            ↓
          </button>
          <button class="icon" title="Remove" onclick={() => remove(index)}>×</button>
        </span>
      </div>
      {#if problems[index]}
        <p class="dlg-error problem">{problems[index]}</p>
      {/if}
    {:else}
      <p class="dlg-empty">No rules yet. Add one to mark errors, timeouts, or a module's lines.</p>
    {/each}
  </div>

  <button class="btn add" onclick={add}>Add a rule</button>

  {#snippet buttons()}
    <button class="btn" onclick={onclose}>Cancel</button>
    <button class="btn primary" onclick={submit}>Save</button>
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

  .rule {
    display: grid;
    grid-template-columns: 12px minmax(0, 1fr) auto auto 96px auto;
    gap: 8px;
    align-items: center;
  }

  .swatch {
    width: 12px;
    height: 12px;
    border-radius: 3px;
  }

  .swatch.red {
    background: rgb(239 68 68);
  }

  .swatch.orange {
    background: rgb(249 115 22);
  }

  .swatch.yellow {
    background: rgb(234 179 8);
  }

  .swatch.green {
    background: rgb(34 197 94);
  }

  .swatch.blue {
    background: rgb(59 130 246);
  }

  .swatch.purple {
    background: rgb(168 85 247);
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
    margin: 0 0 0 20px;
    font-size: 12px;
  }

  .add {
    margin-top: 12px;
  }
</style>
