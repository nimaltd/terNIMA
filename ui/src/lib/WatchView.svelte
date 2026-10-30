<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import { writeText } from "@tauri-apps/plugin-clipboard-manager";
  import { open } from "@tauri-apps/plugin-dialog";
  import { elfVariables, type Variable } from "./serial";
  import { fileName, type Tab } from "./tabs.svelte";
  import { clearActiveTerminal, setActiveTerminal, type TerminalActions } from "./terminal-actions";

  let {
    tab,
    active,
  }: {
    tab: Tab;
    /** Whether this view has the keyboard. */
    active: boolean;
  } = $props();

  /** How often the variables can be read, in ms. */
  const EVERY = [10, 20, 50, 100, 200, 500, 1000];
  /** The most variables the list shows at once. */
  const LISTED_MAX = 500;

  let search: HTMLInputElement;
  /** The search box and its list. */
  let box: HTMLDivElement;
  let list: HTMLDivElement | undefined = $state();
  /** Whether the list of variables is open under the search box. */
  let listing = $state(false);
  /** The ELF file's variables once read, and why they could not be. */
  let variables: Variable[] = $state([]);
  let reading = $state(false);
  let problem: string | null = $state(null);
  let query = $state("");
  /** The match the arrow keys are on. */
  let picked = $state(0);

  const byName = $derived(new Map(variables.map((variable) => [variable.name, variable])));
  /**
   * The variables the list has: those whose names have what is typed in
   * them, or all of them while nothing is, those watched already left out.
   * The first LISTED_MAX of them, and how many there are in all.
   */
  const listed = $derived.by(() => {
    const typed = query.trim().toLowerCase();
    const watched = new Set(tab.watch.names);
    const shown: Variable[] = [];
    let count = 0;
    for (const variable of variables) {
      if (watched.has(variable.name) || (typed && !variable.name.toLowerCase().includes(typed))) continue;
      count += 1;
      if (shown.length < LISTED_MAX) shown.push(variable);
    }
    return { shown, count };
  });
  const matches = $derived(listed.shown);
  /** A line under the list, where it does not have them all. */
  const listNote = $derived.by(() => {
    const typed = query.trim();
    if (listed.count === 0) return typed ? `No variable has “${typed}” in its name` : "They are all watched";
    if (listed.count > matches.length) {
      return `The first ${matches.length.toLocaleString()} of ${listed.count.toLocaleString()}: type part of a name for the others`;
    }
    return "";
  });

  // The file's variables, read again when another file is chosen.
  $effect(() => {
    const path = tab.rtt.elf ?? "";
    untrack(() => void read(path));
  });

  async function read(path: string) {
    if (!path) {
      variables = [];
      problem = null;
      return;
    }
    reading = true;
    try {
      variables = await elfVariables(path);
      problem = variables.length === 0 ? "This file has no variables: was it built with debug information (-g)?" : null;
    } catch (error) {
      variables = [];
      problem = String(error);
    } finally {
      reading = false;
    }
  }

  async function chooseElf() {
    const path = await open({
      title: "The firmware's ELF file",
      multiple: false,
      directory: false,
      defaultPath: tab.rtt.elf || undefined,
      filters: [
        { name: "ELF files", extensions: ["elf", "axf", "out"] },
        { name: "All files", extensions: ["*"] },
      ],
    });
    if (typeof path === "string") {
      tab.reconfigureRtt({ ...tab.rtt, elf: path });
    }
    search?.focus();
  }

  /** Reads the file again, as after the firmware was built anew, and finds the variables in it again. */
  function reload() {
    void read(tab.rtt.elf ?? "");
    tab.sendWatch();
  }

  function add(variable: Variable) {
    tab.addWatch(variable.name);
    query = "";
    picked = 0;
    listing = false;
    search.focus();
  }

  function openList() {
    listing = true;
    picked = 0;
  }

  function onSearchKey(event: KeyboardEvent) {
    if (event.key === "ArrowDown" || event.key === "ArrowUp" || event.key === "Enter") {
      event.preventDefault();
      // Closed, any of them opens the list.
      if (!listing) {
        openList();
        return;
      }
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      const step = event.key === "ArrowDown" ? 1 : -1;
      picked = Math.max(0, Math.min(matches.length - 1, picked + step));
      void tick().then(() => list?.querySelector(".picked")?.scrollIntoView({ block: "nearest" }));
    } else if (event.key === "Enter") {
      const variable = matches[picked];
      if (variable) add(variable);
    } else if (event.key === "Escape") {
      query = "";
      listing = false;
    }
  }

  /** A click anywhere but the search box and its list closes the list. */
  function onWindowDown(event: MouseEvent) {
    if (listing && !box.contains(event.target as Node)) listing = false;
  }

  /** As does the keyboard going elsewhere. */
  function onFocusOut(event: FocusEvent) {
    const to = event.relatedTarget;
    if (to instanceof Node && !box.contains(to)) listing = false;
  }

  /**
   * A value as its kind is written: true or false, a whole number, or a
   * float with the decimals its row keeps. A space stands where a minus sign
   * would be, so the digits stay where they are as values come.
   */
  function shown(value: number | null | undefined, variable: Variable | undefined, index: number): string {
    if (value === null || value === undefined) return "";
    if (variable?.kind === "bool") return value ? "true" : "false";
    const float = variable?.kind === "f32" || variable?.kind === "f64";
    const text = float ? value.toFixed(decimals(index, value)) : String(value);
    return text.startsWith("-") ? text : `\u00a0${text}`;
  }

  /** A float's decimals: about seven digits in all, from the largest it has been, so they do not change as it moves. */
  function decimals(index: number, value: number): number {
    const range = tab.watchRange[index];
    const largest = Math.max(Math.abs(value), range ? Math.max(Math.abs(range.min), Math.abs(range.max)) : 0);
    const whole = largest >= 1 ? Math.floor(Math.log10(largest)) + 1 : 1;
    return Math.max(0, Math.min(6, 7 - whole));
  }

  function hex(address: number): string {
    return `0x${address.toString(16).padStart(8, "0")}`;
  }

  /** The table as text, one variable a line, for copying and saving. */
  function text(): string {
    return tab.watch.names
      .map((name, index) => {
        const variable = byName.get(name);
        const value = shown(tab.watchValues[index], variable, index).trim();
        return [name, variable?.type_name ?? "", variable ? hex(variable.address) : "", value].join("\t");
      })
      .join("\n");
  }

  const actions: TerminalActions = {
    hasSelection: () => tab.watch.names.length > 0,
    copy: async () => {
      const table = text();
      if (table) await writeText(table);
    },
    paste: async () => {},
    selectAll: () => {},
    clear: () => tab.resetWatchRange(),
    text: () => (tab.watch.names.length > 0 ? `${text()}\n` : ""),
    focus: () => search?.focus(),
    openSearch: () => search?.focus(),
    findNext: () => {},
    findPrevious: () => {},
  };

  // The view on screen is the one the menus act on, and has the keyboard.
  $effect(() => {
    if (active) {
      setActiveTerminal(actions);
      search?.focus();
    }
  });

  onMount(() => () => clearActiveTerminal(actions));
</script>

<svelte:window onmousedown={onWindowDown} />

<div class="watchview">
  <div class="bar">
    <span class="file mono" title={tab.rtt.elf || "No ELF file chosen"}>{tab.rtt.elf ? fileName(tab.rtt.elf) : "No ELF file"}</span>
    <button class="btn small" onclick={chooseElf}>{tab.rtt.elf ? "Another ELF..." : "Choose the ELF..."}</button>
    {#if tab.rtt.elf}
      <button class="btn small" title="Read the ELF file again, after the firmware was built anew" onclick={reload}>Reload</button>
    {/if}

    <div class="add" bind:this={box} onfocusout={onFocusOut}>
      <input
        class="field mono"
        placeholder={variables.length > 0 ? `Add a variable: click for all ${variables.length.toLocaleString()}, or type part of a name` : "Add a variable"}
        spellcheck="false"
        disabled={variables.length === 0}
        aria-expanded={listing}
        bind:this={search}
        bind:value={query}
        oninput={openList}
        onclick={() => {
          if (!listing) openList();
        }}
        onkeydown={onSearchKey}
      />
      <button
        class="open"
        tabindex="-1"
        title="All the variables in the ELF file"
        aria-label="All the variables in the ELF file"
        disabled={variables.length === 0}
        onmousedown={(event) => event.preventDefault()}
        onclick={() => {
          if (listing) listing = false;
          else openList();
          search.focus();
        }}
      >
        ▾
      </button>
      {#if listing && variables.length > 0}
        <div class="dropdown">
          {#if matches.length > 0}
            <!-- Not a Tab stop either, as Chromium makes a list that scrolls. -->
            <div class="matches" role="listbox" aria-label="Variables in the ELF file" tabindex="-1" bind:this={list}>
              {#each matches as variable, index (variable.name)}
                <!-- The arrow keys go through them from the box, so Tab goes past them. -->
                <button
                  class="match"
                  class:picked={index === picked}
                  tabindex="-1"
                  role="option"
                  aria-selected={index === picked}
                  onmouseenter={() => (picked = index)}
                  onmousedown={(event) => event.preventDefault()}
                  onclick={() => add(variable)}
                >
                  <span class="mono name" title={variable.name}>{variable.name}</span>
                  <span class="muted">{variable.type_name}</span>
                  <span class="mono muted">{hex(variable.address)}</span>
                </button>
              {/each}
            </div>
          {/if}
          {#if listNote}
            <p class="note">{listNote}</p>
          {/if}
        </div>
      {/if}
    </div>

    <label class="option" title="How often the variables are read from the chip">
      Every
      <select class="field" value={String(tab.watch.every_ms)} onchange={(event) => tab.setWatchEvery(Number(event.currentTarget.value))}>
        {#each [...new Set([...EVERY, tab.watch.every_ms])].sort((a, b) => a - b) as ms (ms)}
          <option value={String(ms)}>{ms < 1000 ? `${ms} ms` : `${ms / 1000} s`}</option>
        {/each}
      </select>
    </label>
    <button class="btn small" disabled={tab.watch.names.length === 0} title="Start the smallest and largest values again" onclick={() => tab.resetWatchRange()}>
      Reset min/max
    </button>
    <button
      class="btn small"
      disabled={tab.watch.names.length === 0}
      title="Draw them over time in the Plot view"
      onclick={() => {
        tab.setPlotView({ ...tab.plotView, mode: "watch" });
        tab.setView("plot");
      }}
    >
      Plot them
    </button>
  </div>

  {#if problem ?? tab.watchProblem}
    <p class="problem">{problem ?? tab.watchProblem}</p>
  {/if}

  <div class="table">
    {#if tab.watch.names.length > 0}
      <table>
        <thead>
          <tr>
            <th class="name">Variable</th>
            <th class="type">Type</th>
            <th class="address">Address</th>
            <th>Value</th>
            <th>Min</th>
            <th>Max</th>
            <th class="remove"></th>
          </tr>
        </thead>
        <tbody>
          {#each tab.watch.names as name, index (name)}
            {@const variable = byName.get(name)}
            {@const missing = tab.watchFound.length > 0 && tab.watchFound[index] === false}
            {@const range = tab.watchRange[index]}
            <tr class:missing>
              <td class="name mono" title={name}>{name}</td>
              <td class="type">{variable?.type_name ?? ""}</td>
              <td class="address mono">{variable ? hex(variable.address) : ""}</td>
              <td class="mono value">{missing ? "not in the ELF" : shown(tab.watchValues[index], variable, index)}</td>
              <td class="mono">{range ? shown(range.min, variable, index) : ""}</td>
              <td class="mono">{range ? shown(range.max, variable, index) : ""}</td>
              <td class="remove">
                <button class="icon" title="Stop watching it" onclick={() => tab.removeWatch(name)}>×</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {:else}
      <div class="empty">
        {#if !tab.rtt.elf}
          <p>Choose the firmware's ELF file, built with debug information (-g), to watch its variables.</p>
          <button class="btn" onclick={chooseElf}>Choose the ELF file...</button>
        {:else if reading}
          <p>Reading the ELF file...</p>
        {:else if variables.length > 0}
          <p>
            Click the box above for the variables in the file, or type part of a name to find one: globals and
            statics, a struct's members as <code>sensor.temp</code>, an array's elements as <code>buf[2]</code>.
          </p>
          <p class="muted">
            They are read through the probe as the chip runs, without stopping it, as often as Every says, and Plot
            them draws them over time.
          </p>
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .watchview {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }

  .bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 10px;
    padding-bottom: 8px;
    font-size: 12px;
    color: var(--muted);
  }

  .file {
    max-width: 220px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text);
  }

  .small {
    height: 24px;
    padding: 0 8px;
    font-size: 12px;
  }

  .option {
    display: flex;
    align-items: center;
    gap: 5px;
  }

  .option select {
    height: 24px;
    padding: 0 4px;
  }

  .add {
    position: relative;
    flex: 1;
    min-width: 240px;
    max-width: 520px;
  }

  .add .field {
    box-sizing: border-box;
    width: 100%;
    height: 26px;
    padding-right: 26px;
    font-size: 13px;
  }

  /* The ▾ at the box's right end, as on a drop-down list. */
  .open {
    position: absolute;
    top: 1px;
    right: 1px;
    width: 24px;
    height: 24px;
    padding: 0;
    font: inherit;
    font-size: 12px;
    color: var(--muted);
    background: none;
    border: 0;
    border-radius: 4px;
    cursor: default;
  }

  .open:hover:not(:disabled) {
    color: var(--text);
    background: var(--hover);
  }

  .dropdown {
    position: absolute;
    top: calc(100% + 2px);
    left: 0;
    right: 0;
    z-index: 10;
    overflow: hidden;
    background: var(--menu-bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: var(--shadow);
  }

  .matches {
    max-height: min(420px, 60vh);
    padding: 4px;
    overflow-y: auto;
  }

  .note {
    margin: 0;
    padding: 6px 12px;
    font-size: 12px;
    color: var(--muted);
  }

  .matches + .note {
    border-top: 1px solid var(--border);
  }

  .match {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto auto;
    gap: 12px;
    align-items: center;
    width: 100%;
    height: 26px;
    padding: 0 8px;
    font: inherit;
    font-size: 12px;
    color: var(--text);
    text-align: left;
    background: none;
    border: 0;
    border-radius: 4px;
    cursor: default;
  }

  .match.picked {
    background: var(--accent-soft);
  }

  .match .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .muted {
    color: var(--muted);
  }

  .problem {
    margin: 0 0 8px;
    font-size: 12px;
    color: var(--bad);
  }

  .table {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }

  table {
    width: 100%;
    font-size: 13px;
    border-collapse: collapse;
  }

  /* Left aligned, with fixed decimals: each value's digits stay where they are as it changes. */
  th {
    position: sticky;
    top: 0;
    padding: 4px 8px;
    font-size: 12px;
    font-weight: 500;
    color: var(--muted);
    text-align: left;
    background: var(--bg);
    border-bottom: 1px solid var(--border);
  }

  td {
    padding: 3px 8px;
    text-align: left;
    font-variant-numeric: tabular-nums;
    border-bottom: 1px solid var(--border);
  }

  td.name {
    max-width: 320px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  td.type,
  td.address {
    color: var(--muted);
  }

  td.value {
    font-weight: 600;
  }

  tr.missing td {
    color: var(--muted);
  }

  tr.missing td.value {
    font-weight: 400;
    color: var(--bad);
  }

  th.remove,
  td.remove {
    width: 32px;
  }

  .icon {
    width: 24px;
    height: 22px;
    font: inherit;
    color: var(--muted);
    background: none;
    border: 0;
    border-radius: 4px;
    cursor: default;
  }

  .icon:hover {
    color: var(--text);
    background: var(--hover);
  }

  .empty {
    max-width: 640px;
    padding: 24px 4px;
    font-size: 13px;
    line-height: 1.6;
  }

  .empty p {
    margin: 0 0 10px;
  }
</style>
