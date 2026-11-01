// Finding code addresses in a line of output, as a crash prints them, and
// writing where they are. Which of them really are code, the ELF file says.

/** Where an address is in the code, as `CodePlace` in ternima-core's code.rs. */
export type CodePlace = { function: string; file: string | null; line: number | null };

const ESCAPE = /\x1b\[[0-9;?]*[ -/]*[@-~]/g;
const WITH_0X = /\b0[xX][0-9a-fA-F]{1,16}\b/g;
const BARE = /\b[0-9a-fA-F]{8}\b/g;
/** A fault handler may print PC = 08001234: bare digits count only on a line that speaks of these. */
const REGISTERS = /\b(?:pc|lr|ra|r14|r15|mepc|elr|fault|faulting|backtrace|stack)\b/i;

/** No line needs more: a line of a binary dump should not cost hundreds of lookups. */
const MOST = 16;

/** The addresses a line may have, as written, each once. */
export function codeAddresses(line: string): string[] {
  const text = line.includes("\x1b") ? line.replace(ESCAPE, "") : line;
  const found = text.match(WITH_0X) ?? [];
  const bare = REGISTERS.test(text) ? (text.match(BARE) ?? []) : [];
  return [...new Set([...found, ...bare])].slice(0, MOST);
}

/** What goes under the line: `↳ 0x08001234  src/main.c:42 in read_sensor()`. */
export function placeText(address: string, place: CodePlace): string {
  const where = place.file ? `${place.file}${place.line ? `:${place.line}` : ""} ` : "";
  return `↳ ${address}  ${where}in ${place.function}()`;
}
