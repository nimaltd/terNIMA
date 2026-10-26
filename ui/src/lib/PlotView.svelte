<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { save } from "@tauri-apps/plugin-dialog";
  import uPlot from "uplot";
  import "uplot/dist/uPlot.min.css";
  import { FFT_MAX, histogram, lastCrossing, peak, spectrum, type Spectrum } from "./analysis";
  import type { Chart, PlotView, Trigger } from "./serial";
  import type { Sample } from "./modbus.svelte";
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
  /** The axes, the cursors, the trigger and the background: --muted, --border, --text and --bg in app.css. */
  const INK: Record<Scheme, { text: string; grid: string; cursor: string; shade: string; bg: string }> = {
    dark: { text: "#7d8896", grid: "#262f3b", cursor: "#d6dde6", shade: "#d6dde614", bg: "#0f1419" },
    light: { text: "#5d6b7a", grid: "#d9dfe6", cursor: "#1d2733", shade: "#1d27330f", bg: "#fbfcfd" },
  };
  const FONT_FAMILY = '"Inter Variable", system-ui, sans-serif';
  const CHARTS: { chart: Chart; label: string; title: string }[] = [
    { chart: "time", label: "Time", title: "The values over time, with cursors and a trigger" },
    { chart: "xy", label: "XY", title: "One series against the others, such as a sensor's x and y" },
    { chart: "histogram", label: "Histogram", title: "How often each value comes: its spread and noise" },
    { chart: "fft", label: "FFT", title: "The frequencies in the values" },
  ];
  const SPANS = [
    { ms: 100, label: "last 100 ms" },
    { ms: 500, label: "last 500 ms" },
    { ms: 1000, label: "last 1 s" },
    { ms: 2000, label: "last 2 s" },
    { ms: 5000, label: "last 5 s" },
    { ms: 10000, label: "last 10 s" },
    { ms: 30000, label: "last 30 s" },
    { ms: 60000, label: "last 1 min" },
    { ms: 300000, label: "last 5 min" },
    { ms: 0, label: "all" },
  ];
  const BINS = [0, 10, 20, 50, 100];
  const FFT_WINDOWS: { value: PlotView["fft_window"]; label: string }[] = [
    { value: "hann", label: "Hann" },
    { value: "hamming", label: "Hamming" },
    { value: "blackman", label: "Blackman" },
    { value: "rect", label: "None" },
  ];
  const EDGES: { value: Trigger["edge"]; label: string }[] = [
    { value: "rising", label: "Rising ↑" },
    { value: "falling", label: "Falling ↓" },
    { value: "both", label: "Either ↕" },
  ];
  /** How often the charts that work the values out again are drawn: an FFT takes a while. */
  const SLOW_MS = 200;
  /** The most points an XY chart draws; more are thinned out, evenly over time. */
  const XY_MAX = 20000;

  let root: HTMLDivElement;
  let host: HTMLDivElement;
  let regexInput: HTMLInputElement | undefined = $state();
  let chart: uPlot | null = null;
  /** What the chart was made for, to make it again when that changes. */
  let builtFor = "";
  let building = false;

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
  let plottedFrom = "";

  type Span = { min: number; max: number };
  /** The time the time chart shows, in seconds, which the other charts work on too. */
  let span: Span = { min: 0, max: 1 };
  /** Where the other charts are zoomed to along their own x axis; null when they show all of it. */
  let zoom: Span | null = null;
  /** Whether the charts keep up with the newest values; zooming the time chart stops that. */
  let follow = $state(true);
  /** The A and B cursors, in seconds. */
  let cursorA: number | null = $state(null);
  let cursorB: number | null = $state(null);
  /** The point under the mouse, while it is over the chart. */
  let hover: number | null = $state(null);
  /** When the trigger last fired, in seconds, and when Single was armed: it waits for a crossing after that. */
  let triggeredAt: number | null = $state(null);
  let armedAt = -Infinity;

  /** What the FFT, the histogram and the XY chart worked out last, for the table. */
  let spectra: (Spectrum | null)[] = [];
  let fftInfo = $state.raw({ rate: 0, measured: 0, samples: 0, size: 0 });
  let bins: { edges: number[]; counts: (number[] | null)[] } = $state.raw({ edges: [], counts: [] });
  let xyRows: number[] = [];

  /** Changes, a few times a second at most, when the stats need working out again. */
  let statsVersion = $state(0);
  let statsTimer: ReturnType<typeof setTimeout> | undefined;
  let frame = 0;
  let slow: ReturnType<typeof setTimeout> | undefined;

  let regexDraft = $state(untrack(() => tab.plotView.regex));
  let regexTimer: ReturnType<typeof setTimeout> | undefined;
  const compiled = $derived(tab.plotView.mode === "regex" ? compileRegex(tab.plotView.regex) : null);
  const pattern = $derived(compiled instanceof RegExp ? compiled : null);
  const draftProblem = $derived.by(() => {
    if (tab.plotView.mode !== "regex") return null;
    const draft = compileRegex(regexDraft);
    return typeof draft === "string" ? draft : null;
  });
  const kind = $derived(tab.plotView.chart);
  const trigger = $derived(tab.plotView.trigger);

  function setPlotView(change: Partial<PlotView>) {
    tab.setPlotView({ ...tab.plotView, ...change });
  }

  /** Changes the trigger, which then waits for a crossing again, as a scope does. */
  function setTrigger(change: Partial<Trigger>) {
    setPlotView({ trigger: { ...tab.plotView.trigger, ...change } });
    triggeredAt = null;
    armedAt = times.at(-1) ?? -Infinity;
    follow = true;
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
    triggeredAt = null;
    armedAt = -Infinity;
    follow = true;
  }

  /** What the plot reads: the lines, or the values read on the Modbus line. */
  function source(): (StoredLine | Sample)[] {
    return tab.plotView.mode === "modbus" ? tab.modbus.samples : tab.textLines;
  }

  /** The number of the first item the plot reads, as Clear left it. */
  function firstPlotted(mode: PlotView["mode"]): number {
    return mode === "modbus" ? tab.modbus.plotFrom : tab.plotFrom;
  }

  /** Reads the values in the lines: all of them again, or only the new ones. */
  function refresh(full: boolean) {
    const lines = source();
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

  function add(item: StoredLine | Sample, mode: PlotView["mode"], regex: RegExp | null) {
    if (item.n < firstPlotted(mode)) return;
    origin ??= item.at;
    const values = valuesOf(item, mode, regex);
    if (values.length === 0) return;
    const row = times.length;
    times.push(Math.max((item.at - origin) / 1000, times[row - 1] ?? 0));
    lineNumbers.push(item.n);
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

  /** A line's values, read as the mode says, or what a Modbus read got. */
  function valuesOf(item: StoredLine | Sample, mode: PlotView["mode"], regex: RegExp | null) {
    if (mode === "modbus") {
      const sample = item as Sample;
      return sample.ok ? sample.values : [];
    }
    const line = item as StoredLine;
    // Zephyr's time and module are no values; a regex gets the whole line.
    return mode === "regex" ? readValues(line.text, "regex", regex) : readValues(line.level ? line.message : line.text);
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
    void [tab.plotView.mode, pattern, tab.plotFrom, tab.modbus.plotFrom];
    untrack(() => {
      const from = `${tab.plotFrom} ${tab.modbus.plotFrom}`;
      if (from !== plottedFrom) {
        plottedFrom = from;
        restart();
      }
      refresh(true);
    });
  });
  // New lines or Modbus values: only they need reading.
  $effect(() => {
    void [tab.linesVersion, tab.modbus.version];
    untrack(() => refresh(false));
  });
  // Another chart, or other settings for it, draws it again at once.
  let lastKind: Chart | null = null;
  $effect(() => {
    const view = tab.plotView;
    void [view.chart, view.x_series, view.bins, view.fft_window, view.fft_db, view.fft_rate, view.window_ms];
    void [trigger.on, trigger.series, trigger.level, trigger.edge, trigger.single];
    untrack(() => {
      if (view.chart !== lastKind) {
        lastKind = view.chart;
        zoom = null;
        hover = null;
      }
      redrawNow();
    });
  });
  // The other theme draws the chart again in its colors.
  $effect(() => {
    void appScheme();
    untrack(() => chart && build());
  });
  // The cursors pick the time the other charts work on.
  $effect(() => {
    void [cursorA, cursorB];
    untrack(() => {
      if (tab.plotView.chart === "time") chart?.redraw(false);
      else redrawNow();
      touchStats(true);
    });
  });

  /** Draws the new values: on the next frame, or a little later for the charts that work them out again. */
  function draw() {
    if (frame || slow) return;
    if (tab.plotView.chart === "time") {
      frame = requestAnimationFrame(render);
    } else {
      slow = setTimeout(() => {
        slow = undefined;
        frame = requestAnimationFrame(render);
      }, SLOW_MS);
    }
  }

  function redrawNow() {
    cancelAnimationFrame(frame);
    clearTimeout(slow);
    slow = undefined;
    frame = requestAnimationFrame(render);
  }

  function render() {
    frame = 0;
    if (follow) span = newestSpan();
    if (!chart || builtFor !== signature()) {
      build();
    } else {
      chart.setData(data(), tab.plotView.chart !== "time");
      showRange();
    }
    touchStats(false);
  }

  /** The series on the chart, by their place in `names`: all of them, but XY's x. */
  function charted(): number[] {
    const all = names.map((_, index) => index);
    return tab.plotView.chart === "xy" ? all.filter((index) => index !== xIndex()) : all;
  }

  function xIndex(): number {
    return columnOf.get(tab.plotView.x_series) ?? 0;
  }

  function triggerIndex(): number {
    return columnOf.get(tab.plotView.trigger.series) ?? 0;
  }

  function shown(index: number): boolean {
    return !tab.plotView.hidden.includes(names[index]);
  }

  function signature(): string {
    const view = tab.plotView;
    return [view.chart, names.length, view.chart === "xy" ? xIndex() : "", view.chart === "fft" ? view.fft_db : ""].join("|");
  }

  /** The time the charts and stats work on: between A and B, or else the time shown. */
  function range(): Span {
    if (cursorA !== null && cursorB !== null) {
      return { min: Math.min(cursorA, cursorB), max: Math.max(cursorA, cursorB) };
    }
    return span;
  }

  /** The points in a time, as a start and an end. */
  function pointsIn(time: Span): [number, number] {
    return [firstFrom(time.min), firstFrom(time.max + 1e-9)];
  }

  /**
   * The time to show while following: the newest values, as much as the span
   * holds; or with the trigger on, the time around its last crossing.
   */
  function newestSpan(): Span {
    const last = times.at(-1) ?? 0;
    const width = tab.plotView.window_ms / 1000;
    if (width <= 0) {
      const first = times[0] ?? 0;
      return { min: first, max: Math.max(last, first + 1e-3) };
    }
    const column = columns[triggerIndex()];
    if (tab.plotView.chart === "time" && trigger.on && column) {
      // A crossing shows once there are values for the half after it.
      const after = trigger.single ? armedAt : -Infinity;
      const crossing = lastCrossing(times, column, trigger.level, trigger.edge, after, last - width / 2);
      if (crossing !== null) {
        triggeredAt = crossing;
        // Single holds the first crossing since it was armed.
        if (trigger.single) follow = false;
        return { min: crossing - width / 2, max: crossing + width / 2 };
      }
    }
    return { min: Math.max(0, last - width), max: Math.max(last, width) };
  }

  function showRange() {
    if (!chart) return;
    if (tab.plotView.chart === "time") {
      chart.setScale("x", span);
    } else if (zoom) {
      chart.setScale("x", zoom);
    }
  }

  function data(): uPlot.AlignedData {
    switch (tab.plotView.chart) {
      case "xy":
        return xyData();
      case "histogram":
        return histogramData();
      case "fft":
        return fftData();
      default:
        return [times, ...columns] as uPlot.AlignedData;
    }
  }

  /** The other series against x, sorted along x, in the time the charts work on. */
  function xyData(): uPlot.AlignedData {
    const x = columns[xIndex()] ?? [];
    const [start, end] = pointsIn(range());
    let order: number[] = [];
    for (let row = start; row < end; row++) {
      if (x[row] !== null && x[row] !== undefined) order.push(row);
    }
    if (order.length > XY_MAX) {
      const step = order.length / XY_MAX;
      order = Array.from({ length: XY_MAX }, (_, at) => order[Math.floor(at * step)]);
    }
    order.sort((a, b) => (x[a] as number) - (x[b] as number));
    xyRows = order;
    return [
      order.map((row) => x[row] as number),
      ...charted().map((index) => (shown(index) ? order.map((row) => columns[index][row]) : order.map(() => null))),
    ] as uPlot.AlignedData;
  }

  /** The shown series' values counted in shared bins, drawn as steps. */
  function histogramData(): uPlot.AlignedData {
    const [start, end] = pointsIn(range());
    const counted = charted().filter(shown);
    const values = counted.map((index) => {
      const out: number[] = [];
      const column = columns[index];
      for (let row = start; row < end; row++) {
        const value = column[row];
        if (value !== null) out.push(value);
      }
      return out;
    });
    const result = histogram(values, tab.plotView.bins);
    bins = {
      edges: result.edges,
      counts: names.map((_, index) => {
        const at = counted.indexOf(index);
        return at >= 0 ? result.counts[at] : null;
      }),
    };
    // A step from each edge to the next: the last count is said again at the top edge.
    return [
      result.edges,
      ...charted().map((index) => {
        const counts = bins.counts[index];
        return counts ? [...counts, counts.at(-1) ?? 0] : result.edges.map(() => null);
      }),
    ] as uPlot.AlignedData;
  }

  /**
   * The shown series' spectra, from the values in the time the charts work
   * on. Lines are taken as evenly spaced, at the measured rate or the one
   * typed; a series that is not on every line keeps its last value.
   */
  function fftData(): uPlot.AlignedData {
    const [first, end] = pointsIn(range());
    const start = Math.max(first, end - FFT_MAX);
    const count = end - start;
    const duration = count > 1 ? times[end - 1] - times[start] : 0;
    const measured = duration > 0 ? (count - 1) / duration : 0;
    const rate = tab.plotView.fft_rate > 0 ? tab.plotView.fft_rate : measured;
    spectra = names.map(() => null);
    let freqs: number[] = [];
    let size = 0;
    if (count >= 8 && rate > 0) {
      for (const index of charted()) {
        if (!shown(index)) continue;
        const column = columns[index];
        let held = heldAt(column, start);
        for (let row = start; held === null && row < end; row++) held = column[row];
        if (held === null) continue;
        const samples = new Float64Array(count);
        for (let row = start; row < end; row++) {
          held = column[row] ?? held;
          samples[row - start] = held as number;
        }
        const result = spectrum(samples, rate, tab.plotView.fft_window, tab.plotView.fft_db);
        spectra[index] = result;
        freqs = result.freqs;
        size = result.size;
      }
    }
    fftInfo = { rate, measured, samples: count, size };
    return [freqs, ...charted().map((index) => spectra[index]?.levels ?? freqs.map(() => null))] as uPlot.AlignedData;
  }

  function seriesFor(index: number, colors: string[]): uPlot.Series {
    const color = colors[index % colors.length];
    const base = {
      label: names[index],
      stroke: color,
      dash: DASHES[Math.floor(index / colors.length)],
      show: shown(index),
    };
    switch (tab.plotView.chart) {
      case "xy":
        return { ...base, paths: () => null, points: { show: true, size: 4, width: 0, fill: color, stroke: color } };
      case "histogram":
        return {
          ...base,
          width: 1.5,
          fill: `${color}26`,
          paths: uPlot.paths.stepped!({ align: 1 }),
          points: { show: false },
        };
      case "fft":
        return { ...base, width: 1.5, points: { show: false } };
      default:
        // A series that is not on every line is still one line.
        return { ...base, width: 2, spanGaps: true };
    }
  }

  /** Makes the chart for the kind picked, with a series for each name. */
  function build() {
    const scheme = appScheme();
    const ink = INK[scheme];
    const colors = COLORS[scheme];
    const view = tab.plotView;
    chart?.destroy();
    const axis = {
      stroke: ink.text,
      font: `11px ${FONT_FAMILY}`,
      grid: { stroke: ink.grid, width: 1 },
      ticks: { stroke: ink.grid, width: 1, size: 4 },
    };
    const unit = (suffix: string) => (_chart: uPlot, splits: number[], _axis: number, _space: number, step: number) =>
      splits.map((value) => `${value.toFixed(decimals(step))}${suffix}`);
    const fromZero = view.chart === "histogram" || (view.chart === "fft" && !view.fft_db);
    // dB go down to 120 below the top, where noise is no longer worth the room.
    const y: uPlot.Scale =
      view.chart === "fft" && view.fft_db
        ? { range: (_chart: uPlot, min: number, max: number) => [Math.max(min, max - 120), max + 6] }
        : fromZero
          ? { range: (_chart: uPlot, _min: number, max: number) => [0, max > 0 ? max * 1.08 : 1] }
          : {};
    building = true;
    chart = new uPlot(
      {
        width: host.clientWidth,
        height: host.clientHeight,
        legend: { show: false },
        scales: {
          x: {
            time: false,
            // XY's points at the ends are not cut in half.
            ...(view.chart === "xy" ? { range: (_chart: uPlot, min: number, max: number) => uPlot.rangeNum(min, max, 0.05, true) } : {}),
          },
          y,
        },
        cursor: {
          y: false,
          drag: { x: true, y: false, setScale: false },
          // A double-click is handled below, rather than showing all.
          bind: { dblclick: () => null },
        },
        series: [{ label: "x" }, ...charted().map((index) => seriesFor(index, colors))],
        axes: [
          {
            ...axis,
            ...(view.chart === "time" ? { values: unit(" s") } : view.chart === "fft" ? { values: unit(" Hz") } : {}),
          },
          {
            ...axis,
            ...(view.chart === "fft" && view.fft_db ? { values: unit(" dB") } : {}),
            size: (_chart: uPlot, values: string[]) => axisWidth(values),
          },
        ],
        hooks: {
          setSelect: [zoomToSelection],
          draw: [drawOverlay],
          setCursor: [(self: uPlot) => (hover = self.cursor.idx ?? null)],
          setScale: [
            (self: uPlot, key: string) => {
              if (key !== "x" || building) return;
              const { min, max } = self.scales.x;
              if (tab.plotView.chart === "time" && min != null && max != null) span = { min, max };
              touchStats(false);
            },
          ],
        },
      },
      data(),
      host,
    );
    building = false;
    builtFor = signature();
    listen(chart.over);
    showRange();
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

  function setFollow(on: boolean) {
    follow = on;
    if (on) redrawNow();
  }

  /** Pause and Follow; with a Single trigger, Stop and Arm. */
  function toggleRun() {
    if (!follow && trigger.on && trigger.single) {
      triggeredAt = null;
      armedAt = times.at(-1) ?? -Infinity;
    }
    setFollow(!follow);
  }

  /** Zooms along x: the time chart stops following, the others keep up with the values. */
  function zoomTo(min: number, max: number) {
    if (!chart || !(max > min)) return;
    if (tab.plotView.chart === "time") {
      follow = false;
    } else {
      zoom = { min, max };
    }
    chart.setScale("x", { min, max });
  }

  function zoomToSelection(self: uPlot) {
    const { left, width } = self.select;
    if (width > 3) {
      zoomTo(self.posToVal(left, "x"), self.posToVal(left + width, "x"));
    }
    self.setSelect({ left: 0, top: 0, width: 0, height: 0 }, false);
  }

  /** Clicks place the cursors on the time chart, the wheel zooms, a double-click shows all and follows. */
  function listen(over: HTMLElement) {
    let downX = 0;
    let before: [number | null, number | null] = [null, null];
    over.addEventListener("mousedown", (event) => (downX = event.clientX));
    over.addEventListener("click", (event) => {
      // A drag zooms instead.
      if (!chart || Math.abs(event.clientX - downX) > 3) return;
      const time = tab.plotView.chart === "time";
      if (event.detail >= 2) {
        // The first click of a double-click placed a cursor: it goes again.
        if (time) [cursorA, cursorB] = before;
        zoom = null;
        setFollow(true);
        return;
      }
      if (time) {
        before = [cursorA, cursorB];
        placeCursor(chart.posToVal(event.clientX - over.getBoundingClientRect().left, "x"));
      }
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
    if (event.shiftKey || Math.abs(event.deltaX) > Math.abs(event.deltaY)) {
      const step = (max - min) * 0.1 * Math.sign(event.deltaX || event.deltaY);
      zoomTo(min + step, max + step);
    } else {
      const at = chart.posToVal(event.clientX - chart.over.getBoundingClientRect().left, "x");
      const factor = event.deltaY > 0 ? 1.25 : 0.8;
      zoomTo(at - (at - min) * factor, at + (max - at) * factor);
    }
  }

  /** Draws, over the time chart, the A and B cursors with the time between them shaded, and the trigger. */
  function drawOverlay(self: uPlot) {
    if (tab.plotView.chart !== "time") return;
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
    const tag = { width: 16 * ratio, height: 15 * ratio };
    const mark = (x: number, label: string, color: string) => {
      const line = Math.round(x) + 0.5;
      ctx.strokeStyle = color;
      ctx.setLineDash([4 * ratio, 3 * ratio]);
      ctx.beginPath();
      ctx.moveTo(line, top);
      ctx.lineTo(line, top + height);
      ctx.stroke();
      ctx.fillStyle = color;
      ctx.fillRect(line - tag.width / 2, top, tag.width, tag.height);
      ctx.fillStyle = ink.bg;
      ctx.fillText(label, line, top + tag.height / 2 + ratio / 2);
    };
    ["A", "B"].forEach((label, index) => {
      const x = at[index];
      if (x !== null && x >= left && x <= left + width) mark(x, label, ink.cursor);
    });
    if (trigger.on) {
      // The level as a dotted line, with T at its right end.
      const y = self.valToPos(trigger.level, "y", true);
      if (y >= top && y <= top + height) {
        const line = Math.round(y) + 0.5;
        ctx.strokeStyle = ink.text;
        ctx.setLineDash([2 * ratio, 3 * ratio]);
        ctx.beginPath();
        ctx.moveTo(left, line);
        ctx.lineTo(left + width, line);
        ctx.stroke();
        ctx.fillStyle = ink.text;
        ctx.fillRect(left + width - tag.width, line - tag.height / 2, tag.width, tag.height);
        ctx.fillStyle = ink.bg;
        ctx.fillText("T", left + width - tag.width / 2, line + ratio / 2);
      }
      if (triggeredAt !== null) {
        const x = self.valToPos(triggeredAt, "x", true);
        if (x >= left && x <= left + width) mark(x, "T", ink.text);
      }
    }
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

  type Stats = { min: number; max: number; mean: number; deviation: number; count: number } | null;

  /** Each series' min, max, mean and standard deviation, in the time the charts work on. */
  const stats = $derived.by(() => {
    void statsVersion;
    const both = cursorA !== null && cursorB !== null;
    const time = range();
    const [start, end] = pointsIn(time);
    const series: Stats[] = columns.map((column) => {
      let min = Infinity;
      let max = -Infinity;
      let mean = 0;
      let squares = 0;
      let count = 0;
      for (let index = start; index < end; index++) {
        const value = column[index];
        if (value === null) continue;
        if (value < min) min = value;
        if (value > max) max = value;
        // Welford's way, which stays exact for values far from 0.
        count++;
        const step = value - mean;
        mean += step / count;
        squares += step * (value - mean);
      }
      return count > 0 ? { min, max, mean, deviation: Math.sqrt(squares / count), count } : null;
    });
    return { both, from: time.min, to: time.max, points: Math.max(0, end - start), series };
  });

  /** The table: each series' stats, and its value under the mouse, at A and B, or its peak. */
  const table = $derived.by(() => {
    void statsVersion;
    const chartKind = tab.plotView.chart;
    return names.map((name, index) => {
      const column = columns[index] ?? [];
      const row = {
        name,
        index,
        stats: stats.series[index],
        value: null as number | null,
        a: null as number | null,
        b: null as number | null,
        peak: null as { freq: number; level: number } | null,
      };
      if (chartKind === "fft") {
        const result = spectra[index];
        row.peak = result ? peak(result.freqs, result.levels) : null;
        row.value = result && hover !== null ? (result.levels[hover] ?? null) : null;
      } else if (chartKind === "histogram") {
        const counts = bins.counts[index];
        row.value = counts && hover !== null ? (counts[Math.min(hover, counts.length - 1)] ?? null) : null;
      } else if (chartKind === "xy") {
        row.value = hover !== null && xyRows[hover] !== undefined ? column[xyRows[hover]] : heldAt(column, column.length - 1);
      } else {
        row.value = heldAt(column, hover ?? column.length - 1);
        row.a = cursorA !== null ? valueAt(column, cursorA) : null;
        row.b = cursorB !== null ? valueAt(column, cursorB) : null;
      }
      return row;
    });
  });

  /** What the column of values under the mouse is called now. */
  const hoverTitle = $derived.by(() => {
    void statsVersion;
    if (kind === "fft") return hover !== null && spectra.some(Boolean) ? `At ${hz(fftAt(hover))}` : null;
    if (kind === "histogram") {
      const [from, to] = [bins.edges[hover ?? -1], bins.edges[(hover ?? -1) + 1]];
      return from !== undefined && to !== undefined ? `${number(from)} to ${number(to)}` : null;
    }
    if (kind === "xy") return hover !== null ? "At the point" : "Last";
    return hover !== null && times[hover] !== undefined ? `At ${number(times[hover])} s` : "Last";
  });

  function fftAt(index: number): number {
    return spectra.find(Boolean)?.freqs[index] ?? 0;
  }

  /** A number short enough for the table: six digits at most after the first. */
  function number(value: number | null | undefined): string {
    if (value === null || value === undefined || !Number.isFinite(value)) return "";
    return Number.isInteger(value) ? String(value) : String(Number(value.toPrecision(6)));
  }

  function level(value: number | null | undefined): string {
    const text = number(value);
    return text && tab.plotView.fft_db ? `${number(Number((value as number).toFixed(2)))} dB` : text;
  }

  function duration(seconds: number): string {
    return Math.abs(seconds) < 1 ? `${number(seconds * 1000)} ms` : `${number(seconds)} s`;
  }

  function hz(value: number): string {
    return Math.abs(value) >= 1000 ? `${number(value / 1000)} kHz` : `${number(value)} Hz`;
  }

  function frequency(seconds: number): string {
    return hz(1 / Math.abs(seconds));
  }

  function toggleSeries(name: string, index: number) {
    const hidden = tab.plotView.hidden.includes(name)
      ? tab.plotView.hidden.filter((other) => other !== name)
      : [...tab.plotView.hidden, name];
    setPlotView({ hidden });
    const position = charted().indexOf(index);
    if (position >= 0) chart?.setSeries(position + 1, { show: !hidden.includes(name) });
    // The other charts work out only what is shown.
    if (tab.plotView.chart !== "time") redrawNow();
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

  /** Turns the trigger on at the middle of its series' values in view, unless its level is among them. */
  function toggleTrigger(on: boolean) {
    if (on) {
      const found = stats.series[triggerIndex()];
      const level = trigger.level;
      if (found && (level < found.min || level > found.max)) {
        setTrigger({ on, level: Number(((found.min + found.max) / 2).toPrecision(3)) });
        return;
      }
    }
    setTrigger({ on });
  }

  /**
   * What the chart shows, as CSV: every value with its time for the time and
   * XY charts, each bin's counts for the histogram, each frequency's levels
   * for the FFT.
   */
  function csv(): string {
    const field = (text: string) => (/[",\n]/.test(text) ? `"${text.replaceAll('"', '""')}"` : text);
    const cell = (value: number | null | undefined) => (value === null || value === undefined ? "" : String(value));
    const out: string[] = [];
    if (kind === "histogram") {
      const counted = names.map((_, index) => index).filter((index) => bins.counts[index]);
      out.push(["from", "to", ...counted.map((index) => names[index])].map(field).join(","));
      for (let bin = 0; bin + 1 < bins.edges.length; bin++) {
        out.push([bins.edges[bin], bins.edges[bin + 1], ...counted.map((index) => bins.counts[index]![bin])].join(","));
      }
    } else if (kind === "fft") {
      const counted = names.map((_, index) => index).filter((index) => spectra[index]);
      const unit = tab.plotView.fft_db ? " (dB)" : "";
      out.push(["hz", ...counted.map((index) => `${names[index]}${unit}`)].map(field).join(","));
      const freqs = spectra[counted[0]]?.freqs ?? [];
      freqs.forEach((freq, bin) => out.push([freq, ...counted.map((index) => cell(spectra[index]!.levels[bin]))].join(",")));
    } else {
      out.push(["time", "seconds", ...names].map(field).join(","));
      for (let row = 0; row < times.length; row++) {
        const cells = [new Date((origin ?? 0) + times[row] * 1000).toISOString(), times[row].toFixed(3)];
        for (const column of columns) cells.push(cell(column[row]));
        out.push(cells.join(","));
      }
    }
    return `${out.join("\n")}\n`;
  }

  /** The tab's name, without the characters file names cannot have. */
  function fileName(extension: string): string {
    const suffix = kind === "time" ? "" : ` ${kind}`;
    return `${tab.name.replace(/[<>:"/\\|?*]/g, "_")}${suffix}.${extension}`;
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
    // The legend, in rows as wide as the chart. XY's series are named against x.
    const against = kind === "xy" ? ` against ${names[xIndex()]}` : "";
    const shownNow = charted()
      .map((index, position) => ({ name: `${names[index]}${against}`, index, position }))
      .filter(({ position }) => chart!.series[position + 1].show);
    const placed: { name: string; index: number; x: number; y: number }[] = [];
    let x = pad;
    let y = pad;
    for (const item of shownNow) {
      const width = swatch + 6 * ratio + ctx.measureText(item.name).width;
      if (x > pad && x + width > source.width - pad) {
        x = pad;
        y += row;
      }
      placed.push({ ...item, x, y });
      x += width + 16 * ratio;
    }
    const legend = shownNow.length > 0 ? y + row : 0;
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
      clearTimeout(slow);
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
      toggleRun();
    } else if (event.key === "Escape" && cursorA !== null) {
      event.preventDefault();
      clearCursors();
    }
  }

  const single = $derived(kind === "time" && trigger.on && trigger.single);
  /** Notes on what the chart cannot show yet. */
  const notice = $derived.by(() => {
    void statsVersion;
    if (names.length === 0) return null;
    if (kind === "xy" && names.length < 2) return "XY needs two series: one along x, the others up.";
    if (kind === "fft" && (fftInfo.samples < 8 || fftInfo.rate <= 0)) return "An FFT needs 8 values or more in the time it works on.";
    return null;
  });
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div class="plotview" tabindex="0" role="region" aria-label="Plot" bind:this={root} {onkeydown}>
  <div class="bar">
    <label class="option" title="How numbers are read from the lines: as they are written (temp=21.5, 21.5,40, JSON), or with a regex">
      Read
      <select
        class="field"
        value={tab.plotView.mode}
        onchange={(event) => setMode(event.currentTarget.value as PlotView["mode"])}
      >
        <option value="auto">Numbers</option>
        <option value="regex">Regex</option>
        {#if tab.kind === "serial"}
          <option value="modbus">Modbus</option>
        {/if}
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

    <div class="kinds" role="group" aria-label="Chart">
      {#each CHARTS as { chart: value, label, title } (value)}
        <button class="kind" class:on={kind === value} aria-pressed={kind === value} {title} onclick={() => setPlotView({ chart: value })}>
          {label}
        </button>
      {/each}
    </div>

    <label class="option" title="The time shown while the chart follows the newest values; the other charts work on it too">
      Show
      <select
        class="field"
        value={tab.plotView.window_ms}
        onchange={(event) => {
          setPlotView({ window_ms: Number(event.currentTarget.value) });
          setFollow(true);
        }}
      >
        {#each SPANS as { ms, label } (ms)}
          <option value={ms}>{label}</option>
        {/each}
      </select>
    </label>

    <button
      class="btn small follow"
      class:paused={!follow}
      title={single
        ? follow
          ? "Stop waiting for the trigger (Space)"
          : "Wait for the next crossing (Space)"
        : follow
          ? "Stop following the newest values (Space)"
          : "Follow the newest values again (Space, or double-click the chart)"}
      onclick={toggleRun}
    >
      {single ? (follow ? "Stop" : "Arm") : follow ? "Pause" : "Follow"}
    </button>
    {#if cursorA !== null}
      <button class="btn small" title="Take the A and B cursors away (Esc)" onclick={clearCursors}>Clear A/B</button>
    {/if}

    {#if kind === "time"}
      <label class="check" title="Show the time around where a series crosses a level, as a scope does">
        <input type="checkbox" checked={trigger.on} onchange={(event) => toggleTrigger(event.currentTarget.checked)} />
        Trigger
      </label>
      {#if trigger.on}
        <select
          class="field compact"
          title="The series the trigger watches"
          value={names[triggerIndex()] ?? ""}
          onchange={(event) => setTrigger({ series: event.currentTarget.value })}
        >
          {#each names as name (name)}
            <option value={name}>{name}</option>
          {/each}
        </select>
        <select
          class="field compact"
          title="Which way the series crosses the level"
          value={trigger.edge}
          onchange={(event) => setTrigger({ edge: event.currentTarget.value as Trigger["edge"] })}
        >
          {#each EDGES as { value, label } (value)}
            <option {value}>{label}</option>
          {/each}
        </select>
        <label class="option" title="The level the series crosses">
          at
          <input
            class="field mono level"
            type="number"
            step="any"
            value={trigger.level}
            onchange={(event) => {
              const value = Number(event.currentTarget.value);
              if (Number.isFinite(value)) setTrigger({ level: value });
            }}
          />
        </label>
        <label class="check" title="Stop at the first crossing; Arm waits for the next">
          <input type="checkbox" checked={trigger.single} onchange={(event) => setTrigger({ single: event.currentTarget.checked })} />
          Single
        </label>
        <span class="status number" title="When the series last crossed the level">
          {triggeredAt !== null ? `T at ${number(Number(triggeredAt.toFixed(3)))} s` : "Waiting"}
        </span>
      {/if}
    {:else if kind === "xy"}
      <label class="option" title="The series along x; the others go up">
        X
        <select class="field" value={names[xIndex()] ?? ""} onchange={(event) => setPlotView({ x_series: event.currentTarget.value })}>
          {#each names as name (name)}
            <option value={name}>{name}</option>
          {/each}
        </select>
      </label>
    {:else if kind === "histogram"}
      <label class="option" title="How many bins the values are counted in">
        Bins
        <select class="field" value={tab.plotView.bins} onchange={(event) => setPlotView({ bins: Number(event.currentTarget.value) })}>
          {#each BINS as count (count)}
            <option value={count}>{count === 0 ? "Auto" : count}</option>
          {/each}
        </select>
      </label>
    {:else}
      <label class="option" title="How the values are weighted before the FFT, so a tone between two bins spreads less">
        Window
        <select
          class="field"
          value={tab.plotView.fft_window}
          onchange={(event) => setPlotView({ fft_window: event.currentTarget.value as PlotView["fft_window"] })}
        >
          {#each FFT_WINDOWS as { value, label } (value)}
            <option {value}>{label}</option>
          {/each}
        </select>
      </label>
      <select
        class="field compact"
        title="Levels in dB, or as amplitudes: a sine of amplitude 1 is 0 dB"
        value={tab.plotView.fft_db ? "db" : "amplitude"}
        onchange={(event) => setPlotView({ fft_db: event.currentTarget.value === "db" })}
      >
        <option value="db">dB</option>
        <option value="amplitude">Amplitude</option>
      </select>
      <label class="option" title="Values a second. Empty measures it from when the lines came; type it when the device sends at a known rate.">
        Rate
        <input
          class="field mono rate"
          type="number"
          min="0"
          step="any"
          placeholder={fftInfo.measured > 0 ? number(Number(fftInfo.measured.toPrecision(4))) : "auto"}
          value={tab.plotView.fft_rate > 0 ? tab.plotView.fft_rate : ""}
          onchange={(event) => {
            const value = Number(event.currentTarget.value);
            setPlotView({ fft_rate: Number.isFinite(value) && value > 0 ? value : 0 });
          }}
        />
        Hz
      </label>
    {/if}

    <span class="spacer"></span>
    <span class="total" title="Lines with values, since the plot was cleared">{points.toLocaleString()} points</span>
    <button class="btn small" disabled={points === 0} title="Save what the chart shows to a CSV file" onclick={exportCsv}>
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
      aria-label="The chart; the table below has its values in numbers"
      bind:this={host}
    ></div>
    {#if names.length === 0}
      <div class="empty">
        <p>
          {#if tab.plotView.mode === "regex"}
            {draftProblem ?? "No line matches the regex yet."}
          {:else if tab.plotView.mode === "modbus"}
            Nothing read yet. Poll some registers in the Modbus view, or listen to a bus where a master reads them,
            and their values are drawn here.
          {:else}
            No numbers yet. Lines such as <code>temp=21.5 hum=40</code>, <code>21.5,40</code> or
            <code>{'{"temp": 21.5}'}</code> are drawn as they come. For other lines, read them with a regex.
          {/if}
        </p>
      </div>
    {:else if notice}
      <p class="notice">{notice}</p>
    {/if}
  </div>

  {#if names.length > 0}
    <div class="range">
      {#if stats.both}
        <span>
          <b>A to B</b>
          {duration(stats.to - stats.from)}{kind === "time" && stats.to > stats.from ? `, ${frequency(stats.to - stats.from)}` : ""}
        </span>
      {:else}
        <span><b>{kind === "time" ? "In view" : "Over"}</b> {duration(Math.max(0, stats.to - stats.from))}</span>
      {/if}
      <span class="number">{stats.points.toLocaleString()} points</span>
      {#if kind === "fft" && fftInfo.size > 0}
        <span>
          {fftInfo.samples > FFT_MAX ? "the newest " : ""}{Math.min(fftInfo.samples, FFT_MAX).toLocaleString()} at
          {hz(fftInfo.rate)}{tab.plotView.fft_rate > 0 ? "" : " (measured)"}, {hz(fftInfo.rate / fftInfo.size)} a bin
        </span>
      {:else if kind === "histogram" && bins.edges.length > 1}
        <span>{bins.edges.length - 1} bins of {number(bins.edges[1] - bins.edges[0])}</span>
      {/if}
      {#if tooMany}
        <span class="warning">Only the first {SERIES_MAX} series are drawn.</span>
      {/if}
      <span class="spacer"></span>
      <span class="hint">
        {kind === "time"
          ? "Click: cursors A and B · Drag or wheel: zoom · Shift+wheel: move · Double-click: follow"
          : "Drag or wheel: zoom · Double-click: all · A and B on the time chart pick the time"}
      </span>
    </div>
    <div class="table">
      <table>
        <thead>
          <tr>
            <th class="name">Series</th>
            {#if kind === "fft"}
              <th>Peak at</th>
              <th>Peak</th>
              {#if hoverTitle}<th>{hoverTitle}</th>{/if}
            {:else}
              {#if kind === "histogram"}
                <th>Count</th>
                {#if hoverTitle}<th>{hoverTitle}</th>{/if}
              {:else}
                <th>{hoverTitle}</th>
              {/if}
              <th>Min</th>
              <th>Max</th>
              <th>Mean</th>
              <th>Std dev</th>
              {#if kind === "time" && cursorA !== null}<th>At A</th>{/if}
              {#if kind === "time" && cursorB !== null}<th>At B</th><th>B − A</th>{/if}
            {/if}
          </tr>
        </thead>
        <tbody>
          {#each table as row (row.name)}
            {@const shownNow = !tab.plotView.hidden.includes(row.name)}
            {@const color = COLORS[appScheme()][row.index % COLORS.light.length]}
            {@const dash = DASHES[Math.floor(row.index / COLORS.light.length)]}
            {@const isX = kind === "xy" && row.index === xIndex()}
            <tr class:off={!shownNow && !isX}>
              <td class="name">
                <label title={isX ? "Along x" : "Show or hide this series"}>
                  <input type="checkbox" checked={shownNow || isX} disabled={isX} onchange={() => toggleSeries(row.name, row.index)} />
                  {#if isX}
                    <span class="axis">x</span>
                  {:else}
                    <svg width="18" height="8" aria-hidden="true">
                      <line x1="0" y1="4" x2="18" y2="4" stroke={color} stroke-width="2" stroke-dasharray={dash?.join(" ")} />
                    </svg>
                  {/if}
                  <span class="mono">{row.name}</span>
                </label>
              </td>
              {#if kind === "fft"}
                <td>{row.peak ? hz(row.peak.freq) : ""}</td>
                <td>{level(row.peak?.level)}</td>
                {#if hoverTitle}<td>{level(row.value)}</td>{/if}
              {:else}
                {#if kind === "histogram"}
                  <td>{row.stats?.count.toLocaleString() ?? ""}</td>
                  {#if hoverTitle}<td>{number(row.value)}</td>{/if}
                {:else}
                  <td>{number(row.value)}</td>
                {/if}
                <td>{number(row.stats?.min)}</td>
                <td>{number(row.stats?.max)}</td>
                <td>{number(row.stats?.mean)}</td>
                <td>{number(row.stats?.deviation)}</td>
                {#if kind === "time" && cursorA !== null}<td>{number(row.a)}</td>{/if}
                {#if kind === "time" && cursorB !== null}
                  <td>{number(row.b)}</td>
                  <td>{row.a !== null && row.b !== null ? number(row.b - row.a) : ""}</td>
                {/if}
              {/if}
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

  /* Where there is no room, the rest goes to a second row on the right. */
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

  .check:has(input:checked) {
    color: var(--text);
  }

  .bar .field {
    height: 24px;
    font-size: 12px;
  }

  .regex {
    width: 200px;
  }

  .regex.bad {
    border-color: var(--bad);
  }

  .level {
    width: 80px;
  }

  .rate {
    width: 72px;
  }

  .kinds {
    display: flex;
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
  }

  .kind {
    height: 24px;
    padding: 0 9px;
    font: inherit;
    color: var(--muted);
    background: none;
    border: 0;
    border-left: 1px solid var(--border);
    cursor: default;
  }

  .kind:first-child {
    border-left: 0;
  }

  .kind:hover {
    color: var(--text);
    background: var(--hover);
  }

  .kind.on {
    color: var(--text);
    background: var(--accent-soft);
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

  .status {
    color: var(--text);
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

  .notice {
    position: absolute;
    top: 12px;
    left: 50%;
    margin: 0;
    padding: 4px 10px;
    font-size: 12px;
    color: var(--text);
    white-space: nowrap;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 6px;
    transform: translateX(-50%);
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

  .axis {
    width: 18px;
    font-family: var(--font-ui);
    font-weight: 600;
    color: var(--muted);
    text-align: center;
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
