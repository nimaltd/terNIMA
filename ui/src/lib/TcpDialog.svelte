<script lang="ts">
  import { untrack } from "svelte";
  import Modal from "./Modal.svelte";
  import { DEFAULT_TCP, PROTOCOLS, address, type Protocol, type TcpConfig } from "./net";
  import { DEFAULT_TERMINAL, type TerminalOptions } from "./serial";
  import { settings } from "./settings.svelte";
  import { addTab, changed, type Tab } from "./tabs.svelte";

  let {
    tab = null,
    protocol: preset = "raw",
    onclose,
  }: {
    /** The TCP tab whose settings to change, or null for a new session. */
    tab?: Tab | null;
    /** What a new session connects to, to start with. */
    protocol?: Protocol;
    onclose: () => void;
  } = $props();

  // A copy of the tab's settings, taken once: nothing changes until Save.
  const start = untrack(() => tab?.setup() ?? null);
  const first: TcpConfig = untrack(
    () => start?.tcp ?? { ...DEFAULT_TCP, protocol: preset, port: PROTOCOLS[preset].port ?? 0 },
  );
  let protocol: Protocol = $state(first.protocol);
  let host = $state(first.host);
  let portText = $state(first.port ? String(first.port) : "");
  let channelText = $state(String(first.channel));
  let terminal: TerminalOptions = $state({ ...(start?.terminal ?? DEFAULT_TERMINAL) });
  let name = $state(start?.name ?? "");
  let error = $state("");

  /** The tab's name when none is typed: the RTT server, or where it connects. */
  const suggested = $derived.by(() => {
    if (protocol === "jlink" || protocol === "openocd") {
      return PROTOCOLS[protocol].name;
    }
    const at = host.trim() || "localhost";
    const port = Number(portText);
    return Number.isInteger(port) && port > 0 ? address({ ...DEFAULT_TCP, host: at, port }) : at;
  });

  /** A new protocol brings its usual port, unless another port was typed. */
  function pickProtocol(next: Protocol) {
    const usual = PROTOCOLS[protocol].port;
    if (portText.trim() === "" || Number(portText) === usual) {
      portText = PROTOCOLS[next].port ? String(PROTOCOLS[next].port) : "";
    }
    protocol = next;
    error = "";
  }

  function submit() {
    const port = Number(portText);
    if (!host.trim()) {
      error = "Enter the host: a name or an IP address, such as localhost.";
      return;
    }
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      error = "Enter the port: a whole number from 1 to 65535.";
      return;
    }
    const channel = protocol === "jlink" ? Number(channelText) : 0;
    if (!Number.isInteger(channel) || channel < 0) {
      error = "Enter the RTT channel: 0, 1, 2 and so on.";
      return;
    }
    const tcp: TcpConfig = { host: host.trim(), port, protocol, channel };
    const sessionName = name.trim() || suggested;

    if (tab) {
      tab.name = sessionName;
      tab.terminal = { ...terminal };
      if (JSON.stringify(tcp) !== JSON.stringify(start?.tcp)) {
        tab.reconfigureTcp(tcp);
      }
      changed();
    } else {
      addTab({
        name: sessionName,
        kind: "tcp",
        connected: false,
        log: settings.log.new_tabs,
        tcp,
        terminal: { ...terminal },
      }).start();
    }
    onclose();
  }
</script>

<Modal title={tab ? "Connection settings" : "New TCP session"} {onclose} onsubmit={submit}>
  <h2 class="dlg-section">Connection</h2>
  <div class="dlg-grid">
    <label class="dlg-label wide">
      Connect to
      <select
        class="field"
        value={protocol}
        onchange={(event) => pickProtocol(event.currentTarget.value as Protocol)}
      >
        <option value="raw">A TCP socket: plain bytes both ways</option>
        <option value="telnet">A Telnet server</option>
        <option value="jlink">J-Link's RTT server, while a debugger has the probe</option>
        <option value="openocd">OpenOCD's RTT server, while a debugger has the probe</option>
      </select>
    </label>
    <label class="dlg-label">
      Host
      <input
        class="field mono"
        spellcheck="false"
        placeholder="localhost"
        bind:value={host}
        oninput={() => (error = "")}
      />
    </label>
    <label class="dlg-label">
      Port
      <input class="field mono" inputmode="numeric" bind:value={portText} oninput={() => (error = "")} />
    </label>
    {#if protocol === "jlink"}
      <label class="dlg-label" title="The RTT channel J-Link sends and types to. 0 is the terminal.">
        RTT channel
        <input class="field mono" inputmode="numeric" bind:value={channelText} oninput={() => (error = "")} />
      </label>
    {/if}
  </div>
  <p class="dlg-hint about">{PROTOCOLS[protocol].hint}</p>

  <h2 class="dlg-section">Terminal</h2>
  <div class="dlg-grid">
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
    <label class="dlg-check" title="Show what you type, for devices that do not echo it">
      <input type="checkbox" bind:checked={terminal.local_echo} /> Local echo
    </label>
    <label class="dlg-check" title="For devices that end lines with LF only">
      <input type="checkbox" bind:checked={terminal.lf_is_newline} /> New line on LF
    </label>
  </div>

  <label class="dlg-label tab-name">
    Tab name
    <input class="field" bind:value={name} placeholder={suggested} />
  </label>

  {#if error}
    <p class="dlg-error">{error}</p>
  {/if}

  {#snippet buttons()}
    <button class="btn" onclick={onclose}>Cancel</button>
    <button class="btn primary" onclick={submit}>{tab ? "Save" : "Connect"}</button>
  {/snippet}
</Modal>

<style>
  .wide {
    grid-column: 1 / -1;
  }

  .about {
    margin-top: 8px;
  }

  .tab-name {
    margin-top: 14px;
  }
</style>
