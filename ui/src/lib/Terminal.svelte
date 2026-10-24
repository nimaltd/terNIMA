<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import { Unicode11Addon } from "@xterm/addon-unicode11";
  import { WebLinksAddon } from "@xterm/addon-web-links";
  import { WebglAddon } from "@xterm/addon-webgl";
  import { settings } from "./settings.svelte";
  import type { Tab } from "./tabs.svelte";
  import { setActiveTerminal, type TerminalActions } from "./terminal-actions";
  import { appScheme, terminalThemes } from "./theme.svelte";

  let {
    tab = null,
    active,
    isAppKey,
  }: {
    /** The session this terminal shows, or null for the welcome tab. */
    tab?: Tab | null;
    /** Whether this terminal is the one on screen. */
    active: boolean;
    /** Says which keys belong to the app, so they are not sent to the device. */
    isAppKey: (event: KeyboardEvent) => boolean;
  } = $props();

  let container: HTMLDivElement;
  let opened = $state(false);

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
    focus: () => term.focus(),
  };

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
    term.options.convertEol = tab?.terminal.lf_is_newline ?? false;
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

      if (tab) {
        tab.attach(term);
      } else {
        // The logo's colors are picked for the theme in use when the tab opens.
        term.write(await invoke<string>("welcome_banner", { background: appScheme() }));
      }
    }

    void start();

    return () => {
      closed = true;
      tab?.attach(null);
      if (active) {
        setActiveTerminal(null);
      }
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

<div class="terminal" bind:this={container}></div>

<style>
  .terminal {
    width: 100%;
    height: 100%;
  }
</style>
