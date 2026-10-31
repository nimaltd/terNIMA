<script lang="ts">
  import { untrack } from "svelte";
  import { elfVariables } from "./serial";
  import type { Tab } from "./tabs.svelte";
  import { shortNames, valueText } from "./watch";

  let { tab }: { tab: Tab } = $props();

  /** Each variable's kind, from the ELF file, so its value is written as its kind is. */
  let kinds: Map<string, string> = $state(new Map());
  const names = $derived(shortNames(tab.watch.names));

  // Read again when another file is chosen; the app keeps the file's variables, so this is quick.
  $effect(() => {
    const path = tab.elf ?? "";
    untrack(() => void read(path));
  });

  async function read(path: string) {
    try {
      kinds = path ? new Map((await elfVariables(path)).map((variable) => [variable.name, variable.kind])) : new Map();
    } catch {
      kinds = new Map();
    }
  }
</script>

<!-- The watched variables beside a terminal view, so both are in view: the Watch view has the rest. -->
<aside class="watchpanel" aria-label="Watched variables">
  <header>
    <button class="title" title="Open the Watch view, to add variables or see their min and max" onclick={() => tab.setView("watch")}>
      Watch
    </button>
    <button class="icon" title="Hide: the Watch view keeps them" onclick={() => tab.setWatchBeside(false)}>×</button>
  </header>
  {#if tab.watchProblem}
    <p class="problem">{tab.watchProblem}</p>
  {/if}
  {#if tab.watch.names.length === 0}
    <p class="empty">Add variables in the Watch view.</p>
  {:else}
    <table>
      <tbody>
        {#each tab.watch.names as name, index (name)}
          <tr>
            <td class="name mono" title={name}>{names[index]}</td>
            <td class="value mono">{valueText(tab.watchValues[index], kinds.get(name), tab.watchRange[index])}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</aside>

<style>
  .watchpanel {
    display: flex;
    flex: 0 0 260px;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    padding: 0 4px 0 10px;
    overflow: auto;
    font-size: 12px;
    border-left: 1px solid var(--border);
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding-bottom: 4px;
  }

  .title {
    padding: 2px 4px;
    font: inherit;
    font-weight: 600;
    color: var(--muted);
    background: none;
    border: 0;
    border-radius: 4px;
    cursor: default;
  }

  .title:hover,
  .icon:hover {
    color: var(--text);
    background: var(--hover);
  }

  .icon {
    width: 22px;
    height: 20px;
    font: inherit;
    color: var(--muted);
    background: none;
    border: 0;
    border-radius: 4px;
    cursor: default;
  }

  table {
    width: 100%;
    border-collapse: collapse;
  }

  td {
    padding: 2px 4px;
    font-variant-numeric: tabular-nums;
    border-bottom: 1px solid var(--border);
  }

  .name {
    max-width: 130px;
    overflow: hidden;
    color: var(--muted);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .value {
    font-weight: 600;
    white-space: nowrap;
  }

  .empty,
  .problem {
    margin: 4px;
    color: var(--muted);
  }

  .problem {
    color: var(--bad);
  }
</style>
