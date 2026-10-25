<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { writeText } from "@tauri-apps/plugin-clipboard-manager";
  import { save } from "@tauri-apps/plugin-dialog";
  import { CHECK_SIZES, FIELD_SIZES, hex, parseHex, readField, type Decoder } from "./frames";
  import type { StoredFrame, Tab } from "./tabs.svelte";
  import { clearActiveTerminal, setActiveTerminal, type TerminalActions } from "./terminal-actions";

  let {
    tab,
    active,
    onlayout,
  }: {
    tab: Tab;
    /** Whether this view has the keyboard. */
    active: boolean;
    /** Opens the frame layout. */
    onlayout: () => void;
  } = $props();

  /** The height of a row, in pixels, the same as `.row` below. */
  const ROW = 20;
  const DECODERS: { value: Decoder; label: string }[] = [
    { value: "off", label: "Off" },
    { value: "layout", label: "My layout" },
    { value: "cobs", label: "COBS" },
    { value: "slip", label: "SLIP" },
    { value: "nmea", label: "NMEA (GPS)" },
    { value: "modbus_rtu", label: "Modbus RTU" },
    { value: "modbus_ascii", label: "Modbus ASCII" },
  ];

  let list: HTMLDivElement;
  let scrollTop = $state(0);
  let height = $state(0);
  /** Whether the list keeps showing the newest frame as frames come. */
  let follow = $state(true);
  let onlyBad = $state(false);
  /** The frame picked, by number, whose details show below the list. */
  let picked: number | null = $state(null);

  /** Where the frames shown are in `tab.frames`; `shownVersion` changes with it. */
  let positions: number[] = [];
  let count = $state(0);
  let shownVersion = $state(0);
  let scanned = 0;
  let firstFrame = -1;

  /** Works out which frames are shown: all again, or only the new ones. */
  function refilter(full: boolean) {
    const frames = tab.frames;
    const first = frames[0]?.n ?? -1;
    if (full || first !== firstFrame || scanned > frames.length) {
      positions = [];
      scanned = 0;
      firstFrame = first;
    }
    for (let at = scanned; at < frames.length; at++) {
      if (!onlyBad || !frames[at].ok) positions.push(at);
    }
    scanned = frames.length;
    count = positions.length;
    shownVersion++;
    if (follow) {
      requestAnimationFrame(() => list && (list.scrollTop = list.scrollHeight));
    }
  }

  $effect(() => {
    void onlyBad;
    untrack(() => refilter(true));
  });
  $effect(() => {
    void tab.framesVersion;
    untrack(() => refilter(false));
  });

  /** The rows on screen, and a few more either side. */
  const rows = $derived.by(() => {
    void shownVersion;
    const start = Math.max(0, Math.floor(scrollTop / ROW) - 5);
    const end = Math.min(count, Math.ceil((scrollTop + height) / ROW) + 5);
    const shown: { index: number; frame: StoredFrame }[] = [];
    for (let index = start; index < end; index++) {
      const frame = tab.frames[positions[index]];
      if (frame) shown.push({ index, frame });
    }
    return shown;
  });

  /** The frame picked, while it is still kept. Frames are numbered one after another. */
  const pickedFrame = $derived.by(() => {
    void shownVersion;
    const first = tab.frames[0]?.n;
    if (picked === null || first === undefined) return null;
    return tab.frames[picked - first] ?? null;
  });

  /** Whether frames are read with the layout's fields and check: not NMEA's or Modbus's. */
  const ownFields = $derived(["layout", "cobs", "slip"].includes(tab.frameSpec.decoder));
  /** Whether frames are text: NMEA and Modbus ASCII. */
  const textFrames = $derived(tab.frameSpec.decoder === "nmea" || tab.frameSpec.decoder === "modbus_ascii");

  /** What the picked frame's bytes are: a field's, the start's or the check's. */
  const byteKinds = $derived.by(() => {
    const frame = pickedFrame;
    if (!frame) return [];
    const spec = tab.frameSpec;
    const kinds: string[] = new Array(frame.bytes.length).fill("");
    if (spec.decoder === "layout") {
      const start = parseHex(spec.start)?.length ?? 0;
      for (let at = 0; at < Math.min(start, kinds.length); at++) kinds[at] = "start";
      if (spec.length) {
        for (let at = 0; at < spec.length.size; at++) kinds[spec.length.offset + at] = "length";
      }
    }
    if (ownFields) {
      for (const field of spec.fields) {
        for (let at = 0; at < FIELD_SIZES[field.type]; at++) {
          if (field.offset + at < kinds.length) kinds[field.offset + at] = "field";
        }
      }
      const size = CHECK_SIZES[spec.check.kind];
      for (let at = kinds.length - size; at < kinds.length; at++) if (at >= 0) kinds[at] = "check";
    }
    return kinds;
  });

  function time(at: number): string {
    const date = new Date(at);
    const two = (value: number) => String(value).padStart(2, "0");
    return `${two(date.getHours())}:${two(date.getMinutes())}:${two(date.getSeconds())}.${String(date.getMilliseconds()).padStart(3, "0")}`;
  }

  const decoder = new TextDecoder();

  /** A sentence's text, for NMEA, which is text already. */
  function text(bytes: Uint8Array): string {
    return decoder.decode(bytes);
  }

  function number(value: number): string {
    return Number.isInteger(value) ? String(value) : String(Number(value.toPrecision(7)));
  }

  function valuesText(frame: StoredFrame): string {
    const values = frame.values.map(([name, value]) => `${name} ${number(value)}`).join("   ");
    return frame.note ? `${frame.note}   ${values}`.trim() : values;
  }

  function setDecoder(decoder: Decoder) {
    tab.setFrameSpec({ ...$state.snapshot(tab.frameSpec), decoder });
    picked = null;
  }

  function pick(frame: StoredFrame) {
    picked = frame.n;
    list.focus();
  }

  /** Moves the pick to the frame shown `step` rows away, and shows it. */
  function move(step: number) {
    if (count === 0) return;
    const index = picked === null ? (step > 0 ? -1 : count) : positions.findIndex((at) => tab.frames[at].n === picked);
    const next = Math.max(0, Math.min(count - 1, index + step));
    picked = tab.frames[positions[next]].n;
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

  /** The frames shown, as CSV: when, whether right, the bytes and each field. */
  function csv(): string {
    const field = (text: string) => (/[",\n]/.test(text) ? `"${text.replaceAll('"', '""')}"` : text);
    const names: string[] = [];
    for (const at of positions) {
      for (const [name] of tab.frames[at].values) if (!names.includes(name)) names.push(name);
    }
    const out = [["time", "ok", "problem", "bytes", ...names].map(field).join(",")];
    for (const at of positions) {
      const frame = tab.frames[at];
      const values = new Map(frame.values);
      out.push(
        [
          new Date(frame.at).toISOString(),
          frame.ok ? "yes" : "no",
          field(frame.problem ?? ""),
          hex(frame.bytes),
          ...names.map((name) => (values.has(name) ? String(values.get(name)) : "")),
        ].join(","),
      );
    }
    return `${out.join("\n")}\n`;
  }

  async function exportCsv() {
    const path = await save({
      title: "Export the frames as CSV",
      defaultPath: `${tab.name.replace(/[<>:"/\\|?*]/g, "_")} frames.csv`,
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
      if (pickedFrame) await writeText(hex(pickedFrame.bytes));
    },
    paste: async () => {},
    selectAll: () => {},
    clear: () => {
      picked = null;
      tab.clearFrames();
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

<div class="framesview">
  <div class="bar">
    <label class="option" title="How the data is split into frames">
      Frames
      <select class="field" value={tab.frameSpec.decoder} onchange={(event) => setDecoder(event.currentTarget.value as Decoder)}>
        {#each DECODERS as { value, label } (value)}
          <option {value}>{label}</option>
        {/each}
      </select>
    </label>
    <button class="btn small" title="Start bytes, length, fields and the check" onclick={onlayout}>Layout...</button>
    <label class="check" title="Show only the frames whose check or fields are wrong">
      <input type="checkbox" bind:checked={onlyBad} /> Only bad
    </label>

    <span class="spacer"></span>
    <span class="total" title="Frames shown, and those whose check or fields are wrong">
      {(void tab.framesVersion, tab.frames.length).toLocaleString()} frames,
      <span class:bad={(void tab.framesVersion, tab.badFrames) > 0}>{(void tab.framesVersion, tab.badFrames).toLocaleString()} bad</span>
    </span>
    <button class="btn small" disabled={count === 0} title="Save the frames shown to a CSV file" onclick={exportCsv}>CSV...</button>
    <button class="btn small" title="Forget the frames read (Ctrl+Shift+K)" onclick={() => actions.clear()}>Clear</button>
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
            class="row"
            class:picked={frame.n === picked}
            class:bad={!frame.ok}
            role="row"
            tabindex="-1"
            style:top={`${index * ROW}px`}
            onmousedown={() => pick(frame)}
          >
            <span class="time mono">{time(frame.at)}</span>
            <span class="mark" title={frame.ok ? "Its check and fields are right" : (frame.problem ?? "")}>{frame.ok ? "✓" : "✗"}</span>
            <span class="size mono">{frame.bytes.length} B</span>
            <span class="values mono" title={frame.problem ?? valuesText(frame)}>
              {frame.ok ? valuesText(frame) : frame.problem}
            </span>
            <span class="bytes mono">{textFrames ? text(frame.bytes).trim() : hex(frame.bytes, 24)}</span>
          </div>
        {/each}
      </div>
      {#if count === 0}
        <p class="empty">
          {#if tab.frameSpec.decoder === "off"}
            Pick how the data is split into frames: your own layout of start bytes, a length, fields and a check,
            COBS, SLIP, NMEA from a GPS receiver, or Modbus RTU or ASCII on a bus.
          {:else if onlyBad && tab.frames.length > 0}
            No bad frames.
          {:else}
            No frames yet. Frames already received are read again when the layout changes.
          {/if}
        </p>
      {/if}
    </div>
  </div>

  {#if pickedFrame}
    <div class="details">
      <div class="hexdump mono" aria-label="The frame's bytes">
        {#each { length: Math.ceil(pickedFrame.bytes.length / 16) } as _, row (row)}
          <div class="hexrow">
            <span class="offset">{(row * 16).toString(16).toUpperCase().padStart(4, "0")}</span>
            {#each pickedFrame.bytes.subarray(row * 16, row * 16 + 16) as byte, column (column)}
              <span class="byte {byteKinds[row * 16 + column]}">{byte.toString(16).toUpperCase().padStart(2, "0")}</span>
            {/each}
          </div>
        {/each}
      </div>
      <div class="fields">
        {#if pickedFrame.problem}
          <p class="problem">{pickedFrame.problem}</p>
        {/if}
        {#if textFrames}
          <p class="mono sentence">{text(pickedFrame.bytes)}</p>
        {/if}
        {#if pickedFrame.note}
          <p class="sentence">{pickedFrame.note}</p>
        {/if}
        <table>
          <tbody>
            {#if !ownFields}
              {#each pickedFrame.values as [name, value] (name)}
                <tr><td class="name">{name}</td><td class="value">{number(value)}</td></tr>
              {/each}
            {:else}
              {#each tab.frameSpec.fields as field (field.name)}
                {@const value = readField(pickedFrame.bytes, field)}
                <tr>
                  <td class="name">{field.name}</td>
                  <td class="muted">{field.type} {field.endian.toUpperCase()} at {field.offset}</td>
                  <td class="mono muted">{hex(pickedFrame.bytes.subarray(field.offset, field.offset + FIELD_SIZES[field.type]))}</td>
                  <td class="value">{value === null ? "—" : number(value)}</td>
                </tr>
              {/each}
            {/if}
          </tbody>
        </table>
        <p class="legend">
          {pickedFrame.bytes.length} bytes, {time(pickedFrame.at)}
          {#if ownFields}
            · <span class="byte start">start</span> <span class="byte length">length</span>
            <span class="byte field">field</span> <span class="byte check">check</span>
          {/if}
        </p>
      </div>
    </div>
  {/if}
</div>

<style>
  .framesview {
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

  .row:hover {
    background: var(--hover);
  }

  .row.picked {
    background: var(--accent-soft);
  }

  .time {
    color: var(--muted);
  }

  .mark {
    width: 12px;
    color: var(--ok);
    text-align: center;
  }

  .row.bad .mark,
  .row.bad .values {
    color: var(--bad);
  }

  .size {
    min-width: 6ch;
    color: var(--muted);
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  .values {
    flex: 0 1 auto;
    min-width: 200px;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .bytes {
    flex: 1;
    overflow: hidden;
    color: var(--muted);
    text-overflow: ellipsis;
  }

  .empty {
    margin: 40px auto;
    max-width: 560px;
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

  .byte.start {
    color: var(--accent-2);
  }

  .byte.length {
    text-decoration: underline;
  }

  .byte.field {
    background: var(--accent-soft);
  }

  .byte.check {
    color: var(--muted);
    outline: 1px dashed var(--border);
  }

  .fields {
    flex: 1;
    min-width: 260px;
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
    color: var(--text);
  }

  .fields .value {
    font-family: var(--font-mono);
    color: var(--text);
    text-align: right;
  }

  .muted {
    color: var(--muted);
  }

  .problem {
    margin: 0 0 6px;
    color: var(--bad);
  }

  .sentence {
    margin: 0 0 6px;
    color: var(--text);
    word-break: break-all;
  }

  .legend {
    margin: 6px 0 0;
    color: var(--muted);
    white-space: nowrap;
  }
</style>
