<script lang="ts">
  import { onMount, untrack } from "svelte";
  import {
    BAUD_RATES,
    DEFAULT_SERIAL,
    DEFAULT_TERMINAL,
    describe,
    listPorts,
    product,
    usbIds,
    type PortId,
    type PortInfo,
    type SerialConfig,
    type TerminalOptions,
  } from "./serial";
  import { addTab, changed, tabs, type Tab } from "./tabs.svelte";

  let {
    tab = null,
    onclose,
  }: {
    /** The tab whose settings to change, or null for a new session. */
    tab?: Tab | null;
    onclose: () => void;
  } = $props();

  // A copy of the tab's settings, taken once: nothing changes until Save.
  const start = untrack(() => tab?.setup() ?? null);
  let ports: PortInfo[] = $state([]);
  let listed = $state(false);
  let selected: string | null = $state(start?.port.name ?? null);
  let serial: SerialConfig = $state({ ...(start?.serial ?? DEFAULT_SERIAL) });
  let terminal: TerminalOptions = $state({ ...(start?.terminal ?? DEFAULT_TERMINAL) });
  let baudText = $state(String(serial.baud));
  let name = $state(start?.name ?? "");
  let nameTyped = start !== null;
  let error = $state("");

  let dialog: HTMLDivElement;

  // The tab's own port stays in the list while it is unplugged, so it can stay chosen.
  const shown: PortInfo[] = $derived.by(() => {
    const own = start?.port;
    if (!own || ports.some((port) => port.name === own.name)) {
      return ports;
    }
    return [{ name: own.name, usb: own.usb ?? null, manufacturer: null, product: null }, ...ports];
  });

  onMount(() => {
    dialog.focus();
    void refresh();
    // The list follows devices being plugged in and out.
    const timer = setInterval(refresh, 1000);
    return () => clearInterval(timer);
  });

  async function refresh() {
    try {
      ports = await listPorts();
    } catch (problem) {
      console.error(problem);
    }
    listed = true;
    if (selected === null) {
      const free = ports.find((port) => !inUse(port));
      if (free) {
        choose(free);
      }
    }
  }

  function choose(port: PortInfo) {
    selected = port.name;
    if (!nameTyped) {
      name = product(port) ?? port.name;
    }
  }

  /** The other tab that has this port open, if any. */
  function inUse(port: PortInfo): Tab | undefined {
    return tabs.find((other) => other !== tab && other.running && other.port.name === port.name);
  }

  function unplugged(port: PortInfo): boolean {
    return listed && !ports.some((listedPort) => listedPort.name === port.name);
  }

  function submit() {
    const baud = Number(baudText);
    if (!Number.isInteger(baud) || baud <= 0) {
      error = "Enter a baud rate: a whole number, such as 115200.";
      return;
    }
    const port = shown.find((info) => info.name === selected);
    if (!port) {
      error = "Choose a port.";
      return;
    }
    serial.baud = baud;
    const portId: PortId = { name: port.name, usb: port.usb };
    const sessionName = name.trim() || port.name;

    if (tab) {
      tab.name = sessionName;
      tab.terminal = { ...terminal };
      if (port.name !== start?.port.name) {
        tab.serial = { ...serial };
        tab.changePort(portId);
      } else if (JSON.stringify(serial) !== JSON.stringify(start?.serial)) {
        tab.reconfigure({ ...serial });
      }
      changed();
    } else {
      addTab({ name: sessionName, connected: false, port: portId, serial: { ...serial }, terminal: { ...terminal } }).start();
    }
    onclose();
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      onclose();
    } else if (event.key === "Enter" && !(event.target instanceof HTMLSelectElement)) {
      event.preventDefault();
      submit();
    }
  }
</script>

<!-- A click on the dark area around the dialog closes it, as Escape does. -->
<div class="backdrop" role="presentation" onmousedown={onclose}>
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby="session-title"
    tabindex="-1"
    bind:this={dialog}
    {onkeydown}
    onmousedown={(event) => event.stopPropagation()}
  >
    <h1 id="session-title">{tab ? "Port settings" : "New serial session"}</h1>

    <h2>Port</h2>
    <div class="ports" role="listbox" aria-label="Serial ports">
      {#each shown as port (port.name)}
        {@const other = inUse(port)}
        <button
          class="port"
          class:chosen={port.name === selected}
          role="option"
          aria-selected={port.name === selected}
          onclick={() => choose(port)}
          ondblclick={() => {
            choose(port);
            submit();
          }}
        >
          <span class="name mono">{port.name}</span>
          <span class="what">
            {describe(port)}
            {#if port.manufacturer && product(port)}
              <span class="muted">· {port.manufacturer}</span>
            {/if}
          </span>
          <span class="ids mono">
            {usbIds(port.usb)}
            {#if port.usb?.serial}
              <span class="muted" title={`Serial number ${port.usb.serial}`}>SN {port.usb.serial}</span>
            {/if}
          </span>
          {#if unplugged(port)}
            <span class="badge">not plugged in</span>
          {:else if other}
            <span class="badge">open in {other.name}</span>
          {/if}
        </button>
      {:else}
        <p class="empty">
          {listed
            ? "No serial ports found. Plug in a device: the list updates by itself."
            : "Looking for serial ports..."}
        </p>
      {/each}
    </div>

    <h2>Settings</h2>
    <div class="grid">
      <label>
        Baud rate
        <input
          class="field mono"
          list="baud-rates"
          inputmode="numeric"
          bind:value={baudText}
          oninput={() => (error = "")}
        />
        <datalist id="baud-rates">
          {#each BAUD_RATES as rate (rate)}
            <option value={rate}></option>
          {/each}
        </datalist>
      </label>
      <label>
        Data bits
        <select class="field" bind:value={serial.data_bits}>
          {#each [8, 7, 6, 5] as bits (bits)}
            <option value={bits}>{bits}</option>
          {/each}
        </select>
      </label>
      <label>
        Parity
        <select class="field" bind:value={serial.parity}>
          <option value="none">None</option>
          <option value="odd">Odd</option>
          <option value="even">Even</option>
        </select>
      </label>
      <label>
        Stop bits
        <select class="field" bind:value={serial.stop_bits}>
          <option value={1}>1</option>
          <option value={2}>2</option>
        </select>
      </label>
      <label>
        Flow control
        <select class="field" bind:value={serial.flow}>
          <option value="none">None</option>
          <option value="rtscts">RTS/CTS</option>
          <option value="xonxoff">XON/XOFF</option>
        </select>
      </label>
      <label>
        Enter sends
        <select class="field" bind:value={terminal.enter}>
          <option value="cr">CR</option>
          <option value="lf">LF</option>
          <option value="crlf">CR LF</option>
        </select>
      </label>
    </div>

    <div class="checks">
      <label><input type="checkbox" bind:checked={serial.dtr} /> DTR on</label>
      <label><input type="checkbox" bind:checked={serial.rts} disabled={serial.flow === "rtscts"} /> RTS on</label>
      <label title="Show what you type, for devices that do not echo it">
        <input type="checkbox" bind:checked={terminal.local_echo} /> Local echo
      </label>
      <label title="For devices that end lines with LF only">
        <input type="checkbox" bind:checked={terminal.lf_is_newline} /> New line on LF
      </label>
    </div>

    <label class="session-name">
      Tab name
      <input class="field" bind:value={name} oninput={() => (nameTyped = true)} placeholder={selected ?? ""} />
    </label>

    {#if error}
      <p class="error">{error}</p>
    {/if}

    <div class="buttons">
      <button class="btn" onclick={onclose}>Cancel</button>
      <button class="btn primary" disabled={selected === null} onclick={submit}>
        {tab ? "Save" : "Connect"}
      </button>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 20;
    display: grid;
    place-items: center;
    background: var(--backdrop);
  }

  .dialog {
    display: flex;
    flex-direction: column;
    width: min(640px, calc(100vw - 32px));
    max-height: calc(100vh - 48px);
    padding: 20px 22px 18px;
    overflow-y: auto;
    background: var(--menu-bg);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: var(--shadow);
    outline: none;
  }

  h1 {
    margin: 0 0 6px;
    font-size: 17px;
    font-weight: 600;
  }

  h2 {
    margin: 14px 0 6px;
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }

  .ports {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-height: 64px;
    max-height: 210px;
    padding: 4px;
    overflow-y: auto;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 8px;
  }

  .port {
    display: grid;
    grid-template-columns: 92px 1fr auto auto;
    align-items: center;
    gap: 10px;
    min-height: 34px;
    padding: 4px 10px;
    font: inherit;
    color: var(--text);
    text-align: left;
    background: none;
    border: 1px solid transparent;
    border-radius: 6px;
    cursor: default;
  }

  .port:hover {
    background: var(--hover);
  }

  .port.chosen {
    background: var(--accent-soft);
    border-color: var(--accent);
  }

  .name {
    font-weight: 600;
  }

  .what {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .ids {
    display: flex;
    gap: 8px;
    font-size: 12px;
    white-space: nowrap;
  }

  .ids .muted {
    max-width: 110px;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .muted {
    color: var(--muted);
  }

  .badge {
    max-width: 140px;
    padding: 1px 6px;
    overflow: hidden;
    font-size: 11px;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--warn);
    border: 1px solid var(--warn);
    border-radius: 4px;
  }

  .empty {
    margin: auto;
    padding: 14px;
    color: var(--muted);
    text-align: center;
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 10px 14px;
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--muted);
  }

  .checks {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 18px;
    margin-top: 12px;
  }

  .checks label {
    flex-direction: row;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    color: var(--text);
  }

  .session-name {
    margin-top: 14px;
  }

  .field {
    font-size: 13px;
  }

  .error {
    margin: 10px 0 0;
    color: var(--bad);
  }

  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 18px;
  }

  .buttons .btn {
    min-width: 88px;
    justify-content: center;
  }
</style>
