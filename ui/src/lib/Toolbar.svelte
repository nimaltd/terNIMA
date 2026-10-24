<script lang="ts">
  import { channelText, hzText } from "./probe";
  import { summary, type LineEnding, type ModemLines } from "./serial";
  import { changed, type Tab } from "./tabs.svelte";
  import { activeTerminal } from "./terminal-actions";

  let { tab, onsettings }: { tab: Tab; onsettings: () => void } = $props();

  const LINES: { key: keyof ModemLines; label: string; title: string }[] = [
    { key: "cts", label: "CTS", title: "Clear to send" },
    { key: "dsr", label: "DSR", title: "Data set ready" },
    { key: "dcd", label: "DCD", title: "Carrier detect" },
    { key: "ri", label: "RI", title: "Ring indicator" },
  ];

  /** Does something, then gives the keyboard back to the terminal. */
  function act(action: () => void) {
    action();
    activeTerminal()?.focus();
  }
</script>

<div class="toolbar">
  {#if tab.running}
    <button class="btn" onclick={() => act(() => tab.stop())}>Disconnect</button>
  {:else}
    <button class="btn primary" onclick={() => act(() => tab.start())}>Connect</button>
  {/if}

  {#if tab.kind === "serial"}
    <button class="btn target" title="Port settings" onclick={onsettings}>
      <span class="mono">{tab.port.name}</span>
      <span class="muted mono">{summary(tab.serial)}</span>
      <span class="muted">▾</span>
    </button>

    <span class="separator"></span>

    <label class="check" title="Data terminal ready: an output line some boards use for reset">
      <input
        type="checkbox"
        checked={tab.serial.dtr}
        onchange={(event) => act(() => tab.setDtr(event.currentTarget.checked))}
      />
      DTR
    </label>
    <label
      class="check"
      title={tab.serial.flow === "rtscts"
        ? "RTS is used by RTS/CTS flow control"
        : "Request to send: an output line some boards use for boot mode"}
    >
      <input
        type="checkbox"
        checked={tab.serial.rts}
        disabled={tab.serial.flow === "rtscts"}
        onchange={(event) => act(() => tab.setRts(event.currentTarget.checked))}
      />
      RTS
    </label>
    <button
      class="btn small"
      disabled={!tab.connected}
      title="Hold the TX line low for 250 ms"
      onclick={() => act(() => tab.sendBreak())}
    >
      Break
    </button>

    <span class="separator"></span>

    <div class="leds" title="Input lines from the device">
      {#each LINES as line (line.key)}
        <span class="led" class:on={tab.lines?.[line.key] ?? false} title={line.title}>{line.label}</span>
      {/each}
    </div>
  {:else if tab.kind === "rtt"}
    <button class="btn target" title="RTT settings" onclick={onsettings}>
      <span class="mono">{tab.rtt.chip || "No chip"}</span>
      <span class="muted">RTT</span>
      <span class="muted">▾</span>
    </button>

    <span class="separator"></span>

    <label class="option" title="The up channel shown in the terminal">
      Channel
      <select
        class="field"
        value={tab.rtt.up_channel}
        onchange={(event) =>
          act(() => tab.reconfigureRtt({ ...tab.rtt, up_channel: Number(event.currentTarget.value) }))}
      >
        {#if tab.channels && tab.channels.up.length > 0}
          {#each tab.channels.up as channel (channel.number)}
            <option value={channel.number}>{channelText(channel)}</option>
          {/each}
        {:else}
          <option value={tab.rtt.up_channel}>{tab.rtt.up_channel}</option>
        {/if}
      </select>
    </label>
    <button
      class="btn small"
      disabled={!tab.connected}
      title="Reset the chip through the probe, then find RTT again"
      onclick={() => act(() => tab.resetTarget())}
    >
      Reset target
    </button>
  {:else}
    <button class="btn target" title="SWO settings" onclick={onsettings}>
      <span class="mono">{tab.swo.chip || "No chip"}</span>
      <span class="muted">SWO {hzText(tab.swo.baud)}</span>
      <span class="muted">▾</span>
    </button>

    <span class="separator"></span>

    <label class="option" title="The ITM stimulus port shown">
      Port
      <select
        class="field"
        value={tab.swo.all_ports ? "all" : String(tab.swo.port)}
        onchange={(event) =>
          act(() => {
            const value = event.currentTarget.value;
            tab.reconfigureSwo(
              value === "all"
                ? { ...tab.swo, all_ports: true }
                : { ...tab.swo, all_ports: false, port: Number(value) },
            );
          })}
      >
        <option value="all">All</option>
        {#each Array.from({ length: 32 }, (_, port) => port) as port (port)}
          <option value={String(port)}>{port}</option>
        {/each}
      </select>
    </label>
    <button
      class="btn small"
      disabled={!tab.connected}
      title="Reset the chip through the probe, then turn SWO on again"
      onclick={() => act(() => tab.resetTarget())}
    >
      Reset target
    </button>
  {/if}

  <span class="spacer"></span>

  <button
    class="btn small log"
    class:on={tab.log}
    title={tab.logPath ?? (tab.log ? "The log starts when the tab connects" : "Write what this tab receives to a log file")}
    onclick={() => act(() => tab.setLog(!tab.log))}
  >
    <span class="dot"></span>
    Log
  </button>

  {#if tab.kind === "swo"}
    <span class="muted" title="SWO only comes from the chip">Output only</span>
  {:else}
    <label class="option" title="What the Enter key sends">
      Enter
      <select
        class="field"
        value={tab.terminal.enter}
        onchange={(event) =>
          act(() => {
            tab.terminal.enter = event.currentTarget.value as LineEnding;
            changed();
          })}
      >
        <option value="cr">CR</option>
        <option value="lf">LF</option>
        <option value="crlf">CR LF</option>
      </select>
    </label>
    <label class="check" title="Show what you type, for devices that do not echo it">
      <input
        type="checkbox"
        checked={tab.terminal.local_echo}
        onchange={(event) =>
          act(() => {
            tab.terminal.local_echo = event.currentTarget.checked;
            changed();
          })}
      />
      Echo
    </label>
  {/if}
</div>

<style>
  .toolbar {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 40px;
    padding: 0 10px;
    overflow: hidden;
    white-space: nowrap;
    background: var(--bg);
    border-bottom: 1px solid var(--border);
  }

  .target {
    gap: 10px;
  }

  .muted {
    color: var(--muted);
  }

  .small {
    height: 24px;
    padding: 0 8px;
    font-size: 12px;
  }

  .separator {
    width: 1px;
    height: 20px;
    background: var(--border);
  }

  .spacer {
    flex: 1;
  }

  .check,
  .option {
    display: flex;
    align-items: center;
    gap: 5px;
    color: var(--muted);
  }

  .check:has(input:checked) {
    color: var(--text);
  }

  .option select {
    height: 24px;
    padding: 0 4px;
  }

  .log .dot {
    width: 8px;
    height: 8px;
    border: 1.5px solid var(--muted);
    border-radius: 50%;
  }

  .log.on {
    color: var(--text);
    border-color: var(--bad);
  }

  .log.on .dot {
    background: var(--bad);
    border-color: var(--bad);
  }

  .leds {
    display: flex;
    gap: 4px;
  }

  .led {
    padding: 2px 6px;
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
    border: 1px solid var(--border);
    border-radius: 4px;
    opacity: 0.6;
  }

  .led.on {
    color: var(--on-accent);
    background: var(--ok);
    border-color: var(--ok);
    opacity: 1;
  }
</style>
