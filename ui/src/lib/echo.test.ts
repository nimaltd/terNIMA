// Run with npm test: Node runs TypeScript as it is.

import assert from "node:assert/strict";
import { test } from "node:test";
import { echoed } from "./echo.ts";

test("text shows as it is typed, Enter as a new line and Backspace as a rubout", () => {
  assert.equal(echoed("ls -l\r"), "ls -l\r\n");
  assert.equal(echoed("a\tb"), "a\tb");
  assert.equal(echoed("ab\x7f"), "ab\b \b");
  assert.equal(echoed("ab\b"), "ab\b \b");
});

test("the keys that move the cursor are not drawn", () => {
  // ↑ and ↓ walk a shell's history: drawn, they moved the cursor over the lines above.
  assert.equal(echoed("\x1b[A"), "");
  assert.equal(echoed("\x1b[B"), "");
  assert.equal(echoed("\x1bOA"), "");
  assert.equal(echoed("\x1b[1;5C"), "");
  assert.equal(echoed("\x1b[3~"), "");
  assert.equal(echoed("\x1b[15~"), "");
  assert.equal(echoed("\x1bOP"), "");
  assert.equal(echoed("ls\x1b[D\x1b[D-a"), "ls-a");
});

test("Escape, Alt with a key and control characters are not drawn", () => {
  assert.equal(echoed("\x1b"), "");
  assert.equal(echoed("\x1bb"), "");
  assert.equal(echoed("\x03"), "");
  assert.equal(echoed("\x0e"), "");
  assert.equal(echoed("\x1b[200~pasted\x1b[201~"), "pasted");
});
