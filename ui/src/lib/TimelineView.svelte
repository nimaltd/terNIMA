<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { open, save } from "@tauri-apps/plugin-dialog";
  import type { Sample } from "./modbus.svelte";
  import { fileName, type Tab } from "./tabs.svelte";
  import { HALTED, exceptionName, readTimeline, sampleAt, timelineText, type Timeline } from "./timeline";
  import { valueText } from "./watch";

  // The timeline at the top of the Live view: a lane for each task, where it
  // ran, sampled or traced; one for each interrupt traced; and one for each
  // watched variable, over the same time. Its times are in ms after the
  // timeline's origin, which keeps a trace's nanoseconds.

  let { tab }: { tab: Tab } = $props();

  /** The parts' sizes, in CSS pixels. */
  const LANE = 18;
  const VARIABLE_LANE = 34;
  /** The column of names at the left. */
  const NAMES = 160;
  /** The time axis at the bottom. */
  const AXIS = 20;
  /** The room between the tasks, the interrupts and the variables, with a line across it. */
  const GAP = 11;
  /** The narrowest time shown, a microsecond, and the widest. */
  const SPAN_MIN = 0.001;
  const SPAN_MAX = 24 * 3600 * 1000;
  /** How far a turn of the wheel zooms: a notch about one and a half times. */
  const ZOOM = 0.004;
  /** The narrowest the scrollbar's thumb gets, in pixels. */
  const THUMB_MIN = 24;
  /** Lengths of time, each with the unit it is told in. */
  const UNITS: [number, string][] = [
    [1e-6, "ns"],
    [1e-3, "µs"],
    [1, "ms"],
    [1000, "s"],
    [60_000, "min"],
    [3_600_000, "h"],
  ];
  /** How far apart the axis's labels are, at least. */
  const LABEL_APART = 120;
  /**
   * How far behind the clock the drawing runs while samples come: they come
   * every 100 ms, and it moves on smoothly between them.
   */
  const LAG = 250;
  /** Colours for the tasks, told apart in both themes; the idle task is grey. */
  const COLORS = ["#2f8fd8", "#e8833a", "#3aa35b", "#d8504b", "#9467bd", "#c2963d", "#d665b5", "#17a9b8", "#8c9a2c"];

  let canvas: HTMLCanvasElement;
  let area: HTMLDivElement;
  let width = $state(0);
  /** Whether the right edge follows the newest sample. */
  let follow = $state(true);
  /** How much time is shown, in ms. */
  let span = $state(2000);
  /** The time at the right edge, while not following. */
  let end = $state(0);
  /** Where the pointer is over the drawing. */
  let pointer: { x: number; y: number } | null = $state(null);
  let dragging: { x: number; end: number } | null = null;
  let problem: string | null = $state(null);
  /** Changed by the theme, to draw again in its colours. */
  let theme = $state(0);
  /** The scrollbar's thumb as last drawn: where it is and how wide, in pixels. */
  let thumb = $state({ left: 0, size: 0, place: 100 });
  /** The thumb as it was taken hold of, while it is dragged: and how much time a pixel moves. */
  let sliding: { x: number; right: number; per: number } | null = null;

  /** What is shown: a file opened, or what this tab recorded. */
  function shown(): { timeline: Timeline; samples: Sample[]; series: string[] } {
    const opened = tab.openedTimeline;
    if (opened) return opened;
    return { timeline: tab.timeline, samples: tab.watchSamples, series: tab.watchSeriesNames };
  }

  /** Now, in ms after a timeline's origin. */
  function nowIn(timeline: Timeline): number {
    return Date.now() - timeline.origin;
  }

  /**
   * The newest time there is anything for: what ran, as the variables are
   * read on after a pause; or else the variables.
   */
  function newest(): number {
    const { timeline, samples } = shown();
    if (timeline.times.length > 0) return timeline.until;
    const last = samples.at(-1);
    return last ? last.at - timeline.origin : nowIn(timeline);
  }

  /** Whether samples are coming now, for the drawing to move on with the clock: not while paused. */
  function live(): boolean {
    return (
      !tab.openedTimeline &&
      !tab.timelinePaused &&
      tab.timelineRecording &&
      tab.connected &&
      nowIn(tab.timeline) - tab.timeline.until < 2000
    );
  }

  /** The time at the right edge, following the newest. */
  function latest(): number {
    return live() ? nowIn(tab.timeline) - LAG : newest();
  }

  /** The time at the right edge now. */
  function rightEdge(): number {
    return follow ? latest() : end;
  }

  /** The oldest time there is anything for. */
  function oldest(): number {
    const { timeline, samples } = shown();
    const times: number[] = [];
    if (timeline.since !== null) times.push(timeline.since);
    if (samples.length > 0) times.push(samples[0].at - timeline.origin);
    return times.length > 0 ? Math.min(...times) : newest();
  }

  /**
   * Shows the time up to `to` at the right edge, kept to what there is:
   * following the newest once it gets there, but while paused, where the
   * view holds still as more comes.
   */
  function moveTo(to: number) {
    const newestEnd = latest();
    const kept = Math.min(newestEnd, Math.max(Math.min(oldest() + span, newestEnd), to));
    if (kept >= newestEnd && !tab.timelinePaused) {
      follow = true;
    } else {
      follow = false;
      end = kept;
    }
  }

  /** Holds the view still where it is, as recording goes on; or shows it live again. */
  function togglePause() {
    if (tab.timelinePaused) {
      follow = true;
      tab.setTimelinePaused(false);
    } else {
      end = rightEdge();
      follow = false;
      tab.setTimelinePaused(true);
    }
  }

  const opened = $derived(tab.openedTimeline);
  /** The lanes there are: the tasks', the interrupts' and the variables', each group shown or not. */
  const lanes = $derived.by(() => {
    void tab.timelineVersion;
    void tab.watchVersion;
    const { timeline, series } = shown();
    return {
      tasks: timeline.order.length,
      interrupts: timeline.interrupts.size,
      variables: series.length,
      showInterrupts: tab.watch.timeline_hide_interrupts !== true,
      showVariables: tab.watch.timeline_hide_variables !== true,
    };
  });
  /** The groups of lanes drawn, top to bottom, with their heights. */
  const groups = $derived(
    (
      [
        ["tasks", lanes.tasks * LANE],
        ["interrupts", lanes.showInterrupts ? lanes.interrupts * LANE : 0],
        ["variables", lanes.showVariables ? lanes.variables * VARIABLE_LANE : 0],
      ] as const
    ).filter(([, size]) => size > 0),
  );
  const height = $derived(
    groups.reduce((sum, [, size]) => sum + size, 0) + Math.max(0, groups.length - 1) * GAP + AXIS,
  );
  /** Whether this tab can trace the interrupts, or what is shown has them. */
  const hasInterrupts = $derived(tab.kind === "swo" || lanes.interrupts > 0);
  const status = $derived.by(() => {
    void tab.timelineVersion;
    if (opened) return `${opened.path ? fileName(opened.path) : "A file"}, as saved`;
    if (!tab.timelineRecording) return "Not recording";
    if (tab.timelinePaused) {
      if (tab.chipHalted) return "Paused: the chip is halted";
      return tab.connected ? "Paused, still recording" : "Paused";
    }
    if (!tab.connected) return "Recording once connected";
    if (tab.timeline.exact) return "Recording every switch, traced over SWO";
    const rate = Math.round(tab.timeline.rate);
    return rate > 0 ? `Recording: ${rate.toLocaleString()} samples a second` : "Recording";
  });
  const pauseTitle = $derived.by(() => {
    if (tab.timelinePaused) {
      return tab.canHalt ? "Run the chip again, and show it live" : "Show it live again, with what was recorded meanwhile";
    }
    return tab.canHalt
      ? "Halt the chip, and hold the view still to look at it, the tasks and values too"
      : "Hold the view still to look at it, the tasks and values too: recording goes on";
  });
  /** Whether the chip dropped some of the trace lately: the timeline has gaps there. */
  const dropped = $derived.by(() => {
    void tab.timelineVersion;
    return !opened && tab.timelineRecording && tab.kind === "swo" && Date.now() - tab.swoDroppedAt < 3000;
  });
  const statusTitle = $derived(
    tab.kind === "swo"
      ? "The chip sends each task switch and each interrupt over SWO, timed to the clock cycle"
      : "Sampled through the probe: a task that runs for less than the time between two samples can go unseen. " +
          "An SWO tab traces every switch, and the interrupts.",
  );

  // Draws again when there is more, or the view of it changed.
  let frame = 0;
  $effect(() => {
    void [tab.timelineVersion, tab.watchVersion, opened, follow, span, end, pointer, width, height, theme];
    if (!frame) frame = requestAnimationFrame(draw);
  });

  onMount(() => {
    // The page's colours change with the theme.
    const observer = new MutationObserver(() => theme++);
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "class", "style"] });
    const scheme = matchMedia("(prefers-color-scheme: dark)");
    const changed = () => theme++;
    scheme.addEventListener("change", changed);
    // A wheel turned over the drawing zooms it, and must not scroll the page.
    canvas.addEventListener("wheel", onWheel, { passive: false });
    return () => {
      observer.disconnect();
      scheme.removeEventListener("change", changed);
      canvas.removeEventListener("wheel", onWheel);
      cancelAnimationFrame(frame);
    };
  });

  function color(name: string): string {
    return getComputedStyle(area).getPropertyValue(name).trim() || "#888";
  }

  function taskColor(timeline: Timeline, task: number, index: number): string {
    return /^idle/i.test(timeline.name(task)) ? color("--muted") : COLORS[index % COLORS.length];
  }

  /** A step between the axis's labels: 1, 2 or 5 times a power of ten, in ms, down to a nanosecond. */
  function stepFor(ms: number): number {
    const power = 10 ** Math.floor(Math.log10(Math.max(ms, 1e-6)));
    for (const times of [1, 2, 5, 10]) {
      if (power * times >= ms) return power * times;
    }
    return power * 10;
  }

  /**
   * A time, in ms after `origin`, as the clock had it, with as many decimals
   * of a second as the step needs, down to the nanosecond: 18:14:11.234 567.
   */
  function clockText(origin: number, at: number, step: number): string {
    const decimals = Math.max(0, Math.min(9, Math.ceil(-Math.log10(step / 1000) - 1e-9)));
    // Whole ms since 1970, and the nanoseconds after: each exact.
    let whole = Math.floor(at);
    let nanos = Math.round((at - whole) * 1e6);
    if (nanos >= 1e6) {
      whole += 1;
      nanos -= 1e6;
    }
    const date = new Date(origin + whole);
    const pad = (value: number, size = 2) => String(value).padStart(size, "0");
    const text = `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
    if (decimals === 0) return text;
    const fraction = `${pad(date.getMilliseconds(), 3)}${pad(nanos, 6)}`.slice(0, decimals);
    // In threes after the point, as 234 567 891.
    return `${text}.${fraction.replace(/(\d{3})(?=\d)/g, "$1\u2009")}`;
  }

  /** A length of time, short: 480 ns, 5.92 µs, 250 ms, 2 s, 1.5 min; with `digits` significant digits. */
  function lengthText(ms: number, digits = 3): string {
    const size = Math.abs(ms);
    let unit = UNITS.length - 1;
    while (unit > 0 && size < UNITS[unit][0]) unit--;
    let value = Number((ms / UNITS[unit][0]).toPrecision(digits));
    // Rounded up into the next unit: 1000 µs is 1 ms.
    if (unit + 1 < UNITS.length && Math.abs(value) * UNITS[unit][0] >= UNITS[unit + 1][0]) {
      unit++;
      value = Number((ms / UNITS[unit][0]).toPrecision(digits));
    }
    return `${value} ${UNITS[unit][1]}`;
  }

  /** Each variable's value at a time: its last sample then. */
  function valuesAt(samples: Sample[], series: string[], at: number): (number | null)[] {
    const values: (number | null)[] = series.map(() => null);
    let left = series.length;
    for (let index = sampleAt(samples, at + 0.0001) - 1; index >= 0 && left > 0; index--) {
      for (const [name, value] of samples[index].values) {
        const lane = series.indexOf(name);
        if (lane >= 0 && values[lane] === null) {
          values[lane] = value;
          left--;
        }
      }
      // Far enough back.
      if (at - samples[index].at > 60_000) break;
    }
    return values;
  }

  function draw() {
    frame = 0;
    if (!canvas || width <= NAMES) return;
    // Following samples as they come, it is drawn again at every frame.
    if (follow && live()) frame = requestAnimationFrame(draw);
    const { timeline, samples, series } = shown();
    const ratio = window.devicePixelRatio || 1;
    canvas.width = Math.round(width * ratio);
    canvas.height = Math.round(height * ratio);
    const g = canvas.getContext("2d");
    if (!g) return;
    g.setTransform(ratio, 0, 0, ratio, 0, 0);
    g.clearRect(0, 0, width, height);

    const text = color("--text");
    const muted = color("--muted");
    const border = color("--border");
    const right = rightEdge();
    const left = right - span;
    const plot = width - NAMES;
    // The samples are timed in ms since 1970, the timeline after its origin.
    const origin = timeline.origin;
    const x = (at: number) => NAMES + ((at - left) / span) * plot;
    g.font = `12px ${color("--font-mono")}`;
    g.textBaseline = "middle";

    // Where each group of lanes starts, with a line across the room between two.
    const tops: Record<string, number> = {};
    let y = 0;
    for (const [group, size] of groups) {
      if (y > 0) {
        g.fillStyle = muted;
        g.globalAlpha = 0.55;
        g.fillRect(0, y + Math.floor(GAP / 2), width, 1);
        g.globalAlpha = 1;
        y += GAP;
      }
      tops[group] = y;
      y += size;
    }

    // The tasks' lanes: where each ran.
    const order = timeline.order;
    const lane = new Map(order.map((task, index) => [task, index]));
    const at = pointer && pointer.x > NAMES ? left + ((pointer.x - NAMES) / plot) * span : null;
    const running = at === null ? null : timeline.taskAt(at);
    order.forEach((task, index) => {
      const y = (tops.tasks ?? 0) + index * LANE;
      g.fillStyle = task === running ? text : muted;
      g.fillText(fit(g, timeline.name(task), NAMES - 16), 8, y + LANE / 2);
      g.fillStyle = border;
      g.fillRect(NAMES, y + LANE - 1, plot, 1);
    });
    // Runs shorter than a pixel still show, one pixel wide, without drawing a pixel twice.
    const drawnTo = order.map(() => -Infinity);
    timeline.runs(left, right, (task, start, stop) => {
      const index = lane.get(task);
      if (index === undefined) return;
      let from = Math.max(NAMES, x(start));
      const to = Math.max(from + 1, Math.min(width, x(stop)));
      if (to <= drawnTo[index]) return;
      from = Math.max(from, drawnTo[index]);
      g.fillStyle = taskColor(timeline, task, index);
      g.fillRect(from, (tops.tasks ?? 0) + index * LANE + 3, to - from, LANE - 7);
      drawnTo[index] = to;
    });

    // The interrupts' lanes: each exception from when it was entered to when it was left.
    const interruptOrder = timeline.interruptOrder;
    if (tops.interrupts !== undefined) {
      interruptOrder.forEach((number, index) => {
        const y = tops.interrupts + index * LANE;
        let active = false;
        if (at !== null) timeline.interruptRuns(number, at, at, () => (active = true));
        g.fillStyle = active ? text : muted;
        g.fillText(fit(g, timeline.interruptName(number), NAMES - 16), 8, y + LANE / 2);
        g.fillStyle = border;
        g.fillRect(NAMES, y + LANE - 1, plot, 1);
        g.fillStyle = color("--bad");
        let drawn = -Infinity;
        timeline.interruptRuns(number, left, right, (start, stop) => {
          let from = Math.max(NAMES, x(start));
          const to = Math.max(from + 1, Math.min(width, x(stop)));
          if (to <= drawn) return;
          from = Math.max(from, drawn);
          g.fillRect(from, y + 3, to - from, LANE - 7);
          drawn = to;
        });
      });
    }

    // The variables' lanes: each drawn between its least and most in sight.
    const top = tops.variables ?? 0;
    const now = valuesAt(samples, series, origin + (at ?? right));
    const first = Math.max(0, sampleAt(samples, origin + left) - 1);
    const last = Math.min(samples.length, sampleAt(samples, origin + right) + 1);
    (tops.variables === undefined ? [] : series).forEach((name, index) => {
      const y = top + index * VARIABLE_LANE;
      let least = Infinity;
      let most = -Infinity;
      for (let at = first; at < last; at++) {
        for (const [series, value] of samples[at].values) {
          if (series === name) {
            least = Math.min(least, value);
            most = Math.max(most, value);
          }
        }
      }
      g.fillStyle = muted;
      g.fillText(fit(g, name, NAMES - 16), 8, y + 10);
      g.fillStyle = text;
      g.fillText(fit(g, valueText(now[index], undefined).trim(), NAMES - 16), 8, y + 24);
      g.fillStyle = border;
      g.fillRect(NAMES, y + VARIABLE_LANE - 1, plot, 1);
      if (least > most) return;
      const spread = most - least || 1;
      const yOf = (value: number) => y + VARIABLE_LANE - 5 - ((value - least) / spread) * (VARIABLE_LANE - 10);
      g.strokeStyle = COLORS[(index + order.length) % COLORS.length];
      g.lineWidth = 1.5;
      g.beginPath();
      let started = false;
      for (let at = first; at < last; at++) {
        const sample = samples[at];
        const pair = sample.values.find(([series]) => series === name);
        if (!pair) continue;
        const px = Math.max(NAMES, Math.min(width, x(sample.at - origin)));
        if (started) g.lineTo(px, yOf(pair[1]));
        else g.moveTo(px, yOf(pair[1]));
        started = true;
      }
      g.stroke();
    });

    // Where the chip was halted, by Pause: across every lane, nothing ran.
    timeline.halts(left, right, (start, stop) => {
      const from = Math.max(NAMES, x(start));
      const to = Math.max(from + 1, Math.min(width, x(stop)));
      g.fillStyle = muted;
      g.globalAlpha = 0.16;
      g.fillRect(from, 0, to - from, height - AXIS);
      g.globalAlpha = 1;
      if (to - from > 64) g.fillText("Halted", from + 8, 9);
    });

    // The axis, in the clock's time. Under a millisecond a step, the labels
    // after the first tell how long after it, as +2 µs: the clock's own
    // would be long, and alike.
    const axis = height - AXIS;
    g.fillStyle = border;
    g.fillRect(NAMES, axis, plot, 1);
    const step = stepFor((span * LABEL_APART) / plot);
    // On the clock's whole steps: the origin is a whole ms, so steps under one count from it.
    const firstTick = step >= 1 ? Math.ceil((origin + left) / step) * step - origin : Math.ceil(left / step) * step;
    g.textBaseline = "top";
    let told: number | null = null;
    let toldTo = -Infinity;
    for (let index = 0; ; index++) {
      const tick = firstTick + index * step;
      if (tick > right) break;
      const px = x(tick);
      g.fillStyle = border;
      g.fillRect(px, 0, 1, axis);
      const label = step >= 1 || told === null ? clockText(origin, tick, step) : `+${lengthText(tick - told)}`;
      const half = g.measureText(label).width / 2;
      if (px - half < NAMES || px + half > width || px - half < toldTo + 12) continue;
      told ??= tick;
      toldTo = px + half;
      // The pointer's own time stands where it is, over the axis's.
      const underPointer = pointer !== null && at !== null && Math.abs(px - pointer.x) < half * 2 + 16;
      if (!underPointer) {
        g.fillStyle = muted;
        g.fillText(label, px - half, axis + 4);
      }
    }

    // Where the pointer is, and when that was; with the run under it, and how long that lasted.
    if (at !== null && pointer) {
      g.fillStyle = text;
      g.fillRect(pointer.x, 0, 1, axis);
      let label = clockText(origin, at, stepFor(span / plot) / 10);
      const run = runUnder(timeline, tops, pointer.y, at);
      if (run) label += `  ${run.name} ${lengthText(run.end - run.start, 4)}`;
      const size = g.measureText(label).width;
      const lx = Math.min(pointer.x + 6, width - size - 4);
      g.fillStyle = color("--panel");
      g.fillRect(lx - 3, axis + 2, size + 6, AXIS - 3);
      g.fillStyle = text;
      g.fillText(label, lx, axis + 4);
    }

    // The scrollbar: where what is shown is, in all there is.
    const start = Math.min(left, oldest());
    const all = Math.max(span, (follow ? right : Math.max(right, latest())) - start);
    const size = Math.min(plot, Math.max(THUMB_MIN, (span / all) * plot));
    const place = all > span ? Math.min(1, Math.max(0, (left - start) / (all - span))) : 1;
    const moved = { left: Math.round(place * (plot - size)), size: Math.round(size), place: Math.round(place * 100) };
    if (moved.left !== thumb.left || moved.size !== thumb.size || moved.place !== thumb.place) thumb = moved;
  }

  type Run = { name: string; start: number; end: number };

  /** The run in the lane at `y`, at a time: a task's or an interrupt's, named, from when to when; or the halt. */
  function runUnder(timeline: Timeline, tops: Record<string, number>, y: number, at: number): Run | null {
    const index = timeline.indexAt(at);
    if (index >= 0 && at <= timeline.until && timeline.tasks[index] === HALTED) {
      const stop = index + 1 < timeline.times.length ? timeline.times[index + 1] : timeline.until;
      return { name: "Halted", start: timeline.times[index], end: stop };
    }
    const lane = (top: number | undefined, count: number) =>
      top !== undefined && y >= top && y < top + count * LANE ? Math.floor((y - top) / LANE) : -1;
    const order = timeline.order;
    const task = order[lane(tops.tasks, order.length)];
    if (task !== undefined) {
      const index = timeline.indexAt(at);
      if (index < 0 || timeline.tasks[index] !== task || at > timeline.until) return null;
      const stop = index + 1 < timeline.times.length ? timeline.times[index + 1] : timeline.until;
      return { name: timeline.name(task), start: timeline.times[index], end: stop };
    }
    const interrupts = timeline.interruptOrder;
    const number = interrupts[lane(tops.interrupts, interrupts.length)];
    if (number === undefined) return null;
    const named = timeline.interruptName(number);
    const plain = exceptionName(number);
    const name = named === plain ? named : `${named} (${plain})`;
    let run: Run | null = null;
    timeline.interruptRuns(number, at, at, (start, stop) => (run = { name, start, end: stop }));
    return run as Run | null;
  }

  /** Text cut to a width, with an ellipsis where it was cut. */
  function fit(g: CanvasRenderingContext2D, text: string, room: number): string {
    if (g.measureText(text).width <= room) return text;
    let cut = text;
    while (cut.length > 1 && g.measureText(`${cut}…`).width > room) cut = cut.slice(0, -1);
    return `${cut}…`;
  }

  /** The wheel zooms; turned sideways, or with Shift, it moves along the time. */
  function onWheel(event: WheelEvent) {
    event.preventDefault();
    const plot = width - NAMES;
    const sideways = event.shiftKey
      ? event.deltaY || event.deltaX
      : Math.abs(event.deltaX) > Math.abs(event.deltaY)
        ? event.deltaX
        : 0;
    if (sideways !== 0) {
      moveTo(rightEdge() + (sideways / plot) * span);
      return;
    }
    const factor = Math.exp(event.deltaY * ZOOM);
    const longest = Math.max(10_000, newest() - oldest());
    const next = Math.min(Math.min(SPAN_MAX, longest * 1.1), Math.max(SPAN_MIN, span * factor));
    const offset = event.offsetX - NAMES;
    // While it follows what comes, the newest stays at the right edge; else
    // the time under the pointer stays under it.
    if (follow && live()) {
      span = next;
    } else if (offset > 0) {
      const at = rightEdge() - span + (offset / plot) * span;
      span = next;
      moveTo(at + ((plot - offset) / plot) * next);
    } else {
      span = next;
    }
  }

  function onDown(event: MouseEvent) {
    if (event.button !== 0 || event.offsetX < NAMES) return;
    dragging = { x: event.clientX, end: rightEdge() };
  }

  function onMove(event: MouseEvent) {
    pointer = { x: event.offsetX, y: event.offsetY };
    if (!dragging) return;
    const moved = event.clientX - dragging.x;
    if (Math.abs(moved) < 2 && follow) return;
    moveTo(dragging.end - (moved / (width - NAMES)) * span);
  }

  /** The scrollbar's thumb, taken hold of to move along all there is. */
  function onThumbDown(event: PointerEvent) {
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    const plot = width - NAMES;
    const right = rightEdge();
    const all = Math.max(span, (follow ? right : Math.max(right, latest())) - Math.min(right - span, oldest()));
    const per = plot > thumb.size ? (all - span) / (plot - thumb.size) : 0;
    sliding = { x: event.clientX, right, per };
  }

  function onThumbMove(event: PointerEvent) {
    if (sliding) moveTo(sliding.right + (event.clientX - sliding.x) * sliding.per);
  }

  /** A click on the scrollbar beside its thumb moves a page that way. */
  function onTrackDown(event: PointerEvent) {
    if (event.button !== 0 || event.target !== event.currentTarget) return;
    moveTo(rightEdge() + (event.offsetX < thumb.left ? -0.9 : 0.9) * span);
  }

  function onUp() {
    dragging = null;
  }

  /** Shows everything there is. */
  function showAll() {
    const all = newest() - oldest();
    span = Math.max(SPAN_MIN, Math.min(SPAN_MAX, all * 1.02 || 2000));
    end = newest();
    follow = !opened && !tab.timelinePaused;
  }

  async function saveFile() {
    problem = null;
    const name = opened?.name || tab.name;
    const path = await save({
      title: "Save the timeline",
      defaultPath: `${name.replace(/[<>:"/\\|?*]/g, "_")} timeline.json`,
      filters: [{ name: "terNIMA timeline", extensions: ["json"] }],
    });
    if (!path) return;
    const { timeline, samples, series } = shown();
    try {
      await invoke("save_text", { path, text: timelineText(name, timeline, samples, series) });
    } catch (error) {
      problem = String(error);
    }
  }

  async function openFile() {
    problem = null;
    const path = await open({
      title: "Open a timeline",
      multiple: false,
      directory: false,
      filters: [
        { name: "terNIMA timeline", extensions: ["json"] },
        { name: "All files", extensions: ["*"] },
      ],
    });
    if (typeof path !== "string") return;
    try {
      const read = readTimeline(await invoke<string>("open_text", { path }));
      tab.openedTimeline = { path, ...read };
      showAll();
    } catch (error) {
      problem = error instanceof Error ? error.message : String(error);
    }
  }

  function backToLive() {
    tab.openedTimeline = null;
    span = 2000;
    end = newest();
    follow = !tab.timelinePaused;
  }
</script>

<section class="timeline" aria-label="Timeline">
  <div class="bar">
    <b>Timeline</b>
    <span class="muted" class:recording={!opened && tab.timelineRecording && tab.connected} title={statusTitle}>{status}</span>
    <span class="muted" title="How much time the drawing shows: turn the wheel over it to zoom, or with Shift to move along">{lengthText(span)} across</span>
    {#if dropped}
      <span
        class="warning"
        title="The chip had more to send than SWO carries, and dropped some: when, and what ran then, is not known, so the timeline leaves a gap"
        >Gaps where SWO had too much{lanes.showInterrupts && lanes.interrupts > 0 ? ": Interrupts off leaves it room" : ""}</span
      >
    {/if}
    <span class="grow"></span>
    {#if lanes.variables > 0}
      <button
        class="btn small toggle"
        aria-pressed={lanes.showVariables}
        title="Show the watched variables under the tasks, or leave them out"
        onclick={() => tab.setTimelineShows("variables", !lanes.showVariables)}>Variables</button
      >
    {/if}
    {#if hasInterrupts}
      <button
        class="btn small toggle"
        aria-pressed={lanes.showInterrupts}
        title="Show each interrupt, traced over SWO, or leave them out, which leaves SWO more room"
        onclick={() => tab.setTimelineShows("interrupts", !lanes.showInterrupts)}>Interrupts</button
      >
    {/if}
    {#if opened}
      <button class="btn small" onclick={backToLive}>Back to this tab's</button>
    {:else}
      <!-- One place and one width, Pause or Resume. -->
      <button class="btn small pause" title={pauseTitle} onclick={togglePause}>
        {tab.timelinePaused ? "Resume" : "Pause"}
      </button>
    {/if}
    <!-- Always there, so the buttons stay where they are. -->
    <button
      class="btn small"
      disabled={follow && !(tab.timelinePaused && !opened)}
      title={tab.timelinePaused && !opened ? "Go to the newest, held still" : "Follow the newest again"}
      onclick={() => (tab.timelinePaused && !opened ? (end = newest()) : (follow = true))}>Newest</button
    >
    <button class="btn small" title="Show all there is" onclick={showAll}>All</button>
    {#if !opened}
      <button class="btn small" title="Forget what was recorded" onclick={() => tab.clearTimeline()}>Clear</button>
    {/if}
    <button class="btn small" title="Save it to a file, to open later" onclick={saveFile}>Save...</button>
    <button class="btn small" title="Open a timeline saved before" onclick={openFile}>Open...</button>
  </div>
  {#if problem}
    <p class="problem">{problem}</p>
  {/if}
  <div class="area" bind:this={area} bind:clientWidth={width} style:height="{height}px">
    <canvas
      bind:this={canvas}
      id="timeline-{tab.id}"
      style:width="{width}px"
      style:height="{height}px"
      aria-label="Which task ran when, sampled through the probe, and the watched variables over the same time"
      onmousedown={onDown}
      onmousemove={onMove}
      onmouseup={onUp}
      onmouseleave={() => {
        pointer = null;
        dragging = null;
      }}
      ondblclick={showAll}
    ></canvas>
  </div>
  {#if groups.length > 0}
    <div
      class="scroll"
      style:margin-left="{NAMES}px"
      role="scrollbar"
      tabindex="-1"
      aria-controls="timeline-{tab.id}"
      aria-orientation="horizontal"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={thumb.place}
      title="Drag to move along all there is; Shift and the wheel do too"
      onpointerdown={onTrackDown}
    >
      <div
        class="thumb"
        role="presentation"
        style:left="{thumb.left}px"
        style:width="{thumb.size}px"
        onpointerdown={onThumbDown}
        onpointermove={onThumbMove}
        onpointerup={() => (sliding = null)}
        onpointercancel={() => (sliding = null)}
      ></div>
    </div>
  {/if}
  {#if groups.length === 0}
    <p class="note">
      {tab.rtos
        ? "Which task runs shows here once sampled."
        : "Add variables below, or choose an ELF file with Zephyr or FreeRTOS, to see them over time."}
    </p>
  {/if}
</section>

<style>
  .timeline {
    margin-bottom: 12px;
    border-bottom: 1px solid var(--border);
  }

  .bar {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 10px;
    align-items: center;
    padding: 2px 8px 6px;
    font-size: 12px;
  }

  .bar b {
    font-weight: 600;
  }

  .muted {
    color: var(--muted);
  }

  .warning {
    color: var(--warn);
  }

  /* As wide as Resume, so it does not move. */
  .pause {
    min-width: 72px;
  }

  .recording::before {
    display: inline-block;
    width: 7px;
    height: 7px;
    margin-right: 6px;
    content: "";
    background: var(--bad);
    border-radius: 50%;
  }

  .grow {
    flex: 1;
  }

  /* A switch of what the timeline shows: lit while on. */
  .toggle[aria-pressed="true"] {
    color: var(--accent);
    border-color: var(--accent);
  }

  .area {
    position: relative;
    overflow: hidden;
  }

  canvas {
    display: block;
    cursor: crosshair;
  }

  /* Under the lanes, as wide as their drawing. */
  .scroll {
    position: relative;
    height: 10px;
    margin: 2px 0 4px;
    background: color-mix(in srgb, var(--border) 45%, transparent);
    border-radius: 5px;
  }

  .thumb {
    position: absolute;
    top: 1px;
    bottom: 1px;
    background: var(--muted);
    border-radius: 4px;
    opacity: 0.55;
    touch-action: none;
  }

  .thumb:hover,
  .thumb:active {
    opacity: 0.85;
  }

  .note {
    margin: 0;
    padding: 4px 8px 8px;
    font-size: 12px;
    color: var(--muted);
  }

  .problem {
    margin: 0 8px 6px;
    font-size: 12px;
    color: var(--bad);
  }
</style>
