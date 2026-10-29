<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { Terminal, type IMarker } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import { SearchAddon } from "@xterm/addon-search";
  import { Unicode11Addon } from "@xterm/addon-unicode11";
  import { WebLinksAddon } from "@xterm/addon-web-links";
  import { WebglAddon } from "@xterm/addon-webgl";
  import { echoed } from "./echo";
  import { settings } from "./settings.svelte";
  import { isMac } from "./shortcuts";
  import type { Tab } from "./tabs.svelte";
  import { clearActiveTerminal, setActiveTerminal, type TerminalActions } from "./terminal-actions";
  import { appScheme, searchColors, terminalThemes } from "./theme.svelte";

  let {
    tab = null,
    hex = false,
    active,
    isAppKey,
  }: {
    /** The session this terminal shows, or null for the welcome tab. */
    tab?: Tab | null;
    /** Shows the session's bytes in hex instead of as a terminal would. */
    hex?: boolean;
    /** Whether this terminal is the one on screen. */
    active: boolean;
    /** Says which keys belong to the app, so they are not sent to the device. */
    isAppKey: (event: KeyboardEvent) => boolean;
  } = $props();

  let container: HTMLDivElement;
  let opened = $state(false);

  // The search bar.
  let searchOpen = $state(false);
  let searchInput: HTMLInputElement | undefined = $state();
  let query = $state("");
  let caseSensitive = $state(false);
  let wholeWord = $state(false);
  let regex = $state(false);
  /** Which match the search is on, counted from 1, and how many there are. */
  let results: { index: number; count: number } | null = $state(null);
  let badRegex = $state(false);

  const term = new Terminal({
    fontFamily: '"JetBrains Mono", ui-monospace, monospace',
    fontSize: settings.font_size,
    cursorBlink: true,
    scrollback: 10_000,
    // The unicode11 addon uses xterm.js's proposed API.
    allowProposedApi: true,
    theme: terminalThemes[appScheme()],
    // A right-click pastes, or does nothing: it does not pick a word, as it would on macOS.
    rightClickSelectsWord: false,
  });

  const fit = new FitAddon();
  term.loadAddon(fit);
  term.loadAddon(new Unicode11Addon());
  term.unicode.activeVersion = "11";
  // Links open in the system browser, not inside the app's window.
  term.loadAddon(new WebLinksAddon((_event, uri) => void openUrl(uri)));
  const search = new SearchAddon();
  term.loadAddon(search);
  search.onDidChangeResults(({ resultIndex, resultCount }) => {
    results = { index: resultIndex + 1, count: resultCount };
  });
  term.attachCustomKeyEventHandler((event) => {
    if (event.type === "keydown" && clipboardKey(event)) {
      return false;
    }
    return !isAppKey(event);
  });

  term.onData((data) => {
    if (tab) {
      tab.type(data);
    } else {
      // The welcome tab has nothing to send to, so it echoes to show the terminal works.
      term.write(echoed(data));
    }
  });

  /**
   * Ctrl+C copies while text is selected, and goes to the device as Ctrl+C
   * when nothing is; Ctrl+V and Shift+Insert paste. On macOS, Cmd+C and
   * Cmd+V do it, from the Edit menu. Says whether the key was one of these.
   */
  function clipboardKey(event: KeyboardEvent): boolean {
    if (isMac || event.altKey || event.metaKey) {
      return false;
    }
    const ctrl = event.ctrlKey && !event.shiftKey;
    if (ctrl && event.code === "KeyC" && term.hasSelection()) {
      event.preventDefault();
      void actions.copy();
      term.clearSelection();
      return true;
    }
    if ((ctrl && event.code === "KeyV") || (event.shiftKey && !event.ctrlKey && event.key === "Insert")) {
      event.preventDefault();
      void actions.paste();
      return true;
    }
    return false;
  }

  /** Selecting text with the mouse copies it, as in PuTTY, once the button is up, wherever it is. */
  function onMouseDown(event: MouseEvent) {
    if (event.button !== 0 || !settings.mouse.select_copies) {
      return;
    }
    window.addEventListener(
      "mouseup",
      () =>
        // After xterm.js has made the selection.
        setTimeout(() => {
          if (term.hasSelection()) {
            void actions.copy();
          }
        }),
      { once: true },
    );
  }

  /**
   * A right-click pastes, as in PuTTY, or copies the selection when selecting
   * does not. Never the web view's own menu, with its emoji and writing direction.
   */
  function onContextMenu(event: MouseEvent) {
    event.preventDefault();
    // A program that reads the mouse, such as htop, gets the click instead.
    if (!settings.mouse.right_click_pastes || term.modes.mouseTrackingMode !== "none") {
      return;
    }
    if (term.hasSelection() && !settings.mouse.select_copies) {
      void actions.copy();
      term.clearSelection();
    } else {
      void actions.paste();
    }
  }

  /**
   * Scroll lock: the line at the top of the screen, which the screen keeps to
   * while lines come below; and whether the screen was at the bottom, where
   * xterm.js follows new lines itself.
   */
  let anchor: IMarker | null = null;
  let wasAtBottom = true;

  /** Keeps the screen where it is from now on, or goes to the newest line and follows it again. */
  function setLock(on: boolean) {
    if (on) {
      anchorHere();
    } else if (anchor) {
      anchor.dispose();
      anchor = null;
      term.scrollToBottom();
    }
  }

  /** Keeps the screen to the line now at its top. */
  function anchorHere() {
    anchor?.dispose();
    const buffer = term.buffer.active;
    anchor = term.registerMarker(buffer.viewportY - buffer.baseY - buffer.cursorY);
    wasAtBottom = buffer.viewportY === buffer.baseY;
  }

  // Moved with the wheel, the scrollbar or the keys, the screen keeps to where it
  // is now. Not when xterm.js follows new lines at the bottom: it goes back once they are written.
  term.onScroll(() => {
    if (!anchor) {
      return;
    }
    const buffer = term.buffer.active;
    const atBottom = buffer.viewportY === buffer.baseY;
    if (!(atBottom && wasAtBottom) && buffer.viewportY !== anchor.line) {
      anchorHere();
    }
    wasAtBottom = atBottom;
  });

  term.onWriteParsed(() => {
    if (!anchor) {
      return;
    }
    if (anchor.isDisposed) {
      // Its line went out of the scrollback, or Clear took it: the screen keeps to where it is now.
      anchorHere();
    }
    if (term.buffer.active.viewportY !== anchor.line) {
      term.scrollToLine(anchor.line);
    }
  });

  const actions: TerminalActions = {
    hasSelection: () => term.hasSelection(),
    copy: async () => {
      const text = term.getSelection();
      if (text) {
        await writeText(text);
      }
    },
    paste: async () => {
      try {
        term.paste(await readText());
      } catch {
        // The clipboard is empty or holds no text.
      }
    },
    selectAll: () => term.selectAll(),
    clear: () => (tab ? tab.clearScreen() : term.clear()),
    text: () => {
      const buffer = term.buffer.active;
      const lines: string[] = [];
      for (let row = 0; row < buffer.length; row++) {
        const line = buffer.getLine(row);
        const text = line?.translateToString(true) ?? "";
        // A line the terminal wrapped goes on the line before it.
        if (line?.isWrapped && lines.length > 0) {
          lines[lines.length - 1] += text;
        } else {
          lines.push(text);
        }
      }
      while (lines.length > 0 && lines[lines.length - 1] === "") {
        lines.pop();
      }
      return `${lines.join("\n")}\n`;
    },
    focus: () => term.focus(),
    openSearch: () => {
      const selected = term.getSelection().split("\n")[0];
      if (selected) {
        query = selected;
      }
      searchOpen = true;
      // Focus once the bar is on screen.
      queueMicrotask(() => {
        searchInput?.focus();
        searchInput?.select();
        find("next", true);
      });
    },
    findNext: () => find("next"),
    findPrevious: () => find("previous"),
  };

  /** Finds the query: the next match, the previous one, or again from here as it is typed. */
  function find(direction: "next" | "previous", incremental = false) {
    if (!query) {
      search.clearDecorations();
      results = null;
      return;
    }
    const colors = searchColors[appScheme()];
    const options = {
      caseSensitive,
      wholeWord,
      regex,
      incremental,
      decorations: {
        matchBackground: colors.match,
        matchOverviewRuler: colors.match,
        activeMatchBackground: colors.current,
        activeMatchColorOverviewRuler: colors.current,
      },
    };
    try {
      badRegex = false;
      const hit =
        direction === "next" ? search.findNext(query, options) : search.findPrevious(query, options);
      if (!hit) {
        results = { index: 0, count: 0 };
      }
    } catch {
      badRegex = true;
      results = null;
    }
  }

  function closeSearch() {
    searchOpen = false;
    search.clearDecorations();
    results = null;
    term.focus();
  }

  function onSearchKey(event: KeyboardEvent) {
    if (event.key === "Enter") {
      event.preventDefault();
      find(event.shiftKey ? "previous" : "next");
    } else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      closeSearch();
    }
  }

  /** Turns a search option on or off, and searches again with it. */
  function toggle(set: () => void) {
    set();
    find("next", true);
    searchInput?.focus();
  }

  $effect(() => {
    term.options.theme = terminalThemes[appScheme()];
  });
  $effect(() => {
    const locked = tab?.scrollLock ?? false;
    if (opened) {
      untrack(() => setLock(locked));
    }
  });
  $effect(() => {
    term.options.fontSize = settings.font_size;
    if (opened) {
      fit.fit();
    }
  });
  $effect(() => {
    // The hex view writes its own line ends.
    term.options.convertEol = !hex && (tab?.terminal.lf_is_newline ?? false);
  });
  // The terminal on screen is the one the menus act on, and has the keyboard.
  $effect(() => {
    if (active && opened) {
      setActiveTerminal(actions);
      fit.fit();
      term.focus();
    }
  });

  onMount(() => {
    const resize = new ResizeObserver(() => fit.fit());
    let closed = false;

    async function start() {
      // xterm.js measures the font when it opens, so the font has to be loaded first.
      try {
        await Promise.all([
          document.fonts.load(`${settings.font_size}px "JetBrains Mono"`),
          document.fonts.load(`bold ${settings.font_size}px "JetBrains Mono"`),
        ]);
      } catch {
        // The fallback monospace font still works.
      }
      if (closed) {
        return;
      }

      term.open(container);
      // Before xterm.js's own handlers, which may keep the events to themselves.
      container.addEventListener("mousedown", onMouseDown, true);
      container.addEventListener("contextmenu", onContextMenu, true);
      loadWebgl();
      fit.fit();
      resize.observe(container);
      opened = true;

      if (tab && hex) {
        tab.attachHex(term);
      } else if (tab) {
        tab.attach(term);
      } else {
        // The logo's colors are picked for the theme in use when the tab opens.
        term.write(await invoke<string>("welcome_banner", { background: appScheme() }));
      }
    }

    void start();

    return () => {
      closed = true;
      if (hex) {
        tab?.attachHex(null);
      } else {
        tab?.attach(null);
      }
      clearActiveTerminal(actions);
      resize.disconnect();
      term.dispose();
    };
  });

  /** Draws with WebGL where the web view has it, and keeps the DOM renderer where not. */
  function loadWebgl() {
    try {
      const webgl = new WebglAddon();
      webgl.onContextLoss(() => webgl.dispose());
      term.loadAddon(webgl);
    } catch {
      // No WebGL, as on some Linux GPUs: xterm.js keeps drawing with the DOM.
    }
  }
</script>

<div class="wrap">
  <div class="terminal" bind:this={container}></div>

  {#if tab?.scrollLock}
    <!-- Scroll lock keeps the screen where it is: a way back to the newest line, as the Zephyr log view has. -->
    <button
      class="btn latest"
      title="Turn Scroll lock off, go to the newest line and follow it again"
      onclick={() => {
        tab?.setScrollLock(false);
        term.focus();
      }}
    >
      Latest ↓
    </button>
  {/if}

  {#if searchOpen}
    <div class="search" role="search">
      <input
        class="field"
        placeholder="Find"
        spellcheck="false"
        bind:this={searchInput}
        bind:value={query}
        oninput={() => find("next", true)}
        onkeydown={onSearchKey}
      />
      <button
        class="option"
        class:on={caseSensitive}
        title="Match case"
        aria-pressed={caseSensitive}
        onclick={() => toggle(() => (caseSensitive = !caseSensitive))}
      >
        Aa
      </button>
      <button
        class="option"
        class:on={wholeWord}
        title="Whole word"
        aria-pressed={wholeWord}
        onclick={() => toggle(() => (wholeWord = !wholeWord))}
      >
        W
      </button>
      <button
        class="option mono"
        class:on={regex}
        title="Regular expression"
        aria-pressed={regex}
        onclick={() => toggle(() => (regex = !regex))}
      >
        .*
      </button>
      <span class="count" class:none={results?.count === 0 || badRegex}>
        {#if badRegex}
          bad regex
        {:else if results && results.count === 0}
          no matches
        {:else if results && results.index > 0}
          {results.index} of {results.count}
        {:else if results}
          {results.count} found
        {/if}
      </span>
      <button class="option" title="Previous match (Shift+Enter)" onclick={() => find("previous")}>↑</button>
      <button class="option" title="Next match (Enter)" onclick={() => find("next")}>↓</button>
      <button class="option" title="Close (Escape)" onclick={closeSearch}>×</button>
    </div>
  {/if}
</div>

<style>
  .wrap {
    position: relative;
    width: 100%;
    height: 100%;
  }

  .terminal {
    width: 100%;
    height: 100%;
  }

  /* The same place as the Zephyr log view's way back to the newest line. */
  .latest {
    position: absolute;
    right: 24px;
    bottom: 12px;
    z-index: 5;
    height: 28px;
    padding: 0 12px;
    font-size: 12px;
    box-shadow: var(--shadow);
  }

  .search {
    position: absolute;
    top: 0;
    right: 16px;
    z-index: 5;
    display: flex;
    /* In a narrow pane of a split view, the text box gives up room first. */
    max-width: calc(100% - 16px);
    align-items: center;
    gap: 2px;
    padding: 4px;
    background: var(--menu-bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: var(--shadow);
  }

  .search .field {
    width: 220px;
    min-width: 0;
    height: 26px;
    margin-right: 4px;
    font-family: var(--font-mono);
    font-size: 13px;
  }

  .option {
    flex-shrink: 0;
    min-width: 26px;
    height: 26px;
    padding: 0 6px;
    font: inherit;
    font-size: 12px;
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

  .count {
    flex-shrink: 0;
    min-width: 76px;
    padding: 0 6px;
    font-size: 12px;
    color: var(--muted);
    text-align: center;
    white-space: nowrap;
  }

  .count.none {
    color: var(--bad);
  }
</style>
