// What the menus can ask of the terminal that has the focus.

export interface TerminalActions {
  hasSelection(): boolean;
  copy(): Promise<void>;
  paste(): Promise<void>;
  selectAll(): void;
  clear(): void;
  focus(): void;
}

let active: TerminalActions | null = null;

/** Called by a terminal when it opens, and with `null` when it closes. */
export function setActiveTerminal(terminal: TerminalActions | null): void {
  active = terminal;
}

/** The terminal the menus act on, if there is one. */
export function activeTerminal(): TerminalActions | null {
  return active;
}
