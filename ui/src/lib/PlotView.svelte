<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { save } from "@tauri-apps/plugin-dialog";
  import uPlot from "uplot";
  import "uplot/dist/uPlot.min.css";
  import type { PlotView } from "./serial";
  import type { StoredLine, Tab } from "./tabs.svelte";
  import { clearActiveTerminal, setActiveTerminal, type TerminalActions } from "./terminal-actions";
  import { appScheme, type Scheme } from "./theme.svelte";
  import { compileRegex, readValues } from "./values";

  let {
    tab,
    active,
  }: {
    tab: Tab;
    /** Whether this view has the keyboard. */
    active: boolean;
  } = $props();

  /**
   * The series' colors, in this order: a palette checked for color blindness
   * against both themes' backgrounds. A series keeps its color while others
   * come, go or are hidden.
   */
  const COLORS: Record<Scheme, string[]> = {
    light: ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#6250d6", "#e34948"],
    dark: ["#3987e5", "#d95926", "#199e70", "#c98500", "#d55181", "#008300", "#9085e9", "#e66767"],
  };
  /** Series past the eighth take the colors again, dashed and then dotted, so no two look alike. */
  const DASHES: (number[] | undefined)[] = [undefined, [8, 4], [2, 3]];
  const SERIES_MAX = COLORS.light.length * DASHES.length;
  /** The axes, the A and B cursors and the background: --muted, --border, --text and --bg in app.css. */
  const INK: Record<Scheme, { text: string; grid: string; cursor: string; shade: string; bg: string }> = {
    dark: { text: "#7d8896", grid: "#262f3b", cursor: "#d6dde6", shade: "#d6dde614", bg: "#0f1419" },
    light: { text: "#5d6b7a", grid: "#d9dfe6", cursor: "#1d2733", shade: "#1d27330f", bg: "#fbfcfd" },
  };
  const FONT_FAMILY = '"Inter Variable", system-ui, sans-serif';
  const WINDOWS = [
    { seconds: 5, label: "5 s" },
    { seconds: 10, label: "10 s" },
    { seconds: 30, label: "30 s" },
    { seconds: 60, label: "1 min" },
    { seconds: 300, label: "5 min" },
    { seconds: 0, label: "All" },
  ];

  let root: HTMLDivElement;
  let host: HTMLDivElement;
  let regexInput: HTMLInputElement | undefined = $state();
  let chart: uPlot | null = null;

  /**
   * The values, a column for the times and one for each series, with the
   * line each point came from. Plain arrays, as there can be many.
   */
  const times: number[] = [];
  const lineNumbers: number[] = [];
  const columns: (number | null)[][] = [];
  const columnOf = new Map<string, number>();
  /** The series' names, in the order they first came. */
  let names: string[] = $state([]);
  /** Set when a line had more values than the chart has series for. */
  let tooMany = $state(false);
  let points = $state(0);
  /** When t = 0 is, in ms since 1970: when the first line came after the plot was cleared. */
  let origin: number | null = null;
  /** How much of `tab.textLines` has been read, and the number of its first line then. */
  let scanned = 0;
  let firstLine = -1;
  let plottedFrom = -1;

  /** Whether the chart keeps showing the newest values; zooming stops that. */
  let follow = $state(true);
  /** The A and B cursors, in seconds. */
  let cursorA: number | null = $state(null);
  let cursorB: number | null = $state(null);
  /** The point under the mouse, while it is over the chart. */
  let hover: number | null = $state(null);
  /** Changes, a few times a second at most, when the stats need working out again. */
  let statsVersion = $state(0);
  let statsTimer: ReturnType<typeof setTimeout> | undefined;
  let frame = 0;

  let regexDraft = $state(untrack(() => tab.plotView.regex));
  let regexTimer: ReturnType<typeof setTimeout> | undefined;
  const compiled = $derived(tab.plotView.mode === "regex" ? compileRegex(tab.plotView.regex) : null);
  const pattern = $derived(compiled instanceof RegExp ? compiled : null);
  const draftProblem = $derived.by(() => {
    if (tab.plotView.mode !== "regex") return null;
    const draft = compileRegex(regexDraft);
    return typeof draft === "string" ? draft : null;
  });

  function setPlotView(change: Partial<PlotView>) {
    tab.setPlotView({ ...tab.plotView, ...change });
  }

  /** Forgets the values read, to read the lines again. */
  function reset() {
    times.length = 0;
    lineNumbers.length = 0;
    columns.length = 0;
    columnOf.clear();
    names = [];
    tooMany = false;
    scanned = 0;
  }

  /** Starts again from nothing: no values, no cursors, and t = 0 at the next line. */
  function restart() {
    reset();
    origin = null;
    cursorA = cursorB = null;
    follow = true;
  }

  /** Reads the values in the lines: all of them again, or only the new ones. */
  function refresh(full: boolean) {
    const lines = tab.textLines;
    const first = lines[0]?.n ?? -1;
    const gone = first - firstLine;
    if (lines.length === 0 || (scanned > 0 && gone >= scanned)) {
      // The lines were cleared.
      restart();
    } else if (full) {
      reset();
    } else if (firstLine >= 0 && gone > 0) {
      // The oldest lines went, so their points go too.
      drop(first);
      scanned -= gone;
    }
    firstLine = first;
    const mode = tab.plotView.mode;
    const regex = pattern;
    for (let at = scanned; at < lines.length; at++) {
      add(lines[at], mode, regex);
    }
    scanned = lines.length;
    points = times.length;
    draw();
  }

  function add(line: StoredLine, mode: PlotView["mode"], regex: RegExp | null) {
    if (line.n < tab.plotFrom) return;
    origin ??= line.at;
    // Zephyr's time and module are no values; a regex gets the whole line.
    const values = mode === "regex" ? readValues(line.text, "regex", regex) : readValues(line.level ? line.message : line.text);
    if (values.length === 0) return;
    const row = times.length;
    times.push(Math.max((line.at - origin) / 1000, times[row - 1] ?? 0));
    lineNumbers.push(line.n);
    for (const column of columns) column.push(null);
    for (const [name, value] of values) {
      let index = columnOf.get(name);
      if (index === undefined) {
        if (columns.length >= SERIES_MAX) {
          tooMany = true;
          continue;
        }
        index = columns.length;
        columnOf.set(name, index);
        columns.push(new Array(row + 1).fill(null));
        names.push(name);
      }
      columns[index][row] = value;
    }
  }

  /** Drops the points of the lines before `first`. */
  function drop(first: number) {
    let count = 0;
    while (count < lineNumbers.length && lineNumbers[count] < first) count++;
    if (count > 0) {
      times.splice(0, count);
      lineNumbers.splice(0, count);
      for (const column of columns) column.splice(0, count);
    }
  }

  // Another way of reading the lines, or Clear, reads them all again.
  $effect(() => {
    void [tab.plotView.mode, pattern, tab.plotFrom];
    untrack(() => {
      if (tab.plotFrom !== plottedFrom) {
        plottedFrom = tab.plotFrom;
        restart();
      }
      refresh(true);
    });
  });
  // New lines: only they need reading.
  $effect(() => {
    void tab.linesVersion;
    untrack(() => refresh(false));
  });
  // The other theme draws the chart again in its colors.
  $effect(() => {
    void appScheme();
    untrack(() => chart && build());
  });
  $effect(() => {
    void [cursorA, cursorB];
    untrack(() => {
      chart?.redraw(false);
      touchStats(true);
    });
  });

  /** Draws the new values on the next frame. */
  function draw() {
    if (frame) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      if (!chart || chart.series.length !== names.length + 1) {
        build();
      } else {
        chart.setData(data(), false);
      }
      if (follow) followNewest();
      touchStats(false);
    });
  }

  function data(): uPlot.AlignedData {
    return [times, ...columns] as uPlot.AlignedData;
  }

  /** Makes the chart, with a series for each name, keeping the range shown unless it follows. */
  function build() {
    const scheme = appScheme();
    const ink = INK[scheme];
    const colors = COLORS[scheme];
    const kept = chart && !follow ? { min: chart.scales.x.min, max: chart.scales.x.max } : null;
    chart?.destroy();
    const axis = {
      stroke: ink.text,
      font: `11px ${FONT_FAMILY}`,
      grid: { stroke: ink.grid, width: 1 },
      ticks: { stroke: ink.grid, width: 1, size: 4 },
    };
    chart = new uPlot(
      {
        width: host.clientWidth,
        height: host.clientHeight,
        legend: { show: false },
        scales: { x: { time: false } },
        cursor: {
          y: false,
          drag: { x: true, y: false, setScale: false },
          // A double-click follows the newest values again, below, rather than showing all.
          bind: { dblclick: () => null },
        },
        series: [
          { label: "Time" },
          ...names.map((name, index) => ({
            label: name,
            stroke: colors[index % colors.length],
            width: 2,
            dash: DASHES[Math.floor(index / colors.length)],
            show: !tab.plotView.hidden.includes(name),
            // A series that is not on every line is still one line.
            spanGaps: true,
          })),
        ],
        axes: [
          {
            ...axis,
            values: (_chart: uPlot, splits: number[], _axis: number, _space: number, step: number) =>
              splits.map((value) => `${value.toFixed(decimals(step))} s`),
          },
          { ...axis, size: (_chart: uPlot, values: string[]) => axisWidth(values) },
        ],
        hooks: {
          setSelect: [zoomToSelection],
          draw: [drawCursors],
          setCursor: [(self: uPlot) => (hover = self.cursor.idx ?? null)],
          setScale: [(_self: uPlot, key: string) => key === "x" && touchStats(false)],
        },
      },
      data(),
      host,
    );
    if (kept?.min != null && kept.max != null) {
      chart.setScale("x", { min: kept.min, max: kept.max });
    }
    listen(chart.over);
    if (follow) followNewest();
  }

  /** The decimals a step between ticks needs, such as 1 for 0.5 or 2.5. */
  function decimals(step: number): number {
    const text = String(Number(step.toPrecision(6)));
    const dot = text.indexOf(".");
    return dot < 0 || text.includes("e") ? 0 : Math.min(6, text.length - dot - 1);
  }

  /** Room for the longest number on the value axis. */
  function axisWidth(values: string[] | null): number {
    const longest = Math.max(1, ...(values ?? []).map((value) => String(value).length));
    return Math.ceil(longest * 6.5) + 16;
  }

  /** Shows the last seconds of values, as many as the window holds, or all of them. */
  function followNewest() {
    if (!chart) return;
    const last = times.at(-1) ?? 0;
    const window = tab.plotView.window_s;
    if (window > 0) {
      chart.setScale("x", { min: Math.max(0, last - window), max: Math.max(last, window) });
    } else {
      const first = times[0] ?? 0;
      chart.setScale("x", { min: first, max: Math.max(last, first + 1) });
    }
  }

  function setFollow(on: boolean) {
    follow = on;
    if (on) followNewest();
  }

  function zoomToSelection(self: uPlot) {
    const { left, width } = self.select;
    if (width > 3) {
      follow = false;
      self.setScale("x", { min: self.posToVal(left, "x"), max: self.posToVal(left + width, "x") });
    }
    self.setSelect({ left: 0, top: 0, width: 0, height: 0 }, false);
  }

  /** Clicks place the cursors, the wheel zooms, a double-click follows the newest values. */
  function listen(over: HTMLElement) {
    let downX = 0;
    let before: [number | null, number | null] = [null, null];
    over.addEventListener("mousedown", (event) => (downX = event.clientX));
    over.addEventListener("click", (event) => {
      // A drag zooms instead.
      if (!chart || Math.abs(event.clientX - downX) > 3) return;
      if (event.detail >= 2) {
        // The first click of a double-click placed a cursor: it goes again.
        [cursorA, cursorB] = before;
        setFollow(true);
        return;
      }
      before = [cursorA, cursorB];
      placeCursor(chart.posToVal(event.clientX - over.getBoundingClientRect().left, "x"));
      root.focus();
    });
    over.addEventListener("wheel", onWheel, { passive: false });
  }

  /** Places A, then B, then moves the nearer one. */
  function placeCursor(at: number) {
    if (cursorA === null) {
      cursorA = at;
    } else if (cursorB === null) {
      cursorB = at;
    } else if (Math.abs(at - cursorA) <= Math.abs(at - cursorB)) {
      cursorA = at;
    } else {
      cursorB = at;
    }
  }

  function clearCursors() {
    cursorA = cursorB = null;
  }

  function onWheel(event: WheelEvent) {
    if (!chart) return;
    event.preventDefault();
    const { min, max } = chart.scales.x;
    if (min == null || max == null) return;
    follow = false;
    if (event.shiftKey || Math.abs(event.deltaX) > Math.abs(event.deltaY)) {
      const step = (max - min) * 0.1 * Math.sign(event.deltaX || event.deltaY);
      chart.setScale("x", { min: min + step, max: max + step });
    } else {
      const at = chart.posToVal(event.clientX - chart.over.getBoundingClientRect().left, "x");
      const factor = event.deltaY > 0 ? 1.25 : 0.8;
      chart.setScale("x", { min: at - (at - min) * factor, max: at + (max - at) * factor });
    }
  }

  /** Draws the A and B cursors over the chart, with the time between them shaded. */
  function drawCursors(self: uPlot) {
    const ink = INK[appScheme()];
    const ratio = window.devicePixelRatio || 1;
    const { left, top, width, height } = self.bbox;
    const ctx = self.ctx;
    const at = [cursorA, cursorB].map((value) => (value === null ? null : self.valToPos(value, "x", true)));
    ctx.save();
    if (at[0] !== null && at[1] !== null) {
      const from = Math.max(left, Math.min(at[0], at[1]));
      const to = Math.min(left + width, Math.max(at[0], at[1]));
      if (to > from) {
        ctx.fillStyle = ink.shade;
        ctx.fillRect(from, top, to - from, height);
      }
    }
    ctx.font = `600 ${11 * ratio}px ${FONT_FAMILY}`;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.lineWidth = ratio;
    ["A", "B"].forEach((label, index) => {
      const x = at[index];
      if (x === null || x < left || x > left + width) return;
      const line = Math.round(x) + 0.5;
      ctx.strokeStyle = ink.cursor;
      ctx.setLineDash([4 * ratio, 3 * ratio]);
      ctx.beginPath();
      ctx.moveTo(line, top);
      ctx.lineTo(line, top + height);
      ctx.stroke();
      const tag = { width: 16 * ratio, height: 15 * ratio };
      ctx.fillStyle = ink.cursor;
      ctx.fillRect(line - tag.width / 2, top, tag.width, tag.height);
      ctx.fillStyle = ink.bg;
      ctx.fillText(label, line, top + tag.height / 2 + ratio / 2);
    });
    ctx.restore();
  }

  /** Has the stats worked out again: now, or soon when values keep coming. */
  function touchStats(now: boolean) {
    if (now) {
      clearTimeout(statsTimer);
      statsTimer = undefined;
      statsVersion++;
    } else {
      statsTimer ??= setTimeout(() => {
        statsTimer = undefined;
        statsVersion++;
      }, 150);
    }
  }

  /** The first point at or after a time. */
  function firstFrom(time: number): number {
    let low = 0;
    let high = times.length;
    while (low < high) {
      const middle = (low + high) >> 1;
      if (times[middle] < time) low = middle + 1;
      else high = middle;
    }
    return low;
  }

  /** A series' value at a point: its own, or else the last one before, as a series is not on every line. */
  function heldAt(column: (number | null)[], index: number): number | null {
    for (index = Math.min(index, column.length - 1); index >= 0; index--) {
      if (column[index] !== null) return column[index];
    }
    return null;
  }

  /** A series' value at a time: the last one at or before it. */
  function valueAt(column: (number | null)[], time: number): number | null {
    return heldAt(column, firstFrom(time + 1e-9) - 1);
  }

  type Stats = { min: number; max: number; mean: number; count: number } | null;

  /** Min, max and mean of each series between A and B, or else in the range shown. */
  const stats = $derived.by(() => {
    void statsVersion;
    const both = cursorA !== null && cursorB !== null;
    const from = both ? Math.min(cursorA!, cursorB!) : (chart?.scales.x.min ?? -Infinity);
    const to = both ? Math.max(cursorA!, cursorB!) : (chart?.scales.x.max ?? Infinity);
    const start = firstFrom(from);
    const end = firstFrom(to + 1e-9);
    const series: Stats[] = columns.map((column) => {
      let min = Infinity;
      let max = -Infinity;
      let sum = 0;
      let count = 0;
      for (let index = start; index < end; index++) {
        const value = column[index];
        if (value === null) continue;
        if (value < min) min = value;
        if (value > max) max = value;
        sum += value;
        count++;
      }
      return count > 0 ? { min, max, mean: sum / count, count } : null;
    });
    return { both, from, to, points: Math.max(0, end - start), series };
  });

  /** The table: each series' value under the mouse or its last, its stats, and its values at A and B. */
  const rows = $derived.by(() => {
    void statsVersion;
    return names.map((name, index) => {
      const column = columns[index] ?? [];
      return {
        name,
        index,
        value: heldAt(column, hover ?? column.length - 1),
        at: stats.series[index],
        a: cursorA !== null ? valueAt(column, cursorA) : null,
        b: cursorB !== null ? valueAt(column, cursorB) : null,
      };
    });
  });

  /** A number short enough for the table: six digits at most after the first. */
  function number(value: number | null | undefined): string {
    if (value === null || value === undefined || !Number.isFinite(value)) return "";
    return Number.isInteger(value) ? String(value) : String(Number(value.toPrecision(6)));
  }

  function duration(seconds: number): string {
    return Math.abs(seconds) < 1 ? `${number(seconds * 1000)} ms` : `${number(seconds)} s`;
  }

  function frequency(seconds: number): string {
    const hz = 1 / Math.abs(seconds);
    return hz >= 1000 ? `${number(hz / 1000)} kHz` : `${number(hz)} Hz`;
  }

  function toggleSeries(name: string, index: number) {
    const hidden = tab.plotView.hidden.includes(name)
      ? tab.plotView.hidden.filter((other) => other !== name)
      : [...tab.plotView.hidden, name];
    setPlotView({ hidden });
    chart?.setSeries(index + 1, { show: !hidden.includes(name) });
  }

  function onRegexInput(event: Event & { currentTarget: HTMLInputElement }) {
    regexDraft = event.currentTarget.value;
    clearTimeout(regexTimer);
    regexTimer = setTimeout(() => setPlotView({ regex: regexDraft }), 300);
  }

  function setMode(mode: PlotView["mode"]) {
    setPlotView({ mode });
    if (mode === "regex") requestAnimationFrame(() => regexInput?.focus());
  }

  /** Every value, a row for each time, with the time as it was and in seconds from the start. */
  function csv(): string {
    const field = (text: string) => (/[",\n]/.test(text) ? `"${text.replaceAll('"', '""')}"` : text);
    const out = [["time", "seconds", ...names].map(field).join(",")];
    for (let row = 0; row < times.length; row++) {
      const cells = [new Date((origin ?? 0) + times[row] * 1000).toISOString(), times[row].toFixed(3)];
      for (const column of columns) cells.push(column[row] === null ? "" : String(column[row]));
      out.push(cells.join(","));
    }
    return `${out.join("\n")}\n`;
  }

  /** The tab's name, without the characters file names cannot have. */
  function fileName(extension: string): string {
    return `${tab.name.replace(/[<>:"/\\|?*]/g, "_")}.${extension}`;
  }

  async function exportCsv() {
    const path = await save({
      title: "Export the plot as CSV",
      defaultPath: fileName("csv"),
      filters: [{ name: "CSV", extensions: ["csv"] }],
    });
    if (path) {
      await invoke("save_text", { path, text: csv() }).catch((error) => console.error(error));
    }
    root.focus();
  }

  /** Saves the chart as it is on screen, with the names of the series above it. */
  async function exportPng() {
    if (!chart) return;
    const path = await save({
      title: "Export the plot as PNG",
      defaultPath: fileName("png"),
      filters: [{ name: "PNG image", extensions: ["png"] }],
    });
    if (!path) {
      root.focus();
      return;
    }
    const scheme = appScheme();
    const source = chart.ctx.canvas;
    const ratio = window.devicePixelRatio || 1;
    const font = `${12 * ratio}px ${FONT_FAMILY}`;
    const pad = 12 * ratio;
    const row = 20 * ratio;
    const swatch = 18 * ratio;
    const out = document.createElement("canvas");
    let ctx = out.getContext("2d")!;
    ctx.font = font;
    // The legend, in rows as wide as the chart.
    const shown = names.map((name, index) => ({ name, index })).filter(({ index }) => chart!.series[index + 1].show);
    const placed: { name: string; index: number; x: number; y: number }[] = [];
    let x = pad;
    let y = pad;
    for (const item of shown) {
      const width = swatch + 6 * ratio + ctx.measureText(item.name).width;
      if (x > pad && x + width > source.width - pad) {
        x = pad;
        y += row;
      }
      placed.push({ ...item, x, y });
      x += width + 16 * ratio;
    }
    const legend = shown.length > 0 ? y + row : 0;
    out.width = source.width;
    out.height = source.height + legend;
    ctx = out.getContext("2d")!;
    ctx.fillStyle = INK[scheme].bg;
    ctx.fillRect(0, 0, out.width, out.height);
    ctx.font = font;
    ctx.textBaseline = "middle";
    ctx.lineWidth = 2 * ratio;
    const colors = COLORS[scheme];
    for (const item of placed) {
      const middle = item.y + row / 2;
      ctx.strokeStyle = colors[item.index % colors.length];
      ctx.setLineDash((DASHES[Math.floor(item.index / colors.length)] ?? []).map((length) => length * ratio));
      ctx.beginPath();
      ctx.moveTo(item.x, middle);
      ctx.lineTo(item.x + swatch, middle);
      ctx.stroke();
      ctx.fillStyle = INK[scheme].cursor;
      ctx.fillText(item.name, item.x + swatch + 6 * ratio, middle);
    }
    ctx.drawImage(source, 0, legend);
    const blob = await new Promise<Blob | null>((done) => out.toBlob(done, "image/png"));
    if (blob) {
      const bytes = new Uint8Array(await blob.arrayBuffer());
      await invoke("save_bytes", { path, data: Array.from(bytes) }).catch((error) => console.error(error));
    }
    root.focus();
  }

  const actions: TerminalActions = {
    hasSelection: () => false,
    copy: async () => {},
    paste: async () => {},
    selectAll: () => {},
    clear: () => tab.clearPlot(),
    text: () => csv(),
    focus: () => root?.focus(),
    openSearch: () => {},
    findNext: () => {},
    findPrevious: () => {},
    exportCsv: () => void exportCsv(),
    exportPng: () => void exportPng(),
  };

  // The view on screen is the one the menus act on, and has the keyboard.
  $effect(() => {
    if (active) {
      setActiveTerminal(actions);
      root.focus();
    }
  });

  onMount(() => {
    const resize = new ResizeObserver(() => {
      if (chart && host.clientWidth > 0 && host.clientHeight > 0) {
        chart.setSize({ width: host.clientWidth, height: host.clientHeight });
      }
    });
    resize.observe(host);
    return () => {
      resize.disconnect();
      cancelAnimationFrame(frame);
      clearTimeout(statsTimer);
      clearTimeout(regexTimer);
      chart?.destroy();
      chart = null;
      clearActiveTerminal(actions);
    };
  });

  function onkeydown(event: KeyboardEvent) {
    if (event.target !== root) return;
    if (event.key === " ") {
      event.preventDefault();
      setFollow(!follow);
    } else if (event.key === "Escape" && cursorA !== null) {
      event.preventDefault();
      clearCursors();
    }
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div class="plotview" tabindex="0" role="region" aria-label="Plot" bind:this={root} {onkeydown}>
  <div class="bar">
    <label class="option" title="How numbers are read from the lines">
      Read
      <select
        class="field"
        value={tab.plotView.mode}
        onchange={(event) => setMode(event.currentTarget.value as PlotView["mode"])}
      >
        <option value="auto">Numbers in the lines</option>
        <option value="regex">With a regex</option>
      </select>
    </label>
    {#if tab.plotView.mode === "regex"}
      <input
        class="field mono regex"
        class:bad={draftProblem !== null}
        placeholder={String.raw`tick (\d+)`}
        spellcheck="false"
        title={draftProblem ?? "A value for each capture group; (?<name>...) names its series"}
        value={regexDraft}
        bind:this={regexInput}
        oninput={onRegexInput}
      />
    {/if}

    <label class="option" title="The seconds shown while the chart follows the newest values">
      Window
      <select
        class="field"
        value={tab.plotView.window_s}
        onchange={(event) => {
          setPlotView({ window_s: Number(event.currentTarget.value) });
          setFollow(true);
        }}
      >
        {#each WINDOWS as { seconds, label } (seconds)}
          <option value={seconds}>{label}</option>
        {/each}
      </select>
    </label>

    <button
      class="btn small follow"
      class:paused={!follow}
      title={follow ? "Stop following the newest values (Space)" : "Follow the newest values again (Space, or double-click the chart)"}
      onclick={() => setFollow(!follow)}
    >
      {follow ? "Pause" : "Follow"}
    </button>
    {#if cursorA !== null}
      <button class="btn small" title="Take the A and B cursors away (Esc)" onclick={clearCursors}>Clear A/B</button>
    {/if}

    <span class="spacer"></span>
    <span class="total" title="Lines with values, since the plot was cleared">{points.toLocaleString()} points</span>
    <button class="btn small" disabled={points === 0} title="Save every value to a CSV file" onclick={exportCsv}>
      CSV...
    </button>
    <button class="btn small" disabled={points === 0} title="Save the chart as an image" onclick={exportPng}>
      PNG...
    </button>
    <button class="btn small" title="Start the plot again from the next line (Ctrl+Shift+K)" onclick={() => tab.clearPlot()}>
      Clear
    </button>
  </div>

  <div class="chart">
    <div
      class="canvas"
      class:hidden={names.length === 0}
      role="img"
      aria-label="The values over time; the table below has them in numbers"
      bind:this={host}
    ></div>
    {#if names.length === 0}
      <div class="empty">
        <p>
          {#if tab.plotView.mode === "regex"}
            {draftProblem ?? "No line matches the regex yet."}
          {:else}
            No numbers yet. Lines such as <code>temp=21.5 hum=40</code>, <code>21.5,40</code> or
            <code>{'{"temp": 21.5}'}</code> are drawn as they come. For other lines, read them with a regex.
          {/if}
        </p>
      </div>
    {/if}
  </div>

  {#if names.length > 0}
    <div class="range">
      {#if stats.both}
        <span>
          <b>A to B</b>
          {duration(stats.to - stats.from)}{stats.to > stats.from ? `, ${frequency(stats.to - stats.from)}` : ""}
        </span>
      {:else}
        <span><b>In view</b> {duration(Math.max(0, stats.to - stats.from))}</span>
      {/if}
      <span class="number">{stats.points.toLocaleString()} points</span>
      {#if tooMany}
        <span class="warning">Only the first {SERIES_MAX} series are drawn.</span>
      {/if}
      <span class="spacer"></span>
      <span class="hint">Click: cursors A and B · Drag or wheel: zoom · Shift+wheel: move · Double-click: follow</span>
    </div>
    <div class="table">
      <table>
        <thead>
          <tr>
            <th class="name">Series</th>
            <th>{hover !== null && times[hover] !== undefined ? `At ${number(times[hover])} s` : "Last"}</th>
            <th>Min</th>
            <th>Max</th>
            <th>Mean</th>
            {#if cursorA !== null}<th>At A</th>{/if}
            {#if cursorB !== null}<th>At B</th><th>B − A</th>{/if}
          </tr>
        </thead>
        <tbody>
          {#each rows as { name, index, value, at, a, b } (name)}
            {@const shown = !tab.plotView.hidden.includes(name)}
            {@const color = COLORS[appScheme()][index % COLORS.light.length]}
            {@const dash = DASHES[Math.floor(index / COLORS.light.length)]}
            <tr class:off={!shown}>
              <td class="name">
                <label title="Show or hide this series">
                  <input type="checkbox" checked={shown} onchange={() => toggleSeries(name, index)} />
                  <svg width="18" height="8" aria-hidden="true">
                    <line x1="0" y1="4" x2="18" y2="4" stroke={color} stroke-width="2" stroke-dasharray={dash?.join(" ")} />
                  </svg>
                  <span class="mono">{name}</span>
                </label>
              </td>
              <td>{number(value)}</td>
              <td>{number(at?.min)}</td>
              <td>{number(at?.max)}</td>
              <td>{number(at?.mean)}</td>
              {#if cursorA !== null}<td>{number(a)}</td>{/if}
              {#if cursorB !== null}<td>{number(b)}</td><td>{a !== null && b !== null ? number(b - a) : ""}</td>{/if}
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<style>
  .plotview {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    outline: none;
  }

  .bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 10px;
    padding-bottom: 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .option {
    display: flex;
    gap: 6px;
    align-items: center;
    white-space: nowrap;
  }

  .option .field {
    height: 24px;
    font-size: 12px;
  }

  .regex {
    width: 200px;
    height: 24px;
    font-size: 12px;
  }

  .regex.bad {
    border-color: var(--bad);
  }

  .small {
    height: 24px;
    padding: 0 8px;
    font-size: 12px;
  }

  .follow {
    min-width: 58px;
  }

  .follow.paused {
    color: var(--accent);
    border-color: var(--accent);
  }

  .spacer {
    flex: 1;
  }

  /* Numbers keep their width as they grow, so nothing next to them moves. */
  .total,
  .number {
    min-width: 13ch;
    font-variant-numeric: tabular-nums;
    text-align: right;
    white-space: nowrap;
  }

  .chart {
    position: relative;
    flex: 1;
    min-height: 120px;
    overflow: hidden;
  }

  .canvas {
    position: absolute;
    inset: 0;
  }

  .canvas.hidden {
    visibility: hidden;
  }

  .empty {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0 24px;
    font-size: 13px;
    color: var(--muted);
    text-align: center;
  }

  .empty p {
    max-width: 640px;
    margin: 0;
    line-height: 1.6;
  }

  .empty code {
    white-space: nowrap;
    margin: 0 2px;
    font-family: var(--font-mono);
    color: var(--text);
  }

  .range {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 14px;
    align-items: center;
    padding: 6px 2px 4px;
    font-size: 12px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  .range b {
    font-weight: 600;
    color: var(--text);
  }

  .range .number {
    min-width: 0;
  }

  .warning {
    color: var(--warn);
  }

  .table {
    max-height: 35%;
    overflow: auto;
    border-top: 1px solid var(--border);
  }

  table {
    width: 100%;
    font-size: 12px;
    border-collapse: collapse;
    font-variant-numeric: tabular-nums;
  }

  th {
    position: sticky;
    top: 0;
    padding: 4px 10px;
    font-weight: 500;
    color: var(--muted);
    text-align: right;
    white-space: nowrap;
    background: var(--bg);
  }

  td {
    padding: 2px 10px;
    font-family: var(--font-mono);
    color: var(--text);
    text-align: right;
    white-space: nowrap;
  }

  th.name,
  td.name {
    width: 30%;
    text-align: left;
  }

  td.name label {
    display: flex;
    gap: 6px;
    align-items: center;
  }

  tr.off td {
    color: var(--muted);
  }

  tr.off svg {
    opacity: 0.35;
  }

  tbody tr:hover {
    background: var(--hover);
  }

  /* uPlot's own parts, in the app's colors. */
  .chart :global(.u-select) {
    background: var(--accent-soft);
  }

  .chart :global(.u-cursor-x) {
    border-right: 1px dashed var(--muted);
  }

  .chart :global(.u-cursor-pt) {
    border-color: var(--bg) !important;
  }
</style>
