<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import { writeText } from "@tauri-apps/plugin-clipboard-manager";
  import { open } from "@tauri-apps/plugin-dialog";
  import { elfInterrupts, elfVariables, type Variable } from "./serial";
  import { fileName, type Tab } from "./tabs.svelte";
  import { clearActiveTerminal, setActiveTerminal, type TerminalActions } from "./terminal-actions";
  import { valueText } from "./watch";
  import {
    KERNEL_NAMES,
    STATES,
    bytes,
    cpuLoad,
    isIdle,
    stackFillNote,
    stackMost,
    stackNow,
    stackPeak,
    type RtosTask,
  } from "./rtos";
  import TimelineView from "./TimelineView.svelte";

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

  // The file's variables, read again when another file is chosen, at each
  // connection, and when it is built anew while connected.
  $effect(() => {
    const path = tab.elf ?? "";
    void tab.elfRead;
    untrack(() => void read(path));
  });

  async function read(path: string) {
    void nameInterrupts(path);
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

  /** The timeline names the interrupts as the firmware does: TIM7, not IRQ 50. */
  async function nameInterrupts(path: string) {
    let names: [number, string][] = [];
    try {
      names = path ? await elfInterrupts(path) : [];
    } catch {
      // Without them, they are named by number.
    }
    tab.timeline.interruptNames = new Map(names);
    tab.timelineVersion++;
  }

  async function chooseElf() {
    const path = await open({
      title: "The firmware's ELF file",
      multiple: false,
      directory: false,
      defaultPath: tab.elf || undefined,
      filters: [
        { name: "ELF files", extensions: ["elf", "axf", "out"] },
        { name: "All files", extensions: ["*"] },
      ],
    });
    if (typeof path === "string") {
      tab.setElf(path);
    }
    search?.focus();
  }

  /** Reads the file again, as after the firmware was built anew, and finds the variables in it again. */
  function reload() {
    void read(tab.elf ?? "");
    tab.sendWatch();
    tab.sendTasks();
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

  /** A value of a row as its kind is written, with the decimals the row keeps for its range. */
  function shown(value: number | null | undefined, variable: Variable | undefined, range: Range | null | undefined): string {
    return valueText(value, variable?.kind, range);
  }

  type Range = { min: number; max: number };

  /** A watched variable's value, smallest and largest: as they were when the timeline was paused, while it is. */
  function watched(name: string, index: number): { value: number | null | undefined; range: Range | null | undefined } {
    const frozen = tab.timelineFrozen;
    const kept = frozen ? frozen.names.indexOf(name) : -1;
    if (frozen && kept >= 0) return { value: frozen.values[kept], range: frozen.range[kept] };
    return { value: tab.watchValues[index], range: tab.watchRange[index] };
  }

  /** A time of day, as 18:14:12. */
  function clockText(at: number): string {
    return new Date(at).toLocaleTimeString([], { hour12: false });
  }

  function hex(address: number): string {
    return `0x${address.toString(16).padStart(8, "0")}`;
  }

  /** What a task's stack holds: in use now and the most it has used, of its size, or what is known of it. */
  function stackText(task: RtosTask): string {
    const peak = stackPeak(task);
    const used = task.stack_used;
    if (task.stack_size !== null) {
      const size = bytes(task.stack_size);
      if (used !== null && peak !== null) return `${bytes(used)}, max ${bytes(peak)} of ${size}`;
      if (peak !== null) return `max ${bytes(peak)} of ${size}`;
      return used === null ? size : `${bytes(used)} of ${size}`;
    }
    if (task.stack_free !== null) return `${bytes(task.stack_free)} never used`;
    return used === null ? "" : `${bytes(used)} in use`;
  }

  /** All that is known of a task's stack, in words. */
  function stackTitle(task: RtosTask): string {
    const lines: string[] = [];
    if (task.stack_size !== null) lines.push(`Size: ${task.stack_size.toLocaleString()} bytes`);
    const peak = stackPeak(task);
    if (peak !== null) lines.push(`The most it has used, its high-water mark: ${peak.toLocaleString()} bytes`);
    if (task.stack_used !== null) lines.push(`In use when it last left the CPU: ${task.stack_used.toLocaleString()} bytes`);
    if (task.stack_free !== null) lines.push(`Never used: ${task.stack_free.toLocaleString()} bytes`);
    return lines.join("\n");
  }

  /** The kernel's word for its tasks. */
  const taskWord = $derived(tab.rtos?.kernel === "zephyr" ? "threads" : "tasks");

  /** The table as text, one variable a line, for copying and saving. */
  function text(): string {
    return tab.watch.names
      .map((name, index) => {
        const variable = byName.get(name);
        const now = watched(name, index);
        const value = shown(now.value, variable, now.range).trim();
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
    <span class="file mono" title={tab.elf || "No ELF file chosen"}>{tab.elf ? fileName(tab.elf) : "No ELF file"}</span>
    <button class="btn small" onclick={chooseElf}>{tab.elf ? "Another ELF..." : "Choose the ELF..."}</button>
    {#if tab.elf}
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
    <button
      class="btn small"
      disabled={tab.watch.names.length === 0 && !tab.rtos}
      title="Start the smallest and largest values again: the variables', and the tasks' shares of the CPU"
      onclick={() => tab.resetWatchRange()}
    >
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
    {#if !tab.watchOnly}
      <button
        class="btn small"
        disabled={tab.watch.names.length === 0}
        title="Show them beside the terminal too, with what the firmware prints"
        onclick={() => {
          tab.setWatchBeside(true);
          tab.setView("terminal");
        }}
      >
        Beside the terminal
      </button>
    {/if}
  </div>

  {#if problem ?? tab.watchProblem ?? tab.tasksProblem}
    <p class="problem">{problem ?? tab.watchProblem ?? tab.tasksProblem}</p>
  {/if}

  <!-- The timeline on top, the tables under it. Every column has its width: nothing moves as the values change. -->
  <div class="table">
    {#if tab.rtos || tab.watch.names.length > 0 || tab.openedTimeline || tab.kind === "swo"}
      <TimelineView {tab} />
    {/if}
    {#if tab.rtos}
      <!-- Paused, the tasks stay as they were then, as the timeline does. -->
      {@const frozen = tab.timelineFrozen}
      {@const snapshot = frozen ? frozen.tasks : tab.tasks}
      <!-- A read that finds none leaves the last ones on screen, dimmed, so the variables under them stay put. -->
      {@const list = snapshot && snapshot.tasks.length > 0 ? snapshot : frozen ? frozen.lastTasks : tab.lastTasks}
      {@const ranges = frozen ? frozen.cpu : tab.cpuRanges}
      {@const load = list ? cpuLoad(list.tasks) : null}
      {@const fillNote = list ? stackFillNote(tab.rtos.kernel, list.tasks) : null}
      {@const notes = fillNote ? [...tab.rtos.missing, fillNote] : tab.rtos.missing}
      <section class="tasks" aria-label="{KERNEL_NAMES[tab.rtos.kernel]} {taskWord}">
        <div class="tasks-bar">
          <b>{KERNEL_NAMES[tab.rtos.kernel]} {taskWord}</b>
          {#if list}
            <span class="muted">{list.tasks.length}</span>
          {/if}
          {#if list?.heap}
            {@const heap = list.heap}
            <span
              class="muted"
              title="The kernel's heap: what is used now, and the most it has ever had in use"
            >
              Heap {bytes(heap.size - heap.free)} of {bytes(heap.size)}{heap.lowest_free !== null
                ? `, at most ${bytes(heap.size - heap.lowest_free)}`
                : ""}
            </span>
          {/if}
          {#if load !== null}
            <span class="muted" title="The CPU's load over the last second: all but the idle one's share">
              Load <span class="mono figure">{load.toFixed(1)}%</span>
            </span>
            {#if ranges.load}
              <span class="muted" title="The least load in a second, since connected or Reset min/max">
                Min <span class="mono figure">{ranges.load.min.toFixed(1)}%</span>
              </span>
              <span class="muted" title="The most load in a second, since connected or Reset min/max">
                Max <span class="mono figure">{ranges.load.max.toFixed(1)}%</span>
              </span>
            {/if}
          {/if}
          {#if snapshot?.problem}
            <span class="warning">{snapshot.problem}{list ? ` Shown: the ${taskWord} as last read.` : ""}</span>
          {:else if frozen}
            <span class="paused" title="Resume, over the timeline, shows them live again">
              Paused at {clockText(frozen.at)}: {tab.chipHalted
                ? "the chip is halted"
                : "as they were then, while the chip runs on"}
            </span>
          {:else}
            <span class="muted">Read every second as the chip runs, without stopping it</span>
          {/if}
        </div>
        {#if !list && !snapshot?.problem}
          <p class="note">
            {#if snapshot}
              No {taskWord} yet.
            {:else}
              {tab.connected ? `Reading the ${taskWord}...` : `Connect to read the ${taskWord}.`}
            {/if}
          </p>
        {/if}
        {#if list}
          <table class="task-table" class:old={list !== snapshot}>
            <colgroup>
              <col style:width="17%" />
              <col style:width="12%" />
              <col style:width="8%" />
              <col style:width="32%" />
              <col style:width="15%" />
              <col style:width="8%" />
              <col style:width="8%" />
            </colgroup>
            <thead>
              <tr>
                <th>Name</th>
                <th>State</th>
                <th title={tab.rtos.kernel === "zephyr" ? "Lower is more urgent; below 0 is cooperative" : "Higher is more urgent"}>Priority</th>
                <th title="In use now, as it last left the CPU, and the line at the most it has ever used, its high-water mark, where the kernel fills the stacks">Stack</th>
                <th title="Its share of the CPU over the last second, and the line at the most it has had">CPU</th>
                <th class="number" title="The least share of the CPU it has had in a second, since connected or Reset min/max">Min</th>
                <th class="number" title="The most share of the CPU it has had in a second, since connected or Reset min/max">Max</th>
              </tr>
            </thead>
            <tbody>
              {#each list.tasks as task (task.address)}
                {@const inUse = stackNow(task)}
                {@const most = stackMost(task)}
                {@const range = ranges.tasks.get(task.address)}
                <tr class:running={task.state === "running"}>
                  <td class="name mono" title="{task.name}, its control block at {hex(task.address)}">{task.name}</td>
                  <td><span class="state {task.state}" title={STATES[task.state].title}>{STATES[task.state].text}</span></td>
                  <td class="mono">{task.priority}</td>
                  <td class="stack" title={stackTitle(task)}>
                    {#if inUse !== null || most !== null}
                      <span class="meter">
                        {#if inUse !== null}
                          <span class="level" class:warn={inUse >= 0.75} class:full={inUse >= 0.9} style:width="{inUse * 100}%"></span>
                        {/if}
                        {#if most !== null}
                          <span class="mark" class:warn={most >= 0.75} class:full={most >= 0.9} style:left="{most * 100}%"></span>
                        {/if}
                      </span>
                    {/if}
                    <span class="mono">{stackText(task)}</span>
                  </td>
                  <td class="cpu">
                    {#if task.cpu !== null}
                      <span class="meter short">
                        <span class="level" class:idle={isIdle(task)} class:some={task.cpu >= 0.05} style:width="{task.cpu}%"></span>
                        {#if range && range.max >= 0.05}
                          <span class="mark" style:left="{range.max}%"></span>
                        {/if}
                      </span>
                      <span class="mono">{task.cpu.toFixed(1)}%</span>
                    {/if}
                  </td>
                  <td class="mono number">{range ? `${range.min.toFixed(1)}%` : ""}</td>
                  <td class="mono number">{range ? `${range.max.toFixed(1)}%` : ""}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
        {#if notes.length > 0}
          <p class="note missing-options">
            For more, turn on in the firmware: {notes.join("; ")}.
          </p>
        {/if}
      </section>
    {/if}
    {#if tab.watch.names.length > 0}
      <table>
        <colgroup>
          <col style:width="22%" />
          <col style:width="13%" />
          <col style:width="11%" />
          <col style:width="18%" />
          <col style:width="18%" />
          <col style:width="18%" />
          <col style:width="32px" />
        </colgroup>
        <thead>
          <tr>
            <th>Variable</th>
            <th>Type</th>
            <th>Address</th>
            <th>Value</th>
            <th>Min</th>
            <th>Max</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {#each tab.watch.names as name, index (name)}
            {@const variable = byName.get(name)}
            {@const missing = tab.watchFound.length > 0 && tab.watchFound[index] === false}
            {@const now = watched(name, index)}
            {@const range = now.range}
            {@const value = missing ? "not in the ELF" : shown(now.value, variable, range)}
            {@const min = range ? shown(range.min, variable, range) : ""}
            {@const max = range ? shown(range.max, variable, range) : ""}
            <tr class:missing>
              <td class="name mono" title={name}>{name}</td>
              <td class="type" title={variable?.type_name}>{variable?.type_name ?? ""}</td>
              <td class="address mono">{variable ? hex(variable.address) : ""}</td>
              <td class="mono value" title={value.trim()}>{value}</td>
              <td class="mono" title={min.trim()}>{min}</td>
              <td class="mono" title={max.trim()}>{max}</td>
              <td class="remove">
                <button class="icon" title="Stop watching it" onclick={() => tab.removeWatch(name)}>×</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {:else}
      <div class="empty">
        {#if !tab.elf}
          <p>
            Choose the firmware's ELF file, built with debug information (-g), to watch its variables, and the tasks
            of its RTOS: Zephyr or FreeRTOS.
          </p>
          <button class="btn" onclick={chooseElf}>Choose the ELF file...</button>
        {:else if reading}
          <p>Reading the ELF file...</p>
        {:else if variables.length > 0}
          <p>
            Click the box above for the variables in the file, or type part of a name to find one: globals and
            statics, a struct's members as <code>sensor.temp</code>, an array's elements as <code>buf[2]</code>.
          </p>
          <p class="muted">
            They are read as the chip runs, without stopping it, as often as Every says, and Plot
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

  /* The columns take the widths their colgroup gives, whatever the values in them. */
  table {
    width: 100%;
    font-size: 13px;
    border-collapse: collapse;
    table-layout: fixed;
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
    overflow: hidden;
    text-align: left;
    font-variant-numeric: tabular-nums;
    text-overflow: ellipsis;
    white-space: nowrap;
    border-bottom: 1px solid var(--border);
  }

  /* Shares of the CPU, their points under each other. */
  th.number,
  td.number {
    text-align: right;
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

  /* The RTOS's tasks, over the variables. */
  .tasks {
    margin-bottom: 14px;
  }

  .tasks-bar {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 12px;
    align-items: baseline;
    padding: 2px 8px 6px;
    font-size: 12px;
  }

  .tasks-bar b {
    font-weight: 600;
    color: var(--text);
  }

  .tasks .note {
    padding: 2px 8px 6px;
  }

  .warning {
    color: var(--warn);
  }

  .paused {
    color: var(--accent);
  }

  /* As last read, while the newest read finds none. */
  .task-table.old {
    opacity: 0.5;
  }

  .missing-options {
    padding-top: 6px;
  }

  /* One table scrolls under the other's headings otherwise. */
  .task-table th {
    position: static;
  }

  .task-table tr.running td {
    background: var(--accent-soft);
  }

  .task-table tr.running td.name {
    font-weight: 600;
  }

  .state {
    display: inline-flex;
    gap: 6px;
    align-items: center;
    font-size: 12px;
  }

  .state::before {
    width: 7px;
    height: 7px;
    content: "";
    background: var(--muted);
    border-radius: 50%;
  }

  .state.running::before {
    background: var(--accent);
  }

  .state.ready::before {
    background: var(--text);
  }

  .state.blocked::before,
  .state.suspended::before {
    background: var(--warn);
  }

  .state.dead::before {
    background: var(--bad);
  }

  td.stack,
  td.cpu {
    white-space: nowrap;
  }

  /* A figure in a line of words, as the load's. */
  .figure {
    color: var(--text);
  }

  .meter {
    position: relative;
    display: inline-block;
    width: 72px;
    height: 6px;
    margin-right: 8px;
    vertical-align: middle;
    background: var(--border);
    border-radius: 3px;
  }

  .level {
    display: block;
    height: 100%;
    background: var(--accent);
    border-radius: 3px;
  }

  .level.warn {
    background: var(--warn);
  }

  .level.full {
    background: var(--bad);
  }

  /* A share of the CPU, on a shorter bar. */
  .meter.short {
    width: 60px;
  }

  /* A share too small to see still shows, once it reads 0.1%: under that, as 0.0%, it is not there. */
  .level.some {
    min-width: 2px;
  }

  /* The idle task's share is the time left free. */
  .level.idle {
    background: var(--muted);
  }

  /* The high-water mark: the most there has been, standing out of the bar. */
  .mark {
    position: absolute;
    top: -3px;
    width: 2px;
    height: 12px;
    margin-left: -1px;
    background: var(--muted);
    border-radius: 1px;
  }

  .mark.warn {
    background: var(--warn);
  }

  .mark.full {
    background: var(--bad);
  }
</style>
