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
  /** Bookmarks, which only the Lines view has. */
  toggleBookmark?(): void;
  nextBookmark?(): void;
  previousBookmark?(): void;
  /** Saving the values or the chart, which only the Plot view has. */
  exportCsv?(): void;
  exportPng?(): void;
}

let active: TerminalActions | null = null;

/** Called by a terminal when it comes on screen. */
export function setActiveTerminal(terminal: TerminalActions | null): void {
  active = terminal;
}

/** Called by a terminal when it closes; another one may have taken over already. */
export function clearActiveTerminal(terminal: TerminalActions): void {
  if (active === terminal) {
    active = null;
  }
}

/** The terminal the menus act on, if there is one. */
export function activeTerminal(): TerminalActions | null {
  return active;
}
