// Watched variables' names and values as the Watch view and the panel beside
// the terminal show them. It has no imports, so its tests run in Node as they are.

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

/** The smallest and the largest value a variable has had. */
export type Range = { min: number; max: number };

/**
 * A value as its kind is written: true or false, a whole number, or a float
 * with about seven digits in all, its decimals from the largest the variable
 * has been, so they do not change as it moves. A space stands where a minus
 * sign would be, so the digits stay where they are as values come.
 */
export function valueText(value: number | null | undefined, kind: string | undefined, range?: Range | null): string {
  if (value === null || value === undefined) return "";
  if (kind === "bool") return value ? "true" : "false";
  const float = kind === "f32" || kind === "f64";
  const text = float ? value.toFixed(decimals(value, range)) : String(value);
  return text.startsWith("-") ? text : ` ${text}`;
}

function decimals(value: number, range?: Range | null): number {
  const largest = Math.max(Math.abs(value), range ? Math.max(Math.abs(range.min), Math.abs(range.max)) : 0);
  const whole = largest >= 1 ? Math.floor(Math.log10(largest)) + 1 : 1;
  return Math.max(0, Math.min(6, 7 - whole));
}
