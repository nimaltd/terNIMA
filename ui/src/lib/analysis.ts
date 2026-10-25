// The plot's number crunching: spectra for the FFT chart, bins for the
// histogram, and the edge a scope's trigger waits for.
//
// It has no imports, so its tests run in Node as they are.

/** How samples are weighted before the FFT, so a tone between two bins leaks less into the others. */
export type FftWindow = "hann" | "hamming" | "blackman" | "rect";

export type Edge = "rising" | "falling" | "both";

/** The biggest FFT; more samples than this use the newest ones. */
export const FFT_MAX = 65536;

/** The FFT of `re` and `im` in place. Their length is a power of 2. */
export function fft(re: Float64Array, im: Float64Array): void {
  const n = re.length;
  for (let i = 1, j = 0; i < n; i++) {
    let bit = n >> 1;
    for (; j & bit; bit >>= 1) j ^= bit;
    j ^= bit;
    if (i < j) {
      [re[i], re[j]] = [re[j], re[i]];
      [im[i], im[j]] = [im[j], im[i]];
    }
  }
  for (let size = 2; size <= n; size <<= 1) {
    const half = size >> 1;
    const step = (-2 * Math.PI) / size;
    for (let k = 0; k < half; k++) {
      const wr = Math.cos(step * k);
      const wi = Math.sin(step * k);
      for (let a = k; a < n; a += size) {
        const b = a + half;
        const tr = re[b] * wr - im[b] * wi;
        const ti = re[b] * wi + im[b] * wr;
        re[b] = re[a] - tr;
        im[b] = im[a] - ti;
        re[a] += tr;
        im[a] += ti;
      }
    }
  }
}

function weight(window: FftWindow, i: number, n: number): number {
  if (n < 2) return 1;
  const x = (2 * Math.PI * i) / (n - 1);
  switch (window) {
    case "hann":
      return 0.5 - 0.5 * Math.cos(x);
    case "hamming":
      return 0.54 - 0.46 * Math.cos(x);
    case "blackman":
      return 0.42 - 0.5 * Math.cos(x) + 0.08 * Math.cos(2 * x);
    default:
      return 1;
  }
}

export type Spectrum = {
  /** The frequency of each bin, in Hz, from 0 to half the sample rate. */
  freqs: number[];
  levels: number[];
  /** The samples' count after padding to a power of 2. */
  size: number;
};

/**
 * The amplitude spectrum of samples taken `rate` times a second, their mean
 * taken out first: a sine of amplitude A is a peak of A, or of 20·log10(A)
 * in dB. Samples past a power of 2 are padded with zeros.
 */
export function spectrum(samples: ArrayLike<number>, rate: number, window: FftWindow, db: boolean): Spectrum {
  const count = samples.length;
  let size = 2;
  while (size < count) size <<= 1;
  const re = new Float64Array(size);
  const im = new Float64Array(size);
  let mean = 0;
  for (let i = 0; i < count; i++) mean += samples[i];
  mean = count > 0 ? mean / count : 0;
  let gain = 0;
  for (let i = 0; i < count; i++) {
    const w = weight(window, i, count);
    re[i] = (samples[i] - mean) * w;
    gain += w;
  }
  fft(re, im);
  const freqs: number[] = [];
  const levels: number[] = [];
  for (let k = 0; k <= size / 2; k++) {
    // One side of the spectrum holds half of each tone, but for 0 Hz and the top bin.
    const sides = k === 0 || k === size / 2 ? 1 : 2;
    const amplitude = gain > 0 ? (sides * Math.hypot(re[k], im[k])) / gain : 0;
    freqs.push((k * rate) / size);
    levels.push(db ? 20 * Math.log10(Math.max(amplitude, 1e-12)) : amplitude);
  }
  return { freqs, levels, size };
}

/** The highest bin above 0 Hz, its frequency found between bins by a parabola. */
export function peak(freqs: number[], levels: number[]): { freq: number; level: number } | null {
  let best = -1;
  for (let k = 1; k < levels.length; k++) {
    if (best < 0 || levels[k] > levels[best]) best = k;
  }
  if (best < 0) return null;
  if (best === 0 || best === levels.length - 1) return { freq: freqs[best], level: levels[best] };
  const [a, b, c] = [levels[best - 1], levels[best], levels[best + 1]];
  const bend = a - 2 * b + c;
  const shift = bend === 0 ? 0 : (0.5 * (a - c)) / bend;
  const binHz = freqs[1] - freqs[0];
  return { freq: freqs[best] + shift * binHz, level: b - 0.25 * (a - c) * shift };
}

export type Histogram = {
  /** The bins' edges: one more than the bins. */
  edges: number[];
  /** For each series, how many of its values are in each bin. */
  counts: number[][];
};

/** Most bins a histogram picks itself. */
const BINS_MAX = 200;

/**
 * Counts each series' values in bins of one width over all of them: `wanted`
 * bins, or with 0 as many as the values' spread suggests. Whole numbers get
 * bins of whole widths centered on them, so none is split.
 */
export function histogram(series: number[][], wanted: number): Histogram {
  let min = Infinity;
  let max = -Infinity;
  let whole = true;
  const all: number[] = [];
  for (const values of series) {
    for (const value of values) {
      if (value < min) min = value;
      if (value > max) max = value;
      if (whole && !Number.isInteger(value)) whole = false;
      all.push(value);
    }
  }
  if (all.length === 0) return { edges: [], counts: series.map(() => []) };
  let bins = wanted > 0 ? wanted : autoBins(all, min, max);
  let start = min;
  let width: number;
  if (whole) {
    width = Math.max(1, Math.ceil((max - min + 1) / bins));
    start = min - 0.5;
    bins = Math.ceil((max - min + 1) / width);
  } else if (max === min) {
    width = 1;
    start = min - 0.5;
    bins = 1;
  } else {
    width = (max - min) / bins;
  }
  const edges = Array.from({ length: bins + 1 }, (_, index) => start + index * width);
  const counts = series.map((values) => {
    const out = new Array<number>(bins).fill(0);
    for (const value of values) {
      out[Math.min(bins - 1, Math.max(0, Math.floor((value - start) / width)))]++;
    }
    return out;
  });
  return { edges, counts };
}

/** Freedman and Diaconis' bin width, or the square root of the count when most values are the same. */
function autoBins(values: number[], min: number, max: number): number {
  if (max === min) return 1;
  const sorted = Float64Array.from(values).sort();
  const quartile = (q: number) => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
  const spread = quartile(0.75) - quartile(0.25);
  const bins =
    spread > 0 ? (max - min) / ((2 * spread) / Math.cbrt(values.length)) : Math.sqrt(values.length);
  return Math.min(BINS_MAX, Math.max(1, Math.ceil(bins)));
}

/**
 * When the values last crossed `level` on the edge, between `after` and
 * `before`, found between two points by a straight line; null when they did
 * not. Points without a value are skipped.
 */
export function lastCrossing(
  times: ArrayLike<number>,
  values: ArrayLike<number | null>,
  level: number,
  edge: Edge,
  after: number,
  before: number,
): number | null {
  let right = -1;
  for (let i = times.length - 1; i >= 0; i--) {
    const value = values[i];
    if (value === null || value === undefined) continue;
    if (right >= 0) {
      const next = values[right] as number;
      const rising = value < level && next >= level;
      const falling = value > level && next <= level;
      if ((edge !== "falling" && rising) || (edge !== "rising" && falling)) {
        const share = next === value ? 1 : (level - value) / (next - value);
        const time = times[i] + share * (times[right] - times[i]);
        if (time <= before && time > after) return time;
      }
    }
    if (times[i] <= after) break;
    right = i;
  }
  return null;
}
