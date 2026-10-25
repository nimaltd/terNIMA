// What the menus can ask of the terminal that has the focus.

export interface TerminalActions {
  hasSelection(): boolean;
  copy(): Promise<void>;
  paste(): Promise<void>;
  selectAll(): void;
  clear(): void;
  /** Everything in the terminal and its scrollback, as plain text. */
  text(): string;
  focus(): void;
  /** Opens the search bar, with the selected text in it if there is some. */
  openSearch(): void;
  /** Goes to the next or the previous match of the last search. */
  findNext(): void;
  findPrevious(): void;
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
