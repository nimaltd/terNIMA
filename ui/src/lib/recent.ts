// The tabs closed lately: kept to open again as they were, and to give a
// device the settings it had last time. Its imports are types only, so its
// tests run in Node as they are.

import type { ProbeId } from "./probe";
import type { PortId, SessionSetup } from "./serial";

/** The most closed tabs kept, the same as CLOSED_MAX in workspace.rs. */
export const CLOSED_MAX = 20;

/**
 * Whether two ports are the same device: by its USB serial number, and the
 * interface for a device with more than one port, where both have one; by
 * the port's name otherwise.
 */
export function sameDevice(a: PortId, b: PortId): boolean {
  const usbA = a.usb;
  const usbB = b.usb;
  if (usbA?.serial && usbB?.serial) {
    return (
      usbA.vid === usbB.vid &&
      usbA.pid === usbB.pid &&
      usbA.serial === usbB.serial &&
      (usbA.interface ?? null) === (usbB.interface ?? null)
    );
  }
  return a.name === b.name;
}

/** Whether two probes are the same one; no probe chosen is the same as no probe chosen. */
function sameProbe(a: ProbeId | null | undefined, b: ProbeId | null | undefined): boolean {
  if (!a || !b) {
    return !a && !b;
  }
  return a.vid === b.vid && a.pid === b.pid && (a.serial ?? null) === (b.serial ?? null);
}

/** Whether two tabs are for the same thing: the same device, chip, address or recording. */
export function sameTarget(a: SessionSetup, b: SessionSetup): boolean {
  if (a.kind !== b.kind) {
    return false;
  }
  switch (a.kind) {
    case "serial":
      return sameDevice(a.port ?? { name: "" }, b.port ?? { name: "" });
    case "rtt":
      return a.rtt?.chip === b.rtt?.chip && sameProbe(a.rtt?.probe, b.rtt?.probe);
    case "swo":
      return a.swo?.chip === b.swo?.chip && sameProbe(a.swo?.probe, b.swo?.probe);
    case "tcp":
      return (
        a.tcp?.host === b.tcp?.host &&
        a.tcp?.port === b.tcp?.port &&
        a.tcp?.protocol === b.tcp?.protocol &&
        a.tcp?.channel === b.tcp?.channel
      );
    case "ssh":
      return a.ssh?.host === b.ssh?.host && a.ssh?.port === b.ssh?.port && a.ssh?.user === b.ssh?.user;
    case "replay":
      return a.replay?.path === b.replay?.path;
  }
}

/** The closed tabs with `setup` in front, without an older one for the same thing, and at most CLOSED_MAX. */
export function withClosed(closed: SessionSetup[], setup: SessionSetup): SessionSetup[] {
  return [setup, ...closed.filter((other) => !sameTarget(other, setup))].slice(0, CLOSED_MAX);
}
