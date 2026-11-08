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
  /** The table's numbers: --font-mono in app.css. */
  const MONO_FAMILY = '"JetBrains Mono", ui-monospace, monospace';
  const CHARTS: { chart: Chart; label: string; title: string }[] = [
    { chart: "time", label: "Time", title: "The values over time, with cursors and a trigger" },
    { chart: "xy", label: "XY", title: "One series against the others, such as a sensor's x and y" },
    { chart: "histogram", label: "Histogram", title: "How often each value comes: its spread and noise" },
    { chart: "fft", label: "FFT", title: "The frequencies in the values" },
  ];
  /** What each chart shows, and a line of C that sends what it needs, with what it prints. */
  const GUIDES: Record<Chart, { title: string; what: string; send: string; code: string; line: string }> = {
    time: {
      title: "Time",
      what: "Each value over time, with cursors, stats and a trigger. R by a series gives it a scale of its own on the right.",
      send: "Send a line for each reading, a name before each value:",
      code: 'printf("temp=%.1f hum=%.1f\\n", temp, hum);',
      line: "temp=21.5 hum=40.2",
    },
    xy: {
      title: "XY",
      what: "One value against another, as an X-Y scope: a circle, a Lissajous figure, a line.",
      send: "Send both values on each line, then pick the one along the bottom in X:",
      code: 'printf("x=%.3f y=%.3f\\n", x, y);',
      line: "x=0.500 y=-0.207",
    },
    histogram: {
      title: "Histogram",
      what: "How often each value comes: the spread and the noise of a reading.",
      send: "Send the reading again and again, one line each:",
      code: 'printf("adc=%d\\n", adc);',
      line: "adc=2048",
    },
    fft: {
      title: "FFT",
      what: "The frequencies in a signal, the strongest named in the table.",
      send: "Send samples at a steady rate, one line each, and type that rate in Rate:",
      code: 'printf("sig=%.4f\\n", sample);',
      line: "sig=0.7071",
    },
  };
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
  /** The most decimals each series' values have had, up to 6, so the table shows them all with as many. */
  const decimalsOf: number[] = [];
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
  /** The frame that moves a following time chart with the clock, while one is asked for. */
  let glideFrame = 0;
  /** About how far apart the values come, in seconds: a gliding chart keeps a little more than that behind the clock. */
  let spacing = 0.05;

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
    decimalsOf.length = 0;
    names = [];
    tooMany = false;
    scanned = 0;
  }

  /** Starts again from nothing: no values, no cursors, and t = 0 at the next line. */
  function restart() {
    reset();
    origin = null;
    spacing = 0.05;
    cursorA = cursorB = null;
    triggeredAt = null;
    armedAt = -Infinity;
    follow = true;
  }

  /** What the plot reads: the lines, the values read on the Modbus line, or the watched variables. */
  function source(): (StoredLine | Sample)[] {
    const mode = tab.plotView.mode;
    return mode === "modbus" ? tab.modbus.samples : mode === "watch" ? tab.watchSamples : tab.textLines;
  }

  /** The number of the first item the plot reads, as Clear left it. */
  function firstPlotted(mode: PlotView["mode"]): number {
    return mode === "modbus" ? tab.modbus.plotFrom : mode === "watch" ? tab.watchFrom : tab.plotFrom;
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
    // The tab's own lines, such as "connected to COM6, 115200 8N1", have no values.
    if ("note" in item && item.note) return;
    origin ??= item.at;
    const values = valuesOf(item, mode, regex);
    if (values.length === 0) return;
    const row = times.length;
    const time = Math.max((item.at - origin) / 1000, times[row - 1] ?? 0);
    if (row > 0) spacing += (time - times[row - 1] - spacing) * 0.05;
    times.push(time);
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
        decimalsOf.push(0);
        names.push(name);
      }
      columns[index][row] = value;
      if (decimalsOf[index] < 6) decimalsOf[index] = Math.max(decimalsOf[index], decimalsIn(value));
    }
  }

  /** A line's values, read as the mode says, or what a Modbus read got. */
  function valuesOf(item: StoredLine | Sample, mode: PlotView["mode"], regex: RegExp | null) {
    if (mode === "modbus" || mode === "watch") {
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

  // Another way of reading the lines, or Clear, starts again from nothing: the
  // values, the cursors and the trigger's last crossing all belonged to the old ones.
  $effect(() => {
    void [tab.plotView.mode, pattern, tab.plotFrom, tab.modbus.plotFrom, tab.watchFrom];
    untrack(() => {
      const from = `${tab.plotView.mode} ${pattern?.source ?? ""} ${tab.plotFrom} ${tab.modbus.plotFrom} ${tab.watchFrom}`;
      if (from !== plottedFrom) {
        plottedFrom = from;
        restart();
      }
      refresh(true);
    });
  });
  // New lines or Modbus values: only they need reading.
  $effect(() => {
    void [tab.linesVersion, tab.modbus.version, tab.watchVersion];
    untrack(() => refresh(false));
  });
  // Another chart, or other settings for it, draws it again at once.
  let lastKind: Chart | null = null;
  $effect(() => {
    const view = tab.plotView;
    void [view.chart, view.x_series, view.hist_series, view.bins, view.fft_window, view.fft_db, view.fft_rate, view.window_ms, view.dots, view.right];
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
    if (gliding() && glideFrame === 0) glideFrame = requestAnimationFrame(glide);
  }

  /** Whether the time chart follows the newest values with the clock: not with the trigger on, nor showing all. */
  function gliding(): boolean {
    return follow && tab.plotView.chart === "time" && tab.plotView.window_ms > 0 && !trigger.on && origin !== null;
  }

  /** Now, in the chart's seconds. */
  function clockTime(): number {
    return (Date.now() - (origin ?? Date.now())) / 1000;
  }

  /** How far behind the clock a gliding chart keeps: a little more than the time between values. */
  function lag(): number {
    return Math.min(1, Math.max(0.02, spacing * 1.5));
  }

  /**
   * Moves a following time chart with the clock, a frame at a time, rather
   * than a step at each new value. A frame that would move it less than half
   * a pixel draws nothing, and it stops once the values have stopped.
   */
  function glide() {
    glideFrame = 0;
    if (!chart || !gliding()) return;
    const next = newestSpan();
    const moved = ((next.max - span.max) / Math.max(1e-9, next.max - next.min)) * chart.bbox.width;
    if (Math.abs(moved) >= 0.5) {
      span = next;
      chart.setScale("x", span);
    } else if (clockTime() - (times.at(-1) ?? 0) > 1 + lag()) {
      // No values for a while: it stands still until the next ones.
      return;
    }
    glideFrame = requestAnimationFrame(glide);
  }

  /**
   * The series on the chart, by their place in `names`: all of them, but
   * XY's x; and only the one the histogram counts, as series far apart, such
   * as 2048 and 1 to 6, would share bins too wide for either.
   */
  function charted(): number[] {
    const all = names.map((_, index) => index);
    switch (tab.plotView.chart) {
      case "xy":
        return all.filter((index) => index !== xIndex());
      case "histogram":
        return all.length > 0 ? [histIndex()] : [];
      default:
        return all;
    }
  }

  function xIndex(): number {
    return columnOf.get(tab.plotView.x_series) ?? 0;
  }

  function histIndex(): number {
    return columnOf.get(tab.plotView.hist_series) ?? 0;
  }

  function triggerIndex(): number {
    return columnOf.get(tab.plotView.trigger.series) ?? 0;
  }

  function shown(index: number): boolean {
    return !tab.plotView.hidden.includes(names[index]);
  }

  /** Whether a series is on the time chart's right axis, with a scale of its own. */
  function onRight(index: number): boolean {
    return tab.plotView.chart === "time" && tab.plotView.right.includes(names[index]);
  }

  /** The scale a series is drawn on: the left axis's, or the right one's. */
  function scaleOf(index: number): string {
    return onRight(index) ? "y2" : "y";
  }

  function signature(): string {
    const view = tab.plotView;
    return [
      view.chart,
      names.length,
      view.chart === "xy" ? xIndex() : "",
      view.chart === "histogram" ? histIndex() : "",
      view.chart === "fft" ? view.fft_db : "",
      view.chart === "time" ? view.dots : "",
      view.chart === "time" ? names.filter((name) => view.right.includes(name)).join(",") : "",
    ].join("|");
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
   * holds, moving with the clock; or with the trigger on, the time around its
   * last crossing.
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
    // Gliding, the right edge goes with the clock: a little behind it, so the
    // newest line always reaches the edge, and no further than the newest
    // values once they stop coming.
    const right = gliding() ? Math.min(clockTime() - lag(), last + lag()) : last;
    return { min: Math.max(0, right - width), max: Math.max(right, width) };
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

  /** The picked series' values counted in bins, drawn as steps. */
  function histogramData(): uPlot.AlignedData {
    const [start, end] = pointsIn(range());
    const counted = charted();
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
      scale: scaleOf(index),
      stroke: color,
      dash: DASHES[Math.floor(index / colors.length)],
      // The histogram's one series is always shown: picking another hides it.
      show: tab.plotView.chart === "histogram" || shown(index),
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
        // A dot on each sample where there is room for it, unless Dots is off.
        return { ...base, width: 2, spanGaps: true, ...(tab.plotView.dots ? {} : { points: { show: false } }) };
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
    // Series far from the others, such as a count beside a voltage, on an axis of their own.
    const right = charted().filter(onRight);
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
          ...(right.length > 0 ? { y2: {} } : {}),
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
          // Named after its series, so it is plain which values it is for.
          ...(right.length > 0
            ? [
                {
                  ...axis,
                  scale: "y2",
                  side: 1,
                  grid: { show: false },
                  label: right.map((index) => names[index]).join(", "),
                  labelFont: `11px ${FONT_FAMILY}`,
                  labelSize: 16,
                  size: (_chart: uPlot, values: string[]) => axisWidth(values),
                },
              ]
            : []),
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

  /**
   * Clicks place the cursors on the time chart, the wheel zooms, a drag with
   * the right button moves the chart along as a hand would, and a
   * double-click shows all and follows.
   */
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
    over.addEventListener("contextmenu", (event) => event.preventDefault());
    over.addEventListener("pointerdown", (event) => {
      if (event.button !== 2 || !chart) return;
      const { min, max } = chart.scales.x;
      if (min == null || max == null) return;
      event.preventDefault();
      over.setPointerCapture(event.pointerId);
      over.classList.add("grabbing");
      const startX = event.clientX;
      const perPixel = (max - min) / Math.max(1, over.clientWidth);
      const move = (moved: PointerEvent) => {
        const shift = (moved.clientX - startX) * perPixel;
        zoomTo(min - shift, max - shift);
      };
      const end = () => {
        over.classList.remove("grabbing");
        over.removeEventListener("pointermove", move);
        over.removeEventListener("pointerup", end);
        over.removeEventListener("pointercancel", end);
      };
      over.addEventListener("pointermove", move);
      over.addEventListener("pointerup", end);
      over.addEventListener("pointercancel", end);
    });
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
      const y = self.valToPos(trigger.level, scaleOf(triggerIndex()), true);
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

  type Stats = { min: number; max: number; mean: number; deviation: number; rms: number; count: number } | null;

  /** Each series' min, max, mean, RMS and standard deviation in a time, and how many values it has there. */
  function statsIn(time: Span) {
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
      if (count === 0) return null;
      // A mean that is only rounding off 0, as a whole sine's is, is 0.
      if (Math.abs(mean) < 1e-9 * Math.max(Math.abs(min), Math.abs(max))) mean = 0;
      const deviation = Math.sqrt(squares / count);
      // The mean of the squares is the variance and the mean squared.
      return { min, max, mean, deviation, rms: Math.sqrt(deviation * deviation + mean * mean), count };
    });
    return { from: time.min, to: time.max, points: Math.max(0, end - start), series };
  }

  /**
   * The stats of what the chart works on: on the time chart, the time shown,
   * as A and B have a table of their own; on the others, the time between A
   * and B when both are placed.
   */
  const stats = $derived.by(() => {
    void statsVersion;
    const both = kind !== "time" && cursorA !== null && cursorB !== null;
    return { both, ...statsIn(kind === "time" ? span : range()) };
  });

  /** The time chart's A and B: each series' value at both, and its stats between them. */
  const section = $derived.by(() => {
    void statsVersion;
    const a = cursorA;
    const b = cursorB;
    if (kind !== "time" || a === null) return null;
    const time = b === null ? { min: a, max: a } : { min: Math.min(a, b), max: Math.max(a, b) };
    const found = b === null ? null : statsIn(time);
    const rows = names.map((name, index) => {
      const column = columns[index] ?? [];
      return {
        name,
        index,
        decimals: decimalsOf[index] ?? 0,
        a: valueAt(column, a),
        b: b === null ? null : valueAt(column, b),
        stats: found?.series[index] ?? null,
      };
    });
    return { both: b !== null, from: time.min, to: time.max, points: found?.points ?? 0, rows };
  });

  /** The table: each series' stats, and its value under the mouse, or its peak. */
  const table = $derived.by(() => {
    void statsVersion;
    const chartKind = tab.plotView.chart;
    return names.map((name, index) => {
      const column = columns[index] ?? [];
      const row = {
        name,
        index,
        decimals: decimalsOf[index] ?? 0,
        stats: stats.series[index],
        value: null as number | null,
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

  type TableRow = (typeof table)[number];
  type SectionRow = NonNullable<typeof section>["rows"][number];
  /** A column of a table under the chart: its heading, its tooltip, and each row's text in it. */
  type TableColumn<Row> = { title: string; tip?: string; cell: (row: Row) => string };
  /** A part of the line over a table: what it is, in bold, then how much. Help is left out of the picture. */
  type RangePart = { label?: string; text: string; number?: boolean; help?: boolean };

  /** Each series' stats, in both tables. */
  const STAT_COLUMNS: TableColumn<{ stats: Stats; decimals: number }>[] = [
    { title: "Min", cell: (row) => fixed(row.stats?.min, row.decimals) },
    { title: "Max", cell: (row) => fixed(row.stats?.max, row.decimals) },
    { title: "Pk-pk", tip: "Max − Min", cell: (row) => (row.stats ? fixed(row.stats.max - row.stats.min, row.decimals) : "") },
    { title: "Mean", cell: (row) => fixed(row.stats?.mean, row.decimals + 2) },
    {
      title: "RMS",
      tip: "Root mean square: the square root of the squares' mean",
      cell: (row) => fixed(row.stats?.rms, row.decimals + 2),
    },
    { title: "Std dev", cell: (row) => fixed(row.stats?.deviation, row.decimals + 2) },
  ];

  /** The table's columns after Series, for the chart shown: the same on screen and in the picture. */
  const tableColumns = $derived.by((): TableColumn<TableRow>[] => {
    if (kind === "fft") {
      return [
        { title: "Peak at", cell: (row) => (row.peak ? hz(row.peak.freq) : "") },
        { title: "Peak", cell: (row) => level(row.peak?.level) },
        ...(hoverTitle ? [{ title: hoverTitle, cell: (row: TableRow) => level(row.value) }] : []),
      ];
    }
    if (kind === "histogram") {
      return [
        { title: "Count", cell: (row) => row.stats?.count.toLocaleString() ?? "" },
        ...(hoverTitle ? [{ title: hoverTitle, cell: (row: TableRow) => number(row.value) }] : []),
        ...STAT_COLUMNS,
      ];
    }
    return [{ title: hoverTitle ?? "", cell: (row) => fixed(row.value, row.decimals) }, ...STAT_COLUMNS];
  });

  /** The line over the table: the time its stats are of, its points, and what the chart is made of. */
  const rangeParts = $derived.by((): RangePart[] => {
    const parts: RangePart[] = stats.both
      ? [
          {
            label: "A to B",
            text: `${duration(stats.to - stats.from)}${kind === "time" && stats.to > stats.from ? `, ${frequency(stats.to - stats.from)}` : ""}`,
          },
        ]
      : [{ label: kind === "time" ? "In view" : "Over", text: duration(Math.max(0, stats.to - stats.from)) }];
    parts.push({ text: `${stats.points.toLocaleString()} points`, number: true });
    if (kind === "fft" && fftInfo.size > 0) {
      const newest = fftInfo.samples > FFT_MAX ? "the newest " : "";
      const measured = tab.plotView.fft_rate > 0 ? "" : " (measured)";
      parts.push({
        text: `${newest}${Math.min(fftInfo.samples, FFT_MAX).toLocaleString()} at ${hz(fftInfo.rate)}${measured}, ${hz(fftInfo.rate / fftInfo.size)} a bin`,
      });
    } else if (kind === "histogram" && bins.edges.length > 1) {
      parts.push({ text: `${bins.edges.length - 1} bins of ${number(bins.edges[1] - bins.edges[0])}` });
    }
    return parts;
  });

  /** The line over A and B's table: where A is, or the time from A to B. */
  const sectionParts = $derived.by((): RangePart[] => {
    if (!section) return [];
    if (!section.both) {
      return [
        { label: "A", text: `at ${number(Number(section.from.toFixed(4)))} s` },
        { text: "Click again for B: the values between A and B come here.", help: true },
      ];
    }
    const between = section.to - section.from;
    return [
      { label: "A → B", text: `${duration(between)}${between > 0 ? `, ${frequency(between)}` : ""}` },
      { text: `${section.points.toLocaleString()} points`, number: true },
    ];
  });

  const atHeading = $derived(section?.both ? "At A → At B (change)" : "At A");

  /** A row's value at A; with B, its value there too and the change: "1.00 → 2.16" and "(+1.16)". */
  function atAandB(row: SectionRow, both: boolean): { values: string; change: string } {
    if (!both) return { values: fixed(row.a, row.decimals), change: "" };
    const change = row.a !== null && row.b !== null ? `(${signed(row.b - row.a, row.decimals)})` : "";
    return { values: `${fixed(row.a, row.decimals)} → ${fixed(row.b, row.decimals)}`, change };
  }

  function fftAt(index: number): number {
    return spectra.find(Boolean)?.freqs[index] ?? 0;
  }

  /** How many decimals a value has, up to 6: 21.5 has 1, 2048 none. */
  function decimalsIn(value: number): number {
    for (let places = 0; places < 6; places++) {
      const scaled = value * 10 ** places;
      if (Math.abs(scaled - Math.round(scaled)) < 1e-9 * Math.max(1, Math.abs(scaled))) return places;
    }
    return 6;
  }

  /**
   * A number with as many decimals as asked, up to 6, so the digits of a
   * column stay where they are as its values change. One too big or too
   * small for them is as precise as it is instead.
   */
  function fixed(value: number | null | undefined, places: number): string {
    if (value === null || value === undefined || !Number.isFinite(value)) return "";
    const decimals = Math.min(6, places);
    if (Math.abs(value) >= 1e15 || (value !== 0 && Math.abs(value) < 10 ** -decimals)) return number(value);
    const text = value.toFixed(decimals);
    // -0.00 is 0.00.
    return /^-0\.?0*$/.test(text) ? text.slice(1) : text;
  }

  /** A change, with its sign: +1.16 or −1.16. */
  function signed(value: number, places: number): string {
    const text = fixed(value, places);
    return text.startsWith("-") ? `−${text.slice(1)}` : `+${text}`;
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

  /** Puts a series on the right axis, with a scale of its own, or back on the left one. */
  function toggleRight(name: string) {
    const right = tab.plotView.right.includes(name)
      ? tab.plotView.right.filter((other) => other !== name)
      : [...tab.plotView.right, name];
    setPlotView({ right });
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

  /** A table under the chart, in the picture: its line, headings and rows, in the words and numbers on screen. */
  type PictureTable = { parts: RangePart[]; head: string[]; rows: { index: number; isX: boolean; cells: string[] }[] };

  /** The tables under the chart, with the rows of the series it draws: the ones not dimmed on screen. */
  function pictureTables(): PictureTable[] {
    const isX = (index: number) => kind === "xy" && index === xIndex();
    const drawn = (index: number) => (kind === "histogram" ? index === histIndex() : shown(index) || isX(index));
    const tables: PictureTable[] = [
      {
        parts: rangeParts,
        head: ["Series", ...tableColumns.map((column) => column.title)],
        rows: table
          .filter((row) => drawn(row.index))
          .map((row) => ({
            index: row.index,
            isX: isX(row.index),
            cells: [row.name, ...tableColumns.map((column) => column.cell(row))],
          })),
      },
    ];
    if (section) {
      const both = section.both;
      tables.push({
        parts: sectionParts.filter((part) => !part.help),
        head: ["Series", atHeading, ...STAT_COLUMNS.map((column) => column.title)],
        rows: section.rows
          .filter((row) => shown(row.index))
          .map((row) => {
            const at = atAandB(row, both);
            const values = at.change ? `${at.values} ${at.change}` : at.values;
            return { index: row.index, isX: false, cells: [row.name, values, ...STAT_COLUMNS.map((column) => column.cell(row))] };
          }),
      });
    }
    return tables.filter((one) => one.rows.length > 0);
  }

  /**
   * Saves the chart as it is on screen: the names of the series above it, and
   * the table's numbers under it, A and B's too, for the series it draws.
   */
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
    const ink = INK[scheme];
    const colors = COLORS[scheme];
    const source = chart.ctx.canvas;
    const ratio = window.devicePixelRatio || 1;
    const font = `${12 * ratio}px ${FONT_FAMILY}`;
    const bold = `600 ${font}`;
    const mono = `${12 * ratio}px ${MONO_FAMILY}`;
    const pad = 12 * ratio;
    const row = 20 * ratio;
    const swatch = 18 * ratio;
    // Room on each side of a table's cell, as on screen.
    const room = 10 * ratio;
    const out = document.createElement("canvas");
    let ctx = out.getContext("2d")!;

    // The tables' columns: as wide as their widest text, and the same in both tables, as on screen.
    const tables = pictureTables();
    const widths: number[] = [];
    const measure = (cells: string[], before: number) =>
      cells.forEach((text, column) => {
        widths[column] = Math.max(widths[column] ?? 0, (column === 0 ? before : 0) + ctx.measureText(text).width);
      });
    for (const one of tables) {
      ctx.font = `500 ${font}`;
      measure(one.head, 0);
      ctx.font = mono;
      for (const entry of one.rows) measure(entry.cells, swatch + 6 * ratio);
    }
    const needed = widths.reduce((sum, wide) => sum + wide + 2 * room, 0);
    // As wide as the chart, unless the table needs more. Its columns of numbers share what is left, as on screen.
    const width = Math.max(source.width, Math.ceil(needed + 2 * (pad - room)));
    const spare = Math.max(0, width - 2 * (pad - room) - needed) / Math.max(1, widths.length - 1);
    const columnWidths = widths.map((wide, column) => wide + 2 * room + (column > 0 ? spare : 0));

    /** Where items this wide go, in rows as wide as the picture, and the height they take. */
    const flow = (sizes: number[], gap: number) => {
      const places: { x: number; y: number }[] = [];
      let x = pad;
      let y = 0;
      for (const size of sizes) {
        if (x > pad && x + size > width - pad) {
          x = pad;
          y += row;
        }
        places.push({ x, y });
        x += size + gap;
      }
      return { places, height: sizes.length > 0 ? y + row : 0 };
    };

    // The legend, above the chart. XY's series are named against x.
    const against = kind === "xy" ? ` against ${names[xIndex()]}` : "";
    const shownNow = charted()
      .map((index, position) => ({ name: `${names[index]}${against}`, index, position }))
      .filter(({ position }) => chart!.series[position + 1].show);
    ctx.font = font;
    const named = flow(
      shownNow.map((item) => swatch + 6 * ratio + ctx.measureText(item.name).width),
      16 * ratio,
    );
    const legend = named.height > 0 ? pad + named.height : 0;
    // Each table's line, with what its stats are of.
    const captions = tables.map((one) =>
      flow(
        one.parts.map((part) => {
          ctx.font = bold;
          const label = part.label ? ctx.measureText(`${part.label} `).width : 0;
          ctx.font = font;
          return label + ctx.measureText(part.text).width;
        }),
        14 * ratio,
      ),
    );
    const under = tables.reduce((sum, one, position) => sum + captions[position].height + row * (1 + one.rows.length), 0);

    out.width = width;
    out.height = legend + source.height + under + (tables.length > 0 ? pad / 2 : 0);
    ctx = out.getContext("2d")!;
    ctx.fillStyle = ink.bg;
    ctx.fillRect(0, 0, out.width, out.height);
    ctx.textBaseline = "middle";

    /** A series' line, as in the chart, or x for XY's x. */
    const mark = (index: number, isX: boolean, x: number, middle: number) => {
      ctx.save();
      if (isX) {
        ctx.font = bold;
        ctx.fillStyle = ink.text;
        ctx.textAlign = "center";
        ctx.fillText("x", x + swatch / 2, middle);
      } else {
        ctx.strokeStyle = colors[index % colors.length];
        ctx.lineWidth = 2 * ratio;
        ctx.setLineDash((DASHES[Math.floor(index / colors.length)] ?? []).map((length) => length * ratio));
        ctx.beginPath();
        ctx.moveTo(x, middle);
        ctx.lineTo(x + swatch, middle);
        ctx.stroke();
      }
      ctx.restore();
    };

    ctx.font = font;
    shownNow.forEach((item, position) => {
      const { x, y } = named.places[position];
      const middle = pad + y + row / 2;
      mark(item.index, false, x, middle);
      ctx.fillStyle = ink.cursor;
      ctx.fillText(item.name, x + swatch + 6 * ratio, middle);
    });
    ctx.drawImage(source, 0, legend);

    /** A table's row: the name on the left, after its series' line, and the numbers on the right of their columns. */
    const cells = (texts: string[], middle: number, series?: { index: number; isX: boolean }) => {
      let x = pad - room;
      texts.forEach((text, column) => {
        if (column === 0) {
          let start = x + room;
          if (series) {
            mark(series.index, series.isX, start, middle);
            start += swatch + 6 * ratio;
          }
          ctx.textAlign = "left";
          ctx.fillText(text, start, middle);
        } else {
          ctx.textAlign = "right";
          ctx.fillText(text, x + columnWidths[column] - room, middle);
        }
        x += columnWidths[column];
      });
    };
    const rule = (y: number) => {
      ctx.fillStyle = ink.grid;
      ctx.fillRect(0, Math.round(y), width, Math.max(1, Math.round(ratio)));
    };
    let top = legend + source.height;
    tables.forEach((one, position) => {
      // As on screen: a rule between the line and the table, and over A and B's line.
      if (position > 0) rule(top);
      one.parts.forEach((part, index) => {
        const { x, y } = captions[position].places[index];
        const middle = top + y + row / 2;
        let at = x;
        ctx.textAlign = "left";
        if (part.label) {
          ctx.font = bold;
          ctx.fillStyle = ink.cursor;
          ctx.fillText(part.label, at, middle);
          at += ctx.measureText(`${part.label} `).width;
        }
        ctx.font = font;
        ctx.fillStyle = ink.text;
        ctx.fillText(part.text, at, middle);
      });
      top += captions[position].height;
      if (position === 0) rule(top);
      ctx.font = `500 ${font}`;
      ctx.fillStyle = ink.text;
      cells(one.head, top + row / 2);
      top += row;
      ctx.font = mono;
      for (const entry of one.rows) {
        ctx.fillStyle = ink.cursor;
        cells(entry.cells, top + row / 2, entry);
        top += row;
      }
    });
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
      cancelAnimationFrame(glideFrame);
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

<!--
  What the chart is and what to send for it, in the middle of the chart: on
  its own while there are no values, and over them when ? is on.
-->
{#snippet guide(closable: boolean)}
  {@const chartGuide = GUIDES[kind]}
  <div class="guide" role="note">
    {#if closable}
      <button class="close" title="Hide the help; ? shows it again" aria-label="Hide the help" onclick={() => setPlotView({ help: false })}>
        ×
      </button>
    {/if}
    {#if tab.plotView.mode === "modbus"}
      <h3>{chartGuide.title}, from Modbus</h3>
      <p>{chartGuide.what}</p>
      <p>
        Nothing to send: poll registers in <b>Modbus › Tester</b>, or listen to a bus where a master reads them. Each
        is named by its address: <code>hr0</code> holding register 0, <code>ir1</code> input register 1,
        <code>c3</code> coil 3, <code>u2.hr0</code> unit 2's.
      </p>
      {#if !closable}<p class="waiting">Nothing read yet.</p>{/if}
    {:else if tab.plotView.mode === "watch"}
      <h3>{chartGuide.title}, from the firmware's variables</h3>
      <p>{chartGuide.what}</p>
      <p>
        Nothing to print: add variables in the <b>Watch</b> view, by their names in the firmware's ELF file. They are
        read through the probe as the chip runs, each a series named after its variable.
      </p>
      {#if !closable}<p class="waiting">No values yet.</p>{/if}
    {:else if tab.plotView.mode === "regex"}
      <h3>{chartGuide.title}, with a regex</h3>
      <p>{chartGuide.what}</p>
      <p>
        For lines with words around the numbers, such as <code>temperature 21.5 C</code>: each group in brackets is a
        series, and <code>?&lt;name&gt;</code> names it.
      </p>
      <pre>temperature (?&lt;temp&gt;[\d.]+) C</pre>
      {#if draftProblem}
        <p class="problem">{draftProblem}</p>
      {:else if !closable}
        <p class="waiting">No line matches the regex yet.</p>
      {/if}
    {:else}
      <h3>{chartGuide.title}</h3>
      <p>{chartGuide.what}</p>
      <p>{chartGuide.send}</p>
      <pre>{chartGuide.code}</pre>
      <p>
        It prints <code>{chartGuide.line}</code>. Plain numbers such as <code>21.5,40.2</code> and JSON such as
        <code>{'{"temp": 21.5}'}</code> work too.
      </p>
      {#if !closable}<p class="waiting">No values yet.</p>{/if}
    {/if}
    {#if kind === "time" && trigger.on}
      <p><b>Trigger</b> holds the time around the last crossing of the level, the crossing in the middle, as a scope does.</p>
    {/if}
  </div>
{/snippet}

<!-- The line over a table: what each part is, in bold, then how much. The picture draws the same parts. -->
{#snippet rangeLine(parts: RangePart[])}
  {#each parts as part}
    <span class:number={part.number}>{#if part.label}<b>{part.label}</b>{" "}{/if}{part.text}</span>
  {/each}
{/snippet}

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
        {#if tab.canWatch}
          <option value="watch">Variables</option>
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
    <button
      class="btn small help-toggle"
      class:on={tab.plotView.help}
      aria-pressed={tab.plotView.help}
      title={tab.plotView.help ? "Hide the help" : "What this chart is, and what to send for it"}
      onclick={() => setPlotView({ help: !tab.plotView.help })}
    >
      ?
    </button>

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
      <label class="check" title="A dot on each sample, where there is room for it; off draws only the lines">
        <input type="checkbox" checked={tab.plotView.dots} onchange={(event) => setPlotView({ dots: event.currentTarget.checked })} />
        Dots
      </label>
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
      <label class="option" title="The series counted; the table below has every series' count and spread">
        Series
        <select class="field" value={names[histIndex()] ?? ""} onchange={(event) => setPlotView({ hist_series: event.currentTarget.value })}>
          {#each names as name (name)}
            <option value={name}>{name}</option>
          {/each}
        </select>
      </label>
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
    {#if names.length === 0 || tab.plotView.help}
      <div class="guide-box">
        {@render guide(names.length > 0)}
      </div>
    {:else if notice}
      <p class="notice">{notice}</p>
    {/if}
  </div>

  {#if names.length > 0}
    <div class="range">
      {@render rangeLine(rangeParts)}
      {#if tooMany}
        <span class="warning">Only the first {SERIES_MAX} series are drawn.</span>
      {/if}
      <span class="spacer"></span>
      <span class="hint">
        {kind === "time"
          ? "Click: cursors A and B · Drag or wheel: zoom · Right-drag or Shift+wheel: move · Double-click: follow"
          : "Drag or wheel: zoom · Right-drag: move · Double-click: all · A and B on the time chart pick the time"}
      </span>
    </div>
    <div class="table" class:with-section={section !== null}>
      <!-- On the time chart, the A and B table below has the same columns, so its values sit under these. -->
      <table class:aligned={kind === "time"}>
        {#if kind === "time"}<colgroup><col class="name-col" /><col class="value-col" /><col span="6" /></colgroup>{/if}
        <thead>
          <tr>
            <th class="name">Series</th>
            {#each tableColumns as column}
              <th title={column.tip}>{column.title}</th>
            {/each}
          </tr>
        </thead>
        <tbody>
          {#each table as row (row.name)}
            {@const shownNow = !tab.plotView.hidden.includes(row.name)}
            {@const color = COLORS[appScheme()][row.index % COLORS.light.length]}
            {@const dash = DASHES[Math.floor(row.index / COLORS.light.length)]}
            {@const isX = kind === "xy" && row.index === xIndex()}
            {@const counted = row.index === histIndex()}
            <tr class:off={kind === "histogram" ? !counted : !shownNow && !isX}>
              <td class="name">
                <div class="row-name">
                  <label title={isX ? "Along x" : kind === "histogram" ? "Count this series" : "Show or hide this series"}>
                    {#if kind === "histogram"}
                      <input type="radio" name="histogram-series" checked={counted} onchange={() => setPlotView({ hist_series: row.name })} />
                    {:else}
                      <input type="checkbox" checked={shownNow || isX} disabled={isX} onchange={() => toggleSeries(row.name, row.index)} />
                    {/if}
                    {#if isX}
                      <span class="axis">x</span>
                    {:else}
                      <svg width="18" height="8" aria-hidden="true">
                        <line x1="0" y1="4" x2="18" y2="4" stroke={color} stroke-width="2" stroke-dasharray={dash?.join(" ")} />
                      </svg>
                    {/if}
                    <span class="mono">{row.name}</span>
                  </label>
                  {#if kind === "time"}
                    {@const right = tab.plotView.right.includes(row.name)}
                    <button
                      class="side"
                      class:on={right}
                      aria-pressed={right}
                      title={right
                        ? "On the right axis, with a scale of its own: click to put it back on the left one"
                        : "Put it on a right axis, with a scale of its own, for values far from the others"}
                      onclick={() => toggleRight(row.name)}
                    >
                      R
                    </button>
                  {/if}
                </div>
              </td>
              {#each tableColumns as column}
                <td>{column.cell(row)}</td>
              {/each}
            </tr>
          {/each}
        </tbody>
      </table>

      {#if section}
        <!-- A and B, with all their values: a table of their own under the one of the time shown. -->
        <div class="range section">
          {@render rangeLine(sectionParts)}
        </div>
        <table class="aligned">
          <colgroup><col class="name-col" /><col class="value-col" /><col span="6" /></colgroup>
          <thead>
            <tr>
              <th class="name">Series</th>
              <th title="The value at A, at B, and the change from A to B">{atHeading}</th>
              {#each STAT_COLUMNS as column}
                <th title={column.tip}>{column.title}</th>
              {/each}
            </tr>
          </thead>
          <tbody>
            {#each section.rows as row (row.name)}
              {@const color = COLORS[appScheme()][row.index % COLORS.light.length]}
              {@const dash = DASHES[Math.floor(row.index / COLORS.light.length)]}
              {@const at = atAandB(row, section.both)}
              <tr class:off={tab.plotView.hidden.includes(row.name)}>
                <td class="name">
                  <span class="series">
                    <svg width="18" height="8" aria-hidden="true">
                      <line x1="0" y1="4" x2="18" y2="4" stroke={color} stroke-width="2" stroke-dasharray={dash?.join(" ")} />
                    </svg>
                    <span class="mono">{row.name}</span>
                  </span>
                </td>
                <td>{at.values}{#if at.change} <span class="change">{at.change}</span>{/if}</td>
                {#each STAT_COLUMNS as column}
                  <td>{column.cell(row)}</td>
                {/each}
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
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

  .help-toggle {
    width: 24px;
    padding: 0;
    font-weight: 600;
  }

  .help-toggle.on {
    color: var(--accent);
    border-color: var(--accent);
  }

  /* A right-drag holds the chart, as a hand would. */
  .chart :global(.u-over.grabbing) {
    cursor: grabbing;
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

  /* What the chart is and what to send for it, in the middle of the chart. */
  .guide-box {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 12px 24px;
    overflow: auto;
    pointer-events: none;
  }

  .guide {
    position: relative;
    max-width: 560px;
    padding: 14px 20px;
    font-size: 13px;
    line-height: 1.55;
    color: var(--muted);
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: var(--shadow);
    pointer-events: auto;
  }

  .guide h3 {
    margin: 0 0 2px;
    font-size: 14px;
    font-weight: 600;
    color: var(--text);
  }

  .guide p {
    margin: 4px 0 0;
  }

  .guide pre {
    margin: 6px 0 0;
    padding: 6px 10px;
    font: 12px/1.5 var(--font-mono);
    color: var(--text);
    white-space: pre-wrap;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  .guide code {
    font-family: var(--font-mono);
    color: var(--text);
    white-space: nowrap;
  }

  .guide b {
    font-weight: 600;
    color: var(--text);
  }

  .guide .waiting {
    margin-top: 10px;
    color: var(--accent);
  }

  .guide .problem {
    margin-top: 10px;
    color: var(--bad);
  }

  .guide .close {
    position: absolute;
    top: 6px;
    right: 6px;
    width: 22px;
    height: 22px;
    font: inherit;
    font-size: 14px;
    color: var(--muted);
    background: none;
    border: 0;
    border-radius: 4px;
    cursor: default;
  }

  .guide .close:hover {
    color: var(--text);
    background: var(--hover);
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

  /* With the A and B table under it, both get more room. */
  .table.with-section {
    max-height: 50%;
  }

  /* The columns keep their widths as the values change, so the digits stay still. */
  table {
    width: 100%;
    font-size: 12px;
    table-layout: fixed;
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
    overflow: hidden;
    font-family: var(--font-mono);
    color: var(--text);
    text-align: right;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  th.name,
  td.name {
    width: 30%;
    text-align: left;
  }

  td.name label,
  td.name .series {
    display: flex;
    gap: 6px;
    align-items: center;
  }

  /* The time chart's two tables share their columns: A and B's values sit under the ones of the time shown. */
  table.aligned th.name,
  table.aligned td.name {
    width: auto;
  }

  table.aligned .name-col {
    width: 16%;
  }

  table.aligned .value-col {
    width: 24%;
  }

  .change {
    margin-left: 4px;
    color: var(--muted);
  }

  /* The A and B table's heading, under the table of the time shown. */
  .range.section {
    padding: 8px 10px 4px;
    border-top: 1px solid var(--border);
  }

  .row-name {
    display: flex;
    gap: 6px;
    align-items: center;
  }

  /* R: the series on a right axis of its own. */
  .side {
    height: 16px;
    padding: 0 4px;
    font: inherit;
    font-size: 10px;
    font-weight: 600;
    line-height: 14px;
    color: var(--muted);
    background: none;
    border: 1px solid var(--border);
    border-radius: 3px;
    cursor: pointer;
    opacity: 0.6;
  }

  .side:hover {
    opacity: 1;
  }

  .side.on {
    color: var(--accent);
    border-color: var(--accent);
    opacity: 1;
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
