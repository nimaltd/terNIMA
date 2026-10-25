// Reading numbers out of received lines, for the plotter. A line gives
// [name, value] pairs, in these forms:
//
//   JSON           {"temp": 21.5, "hum": 40}
//   named          temp=21.5 hum=40, or temp:21.5,hum:40 as the Arduino Serial Plotter
//                  reads it, also inside a log line: <inf> sensor: temp=21.5
//   plain          21.5 40, 21.5,40 or 21.5;40, named "1", "2" and so on
//   your regex     one value per capture group, named after the group if it has a name
//
// It has no imports, so its tests run in Node as they are.

/** How a line is read. */
export type ValueMode = "auto" | "regex";

export type Value = [name: string, value: number];

/** A number as text: sign, digits, decimals, exponent. */
const NUMBER = String.raw`[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?`;
/** "name=value", "name = value" or "name:value": a name starts with a letter. */
const NAMED = new RegExp(String.raw`([A-Za-z_][\w.\-]*)\s*(?:=\s*|:)(${NUMBER})(?![\w.])`, "g");
const PLAIN_NUMBER = new RegExp(`^${NUMBER}$`);
/**
 * A capturing bracket in a regex's source, with its name if it has one: not
 * an escaped one, nor (?: (?= (?! (?<= or (?<!.
 */
const GROUP = /(?<!\\)\((?!\?(?:[:=!]|<[=!]))(?:\?<([A-Za-z_]\w*)>)?/g;

/** Reads the values in a line, in the order they come; none when it holds no numbers this way. */
export function readValues(text: string, mode: ValueMode = "auto", pattern: RegExp | null = null): Value[] {
  if (mode === "regex") {
    return pattern ? fromRegex(text, pattern) : [];
  }
  const trimmed = text.trim();
  if (trimmed.startsWith("{") && trimmed.endsWith("}")) {
    const json = fromJson(trimmed);
    if (json.length > 0) return json;
  }
  const named = fromNamed(trimmed);
  if (named.length > 0) return named;
  return fromPlain(trimmed);
}

function fromJson(text: string): Value[] {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return [];
  }
  const out: Value[] = [];
  const walk = (value: unknown, name: string) => {
    if (typeof value === "number" && Number.isFinite(value)) {
      out.push([name, value]);
    } else if (typeof value === "boolean") {
      out.push([name, value ? 1 : 0]);
    } else if (value && typeof value === "object" && !Array.isArray(value)) {
      // Nested objects get dotted names: {"imu": {"x": 1}} gives "imu.x".
      for (const [key, inner] of Object.entries(value)) {
        walk(inner, name ? `${name}.${key}` : key);
      }
    }
  };
  walk(parsed, "");
  return out;
}

function fromNamed(text: string): Value[] {
  const out: Value[] = [];
  for (const found of text.matchAll(NAMED)) {
    out.push([found[1], Number(found[2])]);
  }
  return out;
}

function fromPlain(text: string): Value[] {
  const words = text.split(/[\s,;]+/).filter((word) => word !== "");
  if (words.length === 0 || !words.every((word) => PLAIN_NUMBER.test(word))) {
    return [];
  }
  return words.map((word, index) => [String(index + 1), Number(word)]);
}

function fromRegex(text: string, pattern: RegExp): Value[] {
  pattern.lastIndex = 0;
  const found = pattern.exec(text);
  if (!found) return [];
  const names = new Map<number, string>();
  // Named groups keep their names; the others are numbered.
  if (found.groups) {
    let index = 0;
    for (const group of pattern.source.matchAll(GROUP)) {
      index++;
      if (group[1]) names.set(index, group[1]);
    }
  }
  const out: Value[] = [];
  for (let index = 1; index < found.length; index++) {
    const value = found[index] === undefined ? NaN : Number.parseFloat(found[index]);
    if (Number.isFinite(value)) {
      out.push([names.get(index) ?? String(index), value]);
    }
  }
  return out;
}

/** Makes the user's regex, or says what is wrong with it. */
export function compileRegex(source: string): RegExp | string {
  if (!source) return "Enter a regex with a capture group for each value, such as temp=(\\d+)";
  try {
    const pattern = new RegExp(source);
    if ([...source.matchAll(GROUP)].length === 0) {
      return "The regex needs a capture group, in brackets, for each value.";
    }
    return pattern;
  } catch (error) {
    return (error as Error).message;
  }
}
