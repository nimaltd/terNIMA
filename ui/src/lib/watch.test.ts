// Run with npm test: Node runs TypeScript as it is.

import assert from "node:assert/strict";
import { test } from "node:test";
import { shortNames } from "./watch.ts";

test("names lose their namespaces where they still differ", () => {
  assert.deepEqual(shortNames(["app::SENSORS.temp", "app::inner::UPTIME_MS"]), ["SENSORS.temp", "UPTIME_MS"]);
  // C names have none.
  assert.deepEqual(shortNames(["sensor.temp", "counter"]), ["sensor.temp", "counter"]);
});

test("names that would be the same stay whole", () => {
  assert.deepEqual(shortNames(["a::COUNT", "b::COUNT"]), ["a::COUNT", "b::COUNT"]);
});
