<script lang="ts">
  import { onMount } from "svelte";
  import Modal from "./Modal.svelte";
  import type { SshAnswer, SshQuestion } from "./ssh";

  let {
    question,
    onanswer,
  }: {
    /** What the session asks. */
    question: SshQuestion;
    onanswer: (answer: SshAnswer) => void;
  } = $props();

  let text = $state("");
  let field: HTMLInputElement | undefined = $state();

  // After the dialog itself takes the focus, which it does as it opens.
  onMount(() => field?.focus());

  const no = () => onanswer({ answer: "no" });
</script>

{#if question.ask === "host_key"}
  <!-- Enter does not trust a server: it may be meant for the terminal this came up over. -->
  <Modal title="Trust this server?" onclose={no} onsubmit={() => {}}>
    <p class="text">
      <span class="mono">{question.address}</span> has no key in known_hosts yet, so nothing says it is the server you
      mean. Check that this is its key.
    </p>
    <dl>
      <dt>Key type</dt>
      <dd class="mono">{question.key_type}</dd>
      <dt>Fingerprint</dt>
      <dd class="mono">{question.fingerprint}</dd>
    </dl>
    {#if question.others.length > 0}
      <p class="warning">
        This server is known with another type of key: {question.others.join(", ")}. A new type can mean it was set up
        again, or that someone is in between.
      </p>
    {/if}
    <p class="dlg-hint">
      On the server, <span class="mono">ssh-keygen -lf</span> shows the fingerprint of its key files in
      <span class="mono">/etc/ssh</span>. Once trusted, the key is saved in
      <span class="mono path">{question.file}</span> and this is not asked again.
    </p>

    {#snippet buttons()}
      <button class="btn" onclick={no}>Cancel</button>
      <button class="btn primary" onclick={() => onanswer({ answer: "yes" })}>Trust and connect</button>
    {/snippet}
  </Modal>
{:else if question.ask === "key_changed"}
  <Modal title="The server's key has changed" onclose={no} onsubmit={no}>
    <p class="warning">
      terNIMA did not connect to <span class="mono">{question.address}</span>: its key is not the one saved for it.
      Someone may be in between, or the server was set up again, as a board is when it is flashed again.
    </p>
    <dl>
      <dt>Key type</dt>
      <dd class="mono">{question.key_type}</dd>
      <dt>Saved</dt>
      <dd class="mono">{question.known}</dd>
      <dt>Shown now</dt>
      <dd class="mono">{question.fingerprint}</dd>
    </dl>
    <p class="dlg-hint">
      If you know why it changed, take the old key out of <span class="mono path">{question.file}</span>, then connect
      again. This command does it:
    </p>
    <p class="command mono">{question.forget}</p>

    {#snippet buttons()}
      <button class="btn primary" onclick={no}>Close</button>
    {/snippet}
  </Modal>
{:else}
  <Modal title={question.title} onclose={no} onsubmit={() => onanswer({ answer: "text", text })}>
    <label class="dlg-label secret">
      <span class="prompt">{question.prompt}</span>
      <input
        class="field"
        class:mono={question.echo}
        type={question.echo ? "text" : "password"}
        spellcheck="false"
        autocomplete="off"
        bind:this={field}
        bind:value={text}
      />
    </label>
    {#if question.problem}
      <p class="dlg-error">{question.problem}</p>
    {/if}
    <p class="dlg-hint hint">{question.note}</p>

    {#snippet buttons()}
      <button class="btn" onclick={no}>Cancel</button>
      <button class="btn primary" onclick={() => onanswer({ answer: "text", text })}>Log in</button>
    {/snippet}
  </Modal>
{/if}

<style>
  .text,
  .warning {
    margin: 6px 0;
    line-height: 1.45;
  }

  .warning {
    color: var(--warn);
  }

  dl {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 4px 14px;
    margin: 10px 0;
    padding: 8px 10px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 8px;
  }

  dt {
    color: var(--muted);
  }

  /* To copy, and to compare with what the server shows. */
  dd,
  .command,
  .path {
    user-select: text;
    cursor: text;
  }

  dd {
    margin: 0;
    overflow-wrap: anywhere;
  }

  .path {
    overflow-wrap: anywhere;
  }

  .command {
    margin: 6px 0 0;
    padding: 6px 10px;
    overflow-wrap: anywhere;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  .secret {
    margin-top: 8px;
  }

  /* A server's question can have its instructions on lines before it. */
  .prompt {
    white-space: pre-wrap;
  }

  .hint {
    margin-top: 10px;
  }
</style>
