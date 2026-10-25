<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import { SearchAddon } from "@xterm/addon-search";
  import { Unicode11Addon } from "@xterm/addon-unicode11";
  import { WebLinksAddon } from "@xterm/addon-web-links";
  import { WebglAddon } from "@xterm/addon-webgl";
  import { settings } from "./settings.svelte";
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
  term.attachCustomKeyEventHandler((event) => !isAppKey(event));

  term.onData((data) => {
    if (tab) {
      tab.type(data);
    } else {
      // The welcome tab has nothing to send to, so it echoes to show the terminal works.
      term.write(data.replace(/\r/g, "\r\n").replace(/\x7f/g, "\b \b"));
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
    clear: () => term.clear(),
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
