<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { writeText } from "@tauri-apps/plugin-clipboard-manager";
  import { settings, type Highlight, type HighlightColor } from "./settings.svelte";
  import type { Level } from "./serial";
  import type { StoredLine, Tab } from "./tabs.svelte";
  import { clearActiveTerminal, setActiveTerminal, type TerminalActions } from "./terminal-actions";
  import { appScheme } from "./theme.svelte";

  let {
    tab,
    active,
    onhighlights,
  }: {
    tab: Tab;
    /** Whether this view has the keyboard. */
    active: boolean;
    /** Opens the highlight rules. */
    onhighlights: () => void;
  } = $props();

  /** The height of a row, in pixels, the same as `.row` below. */
  const ROW = 20;
  const LEVELS: { level: Level | "other"; label: string; title: string }[] = [
    { level: "err", label: "ERR", title: "Errors" },
    { level: "wrn", label: "WRN", title: "Warnings" },
    { level: "inf", label: "INF", title: "Information" },
    { level: "dbg", label: "DBG", title: "Debug" },
    { level: "other", label: "Other", title: "Lines that are not log lines, such as the shell's" },
  ];

  let list: HTMLDivElement;
  let searchInput: HTMLInputElement;
  let scrollTop = $state(0);
  let height = $state(0);
  /** Whether the list keeps showing the newest line as lines come. */
  let follow = $state(true);

  let query = $state("");
  let regex = $state(false);
  let caseSensitive = $state(false);
  let onlyMatching = $state(false);
  let onlyBookmarks = $state(false);
  let modulesOpen = $state(false);

  /** The selected lines, by number: from `anchor` to `focus`. */
  let anchor: number | null = $state(null);
  let focus: number | null = $state(null);

  /**
   * Where the lines shown are in `tab.textLines`. A plain array, as there can be
   * many; `shownVersion` changes with it.
   */
  let positions: number[] = [];
  let count = $state(0);
  let shownVersion = $state(0);
  /** How much of `tab.textLines` has been looked at, and the number of its first line then. */
  let scanned = 0;
  let firstLine = -1;

  /** The search as a regular expression, or null without one; `badRegex` when it is wrong. */
  const search = $derived.by(() => compile(query, regex, caseSensitive));
  const badRegex = $derived(query !== "" && search === null);
  const rules = $derived(
    settings.highlights
      .map((rule) => ({ rule, pattern: compile(rule.text, rule.regex, rule.case) }))
      .filter((compiled): compiled is { rule: Highlight; pattern: RegExp } => compiled.pattern !== null),
  );

  function compile(text: string, isRegex: boolean, matchCase: boolean): RegExp | null {
    if (!text) {
      return null;
    }
    try {
      const source = isRegex ? text : text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      return new RegExp(source, matchCase ? "g" : "gi");
    } catch {
      return null;
    }
  }

  function matches(pattern: RegExp, text: string): boolean {
    pattern.lastIndex = 0;
    return pattern.test(text);
  }

  function passes(line: StoredLine): boolean {
    const view = tab.linesView;
    if (line.level) {
      if (view.hidden_levels.includes(line.level)) return false;
      if (line.module && view.hidden_modules.includes(line.module)) return false;
    } else if (!line.note && !view.others) {
      // A note, such as "connected to COM6", stays whatever levels are shown.
      return false;
    }
    if (onlyMatching && search && !matches(search, line.text)) return false;
    if (onlyBookmarks && !tab.bookmarks.has(line.n)) return false;
    return true;
  }

  /** Works out which lines are shown: all again, or only the new ones when nothing else changed. */
  function refilter(full: boolean) {
    const lines = tab.textLines;
    const first = lines.length > 0 ? lines[0].n : -1;
    if (full || first !== firstLine || scanned > lines.length) {
      positions = [];
      scanned = 0;
      firstLine = first;
    }
    for (let at = scanned; at < lines.length; at++) {
      if (passes(lines[at])) {
        positions.push(at);
      }
    }
    scanned = lines.length;
    count = positions.length;
    shownVersion++;
    if (follow) {
      requestAnimationFrame(() => list && (list.scrollTop = list.scrollHeight));
    }
  }

  // Any change to what is left out lays the list out again.
  $effect(() => {
    void [
      tab.linesView.hidden_levels.length,
      tab.linesView.hidden_modules.length,
      tab.linesView.others,
      search,
      onlyMatching,
      onlyBookmarks,
    ];
    untrack(() => refilter(true));
  });
  // New lines, or bookmarks changed: only the new lines need looking at.
  $effect(() => {
    void tab.linesVersion;
    untrack(() => refilter(onlyBookmarks));
  });

  /** The rows on screen, and a few more either side. */
  const rows = $derived.by(() => {
    void shownVersion;
    const start = Math.max(0, Math.floor(scrollTop / ROW) - 5);
    const end = Math.min(count, Math.ceil((scrollTop + height) / ROW) + 5);
    const shown: { index: number; line: StoredLine }[] = [];
    for (let index = start; index < end; index++) {
      const line = tab.textLines[positions[index]];
      if (line) {
        shown.push({ index, line });
      }
    }
    return shown;
  });

  const modules = $derived.by(() => {
    void tab.linesVersion;
    return [...tab.moduleCounts.entries()].sort((a, b) => a[0].localeCompare(b[0]));
  });

  function onscroll() {
    scrollTop = list.scrollTop;
    follow = list.scrollTop + list.clientHeight >= list.scrollHeight - ROW;
  }

  /** The line's text in pieces, with the search's matches marked. */
  function pieces(text: string): { text: string; match: boolean }[] {
    if (!search) {
      return [{ text, match: false }];
    }
    const parts: { text: string; match: boolean }[] = [];
    let at = 0;
    search.lastIndex = 0;
    for (const found of text.matchAll(search)) {
      if (found[0] === "") break;
      if (found.index > at) parts.push({ text: text.slice(at, found.index), match: false });
      parts.push({ text: found[0], match: true });
      at = found.index + found[0].length;
    }
    if (at < text.length) parts.push({ text: text.slice(at), match: false });
    return parts;
  }

  function ruleColor(line: StoredLine): HighlightColor | null {
    return rules.find(({ pattern }) => matches(pattern, line.text))?.rule.color ?? null;
  }

  /** A color of its own for each module, so they are told apart at a glance. */
  function moduleColor(module: string): string {
    let hash = 0;
    for (const ch of module) hash = (hash * 31 + ch.charCodeAt(0)) >>> 0;
    const hue = hash % 360;
    return appScheme() === "dark" ? `hsl(${hue} 55% 68%)` : `hsl(${hue} 60% 34%)`;
  }

  function toggleLevel(level: Level | "other") {
    const view = tab.linesView;
    if (level === "other") {
      tab.setLinesView({ ...view, others: !view.others });
      return;
    }
    const hidden = view.hidden_levels.includes(level)
      ? view.hidden_levels.filter((other) => other !== level)
      : [...view.hidden_levels, level];
    tab.setLinesView({ ...view, hidden_levels: hidden });
  }

  function levelShown(level: Level | "other"): boolean {
    return level === "other" ? tab.linesView.others : !tab.linesView.hidden_levels.includes(level);
  }

  function toggleModule(module: string) {
    const view = tab.linesView;
    const hidden = view.hidden_modules.includes(module)
      ? view.hidden_modules.filter((other) => other !== module)
      : [...view.hidden_modules, module];
    tab.setLinesView({ ...view, hidden_modules: hidden });
  }

  function allModules(shown: boolean) {
    tab.setLinesView({ ...tab.linesView, hidden_modules: shown ? [] : modules.map(([module]) => module) });
  }

  // Selection.

  function isSelected(n: number): boolean {
    if (anchor === null || focus === null) return false;
    return n >= Math.min(anchor, focus) && n <= Math.max(anchor, focus);
  }

  function select(n: number, extend: boolean) {
    if (!extend || anchor === null) anchor = n;
    focus = n;
  }

  /** The index in the shown lines of the line with number `n`, or -1. */
  function indexOf(n: number | null): number {
    if (n === null) return -1;
    // Numbers grow along the list, so a binary search finds it.
    let low = 0;
    let high = count - 1;
    while (low <= high) {
      const middle = (low + high) >> 1;
      const at = tab.textLines[positions[middle]].n;
      if (at === n) return middle;
      if (at < n) low = middle + 1;
      else high = middle - 1;
    }
    return -1;
  }

  /** Selects the shown line at `index`, and scrolls it into view. */
  function selectIndex(index: number, extend = false) {
    if (count === 0) return;
    const clamped = Math.max(0, Math.min(count - 1, index));
    select(tab.textLines[positions[clamped]].n, extend);
    const top = clamped * ROW;
    if (top < list.scrollTop) list.scrollTop = top;
    else if (top + ROW > list.scrollTop + list.clientHeight) list.scrollTop = top + ROW - list.clientHeight;
  }

  function selectedLines(): StoredLine[] {
    if (anchor === null || focus === null) return [];
    const low = Math.min(anchor, focus);
    const high = Math.max(anchor, focus);
    const out: StoredLine[] = [];
    for (let index = 0; index < count; index++) {
      const line = tab.textLines[positions[index]];
      if (line.n >= low && line.n <= high) out.push(line);
    }
    return out;
  }

  /** Goes to the next (1) or previous (-1) shown line that `wanted` takes, from the selection. */
  function goTo(step: number, wanted: (line: StoredLine) => boolean) {
    const from = indexOf(focus);
    for (let index = from + step; index >= 0 && index < count; index += step) {
      if (wanted(tab.textLines[positions[index]])) {
        follow = false;
        selectIndex(index);
        return;
      }
    }
  }

  function onkeydown(event: KeyboardEvent) {
    const page = Math.max(1, Math.floor(list.clientHeight / ROW) - 1);
    const from = indexOf(focus);
    const moves: Record<string, number> = {
      ArrowDown: from + 1,
      ArrowUp: from - 1,
      PageDown: from + page,
      PageUp: from - page,
      Home: 0,
      End: count - 1,
    };
    if (event.key in moves) {
      event.preventDefault();
      follow = event.key === "End";
      selectIndex(moves[event.key], event.shiftKey);
    }
  }

  const actions: TerminalActions = {
    hasSelection: () => anchor !== null,
    copy: async () => {
      const text = selectedLines()
        .map((line) => line.text)
        .join("\n");
      if (text) await writeText(text);
    },
    paste: async () => {},
    selectAll: () => {
      if (count > 0) {
        anchor = tab.textLines[positions[0]].n;
        focus = tab.textLines[positions[count - 1]].n;
      }
    },
    clear: () => {
      anchor = focus = null;
      tab.clearLines();
    },
    text: () => {
      const out: string[] = [];
      for (let index = 0; index < count; index++) out.push(tab.textLines[positions[index]].text);
      return out.length > 0 ? `${out.join("\n")}\n` : "";
    },
    focus: () => list?.focus(),
    openSearch: () => {
      searchInput.focus();
      searchInput.select();
    },
    findNext: () => search && goTo(1, (line) => matches(search, line.text)),
    findPrevious: () => search && goTo(-1, (line) => matches(search, line.text)),
    toggleBookmark: () => {
      if (focus !== null) tab.toggleBookmark(focus);
    },
    nextBookmark: () => goTo(1, (line) => tab.bookmarks.has(line.n)),
    previousBookmark: () => goTo(-1, (line) => tab.bookmarks.has(line.n)),
  };

  // The view on screen is the one the menus act on, and has the keyboard.
  $effect(() => {
    if (active) {
      setActiveTerminal(actions);
      list.focus();
    }
  });

  onMount(() => {
    const resize = new ResizeObserver(() => (height = list.clientHeight));
    resize.observe(list);
    height = list.clientHeight;
    return () => {
      resize.disconnect();
      clearActiveTerminal(actions);
    };
  });

  function onSearchKey(event: KeyboardEvent) {
    if (event.key === "Enter") {
      event.preventDefault();
      if (event.shiftKey) actions.findPrevious();
      else actions.findNext();
    } else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      query = "";
      list.focus();
    }
  }

  function onWindowMouseDown(event: MouseEvent) {
    if (modulesOpen && !(event.target as Element).closest(".modules")) {
      modulesOpen = false;
    }
  }
</script>

<svelte:window onmousedown={onWindowMouseDown} />

<div class="linesview">
  <div class="bar">
    <div class="chips" role="group" aria-label="Levels">
      {#each LEVELS as { level, label, title } (level)}
        <button
          class="chip {level}"
          class:off={!levelShown(level)}
          title={`${title}: click to show or hide`}
          onclick={() => toggleLevel(level)}
        >
          {label}
          <span class="count">{(void tab.linesVersion, tab.levelCounts.get(level) ?? 0)}</span>
        </button>
      {/each}
    </div>

    <div class="modules">
      <button class="btn small" onclick={() => (modulesOpen = !modulesOpen)} disabled={modules.length === 0}>
        Modules
        {#if tab.linesView.hidden_modules.length > 0}
          <span class="count">{modules.length - tab.linesView.hidden_modules.length} of {modules.length}</span>
        {/if}
        ▾
      </button>
      {#if modulesOpen}
        <div class="popover" role="menu">
          <div class="popover-head">
            <button class="link" onclick={() => allModules(true)}>All</button>
            <button class="link" onclick={() => allModules(false)}>None</button>
          </div>
          {#each modules as [module, number] (module)}
            <label class="choice">
              <input
                type="checkbox"
                checked={!tab.linesView.hidden_modules.includes(module)}
                onchange={() => toggleModule(module)}
              />
              <span class="name mono" style:color={moduleColor(module)}>{module}</span>
              <span class="count">{number}</span>
            </label>
          {/each}
        </div>
      {/if}
    </div>

    <div class="search" class:bad={badRegex}>
      <input
        class="field mono"
        placeholder="Search"
        spellcheck="false"
        bind:this={searchInput}
        bind:value={query}
        onkeydown={onSearchKey}
      />
      <button class="option" class:on={caseSensitive} title="Match case" onclick={() => (caseSensitive = !caseSensitive)}>
        Aa
      </button>
      <button class="option" class:on={regex} title="Regular expression" onclick={() => (regex = !regex)}>.*</button>
      <label class="check" title="Show only the lines that match the search">
        <input type="checkbox" bind:checked={onlyMatching} /> Only
      </label>
    </div>

    <div class="marks">
      <button class="option" title="Previous bookmark (Shift+F2)" onclick={() => actions.previousBookmark?.()}>▲</button>
      <button class="option" title="Next bookmark (F2)" onclick={() => actions.nextBookmark?.()}>▼</button>
      <label class="check" title="Show only the lines with a bookmark">
        <input type="checkbox" bind:checked={onlyBookmarks} /> Only ★
      </label>
    </div>

    <button class="btn small" title="Mark lines that match in a color" onclick={onhighlights}>Highlights...</button>

    <span class="spacer"></span>
    <span class="total" title="Lines shown, of the lines kept">
      {count.toLocaleString()} / {(void tab.linesVersion, tab.textLines.length).toLocaleString()}
    </span>
  </div>

  {#if !follow && count > 0}
    <!-- Scrolled up: a way back to the newest line, which the list then keeps showing. -->
    <button
      class="btn latest"
      title="Go to the newest line and keep showing it"
      onclick={() => {
        follow = true;
        list.scrollTop = list.scrollHeight;
        list.focus();
      }}
    >
      Latest ↓
    </button>
  {/if}

  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div class="list" tabindex="0" role="grid" aria-rowcount={count} bind:this={list} {onscroll} {onkeydown}>
    <div class="space" style:height={`${count * ROW}px`}>
      {#each rows as { index, line } (line.n)}
        {@const color = ruleColor(line)}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="row {color ?? ''}"
          class:selected={isSelected(line.n)}
          class:marked={color !== null}
          class:other={line.level === null && !line.note}
          class:note={line.note}
          role="row"
          tabindex="-1"
          style:top={`${index * ROW}px`}
          onmousedown={(event) => select(line.n, event.shiftKey)}
        >
          <button
            class="star"
            class:on={tab.bookmarks.has(line.n)}
            title="Bookmark (Ctrl+F2)"
            onmousedown={(event) => event.stopPropagation()}
            onclick={() => tab.toggleBookmark(line.n)}
          >
            {tab.bookmarks.has(line.n) ? "★" : "☆"}
          </button>
          {#if line.note}
            <!-- The tab's own line, as the terminal shows it, across the columns. -->
            <span class="message mono" title={new Date(line.at).toLocaleTimeString()}>
              {#each pieces(line.text) as piece, at (at)}
                {#if piece.match}<mark>{piece.text}</mark>{:else}{piece.text}{/if}
              {/each}
            </span>
          {:else}
            <span class="time mono" title={new Date(line.at).toLocaleTimeString()}>{line.time ?? ""}</span>
            <span class="level {line.level ?? ''}">{line.level?.toUpperCase() ?? ""}</span>
            <span class="mod mono" title={line.module ?? ""} style:color={line.module ? moduleColor(line.module) : ""}>
              {line.module ?? ""}
            </span>
            <span class="message mono" title={line.text}>
              {#each pieces(line.level ? line.message : line.text) as piece, at (at)}
                {#if piece.match}<mark>{piece.text}</mark>{:else}{piece.text}{/if}
              {/each}
            </span>
          {/if}
        </div>
      {/each}
    </div>
    {#if count === 0}
      <p class="empty">
        {tab.textLines.length === 0
          ? "No lines yet. Zephyr's log lines are read into time, level and module as they come."
          : "No line is left by the filters."}
      </p>
    {/if}
  </div>
</div>

<style>
  .linesview {
    position: relative;
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }

  .latest {
    position: absolute;
    right: 24px;
    bottom: 12px;
    z-index: 2;
    height: 28px;
    padding: 0 12px;
    font-size: 12px;
    box-shadow: var(--shadow);
  }

  .bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 10px;
    padding-bottom: 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .chips,
  .search,
  .marks {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .chip {
    display: flex;
    gap: 4px;
    align-items: center;
    height: 24px;
    padding: 0 7px;
    font: inherit;
    font-weight: 600;
    color: var(--text);
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 12px;
    cursor: default;
  }

  .chip.err {
    color: var(--bad);
  }

  .chip.wrn {
    color: var(--warn);
  }

  .chip.inf {
    color: var(--ok);
  }

  .chip.dbg,
  .chip.other {
    color: var(--muted);
  }

  .chip.off {
    opacity: 0.4;
    text-decoration: line-through;
  }

  /* Numbers keep their width as they grow, so nothing next to them moves. */
  .count {
    font-weight: 400;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  .chip .count {
    min-width: 5ch;
    text-align: right;
  }

  .modules {
    position: relative;
  }

  .small {
    height: 24px;
    padding: 0 8px;
    font-size: 12px;
  }

  .popover {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    z-index: 10;
    min-width: 220px;
    max-height: 320px;
    padding: 4px;
    overflow-y: auto;
    background: var(--menu-bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: var(--shadow);
  }

  .popover-head {
    display: flex;
    gap: 12px;
    padding: 4px 8px 6px;
    border-bottom: 1px solid var(--border);
  }

  .link {
    padding: 0;
    font: inherit;
    color: var(--accent);
    background: none;
    border: 0;
    cursor: default;
  }

  .choice {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 4px 8px;
    border-radius: 5px;
  }

  .choice:hover {
    background: var(--hover);
  }

  .choice .name {
    flex: 1;
  }

  .search .field {
    width: 140px;
    height: 24px;
    font-size: 12px;
  }

  .search.bad .field {
    border-color: var(--bad);
  }

  .option {
    min-width: 24px;
    height: 24px;
    padding: 0 6px;
    font: inherit;
    color: var(--muted);
    background: none;
    border: 1px solid transparent;
    border-radius: 5px;
    cursor: default;
  }

  .option:hover {
    color: var(--text);
    background: var(--hover);
  }

  .option.on {
    color: var(--text);
    background: var(--accent-soft);
    border-color: var(--accent);
  }

  .check {
    display: flex;
    gap: 4px;
    align-items: center;
    margin-left: 4px;
    white-space: nowrap;
  }

  .spacer {
    flex: 1;
  }

  .total {
    min-width: 15ch;
    font-variant-numeric: tabular-nums;
    text-align: right;
    white-space: nowrap;
  }

  .list {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: auto;
    outline: none;
  }

  .space {
    position: relative;
  }

  .row {
    position: absolute;
    right: 0;
    left: 0;
    display: flex;
    gap: 10px;
    align-items: center;
    height: 20px;
    padding-right: 8px;
    font-size: 13px;
    white-space: nowrap;
    border-left: 3px solid transparent;
  }

  .row:hover {
    background: var(--hover);
  }

  .row.selected {
    background: var(--accent-soft);
  }

  .row.other .message {
    color: var(--muted);
  }

  .row.note .message {
    color: var(--muted);
    opacity: 0.8;
  }

  /* Highlight rules: a tint and a bar in the rule's color. */
  .row.marked.red {
    background: rgb(239 68 68 / 16%);
    border-left-color: rgb(239 68 68);
  }

  .row.marked.orange {
    background: rgb(249 115 22 / 16%);
    border-left-color: rgb(249 115 22);
  }

  .row.marked.yellow {
    background: rgb(234 179 8 / 18%);
    border-left-color: rgb(234 179 8);
  }

  .row.marked.green {
    background: rgb(34 197 94 / 16%);
    border-left-color: rgb(34 197 94);
  }

  .row.marked.blue {
    background: rgb(59 130 246 / 16%);
    border-left-color: rgb(59 130 246);
  }

  .row.marked.purple {
    background: rgb(168 85 247 / 16%);
    border-left-color: rgb(168 85 247);
  }

  .star {
    width: 18px;
    padding: 0;
    font: inherit;
    color: var(--border);
    background: none;
    border: 0;
    cursor: default;
  }

  .row:hover .star {
    color: var(--muted);
  }

  .star.on,
  .row:hover .star.on {
    color: var(--warn);
  }

  .time {
    flex-shrink: 0;
    width: 17ch;
    color: var(--muted);
  }

  .level {
    flex-shrink: 0;
    width: 3.2em;
    font-size: 11px;
    font-weight: 700;
  }

  .level.err {
    color: var(--bad);
  }

  .level.wrn {
    color: var(--warn);
  }

  .level.inf {
    color: var(--ok);
  }

  .level.dbg {
    color: var(--muted);
  }

  .mod {
    flex-shrink: 0;
    width: 14ch;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .message {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  mark {
    color: inherit;
    background: rgb(234 179 8 / 45%);
    border-radius: 2px;
  }

  .empty {
    position: absolute;
    top: 24px;
    right: 0;
    left: 0;
    color: var(--muted);
    text-align: center;
  }
</style>
