<script lang="ts">
  import { onMount, untrack } from "svelte";
  import Modal from "./Modal.svelte";
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
    type SessionSetup,
    type TerminalOptions,
  } from "./serial";
  import { sameDevice } from "./recent";
  import { settings } from "./settings.svelte";
  import { addTab, changed, closedTabs, forgetClosed, tabs, type Tab } from "./tabs.svelte";

  let {
    tab = null,
    onclose,
  }: {
    /** The serial tab whose settings to change, or null for a new session. */
    tab?: Tab | null;
    onclose: () => void;
  } = $props();

  // A copy of the tab's settings, taken once: nothing changes until Save.
  const start = untrack(() => tab?.setup() ?? null);
  let ports: PortInfo[] = $state([]);
  let listed = $state(false);
  let selected: string | null = $state(start?.port?.name ?? null);
  let serial: SerialConfig = $state({ ...DEFAULT_SERIAL, ...start?.serial });
  let terminal: TerminalOptions = $state({ ...(start?.terminal ?? DEFAULT_TERMINAL) });
  let baudText = $state(String(start?.serial?.baud ?? DEFAULT_SERIAL.baud));
  let name = $state(start?.name ?? "");
  let nameTyped = start !== null;
  let error = $state("");
  /** For a new session: the chosen device's tab as it was when it was closed, whose settings it gets again. */
  let known: SessionSetup | null = $state(null);

  // The tab's own port stays in the list while it is unplugged, so it can stay chosen.
  const shown: PortInfo[] = $derived.by(() => {
    const own = start?.port;
    if (!own || ports.some((port) => port.name === own.name)) {
      return ports;
    }
    return [{ name: own.name, usb: own.usb ?? null, manufacturer: null, product: null }, ...ports];
  });

  onMount(() => {
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
    if (!tab) {
      recall(port);
    }
    if (!nameTyped) {
      name = known?.name ?? product(port) ?? port.name;
    }
  }

  /** Takes the settings the device had when its tab was last closed, or the defaults for one not seen before. */
  function recall(port: PortInfo) {
    const found = closedTabs.find((setup) => setup.kind === "serial" && setup.port && sameDevice(setup.port, port)) ?? null;
    if (found === known) {
      return;
    }
    serial = { ...DEFAULT_SERIAL, ...found?.serial };
    terminal = { ...(found?.terminal ?? DEFAULT_TERMINAL) };
    baudText = String(serial.baud);
    known = found;
  }

  /** The other tab that has this port open, if any. */
  function inUse(port: PortInfo): Tab | undefined {
    return tabs.find(
      (other) => other !== tab && other.kind === "serial" && other.running && other.port.name === port.name,
    );
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
      if (port.name !== start?.port?.name) {
        tab.serial = { ...serial };
        tab.changePort(portId);
      } else if (JSON.stringify(serial) !== JSON.stringify(start?.serial)) {
        tab.reconfigure({ ...serial });
      }
      changed();
    } else {
      // A device seen before gets its tab back as it was: its views, log and sent lines too.
      const before = known ? $state.snapshot(known) : null;
      if (known) {
        forgetClosed(known);
      }
      addTab({
        ...before,
        name: sessionName,
        kind: "serial",
        connected: false,
        log: before?.log ?? settings.log.new_tabs,
        port: portId,
        serial: { ...serial },
        terminal: { ...terminal },
      }).start();
    }
    onclose();
  }
</script>

<Modal title={tab ? "Port settings" : "New serial session"} {onclose} onsubmit={submit}>
  <h2 class="dlg-section">Port</h2>
  <div class="dlg-list" role="listbox" aria-label="Serial ports">
    {#each shown as port (port.name)}
      {@const other = inUse(port)}
      <button
        class="dlg-row port"
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
            <span class="dlg-muted">· {port.manufacturer}</span>
          {/if}
        </span>
        <span class="ids mono">
          {usbIds(port.usb)}
          {#if port.usb?.serial}
            <span class="dlg-muted serial" title={`Serial number ${port.usb.serial}`}>
              SN {port.usb.serial}
            </span>
          {/if}
        </span>
        {#if unplugged(port)}
          <span class="dlg-badge">not plugged in</span>
        {:else if other}
          <span class="dlg-badge">open in {other.name}</span>
        {/if}
      </button>
    {:else}
      <p class="dlg-empty">
        {listed
          ? "No serial ports found. Plug in a device: the list updates by itself."
          : "Looking for serial ports..."}
      </p>
    {/each}
  </div>

  <h2 class="dlg-section">Settings</h2>
  {#if known}
    <p class="dlg-hint">As this device's tab, {known.name}, had them when it was closed.</p>
  {/if}
  <div class="dlg-grid">
    <label class="dlg-label">
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
    <label class="dlg-label">
      Data bits
      <select class="field" bind:value={serial.data_bits}>
        {#each [8, 7, 6, 5] as bits (bits)}
          <option value={bits}>{bits}</option>
        {/each}
      </select>
    </label>
    <label class="dlg-label">
      Parity
      <select class="field" bind:value={serial.parity}>
        <option value="none">None</option>
        <option value="odd">Odd</option>
        <option value="even">Even</option>
      </select>
    </label>
    <label class="dlg-label">
      Stop bits
      <select class="field" bind:value={serial.stop_bits}>
        <option value={1}>1</option>
        <option value={2}>2</option>
      </select>
    </label>
    <label class="dlg-label">
      Flow control
      <select class="field" bind:value={serial.flow}>
        <option value="none">None</option>
        <option value="rtscts">RTS/CTS</option>
        <option value="xonxoff">XON/XOFF</option>
      </select>
    </label>
    <label class="dlg-label">
      Enter sends
      <select class="field" bind:value={terminal.enter}>
        <option value="cr">CR</option>
        <option value="lf">LF</option>
        <option value="crlf">CR LF</option>
      </select>
    </label>
  </div>

  <div class="dlg-checks">
    <label class="dlg-check"><input type="checkbox" bind:checked={serial.dtr} /> DTR on</label>
    <label class="dlg-check">
      <input type="checkbox" bind:checked={serial.rts} disabled={serial.flow === "rtscts"} /> RTS on
    </label>
    <label class="dlg-check" title="Show what you type, for devices that do not echo it">
      <input type="checkbox" bind:checked={terminal.local_echo} /> Local echo
    </label>
    <label class="dlg-check" title="For devices that end lines with LF only">
      <input type="checkbox" bind:checked={terminal.lf_is_newline} /> New line on LF
    </label>
  </div>

  <label class="dlg-label tab-name">
    Tab name
    <input class="field" bind:value={name} oninput={() => (nameTyped = true)} placeholder={selected ?? ""} />
  </label>

  {#if error}
    <p class="dlg-error">{error}</p>
  {/if}

  {#snippet buttons()}
    <button class="btn" onclick={onclose}>Cancel</button>
    <button class="btn primary" disabled={selected === null} onclick={submit}>
      {tab ? "Save" : "Connect"}
    </button>
  {/snippet}
</Modal>

<style>
  .port {
    grid-template-columns: 92px minmax(0, 1fr) auto auto;
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

  .serial {
    max-width: 110px;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .tab-name {
    margin-top: 14px;
  }
</style>
