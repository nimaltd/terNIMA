<script lang="ts">
  import { onMount, untrack, type Snippet } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { writeText } from "@tauri-apps/plugin-clipboard-manager";
  import { save } from "@tauri-apps/plugin-dialog";
  import { FUNCTION_NAMES, type FunctionCode, type Request } from "./modbus";
  import { formatRegisters, isBad, type RegisterFormat, type TrafficFrame } from "./modbus.svelte";
  import { changed, type Tab } from "./tabs.svelte";
  import { clearActiveTerminal, setActiveTerminal, type TerminalActions } from "./terminal-actions";

  let {
    tab,
    active,
    head,
  }: {
    tab: Tab;
    /** Whether this view has the keyboard. */
    active: boolean;
    /** The start of the bar: the page and the mode, as the Modbus view has them. */
    head: Snippet;
  } = $props();

  /** The height of a row, in pixels, the same as `.row` below. */
  const ROW = 20;
  /** How many of the last frames shown are looked at again: a request is found unanswered when the next one comes. */
  const TAIL = 16;
  const FORMATS: { value: RegisterFormat; label: string }[] = [
    { value: "unsigned", label: "Unsigned" },
    { value: "signed", label: "Signed" },
    { value: "hex", label: "Hex" },
    { value: "float", label: "Float (high word first)" },
    { value: "float_swapped", label: "Float (low word first)" },
  ];

  const setup = $derived(tab.modbusSetup);
  const session = $derived(tab.modbus);
  const ascii = $derived(setup.mode === "ascii");

  let list: HTMLDivElement;
  let scrollTop = $state(0);
  let height = $state(0);
  /** Whether the list keeps showing the newest frame as frames come. */
  let follow = $state(true);
  let onlyBad = $state(false);
  let unitFilter: number | null = $state(null);
  let functionFilter: number | null = $state(null);
  /** The frame picked, by number, whose details show below the list. */
  let picked: number | null = $state(null);

  /** Where the frames shown are in `session.traffic`; `shownVersion` changes with it. */
  let positions: number[] = [];
  let count = $state(0);
  let shownVersion = $state(0);
  let scanned = 0;
  let firstFrame = -1;

  const shows = (frame: TrafficFrame) =>
    (!onlyBad || isBad(frame)) &&
    (unitFilter === null || frame.unit === unitFilter) &&
    (functionFilter === null || frame.fc === functionFilter);

  /** Works out which frames are shown: all again, or the new ones and the last few. */
  function refilter(full: boolean) {
    const frames = session.traffic;
    const first = frames[0]?.n ?? -1;
    if (full || first !== firstFrame || scanned > frames.length) {
      positions = [];
      scanned = 0;
      firstFrame = first;
    }
    const from = Math.max(0, scanned - TAIL);
    while (positions.length > 0 && positions[positions.length - 1] >= from) positions.pop();
    for (let at = from; at < frames.length; at++) {
      if (shows(frames[at])) positions.push(at);
    }
    scanned = frames.length;
    count = positions.length;
    shownVersion++;
    if (follow) {
      requestAnimationFrame(() => list && (list.scrollTop = list.scrollHeight));
    }
  }

  $effect(() => {
    void [onlyBad, unitFilter, functionFilter];
    untrack(() => refilter(true));
  });
  $effect(() => {
    void session.trafficVersion;
    untrack(() => refilter(false));
  });

  /** The units and functions seen, for the filters. */
  const units = $derived.by(() => {
    void session.trafficVersion;
    return [...session.units].sort((a, b) => a - b);
  });
  const functions = $derived.by(() => {
    void session.trafficVersion;
    return [...session.functions].sort((a, b) => a - b);
  });

  /** The rows on screen, and a few more either side. */
  const rows = $derived.by(() => {
    void shownVersion;
    const start = Math.max(0, Math.floor(scrollTop / ROW) - 5);
    const end = Math.min(count, Math.ceil((scrollTop + height) / ROW) + 5);
    const shown: { index: number; frame: TrafficFrame }[] = [];
    for (let index = start; index < end; index++) {
      const frame = session.traffic[positions[index]];
      if (frame) shown.push({ index, frame });
    }
    return shown;
  });

  /** The frame picked, while it is still kept. Frames are numbered one after another. */
  const pickedFrame = $derived.by(() => {
    void shownVersion;
    const first = session.traffic[0]?.n;
    if (picked === null || first === undefined) return null;
    return session.traffic[picked - first] ?? null;
  });

  function time(at: number): string {
    const date = new Date(at);
    const two = (value: number) => String(value).padStart(2, "0");
    return `${two(date.getHours())}:${two(date.getMinutes())}:${two(date.getSeconds())}.${String(date.getMilliseconds()).padStart(3, "0")}`;
  }

  const hexByte = (byte: number) => byte.toString(16).toUpperCase().padStart(2, "0");
  const hex = (bytes: Uint8Array, max = Infinity) => {
    const text = Array.from(bytes.subarray(0, max), hexByte).join(" ");
    return bytes.length > max ? `${text} …` : text;
  };
  const textOf = (bytes: Uint8Array) => new TextDecoder().decode(bytes).trim();

  function functionText(fc: number | null): string {
    if (fc === null) return "";
    return `${fc} ${FUNCTION_NAMES[fc as FunctionCode] ?? "unknown"}`;
  }

  /** The addresses a request reads or writes: "0-9", or "2". */
  function range(request: Request | null, frame: TrafficFrame): string {
    if (request) {
      return request.count > 1 ? `${request.address}-${request.address + request.count - 1}` : String(request.address);
    }
    return frame.response?.address !== undefined ? String(frame.response.address) : "";
  }

  /** Registers in the format Show picks; a float's second register shows nothing. */
  const registers = (values: number[]) => formatRegisters(values, setup.format).filter((text) => text !== "");

  /** What a frame asks or says, in short. */
  function valuesText(frame: TrafficFrame): string {
    if (frame.problem && !frame.ok) return frame.problem;
    const { request, response } = frame;
    if (frame.kind === "request" && request) {
      switch (request.fc) {
        case 1:
          return `${request.count} coils`;
        case 2:
          return `${request.count} inputs`;
        case 3:
        case 4:
          return `${request.count} register${request.count === 1 ? "" : "s"}`;
        case 5:
        case 15:
          return request.values.join(" ");
        default:
          return registers(request.values).join("  ");
      }
    }
    if (!response) return "";
    if (frame.problem) return frame.problem;
    if (response.registers) return registers(response.registers).join("  ");
    if (response.bits) return response.bits.map(Number).join(" ");
    if (response.value !== undefined) return `wrote ${response.value}`;
    return `wrote ${response.count}`;
  }

  /** What a frame's bytes are, for the details: the unit, the function, its fields and the check. */
  function partsOf(frame: TrafficFrame): string[] {
    const bytes = frame.bytes;
    const parts: string[] = new Array(bytes.length).fill("");
    if (!frame.checked || bytes.length < 3) return parts;
    const end = bytes.length - (ascii ? 1 : 2);
    const set = (from: number, to: number, part: string) => {
      for (let at = from; at < Math.min(to, end); at++) parts[at] = part;
    };
    set(0, 1, "b-unit");
    set(1, 2, "b-function");
    const fc = bytes[1];
    if (fc & 0x80) {
      set(2, 3, "b-values");
    } else if (frame.kind === "response" && fc <= 4) {
      set(2, 3, "b-count");
      set(3, end, "b-values");
    } else if (frame.kind === "request" && (fc === 15 || fc === 16)) {
      set(2, 4, "b-address");
      set(4, 7, "b-count");
      set(7, end, "b-values");
    } else if (frame.kind !== null) {
      set(2, 4, "b-address");
      set(4, 6, fc === 5 || fc === 6 ? "b-values" : "b-count");
    }
    for (let at = end; at < bytes.length; at++) parts[at] = "b-check";
    return parts;
  }
  const pickedParts = $derived(pickedFrame ? partsOf(pickedFrame) : []);

  /** The picked frame's fields, and each value it carries with its address. */
  function fieldsOf(frame: TrafficFrame): { name: string; value: string; note: string }[] {
    const fields: { name: string; value: string; note: string }[] = [];
    if (frame.unit === null) return fields;
    fields.push({ name: "Unit", value: String(frame.unit), note: frame.unit === 0 ? "every unit" : "" });
    fields.push({ name: "Function", value: functionText(frame.fc), note: "" });
    const { request, response } = frame;
    if (frame.kind === "request" && request) {
      fields.push({ name: "Address", value: String(request.address), note: "" });
      if (request.fc <= 4 || request.fc >= 15) fields.push({ name: "Count", value: String(request.count), note: "" });
      if (request.fc >= 5) {
        const shown = request.fc === 6 || request.fc === 16 ? formatRegisters(request.values, setup.format) : request.values.map(String);
        request.values.forEach((value, index) =>
          fields.push({ name: String(request.address + index), value: shown[index], note: `0x${value.toString(16).toUpperCase().padStart(4, "0")}` }),
        );
      }
      return fields;
    }
    if (!response) return fields;
    if (response.exception !== undefined) {
      fields.push({ name: "Exception", value: String(response.exception), note: frame.problem?.replace(/^exception \d+: /, "") ?? "" });
      return fields;
    }
    const from = request?.address ?? 0;
    const name = (index: number) => (request ? String(from + index) : `#${index}`);
    if (response.registers) {
      const shown = formatRegisters(response.registers, setup.format);
      response.registers.forEach((value, index) =>
        fields.push({ name: name(index), value: shown[index], note: `0x${value.toString(16).toUpperCase().padStart(4, "0")}` }),
      );
    } else if (response.bits) {
      response.bits.forEach((bit, index) => fields.push({ name: name(index), value: bit ? "1" : "0", note: "" }));
    } else {
      fields.push({ name: "Address", value: String(response.address), note: "" });
      fields.push(
        response.value !== undefined
          ? { name: "Value", value: String(response.value), note: "" }
          : { name: "Count", value: String(response.count), note: "" },
      );
    }
    return fields;
  }

  /** The CRC or LRC at the end of a frame, and whether it is right. */
  function checkText(frame: TrafficFrame): string {
    const bytes = frame.bytes;
    if (bytes.length < 3) return "";
    if (ascii) return `LRC 0x${hexByte(bytes[bytes.length - 1])}, ${frame.checked ? "right" : "wrong"}`;
    const crc = bytes[bytes.length - 2] | (bytes[bytes.length - 1] << 8);
    return `CRC 0x${crc.toString(16).toUpperCase().padStart(4, "0")}, ${frame.checked ? "right" : "wrong"}`;
  }

  function pick(frame: TrafficFrame) {
    picked = frame.n;
    list.focus();
  }

  /** Moves the pick to the frame shown `step` rows away, and shows it. */
  function move(step: number) {
    if (count === 0) return;
    const index = picked === null ? (step > 0 ? -1 : count) : positions.findIndex((at) => session.traffic[at].n === picked);
    const next = Math.max(0, Math.min(count - 1, index + step));
    picked = session.traffic[positions[next]].n;
    const top = next * ROW;
    if (top < list.scrollTop) list.scrollTop = top;
    else if (top + ROW > list.scrollTop + list.clientHeight) list.scrollTop = top + ROW - list.clientHeight;
  }

  function onscroll() {
    scrollTop = list.scrollTop;
    follow = list.scrollTop + list.clientHeight >= list.scrollHeight - ROW;
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      follow = false;
      move(event.key === "ArrowDown" ? 1 : -1);
    } else if (event.key === "Escape" && picked !== null) {
      event.preventDefault();
      picked = null;
    }
  }

  /** The frames shown, as CSV: one row each, with what it asks or says and its bytes. */
  function csv(): string {
    const field = (text: string) => (/[",\n]/.test(text) ? `"${text.replaceAll('"', '""')}"` : text);
    const out = [["time", "kind", "sent", "unit", "function", "address", "values", "ms", "ok", "problem", "bytes"].join(",")];
    for (const at of positions) {
      const frame = session.traffic[at];
      out.push(
        [
          new Date(frame.at).toISOString(),
          frame.kind ?? "bad",
          frame.sent ? "yes" : "no",
          frame.unit === null ? "" : String(frame.unit),
          frame.fc === null ? "" : field(functionText(frame.fc)),
          range(frame.request, frame),
          field(frame.ok ? valuesText(frame) : ""),
          frame.ms === null ? "" : String(frame.ms),
          isBad(frame) ? "no" : "yes",
          field(frame.unanswered ? "no answer" : (frame.problem ?? "")),
          ascii ? field(textOf(frame.raw)) : hex(frame.raw),
        ].join(","),
      );
    }
    return `${out.join("\n")}\n`;
  }

  async function exportCsv() {
    const path = await save({
      title: "Export the Modbus traffic as CSV",
      defaultPath: `${tab.name.replace(/[<>:"/\\|?*]/g, "_")} Modbus.csv`,
      filters: [{ name: "CSV", extensions: ["csv"] }],
    });
    if (path) {
      await invoke("save_text", { path, text: csv() }).catch((error) => console.error(error));
    }
    list.focus();
  }

  const actions: TerminalActions = {
    hasSelection: () => pickedFrame !== null,
    copy: async () => {
      if (pickedFrame) await writeText(ascii ? textOf(pickedFrame.raw) : hex(pickedFrame.raw));
    },
    paste: async () => {},
    selectAll: () => {},
    clear: () => {
      picked = null;
      session.clearTraffic();
    },
    text: () => csv(),
    focus: () => list?.focus(),
    openSearch: () => {},
    findNext: () => {},
    findPrevious: () => {},
    exportCsv: () => void exportCsv(),
  };

  // The view on screen is the one the menus act on, and has the keyboard.
  $effect(() => {
    if (active) {
      setActiveTerminal(actions);
      list.focus();
    }
  });

  onMount(() => {
    const resize = new ResizeObserver(() => (height = list.clientHeight));
    resize.observe(list);
    height = list.clientHeight;
    return () => {
      resize.disconnect();
      clearActiveTerminal(actions);
    };
  });
</script>

<div class="traffic">
  <div class="bar">
    {@render head()}
    <label class="option" title="How register values are shown">
      Show
      <select class="field" bind:value={setup.format} onchange={() => changed()}>
        {#each FORMATS as { value, label } (value)}
          <option {value}>{label}</option>
        {/each}
      </select>
    </label>
    <label class="option" title="Only the frames to or from one unit">
      Unit
      <select class="field" bind:value={unitFilter}>
        <option value={null}>All</option>
        {#each units as unit (unit)}
          <option value={unit}>{unit}</option>
        {/each}
      </select>
    </label>
    <label class="option" title="Only the frames of one function">
      Function
      <select class="field" bind:value={functionFilter}>
        <option value={null}>All</option>
        {#each functions as fc (fc)}
          <option value={fc}>{functionText(fc)}</option>
        {/each}
      </select>
    </label>
    <label class="check" title="Only broken frames, exceptions, and requests nothing answered">
      <input type="checkbox" bind:checked={onlyBad} /> Only bad
    </label>

    <span class="spacer"></span>
    <span class="total" title="Frames kept, and those broken, answered with an exception, or not answered">
      {(void session.trafficVersion, session.traffic.length).toLocaleString()} frames,
      <span class:bad={(void session.trafficVersion, session.badFrames) > 0}>{(void session.trafficVersion, session.badFrames).toLocaleString()} bad</span>
    </span>
    <button class="btn small" disabled={count === 0} title="Save the frames shown to a CSV file" onclick={exportCsv}>CSV...</button>
    <button class="btn small" title="Forget the frames kept (Ctrl+Shift+K)" onclick={() => actions.clear()}>Clear</button>
  </div>

  <div class="header row" aria-hidden="true">
    <span class="time">Time</span>
    <span class="arrow"></span>
    <span class="unit">Unit</span>
    <span class="function">Function</span>
    <span class="address">Address</span>
    <span class="values">Values</span>
    <span class="ms">Time to answer</span>
    <span class="mark">Check</span>
  </div>

  <div class="listbox">
    {#if !follow && count > 0}
      <button
        class="btn latest"
        title="Go to the newest frame and keep showing it"
        onclick={() => {
          follow = true;
          list.scrollTop = list.scrollHeight;
          list.focus();
        }}
      >
        Latest ↓
      </button>
    {/if}

    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div class="list" tabindex="0" role="grid" aria-rowcount={count} bind:this={list} {onscroll} {onkeydown}>
      <div class="space" style:height={`${count * ROW}px`}>
        {#each rows as { index, frame } (frame.n)}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <div
            class="row {frame.kind ?? 'broken'}"
            class:picked={frame.n === picked}
            class:bad={isBad(frame)}
            role="row"
            tabindex="-1"
            style:top={`${index * ROW}px`}
            onmousedown={() => pick(frame)}
          >
            <span class="time mono" title={frame.sent ? "Sent by terNIMA" : "Received"}>{time(frame.at)}</span>
            <span class="arrow" title={frame.kind === "request" ? "A request, master to slave" : frame.kind === "response" ? "A response, slave to master" : "Not a whole, right frame"}>
              {frame.kind === "request" ? "→" : frame.kind === "response" ? "←" : "!"}
            </span>
            <span class="unit mono">{frame.unit ?? ""}</span>
            <span class="function">{functionText(frame.fc)}</span>
            <span class="address mono">{frame.ok ? range(frame.request, frame) : ""}</span>
            <span class="values mono" class:problem={frame.problem !== null} title={valuesText(frame)}>{valuesText(frame)}</span>
            <span class="ms mono" class:problem={frame.unanswered}>
              {frame.unanswered ? "no answer" : frame.ms === null ? "" : `${frame.ms} ms`}
            </span>
            <span class="mark" title={frame.checked ? (ascii ? "Its LRC is right" : "Its CRC is right") : (frame.problem ?? "")}>
              {frame.checked ? "✓" : "✗"}
            </span>
          </div>
        {/each}
      </div>
      {#if count === 0}
        <p class="empty">
          {#if session.traffic.length > 0}
            No frame is like that.
          {:else}
            Every Modbus frame on this port is listed here as it comes: what terNIMA asks or answers in the Tester,
            and what a master and its slaves say on a bus terNIMA only listens to. Each response is under its request,
            with the time it took. Pick RTU or ASCII as the bus has it.
          {/if}
        </p>
      {/if}
    </div>
  </div>

  {#if pickedFrame}
    <div class="details">
      <div class="bytes">
        {#if ascii}
          <p class="mono line">{textOf(pickedFrame.raw)}</p>
        {/if}
        <div class="hexdump mono" aria-label="The frame's bytes">
          {#each { length: Math.ceil(pickedFrame.bytes.length / 16) } as _, row (row)}
            <div class="hexrow">
              <span class="offset">{(row * 16).toString(16).toUpperCase().padStart(4, "0")}</span>
              {#each pickedFrame.bytes.subarray(row * 16, row * 16 + 16) as byte, column (column)}
                <span class="byte {pickedParts[row * 16 + column]}">{hexByte(byte)}</span>
              {/each}
            </div>
          {/each}
        </div>
        {#if pickedFrame.checked}
          <p class="legend">
            <span class="byte b-unit">unit</span> <span class="byte b-function">function</span>
            <span class="byte b-address">address</span> <span class="byte b-count">count</span>
            <span class="byte b-values">values</span> <span class="byte b-check">check</span>
          </p>
        {/if}
      </div>
      <div class="fields">
        <p class="summary">
          {time(pickedFrame.at)} · {pickedFrame.sent ? "sent by terNIMA" : "received"} · {pickedFrame.bytes.length} bytes
          {#if pickedFrame.ms !== null}· answered in {pickedFrame.ms} ms{/if}
          {#if pickedFrame.unanswered}· <span class="problem">nothing answered it</span>{/if}
        </p>
        {#if pickedFrame.problem}
          <p class="problem">{pickedFrame.problem}</p>
        {/if}
        <table>
          <tbody>
            {#each fieldsOf(pickedFrame) as field, index (index)}
              <tr>
                <td class="name">{field.name}</td>
                <td class="value">{field.value}</td>
                <td class="note">{field.note}</td>
              </tr>
            {/each}
          </tbody>
        </table>
        <p class="legend">{checkText(pickedFrame)}</p>
      </div>
    </div>
  {/if}
</div>

<style>
  .traffic {
    position: relative;
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }

  .bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-end;
    gap: 6px 10px;
    padding-bottom: 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .option,
  .check {
    display: flex;
    gap: 6px;
    align-items: center;
    white-space: nowrap;
  }

  .check {
    gap: 4px;
  }

  .bar .field {
    height: 24px;
    font-size: 12px;
  }

  .small {
    height: 24px;
    padding: 0 8px;
    font-size: 12px;
  }

  .spacer {
    flex: 1;
  }

  /* Numbers keep their width as they grow, so nothing next to them moves. */
  .total {
    min-width: 22ch;
    font-variant-numeric: tabular-nums;
    text-align: right;
    white-space: nowrap;
  }

  .total .bad {
    color: var(--bad);
  }

  .latest {
    position: absolute;
    right: 24px;
    bottom: 12px;
    z-index: 2;
    height: 28px;
    padding: 0 12px;
    font-size: 12px;
    box-shadow: var(--shadow);
  }

  /* The list, with the way back to the newest frame over its bottom. */
  .listbox {
    position: relative;
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 80px;
  }

  .list {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: auto;
    outline: none;
    scrollbar-gutter: stable;
  }

  .space {
    position: relative;
  }

  .row {
    position: absolute;
    right: 0;
    left: 0;
    display: flex;
    gap: 12px;
    align-items: center;
    height: 20px;
    padding: 0 8px;
    font-size: 13px;
    white-space: nowrap;
  }

  /* The column names, in line with the rows below them. */
  .header {
    position: static;
    flex: none;
    overflow: hidden;
    font-size: 11px;
    color: var(--muted);
    border-bottom: 1px solid var(--border);
    scrollbar-gutter: stable;
  }

  .list .row:hover {
    background: var(--hover);
  }

  .row.picked {
    background: var(--accent-soft);
  }

  .time {
    flex: none;
    width: 12ch;
    color: var(--muted);
  }

  .header .time {
    font-family: var(--font-mono);
  }

  .arrow {
    flex: none;
    width: 12px;
    text-align: center;
  }

  .request .arrow {
    color: var(--accent);
  }

  .response .arrow {
    color: var(--ok);
  }

  .broken .arrow,
  .problem {
    color: var(--bad);
  }

  .unit {
    flex: none;
    width: 4ch;
    text-align: right;
  }

  .function {
    flex: none;
    width: 200px;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .address {
    flex: none;
    width: 11ch;
  }

  .values {
    flex: 1;
    min-width: 120px;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .ms {
    flex: none;
    width: 13ch;
    color: var(--muted);
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  .header .ms {
    font-family: var(--font-mono);
  }

  .mark {
    flex: none;
    width: 5ch;
    color: var(--ok);
    text-align: center;
  }

  .header .mark {
    color: var(--muted);
  }

  .row.broken .mark,
  .row.broken .values {
    color: var(--bad);
  }

  .empty {
    margin: 40px auto;
    max-width: 600px;
    padding: 0 24px;
    font-size: 13px;
    line-height: 1.6;
    color: var(--muted);
    text-align: center;
  }

  .details {
    display: flex;
    flex-wrap: wrap;
    gap: 10px 28px;
    max-height: 40%;
    padding: 10px 8px 4px;
    overflow: auto;
    font-size: 12px;
    border-top: 1px solid var(--border);
  }

  .line {
    margin: 0 0 6px;
    color: var(--text);
    word-break: break-all;
  }

  .hexrow {
    display: flex;
    gap: 6px;
    line-height: 20px;
  }

  .offset {
    margin-right: 6px;
    color: var(--muted);
  }

  .byte {
    padding: 0 1px;
    border-radius: 3px;
  }

  .byte.b-unit {
    color: var(--accent-2);
  }

  .byte.b-function {
    font-weight: 700;
    color: var(--accent);
  }

  .byte.b-address {
    text-decoration: underline;
  }

  .byte.b-count {
    text-decoration: underline dotted;
  }

  .byte.b-values {
    background: var(--accent-soft);
  }

  .byte.b-check {
    color: var(--muted);
    outline: 1px dashed var(--border);
  }

  .fields {
    flex: 1;
    min-width: 260px;
  }

  .summary {
    margin: 0 0 6px;
    color: var(--muted);
  }

  .fields table {
    border-collapse: collapse;
    font-variant-numeric: tabular-nums;
  }

  .fields td {
    padding: 2px 14px 2px 0;
    white-space: nowrap;
  }

  .fields .name {
    font-family: var(--font-mono);
    color: var(--muted);
    text-align: right;
  }

  .fields .value {
    font-family: var(--font-mono);
    color: var(--text);
    text-align: right;
  }

  .fields .note {
    font-family: var(--font-mono);
    color: var(--muted);
  }

  .legend {
    margin: 6px 0 0;
    color: var(--muted);
    white-space: nowrap;
  }
</style>
