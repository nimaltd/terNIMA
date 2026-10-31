// Run with npm test: Node runs TypeScript as it is.

import assert from "node:assert/strict";
import { test } from "node:test";
import { shortNames, valueText } from "./watch.ts";

test("names lose their namespaces where they still differ", () => {
  assert.deepEqual(shortNames(["app::SENSORS.temp", "app::inner::UPTIME_MS"]), ["SENSORS.temp", "UPTIME_MS"]);
  // C names have none.
  assert.deepEqual(shortNames(["sensor.temp", "counter"]), ["sensor.temp", "counter"]);
});

test("names that would be the same stay whole", () => {
  assert.deepEqual(shortNames(["a::COUNT", "b::COUNT"]), ["a::COUNT", "b::COUNT"]);
});

test("a float keeps the decimals of the largest it has been", () => {
  // 123.4 has three whole digits, so four decimals: seven digits in all.
  assert.equal(valueText(1.5, "f32", { min: 0, max: 123.4 }), " 1.5000");
  assert.equal(valueText(0.25, "f64"), " 0.250000");
  assert.equal(valueText(-0.5, "f32", { min: -1, max: 1 }), "-0.500000");
});

test("whole numbers and true or false are written as they are", () => {
  assert.equal(valueText(1000, "u32"), " 1000");
  assert.equal(valueText(-2, "i16"), "-2");
  assert.equal(valueText(1, "bool"), "true");
  assert.equal(valueText(null, "u8"), "");
});
