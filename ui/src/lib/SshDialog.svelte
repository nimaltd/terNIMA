<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";
  import Modal from "./Modal.svelte";
  import { DEFAULT_TERMINAL } from "./serial";
  import { settings } from "./settings.svelte";
  import { DEFAULT_SSH, SSH_AUTH_HINTS, SSH_PORT, type SshAuth, type SshConfig } from "./ssh";
  import { addTab, changed, type Tab } from "./tabs.svelte";

  let {
    tab = null,
    onclose,
  }: {
    /** The SSH tab whose settings to change, or null for a new session. */
    tab?: Tab | null;
    onclose: () => void;
  } = $props();

  // A copy of the tab's settings, taken once: nothing changes until Save.
  const start = untrack(() => tab?.setup() ?? null);
  const first: SshConfig = untrack(() => start?.ssh ?? { ...DEFAULT_SSH });
  let host = $state(first.host);
  let portText = $state(String(first.port || SSH_PORT));
  let user = $state(first.user);
  let auth: SshAuth = $state(first.auth);
  let keyFile = $state(first.key_file ?? "");
  let name = $state(start?.name ?? "");
  let error = $state("");
  let hostField: HTMLInputElement | undefined = $state();

  // A new session starts at the host, with the key OpenSSH would use first, if there is one.
  onMount(() => {
    if (!tab) {
      hostField?.focus();
      void invoke<string | null>("usual_ssh_key").then((usual) => {
        if (usual && !keyFile) {
          keyFile = usual;
          auth = "key";
        }
      });
    }
  });

  /** The tab's name when none is typed: "root@board", with a port other than the usual one. */
  const suggested = $derived.by(() => {
    const at = host.trim() || "board";
    const port = Number(portText);
    const where = Number.isInteger(port) && port > 0 && port !== SSH_PORT ? `${at}:${port}` : at;
    return user.trim() ? `${user.trim()}@${where}` : where;
  });

  async function browse() {
    const path = await open({
      title: "The private key file",
      multiple: false,
      directory: false,
      defaultPath: keyFile.trim() || undefined,
    });
    if (typeof path === "string") {
      keyFile = path;
      error = "";
    }
  }

  function submit() {
    const port = Number(portText);
    if (!host.trim()) {
      error = "Enter the host: a name or an IP address.";
      return;
    }
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      error = "Enter the port: a whole number from 1 to 65535, usually 22.";
      return;
    }
    if (!user.trim()) {
      error = "Enter the user to log in as, such as root.";
      return;
    }
    if (auth === "key" && !keyFile.trim()) {
      error = "Choose the private key file to log in with.";
      return;
    }
    const ssh: SshConfig = { ...first, host: host.trim(), port, user: user.trim(), auth, key_file: keyFile.trim() };
    const sessionName = name.trim() || suggested;

    if (tab) {
      tab.name = sessionName;
      if (JSON.stringify(ssh) !== JSON.stringify(start?.ssh)) {
        tab.reconfigureSsh(ssh);
      }
      changed();
    } else {
      addTab({
        name: sessionName,
        kind: "ssh",
        connected: false,
        log: settings.log.new_tabs,
        ssh,
        terminal: { ...DEFAULT_TERMINAL },
      }).start();
    }
    onclose();
  }
</script>

<Modal title={tab ? "SSH settings" : "New SSH session"} {onclose} onsubmit={submit}>
  <h2 class="dlg-section">Connection</h2>
  <div class="dlg-grid">
    <label class="dlg-label">
      Host
      <input
        class="field mono"
        spellcheck="false"
        placeholder="board.local"
        bind:this={hostField}
        bind:value={host}
        oninput={() => (error = "")}
      />
    </label>
    <label class="dlg-label">
      Port
      <input class="field mono" inputmode="numeric" bind:value={portText} oninput={() => (error = "")} />
    </label>
    <label class="dlg-label">
      User
      <input
        class="field mono"
        spellcheck="false"
        placeholder="root"
        bind:value={user}
        oninput={() => (error = "")}
      />
    </label>
  </div>

  <h2 class="dlg-section">Log in with</h2>
  <div class="dlg-grid">
    <label class="dlg-label wide">
      <select
        class="field"
        aria-label="Log in with"
        bind:value={auth}
        onchange={() => (error = "")}
      >
        <option value="key">A key file</option>
        <option value="agent">The SSH agent</option>
        <option value="password">A password, asked each time and never saved</option>
      </select>
    </label>
    {#if auth === "key"}
      <label class="dlg-label wide">
        Private key file
        <span class="file">
          <input
            class="field mono"
            spellcheck="false"
            placeholder="~/.ssh/id_ed25519"
            bind:value={keyFile}
            oninput={() => (error = "")}
          />
          <button class="btn" onclick={browse}>Browse...</button>
        </span>
      </label>
    {/if}
  </div>
  <p class="dlg-hint about">{SSH_AUTH_HINTS[auth]}</p>
  <p class="dlg-hint">
    The server's key is checked against known_hosts in your .ssh folder: a new one is shown and asked about first, and a
    changed one is refused.
  </p>

  <label class="dlg-label tab-name">
    Tab name
    <input class="field" bind:value={name} placeholder={suggested} />
  </label>

  {#if error}
    <p class="dlg-error">{error}</p>
  {/if}

  {#snippet buttons()}
    <button class="btn" onclick={onclose}>Cancel</button>
    <button class="btn primary" onclick={submit}>{tab ? "Save" : "Connect"}</button>
  {/snippet}
</Modal>

<style>
  .wide {
    grid-column: 1 / -1;
  }

  .about {
    margin-top: 8px;
  }

  .tab-name {
    margin-top: 14px;
  }

  .file {
    display: flex;
    gap: 8px;
  }

  .file .field {
    flex: 1;
    min-width: 0;
  }
</style>
