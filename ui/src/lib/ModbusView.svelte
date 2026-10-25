<script lang="ts">
  import { onMount } from "svelte";
  import { FUNCTION_NAMES } from "./modbus";
  import { formatRegisters, type LogEntry, type ModbusRequestRow, type RegisterFormat } from "./modbus.svelte";
  import { changed, type Tab } from "./tabs.svelte";
  import { clearActiveTerminal, setActiveTerminal, type TerminalActions } from "./terminal-actions";

  let {
    tab,
    active,
  }: {
    tab: Tab;
    /** Whether this view has the keyboard. */
    active: boolean;
  } = $props();

  const setup = $derived(tab.modbusSetup);
  const session = $derived(tab.modbus);
  const master = $derived(setup.role === "master");

  const FUNCTIONS = Object.entries(FUNCTION_NAMES).map(([code, name]) => ({ code: Number(code), name }));
  const POLLS = [
    { ms: 100, label: "100 ms" },
    { ms: 250, label: "250 ms" },
    { ms: 500, label: "500 ms" },
    { ms: 1000, label: "1 s" },
    { ms: 2000, label: "2 s" },
    { ms: 5000, label: "5 s" },
  ];
  const FORMATS: { value: RegisterFormat; label: string }[] = [
    { value: "unsigned", label: "Unsigned" },
    { value: "signed", label: "Signed" },
    { value: "hex", label: "Hex" },
    { value: "float", label: "Float (high word first)" },
    { value: "float_swapped", label: "Float (low word first)" },
  ];
  const TABLES: { key: "holding" | "input" | "coils" | "discrete"; label: string; bits: boolean }[] = [
    { key: "holding", label: "Holding registers", bits: false },
    { key: "input", label: "Input registers", bits: false },
    { key: "coils", label: "Coils", bits: true },
    { key: "discrete", label: "Discrete inputs", bits: true },
  ];

  let root: HTMLDivElement;
  let logBox: HTMLDivElement | undefined = $state();

  const isWrite = (code: number) => code === 5 || code === 6 || code === 15 || code === 16;
  const isBits = (code: number) => code === 1 || code === 2;

  function save() {
    changed();
  }

  function addRequest() {
    const last = setup.requests.at(-1);
    setup.requests.push({ unit: last?.unit ?? 1, function: 3, address: 0, count: 10, values: "", poll: true });
    save();
  }

  function removeRequest(index: number) {
    setup.requests.splice(index, 1);
    session.results.splice(index, 1);
    save();
  }

  /** Grows or shrinks the slave's tables to `size` of each. */
  function resize(size: number) {
    const count = Math.max(1, Math.min(1000, Math.floor(size) || 1));
    for (const { key, bits } of TABLES) {
      const table = setup[key] as (number | boolean)[];
      while (table.length < count) table.push(bits ? false : 0);
      table.length = count;
    }
    save();
  }

  /** What a read got, shown in the format picked: each value with its address. */
  function cells(row: ModbusRequestRow, values: number[]): { address: number; text: string }[] {
    const texts = isBits(row.function) ? values.map(String) : formatRegisters(values, setup.format);
    return texts.map((text, index) => ({ address: row.address + index, text })).filter((cell) => cell.text !== "");
  }

  function time(at: number): string {
    const date = new Date(at);
    const two = (value: number) => String(value).padStart(2, "0");
    return `${two(date.getHours())}:${two(date.getMinutes())}:${two(date.getSeconds())}.${String(date.getMilliseconds()).padStart(3, "0")}`;
  }

  const arrow = (entry: LogEntry) => (entry.kind === "out" ? "→" : entry.kind === "in" ? "←" : "!");

  // The log keeps showing its newest line.
  $effect(() => {
    void session.log.length;
    requestAnimationFrame(() => logBox && (logBox.scrollTop = logBox.scrollHeight));
  });

  const actions: TerminalActions = {
    hasSelection: () => false,
    copy: async () => {},
    paste: async () => {},
    selectAll: () => {},
    clear: () => session.clearLog(),
    text: () => session.log.map((entry) => `${time(entry.at)} ${arrow(entry)} ${entry.text}  ${entry.hex}`).join("\n") + "\n",
    focus: () => root?.focus(),
    openSearch: () => {},
    findNext: () => {},
    findPrevious: () => {},
  };

  // The view on screen is the one the menus act on, and has the keyboard.
  $effect(() => {
    if (active) {
      setActiveTerminal(actions);
      root.focus();
    }
  });

  onMount(() => () => clearActiveTerminal(actions));
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div class="modbusview" tabindex="0" role="region" aria-label="Modbus" bind:this={root}>
  <div class="bar">
    <div class="segment" role="group" aria-label="Role">
      <button class:on={master} title="Ask devices" onclick={() => ((setup.role = "master"), session.setAnswering(false), save())}>
        Master
      </button>
      <button class:on={!master} title="Answer a master from tables" onclick={() => ((setup.role = "slave"), session.stopPolling(), save())}>
        Slave
      </button>
    </div>
    <label class="option" title="How messages are framed: RTU with a CRC, ASCII lines with an LRC, or TCP with its header">
      Mode
      <select class="field" bind:value={setup.mode} onchange={save}>
        <option value="rtu">RTU</option>
        <option value="ascii">ASCII</option>
        <option value="tcp">TCP</option>
      </select>
    </label>
    {#if master}
      <label class="option" title="How long to wait for an answer">
        Timeout
        <input class="field mono short" type="number" min="10" max="60000" bind:value={setup.timeout_ms} onchange={save} />
        ms
      </label>
      <label class="option" title="How often the requests marked Poll are asked">
        Poll every
        <select class="field" bind:value={setup.poll_ms} onchange={save}>
          {#each POLLS as { ms, label } (ms)}
            <option value={ms}>{label}</option>
          {/each}
        </select>
      </label>
      {#if session.polling}
        <button class="btn small on" onclick={() => session.stopPolling()}>Stop polling</button>
      {:else}
        <button class="btn small primary" disabled={!tab.connected} onclick={() => session.startPolling()}>Poll</button>
      {/if}
    {:else}
      <label class="option" title="The unit this slave answers as">
        Unit
        <input class="field mono short" type="number" min="1" max="247" bind:value={setup.unit} onchange={save} />
      </label>
      {#if session.answering}
        <button class="btn small on" onclick={() => session.setAnswering(false)}>Stop answering</button>
      {:else}
        <button class="btn small primary" disabled={!tab.connected} onclick={() => session.setAnswering(true)}>Answer</button>
      {/if}
    {/if}
    <label class="option" title="How register values are shown">
      Show
      <select class="field" bind:value={setup.format} onchange={save}>
        {#each FORMATS as { value, label } (value)}
          <option {value}>{label}</option>
        {/each}
      </select>
    </label>
    <span class="spacer"></span>
    <span class="stats" title="Since the log was cleared">
      {session.stats.sent} sent · {session.stats.answered} answered · {session.stats.timeouts} timeouts · {session.stats.problems} errors
    </span>
  </div>

  <div class="body">
    {#if master}
      <div class="requests">
        {#each setup.requests as row, index (index)}
          {@const result = session.results[index]}
          <div class="request">
            <div class="fields">
              <label class="small-label">
                Unit
                <input class="field mono" type="number" min="0" max="247" bind:value={row.unit} onchange={save} />
              </label>
              <label class="small-label wide">
                Function
                <select class="field" bind:value={row.function} onchange={save}>
                  {#each FUNCTIONS as { code, name } (code)}
                    <option value={code}>{code} {name}</option>
                  {/each}
                </select>
              </label>
              <label class="small-label">
                Address
                <input class="field mono" type="number" min="0" max="65535" bind:value={row.address} onchange={save} />
              </label>
              {#if isWrite(row.function)}
                <label class="small-label wide" title="Numbers such as 1, 2, 0x10; 1 or 0 for coils">
                  {row.function === 5 || row.function === 6 ? "Value" : "Values"}
                  <input class="field mono" spellcheck="false" placeholder="1, 2, 3" bind:value={row.values} onchange={save} />
                </label>
              {:else}
                <label class="small-label">
                  Count
                  <input class="field mono" type="number" min="1" max={isBits(row.function) ? 2000 : 125} bind:value={row.count} onchange={save} />
                </label>
              {/if}
              <label class="check" title="Ask it again and again while polling">
                <input type="checkbox" bind:checked={row.poll} onchange={save} /> Poll
              </label>
              <button class="btn small" disabled={!tab.connected} onclick={() => session.sendRow(index)}>Send</button>
              <button class="icon" title="Remove" onclick={() => removeRequest(index)}>×</button>
            </div>
            {#if result}
              <div class="result">
                <span class="when mono">{time(result.at)}{result.ms ? ` · ${result.ms} ms` : ""}</span>
                {#if result.problem}
                  <span class="problem">{result.problem}</span>
                {:else if result.values}
                  <div class="cells mono">
                    {#each cells(row, result.values) as cell (cell.address)}
                      <span class="cell"><span class="address">{cell.address}</span>{cell.text}</span>
                    {/each}
                  </div>
                {:else}
                  <span>{result.text}</span>
                {/if}
              </div>
            {/if}
          </div>
        {/each}
        <div class="under">
          <button class="btn small" onclick={addRequest}>Add a request</button>
          {#if session.scan?.running}
            <button class="btn small" onclick={() => session.stopScan()}>Stop scanning</button>
            <span class="muted">Asking unit {session.scan.unit} of 247{session.scan.found.length ? `; found ${session.scan.found.join(", ")}` : ""}</span>
          {:else}
            <button
              class="btn small"
              disabled={!tab.connected}
              title="Ask units 1 to 247 for holding register 0, 150 ms each, and list the ones that answer"
              onclick={() => session.startScan()}
            >
              Scan units...
            </button>
            {#if session.scan}
              <span class="muted">Found: {session.scan.found.length ? session.scan.found.join(", ") : "none"}</span>
            {/if}
          {/if}
        </div>
      </div>
    {:else}
      <div class="tables">
        <div class="table-bar">
          <label class="option" title="The address of each table's first value">
            From address
            <input class="field mono short" type="number" min="0" max="65535" bind:value={setup.start} onchange={save} />
          </label>
          <label class="option" title="How many values each table holds">
            Size
            <input class="field mono short" type="number" min="1" max="1000" value={setup.holding.length} onchange={(event) => resize(Number(event.currentTarget.value))} />
          </label>
          <span class="muted">A master's writes change these as they come.</span>
        </div>
        {#each TABLES as { key, label, bits } (key)}
          <section>
            <h3>{label}</h3>
            <div class="grid">
              {#each setup[key] as _, index (index)}
                <label class="slot">
                  <span class="address mono">{setup.start + index}</span>
                  {#if bits}
                    <input type="checkbox" bind:checked={(setup[key] as boolean[])[index]} onchange={save} />
                  {:else}
                    <input class="field mono" type="number" min="0" max="65535" bind:value={(setup[key] as number[])[index]} onchange={save} />
                  {/if}
                </label>
              {/each}
            </div>
          </section>
        {/each}
      </div>
    {/if}

    <div class="log mono" bind:this={logBox} aria-label="What was sent and what came">
      {#each session.log as entry, index (index)}
        <div class="entry {entry.kind}">
          <span class="when">{time(entry.at)}</span>
          <span class="arrow">{arrow(entry)}</span>
          <span class="text">{entry.text}</span>
          <span class="hex">{entry.hex}</span>
        </div>
      {:else}
        <p class="empty">
          {master
            ? "Send a request, or Poll the ones ticked. What goes out and what comes back is listed here."
            : "Answer to start: requests to this unit are answered from the tables, and listed here."}
        </p>
      {/each}
    </div>
  </div>
</div>

<style>
  .modbusview {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    font-size: 12px;
    outline: none;
  }

  .bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-end;
    gap: 6px 10px;
    padding-bottom: 6px;
    color: var(--muted);
  }

  .segment {
    display: flex;
    overflow: hidden;
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  .segment button {
    height: 24px;
    padding: 0 10px;
    font: inherit;
    color: var(--muted);
    background: none;
    border: 0;
    cursor: default;
  }

  .segment button + button {
    border-left: 1px solid var(--border);
  }

  .segment button.on {
    color: var(--text);
    background: var(--accent-soft);
  }

  .option,
  .check {
    display: flex;
    gap: 6px;
    align-items: center;
    white-space: nowrap;
  }

  .check:has(input:checked) {
    color: var(--text);
  }

  .bar .field,
  .table-bar .field {
    height: 24px;
    font-size: 12px;
  }

  .short {
    width: 72px;
  }

  .small {
    height: 24px;
    padding: 0 10px;
    font-size: 12px;
  }

  .btn.on {
    color: var(--accent);
    border-color: var(--accent);
  }

  .spacer {
    flex: 1;
  }

  .stats {
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .body {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 8px;
    min-height: 0;
  }

  .requests,
  .tables {
    flex: 1;
    min-height: 120px;
    overflow: auto;
  }

  .request {
    padding: 8px 6px;
    border-bottom: 1px solid var(--border);
  }

  .fields {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: flex-end;
  }

  .small-label {
    display: flex;
    flex-direction: column;
    gap: 3px;
    color: var(--muted);
  }

  .small-label .field {
    width: 84px;
    height: 26px;
    font-size: 12px;
  }

  .small-label.wide .field {
    width: 220px;
  }

  .fields .check {
    height: 26px;
  }

  .fields .btn {
    height: 26px;
  }

  .icon {
    width: 26px;
    height: 26px;
    font: inherit;
    font-size: 14px;
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

  .result {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 12px;
    align-items: baseline;
    margin-top: 8px;
    color: var(--text);
  }

  .when {
    color: var(--muted);
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }

  .problem {
    color: var(--bad);
  }

  .cells {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  .cell {
    min-width: 92px;
    padding: 2px 6px;
    text-align: right;
    background: var(--panel);
    border-radius: 4px;
    font-variant-numeric: tabular-nums;
  }

  .address {
    float: left;
    margin-right: 8px;
    color: var(--muted);
  }

  .under {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    align-items: center;
    padding: 10px 6px;
  }

  .muted {
    color: var(--muted);
  }

  .table-bar {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 14px;
    align-items: center;
    padding: 4px 6px 8px;
    color: var(--muted);
  }

  section {
    padding: 4px 6px 10px;
  }

  h3 {
    margin: 4px 0 6px;
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
    gap: 4px 8px;
  }

  .slot {
    display: flex;
    gap: 6px;
    align-items: center;
  }

  .slot .address {
    min-width: 4ch;
    margin: 0;
    text-align: right;
  }

  .slot .field {
    width: 70px;
    height: 24px;
    font-size: 12px;
  }

  .log {
    max-height: 35%;
    min-height: 80px;
    padding: 4px 6px;
    overflow: auto;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  .entry {
    display: flex;
    gap: 10px;
    line-height: 19px;
    white-space: nowrap;
  }

  .entry .arrow {
    width: 1ch;
    color: var(--muted);
  }

  .entry.in .arrow {
    color: var(--ok);
  }

  .entry.out .arrow {
    color: var(--accent);
  }

  .entry.problem .arrow,
  .entry.problem .text {
    color: var(--bad);
  }

  .entry .hex {
    overflow: hidden;
    color: var(--muted);
    text-overflow: ellipsis;
  }

  .empty {
    margin: 10px 4px;
    color: var(--muted);
  }
</style>
