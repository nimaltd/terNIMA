// Run with npm test: Node runs TypeScript as it is.

import assert from "node:assert/strict";
import { test } from "node:test";
import { CLOSED_MAX, sameDevice, sameTarget, withClosed } from "./recent.ts";
import type { SessionSetup } from "./serial.ts";

const TERMINAL = { enter: "cr", local_echo: false, lf_is_newline: true, pause_ms: 0 } as const;
const STLINK = { vid: "0483", pid: "3754", serial: "003600333132511238363431", interface: 1 };

function serialTab(name: string, port: string, usb = STLINK as typeof STLINK | null, baud = 115200): SessionSetup {
  return {
    name,
    kind: "serial",
    connected: false,
    port: { name: port, usb },
    serial: { baud, data_bits: 8, parity: "none", stop_bits: 1, flow: "none", dtr: true, rts: true },
    terminal: { ...TERMINAL },
  };
}

test("a USB device is the same by its serial number, whatever its COM number now", () => {
  assert.equal(sameDevice({ name: "COM6", usb: STLINK }, { name: "COM9", usb: STLINK }), true);
  assert.equal(sameDevice({ name: "COM6", usb: STLINK }, { name: "COM6", usb: { ...STLINK, serial: "0042" } }), false);
  // A board with two virtual COM ports: the same serial number, another interface.
  assert.equal(sameDevice({ name: "COM6", usb: STLINK }, { name: "COM7", usb: { ...STLINK, interface: 3 } }), false);
});

test("without a serial number, a port is the same by its name", () => {
  const ch340 = { vid: "1a86", pid: "7523", serial: null };
  assert.equal(sameDevice({ name: "COM3", usb: ch340 }, { name: "COM3", usb: ch340 }), true);
  assert.equal(sameDevice({ name: "COM3", usb: ch340 }, { name: "COM4", usb: ch340 }), false);
  assert.equal(sameDevice({ name: "/dev/ttyS0" }, { name: "/dev/ttyS0", usb: null }), true);
});

test("SSH tabs are the same by their host, port and user", () => {
  const ssh = (host: string, port: number, user: string, auth: "password" | "key" = "password"): SessionSetup => ({
    name: `${user}@${host}`,
    kind: "ssh",
    connected: false,
    ssh: { host, port, user, auth },
    terminal: { ...TERMINAL },
  });
  assert.equal(sameTarget(ssh("board", 22, "root"), ssh("board", 22, "root", "key")), true);
  assert.equal(sameTarget(ssh("board", 22, "root"), ssh("board", 2222, "root")), false);
  assert.equal(sameTarget(ssh("board", 22, "root"), ssh("board", 22, "nima")), false);
  assert.equal(sameTarget(ssh("board", 22, "root"), ssh("other", 22, "root")), false);
});

test("tabs of other kinds, chips, probes or addresses are not the same", () => {
  const rtt = (chip: string, serial: string | null): SessionSetup => ({
    name: chip,
    kind: "rtt",
    connected: false,
    rtt: { chip, probe: { vid: "0483", pid: "3754", serial }, speed_khz: 0, under_reset: false, up_channel: 0, down_channel: 0 },
    terminal: { ...TERMINAL },
  });
  assert.equal(sameTarget(rtt("STM32H503RB", "A"), rtt("STM32H503RB", "A")), true);
  assert.equal(sameTarget(rtt("STM32H503RB", "A"), rtt("STM32H503RB", "B")), false);
  assert.equal(sameTarget(rtt("STM32H503RB", "A"), rtt("nRF52840_xxAA", "A")), false);
  assert.equal(sameTarget(rtt("STM32H503RB", "A"), serialTab("H503", "COM6")), false);
  const tcp = (port: number, protocol: "raw" | "jlink"): SessionSetup => ({
    name: "tcp",
    kind: "tcp",
    connected: false,
    tcp: { host: "localhost", port, protocol, channel: 0 },
    terminal: { ...TERMINAL },
  });
  assert.equal(sameTarget(tcp(19021, "jlink"), tcp(19021, "jlink")), true);
  assert.equal(sameTarget(tcp(19021, "jlink"), tcp(19021, "raw")), false);
});

test("a closed tab goes in front, in place of an older one for the same device", () => {
  let closed: SessionSetup[] = [];
  closed = withClosed(closed, serialTab("H503", "COM6", STLINK, 115200));
  closed = withClosed(closed, serialTab("Adapter", "COM3", null));
  closed = withClosed(closed, serialTab("H503 fast", "COM9", STLINK, 921600));
  assert.deepEqual(
    closed.map((setup) => setup.name),
    ["H503 fast", "Adapter"],
  );
  assert.equal(closed[0].serial?.baud, 921600);
});

test("no more than CLOSED_MAX are kept, the oldest go", () => {
  let closed: SessionSetup[] = [];
  for (let n = 0; n < CLOSED_MAX + 5; n++) {
    closed = withClosed(closed, serialTab(`tab ${n}`, `COM${n}`, null));
  }
  assert.equal(closed.length, CLOSED_MAX);
  assert.equal(closed[0].name, `tab ${CLOSED_MAX + 4}`);
});
