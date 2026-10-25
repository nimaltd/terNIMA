// Run with npm test: Node runs TypeScript as it is.

import assert from "node:assert/strict";
import { test } from "node:test";
import { fft, histogram, lastCrossing, peak, spectrum } from "./analysis.ts";

const close = (actual: number, expected: number, within: number) =>
  assert.ok(Math.abs(actual - expected) <= within, `${actual} is not within ${within} of ${expected}`);

const sine = (count: number, rate: number, hz: number, amplitude: number, offset = 0) =>
  Array.from({ length: count }, (_, i) => offset + amplitude * Math.sin((2 * Math.PI * hz * i) / rate));

test("the FFT of an impulse is flat, and of a cosine is two bins", () => {
  const re = new Float64Array(8);
  const im = new Float64Array(8);
  re[0] = 1;
  fft(re, im);
  for (let k = 0; k < 8; k++) {
    close(re[k], 1, 1e-12);
    close(im[k], 0, 1e-12);
  }
  const wave = Float64Array.from({ length: 16 }, (_, i) => Math.cos((2 * Math.PI * 3 * i) / 16));
  const zero = new Float64Array(16);
  fft(wave, zero);
  for (let k = 0; k < 16; k++) {
    close(Math.hypot(wave[k], zero[k]), k === 3 || k === 13 ? 8 : 0, 1e-9);
  }
});

test("a sine on a bin shows its amplitude, with any window, and its mean is taken out", () => {
  // 1024 samples at 1024 Hz: bins are 1 Hz apart.
  for (const window of ["rect", "hann", "hamming", "blackman"] as const) {
    const { freqs, levels, size } = spectrum(sine(1024, 1024, 50, 2, 5), 1024, window, false);
    assert.equal(size, 1024);
    assert.equal(freqs[50], 50);
    close(levels[50], 2, 0.01);
    close(levels[0], 0, 1e-5);
  }
  const db = spectrum(sine(1024, 1024, 50, 1), 1024, "hann", true);
  close(db.levels[50], 0, 0.01);
});

test("the peak is found between bins", () => {
  // 1000 samples at 100 Hz, padded to 1024: a 12.3 Hz tone falls between bins.
  const { freqs, levels } = spectrum(sine(1000, 100, 12.3, 1), 100, "hann", true);
  const top = peak(freqs, levels);
  assert.ok(top);
  close(top.freq, 12.3, 0.02);
  close(top.level, 0, 0.5);
});

test("whole numbers get a bin each, centered on them", () => {
  const { edges, counts } = histogram([[0, 1, 2, 2, 3, 9]], 0);
  assert.equal(edges[0], -0.5);
  assert.equal(edges.at(-1), 9.5);
  assert.equal(counts[0].reduce((sum, count) => sum + count, 0), 6);
  const ten = histogram([[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]], 10);
  assert.deepEqual(ten.counts[0], [1, 1, 1, 1, 1, 1, 1, 1, 1, 1]);
  // Fewer bins than whole numbers: two to a bin.
  const five = histogram([[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]], 5);
  assert.deepEqual(five.counts[0], [2, 2, 2, 2, 2]);
});

test("series share their bins, and the top value is in the last bin", () => {
  const { edges, counts } = histogram(
    [
      Array.from({ length: 100 }, (_, i) => i / 100),
      [0.5, 0.99],
    ],
    4,
  );
  assert.equal(edges.length, 5);
  assert.deepEqual(counts[0], [25, 25, 25, 25]);
  assert.deepEqual(counts[1], [0, 0, 1, 1]);
  assert.deepEqual(histogram([[2.5, 2.5]], 0).counts, [[2]]);
  assert.deepEqual(histogram([[]], 0), { edges: [], counts: [[]] });
});

test("a trigger finds the last crossing on its edge, between two points", () => {
  const times = [0, 1, 2, 3, 4, 5, 6, 7];
  const values = [0, 1, 2, 1, 0, -1, 0, 1];
  assert.equal(lastCrossing(times, values, 0.5, "rising", -Infinity, Infinity), 6.5);
  assert.equal(lastCrossing(times, values, 0.5, "falling", -Infinity, Infinity), 3.5);
  assert.equal(lastCrossing(times, values, 0.5, "both", -Infinity, Infinity), 6.5);
  assert.equal(lastCrossing(times, values, 0.5, "rising", -Infinity, 6), 0.5);
  assert.equal(lastCrossing(times, values, 0.5, "rising", 1, 6), null);
  assert.equal(lastCrossing(times, values, 5, "rising", -Infinity, Infinity), null);
});

test("a trigger skips points without a value", () => {
  assert.equal(lastCrossing([0, 1, 2, 3], [0, null, null, 3], 1.5, "rising", -Infinity, Infinity), 1.5);
});
