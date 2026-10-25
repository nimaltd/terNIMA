// Run with npm test: Node runs TypeScript as it is.

import assert from "node:assert/strict";
import { test } from "node:test";
import { compileRegex, readValues } from "./values.ts";

test("named values, with = or the Arduino Serial Plotter's colon", () => {
  assert.deepEqual(readValues("temp=21.5 hum=40"), [
    ["temp", 21.5],
    ["hum", 40],
  ]);
  assert.deepEqual(readValues("temp:21.5,hum:-3e2"), [
    ["temp", 21.5],
    ["hum", -300],
  ]);
  assert.deepEqual(readValues("rssi = -60"), [["rssi", -60]]);
});

test("named values inside a log message", () => {
  assert.deepEqual(readValues("sensor read: temp=21.5 status ok"), [["temp", 21.5]]);
});

test("a colon with a space after it is text, not a value", () => {
  assert.deepEqual(readValues("read failed: -5"), []);
});

test("times and versions are not named values", () => {
  assert.deepEqual(readValues("[00:00:01.234,567] booted"), []);
  assert.deepEqual(readValues("version 1.2.3"), []);
});

test("plain numbers are numbered from 1", () => {
  assert.deepEqual(readValues("1.5, 2 ,-3"), [
    ["1", 1.5],
    ["2", 2],
    ["3", -3],
  ]);
  assert.deepEqual(readValues("10\t20;30"), [
    ["1", 10],
    ["2", 20],
    ["3", 30],
  ]);
  assert.deepEqual(readValues("tick 10"), []);
});

test("JSON, with nested names and booleans", () => {
  assert.deepEqual(readValues('{"temp": 21.5, "imu": {"x": 1, "y": -2}, "on": true, "name": "a"}'), [
    ["temp", 21.5],
    ["imu.x", 1],
    ["imu.y", -2],
    ["on", 1],
  ]);
  // Not JSON after all: read as named values.
  assert.deepEqual(readValues("{temp=1}"), [["temp", 1]]);
});

test("a regex gives one value per group, named after the group", () => {
  const pattern = compileRegex(String.raw`tick (?<tick>\d+), load (\d+)%`);
  assert.ok(pattern instanceof RegExp);
  assert.deepEqual(readValues("tick 42, load 7%", "regex", pattern), [
    ["tick", 42],
    ["2", 7],
  ]);
  assert.deepEqual(readValues("nothing here", "regex", pattern), []);
});

test("a regex without a group, or a broken one, says what is wrong", () => {
  assert.equal(typeof compileRegex(String.raw`\d+`), "string");
  assert.equal(typeof compileRegex(String.raw`\(\d+\)`), "string");
  assert.equal(typeof compileRegex(String.raw`(?:\d+)`), "string");
  assert.equal(typeof compileRegex("(oops"), "string");
  assert.ok(compileRegex(String.raw`v=(\d+)`) instanceof RegExp);
});
