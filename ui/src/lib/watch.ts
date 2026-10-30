// Names for watched variables where room is short, as the plot's series. It
// has no imports, so its tests run in Node as they are.

/**
 * Each watched variable's name without the namespaces or modules in front,
 * `app::SENSORS.temp` as `SENSORS.temp`, where that still tells them apart;
 * the whole names where it does not.
 */
export function shortNames(names: string[]): string[] {
  const short = names.map((name) => {
    const at = name.lastIndexOf("::");
    return at < 0 ? name : name.slice(at + 2);
  });
  return new Set(short).size === short.length && short.every(Boolean) ? short : names;
}
