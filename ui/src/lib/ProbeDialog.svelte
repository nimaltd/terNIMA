<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import Modal from "./Modal.svelte";
  import {
    DEFAULT_RTT,
    DEFAULT_SWO,
    SPEEDS,
    SWO_BAUDS,
    detectChip,
    hzText,
    listProbes,
    sameProbe,
    searchChips,
    speedText,
    swoProblem,
    type ProbeId,
    type ProbeInfo,
    type RttConfig,
    type SwoConfig,
  } from "./probe";
  import { DEFAULT_TERMINAL, type TerminalOptions } from "./serial";
  import { settings } from "./settings.svelte";
  import { addTab, changed, tabs, type Tab } from "./tabs.svelte";

  let {
    kind,
    tab = null,
    onclose,
    onserver,
  }: {
    /** What the session reads through the probe. */
    kind: "rtt" | "swo";
    /** The tab whose settings to change, or null for a new session. */
    tab?: Tab | null;
    onclose: () => void;
    /** Opens the dialog for RTT from a debugger's own server instead, for a new session. */
    onserver?: () => void;
  } = $props();

  const KIND = untrack(() => kind);

  // A copy of the tab's settings, taken once: nothing changes until Save.
  const start = untrack(() => tab?.setup() ?? null);
  let rtt: RttConfig = $state({ ...DEFAULT_RTT, ...start?.rtt });
  const LABEL = $derived(KIND === "swo" ? "SWO" : rtt.watch_only ? "Watch" : "RTT");
  let swo: SwoConfig = $state({ ...DEFAULT_SWO, ...start?.swo });
  /** The settings both kinds have: which probe, which chip, and how to attach. */
  const own = KIND === "rtt" ? rtt : swo;
  let target = $state({
    probe: own.probe ?? null,
    chip: own.chip,
    speed_khz: own.speed_khz,
    under_reset: own.under_reset,
  });
  let terminal: TerminalOptions = $state({ ...(start?.terminal ?? DEFAULT_TERMINAL) });
  let probes: ProbeInfo[] = $state([]);
  let listed = $state(false);
  let addressText = $state(rtt.address ?? "");
  let clockText = $state(swo.clock_hz ? String(swo.clock_hz) : "");
  let baudText = $state(String(swo.baud));
  let chips: string[] = $state([]);
  let name = $state(start?.name ?? "");
  let nameTyped = start !== null;
  let error = $state("");
  let detecting = $state(false);
  /** What detection found, in words. */
  let found = $state("");
  /** Detection runs by itself once, for a new session with no chip yet. */
  let detectedOnce = start !== null;

  // The tab's own probe stays in the list while it is unplugged, so it can stay chosen.
  const shown: ProbeInfo[] = $derived.by(() => {
    const ownProbe = own.probe;
    if (!ownProbe || probes.some((probe) => sameProbe(probe.id, ownProbe))) {
      return probes;
    }
    return [{ name: "Debug probe", id: ownProbe }, ...probes];
  });

  onMount(() => {
    void refresh();
    void suggest();
    // The list follows probes being plugged in and out.
    const timer = setInterval(refresh, 1000);
    return () => clearInterval(timer);
  });

  async function refresh() {
    try {
      probes = await listProbes();
    } catch (problem) {
      console.error(problem);
    }
    listed = true;
    if (!target.probe && probes.length > 0) {
      choose(probes[0]);
    }
  }

  /** The probe settings of another tab, if it is an RTT or SWO tab. */
  function probeOf(other: Tab): ProbeId | null | undefined {
    return other.kind === "rtt" ? other.rtt.probe : other.kind === "swo" ? other.swo.probe : null;
  }

  function choose(probe: ProbeInfo) {
    target.probe = probe.id;
    // A chip used with this probe before is most likely the one on the board.
    if (!target.chip) {
      const before = tabs.find((other) => sameProbe(probeOf(other), probe.id));
      const chip = before?.kind === "rtt" ? before.rtt.chip : before?.swo.chip;
      if (chip) {
        target.chip = chip;
        nameChip();
      }
    }
    if (!target.chip && !detectedOnce && !inUse(probe)) {
      detectedOnce = true;
      void detect();
    }
  }

  /** Asks the chip what it is, through the probe chosen. */
  async function detect() {
    if (!target.probe) {
      return;
    }
    detecting = true;
    error = "";
    found = "";
    try {
      const result = await detectChip(target.probe, target.speed_khz);
      if (result.chip) {
        target.chip = result.chip;
        chips = result.candidates;
        nameChip();
        found =
          result.candidates.length > 1
            ? `Found ${result.description}. It comes in other packages too: pick yours from the list if you like.`
            : `Found ${result.description}.`;
      } else {
        found = `Found ${result.description}. Type its name.`;
      }
    } catch (problem) {
      error = String(problem);
    } finally {
      detecting = false;
    }
  }

  async function suggest() {
    chips = target.chip.trim() ? await searchChips(target.chip) : [];
  }

  function nameChip() {
    if (!nameTyped) {
      name = target.chip.trim() ? `${target.chip.trim()} ${LABEL}` : "";
    }
  }

  /** The other tab that has this probe open, if any. */
  function inUse(probe: ProbeInfo): Tab | undefined {
    return tabs.find((other) => other !== tab && other.running && sameProbe(probeOf(other), probe.id));
  }

  function unplugged(probe: ProbeInfo): boolean {
    return listed && !probes.some((listedProbe) => sameProbe(listedProbe.id, probe.id));
  }

  async function browse() {
    const path = await open({
      title: "The firmware's ELF file",
      multiple: false,
      directory: false,
      filters: [
        { name: "ELF files", extensions: ["elf", "axf", "out"] },
        { name: "All files", extensions: ["*"] },
      ],
    });
    if (typeof path === "string") {
      if (KIND === "rtt") {
        rtt.elf = path;
      } else {
        swo.elf = path;
      }
    }
  }

  async function submit() {
    if (!target.probe) {
      error = "Choose a probe. Plug one in if the list is empty.";
      return;
    }
    if (!target.chip.trim()) {
      error = "Enter the chip on the board, such as STM32H503RB.";
      return;
    }
    // probe-rs only knows full chip names, written its way.
    const known = (await searchChips(target.chip, 200)).find(
      (chip) => chip.toUpperCase() === target.chip.trim().toUpperCase(),
    );
    if (!known) {
      error = `probe-rs does not know a chip called "${target.chip.trim()}". Pick one from the list as you type.`;
      return;
    }
    target.chip = known;
    nameChip();
    const sessionName = name.trim() || `${known} ${LABEL}`;

    if (KIND === "rtt") {
      const address = addressText.trim();
      if (address && !/^(0x)?[0-9a-f]+$/i.test(address)) {
        error = "The control block address must be hex, such as 0x20000400. Leave it empty to search RAM.";
        return;
      }
      const config: RttConfig = {
        ...rtt,
        ...target,
        address: address ? (address.toLowerCase().startsWith("0x") ? address : `0x${address}`) : null,
        elf: rtt.elf?.trim() || null,
      };
      save(sessionName, () => tab?.reconfigureRtt(config), JSON.stringify(config) !== JSON.stringify(start?.rtt), {
        rtt: config,
      });
    } else {
      const clockHz = Number(clockText);
      const baud = Number(baudText);
      const problem = swoProblem(clockHz, baud);
      if (problem) {
        error = problem;
        return;
      }
      const config: SwoConfig = { ...swo, ...target, clock_hz: clockHz, baud, elf: swo.elf?.trim() || null };
      save(sessionName, () => tab?.reconfigureSwo(config), JSON.stringify(config) !== JSON.stringify(start?.swo), {
        swo: config,
      });
    }
  }

  /** Changes the tab, or opens a new one, then closes the dialog. */
  function save(sessionName: string, reconfigure: () => void, settingsChanged: boolean, config: object) {
    if (tab) {
      tab.name = sessionName;
      tab.terminal = { ...terminal };
      if (settingsChanged) {
        reconfigure();
      }
      changed();
    } else {
      addTab({
        name: sessionName,
        kind: KIND,
        connected: false,
        log: settings.log.new_tabs,
        terminal: { ...terminal },
        ...config,
      }).start();
    }
    onclose();
  }
</script>

<Modal title={tab ? `${LABEL} settings` : `New ${LABEL} session`} {onclose} onsubmit={submit}>
  <h2 class="dlg-section">Probe</h2>
  <div class="dlg-list" role="listbox" aria-label="Debug probes">
    {#each shown as probe (`${probe.id.vid}:${probe.id.pid}:${probe.id.serial}`)}
      {@const other = inUse(probe)}
      <button
        class="dlg-row probe"
        class:chosen={sameProbe(probe.id, target.probe)}
        role="option"
        aria-selected={sameProbe(probe.id, target.probe)}
        onclick={() => choose(probe)}
      >
        <span class="name">{probe.name}</span>
        <span class="ids mono">
          {probe.id.vid}:{probe.id.pid}
          {#if probe.id.serial}
            <span class="dlg-muted serial" title={`Serial number ${probe.id.serial}`}>SN {probe.id.serial}</span>
          {/if}
        </span>
        {#if unplugged(probe)}
          <span class="dlg-badge">not plugged in</span>
        {:else if other}
          <span class="dlg-badge">open in {other.name}</span>
        {/if}
      </button>
    {:else}
      <p class="dlg-empty">
        {listed
          ? "No debug probes found. Plug in a CMSIS-DAP, J-Link or ST-Link probe: the list updates by itself."
          : "Looking for debug probes..."}
      </p>
    {/each}
  </div>

  {#if KIND === "rtt" && onserver}
    <!-- A probe has one owner: while an IDE debugs through it, it cannot be opened here too. -->
    <p class="dlg-hint server">
      A debugger has the probe? Read RTT from its own server instead, and keep debugging:
      <button class="link" onclick={onserver}>RTT through a debugger's server</button>
    </p>
  {/if}

  <h2 class="dlg-section">Target</h2>
  <label class="dlg-label">
    Chip
    <input
      class="field mono"
      list="chip-names"
      placeholder="STM32H503RB, nRF52840_xxAA, RP2040..."
      autocomplete="off"
      spellcheck="false"
      bind:value={target.chip}
      oninput={() => {
        error = "";
        nameChip();
        void suggest();
      }}
    />
    <datalist id="chip-names">
      {#each chips as chip (chip)}
        <option value={chip}></option>
      {/each}
    </datalist>
  </label>
  <div class="detect">
    <p class="dlg-hint">
      {#if detecting}
        Asking the chip what it is...
      {:else if found}
        {found}
      {:else}
        Type part of the name and pick it from the list, as probe-rs names it, or detect it.
      {/if}
    </p>
    <button class="btn small" disabled={!target.probe || detecting} onclick={detect}>Detect</button>
  </div>

  <label class="dlg-label elf">
    {#if KIND === "swo"}
      Firmware ELF file, for the variables in the Watch view (optional)
    {:else if rtt.watch_only}
      Firmware ELF file, for the variables in the Watch view
    {:else}
      Firmware ELF file, to find RTT quickly and exactly, and the variables (optional)
    {/if}
    <span class="file">
      {#if KIND === "rtt"}
        <input
          class="field mono"
          placeholder={rtt.watch_only ? "Or choose it in the Watch view" : "Without it, RAM is searched"}
          bind:value={rtt.elf}
        />
      {:else}
        <input class="field mono" placeholder="Or choose it in the Watch view" bind:value={swo.elf} />
      {/if}
      <button class="btn" onclick={browse}>Browse...</button>
    </span>
  </label>

  {#if KIND === "rtt"}

    {#if !rtt.watch_only}
      <h2 class="dlg-section">RTT</h2>
    {/if}
    <div class="dlg-grid">
      {#if !rtt.watch_only}
        <label class="dlg-label">
          Up channel (shown)
          <input class="field mono" type="number" min="0" max="15" bind:value={rtt.up_channel} />
        </label>
        <label class="dlg-label">
          Down channel (typed)
          <input class="field mono" type="number" min="0" max="15" bind:value={rtt.down_channel} />
        </label>
        <label class="dlg-label">
          Control block address
          <input class="field mono" placeholder="Search RAM" bind:value={addressText} />
        </label>
      {/if}
      <label class="dlg-label">
        SWD speed
        <select class="field" bind:value={target.speed_khz}>
          {#each SPEEDS as speed (speed)}
            <option value={speed}>{speedText(speed)}</option>
          {/each}
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
  {:else}
    <h2 class="dlg-section">SWO</h2>
    <div class="dlg-grid">
      <label class="dlg-label">
        Core clock in Hz
        <input
          class="field mono"
          inputmode="numeric"
          placeholder="64000000"
          bind:value={clockText}
          oninput={() => (error = "")}
        />
      </label>
      <label class="dlg-label">
        SWO baud rate
        <input class="field mono" list="swo-bauds" inputmode="numeric" bind:value={baudText} oninput={() => (error = "")} />
        <datalist id="swo-bauds">
          {#each SWO_BAUDS as rate (rate)}
            <option value={rate}>{hzText(rate)}</option>
          {/each}
        </datalist>
      </label>
      <label class="dlg-label">
        ITM stimulus port
        <select class="field" bind:value={swo.port} disabled={swo.all_ports}>
          {#each Array.from({ length: 32 }, (_, port) => port) as port (port)}
            <option value={port}>{port}{port === 0 ? " (printf)" : ""}</option>
          {/each}
        </select>
      </label>
      <label class="dlg-label">
        SWD speed
        <select class="field" bind:value={target.speed_khz}>
          {#each SPEEDS as speed (speed)}
            <option value={speed}>{speedText(speed)}</option>
          {/each}
        </select>
      </label>
    </div>
    <p class="dlg-hint">
      The baud rate must divide the clock exactly. The firmware sets the clock; SWO needs the trace pin
      (often PB3 on STM32) wired to the probe.
    </p>
  {/if}

  <div class="dlg-checks">
    <label class="dlg-check" title="For firmware that sleeps or turns the debug pins off. It resets the chip.">
      <input type="checkbox" bind:checked={target.under_reset} /> Connect under reset
    </label>
    {#if KIND === "rtt"}
      <label class="dlg-check" title="Off, only the variables in the Watch view are read: for firmware without RTT">
        <input
          type="checkbox"
          checked={!rtt.watch_only}
          onchange={(event) => (rtt.watch_only = !event.currentTarget.checked)}
        />
        Read RTT
      </label>
      <label class="dlg-check" title="Show what you type, for firmware that does not echo it">
        <input type="checkbox" bind:checked={terminal.local_echo} /> Local echo
      </label>
    {:else}
      <label class="dlg-check" title="Show what every stimulus port writes, mixed together">
        <input type="checkbox" bind:checked={swo.all_ports} /> All ports
      </label>
    {/if}
    <label class="dlg-check" title="For firmware that ends lines with LF only">
      <input type="checkbox" bind:checked={terminal.lf_is_newline} /> New line on LF
    </label>
  </div>

  <label class="dlg-label tab-name">
    Tab name
    <input class="field" bind:value={name} oninput={() => (nameTyped = true)} placeholder={`Chip ${LABEL}`} />
  </label>

  {#if error}
    <p class="dlg-error">{error}</p>
  {/if}

  {#snippet buttons()}
    <button class="btn" onclick={onclose}>Cancel</button>
    <button class="btn primary" disabled={!target.probe} onclick={submit}>
      {tab ? "Save" : "Connect"}
    </button>
  {/snippet}
</Modal>

<style>
  .probe {
    grid-template-columns: minmax(0, 1fr) auto auto;
  }

  .name {
    font-weight: 600;
  }

  .ids {
    display: flex;
    gap: 8px;
    font-size: 12px;
    white-space: nowrap;
  }

  .serial {
    max-width: 150px;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .detect {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
  }

  .detect .btn {
    flex-shrink: 0;
    height: 24px;
    margin-top: 4px;
    padding: 0 10px;
    font-size: 12px;
  }

  .elf {
    margin-top: 12px;
  }

  .file {
    display: flex;
    gap: 8px;
  }

  .file .field {
    flex: 1;
    min-width: 0;
  }

  .tab-name {
    margin-top: 14px;
  }

  .server {
    margin-top: 8px;
  }

  .link {
    padding: 0;
    font: inherit;
    color: var(--accent);
    text-decoration: underline;
    background: none;
    border: 0;
    cursor: pointer;
  }
</style>
