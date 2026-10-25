<script lang="ts">
  import { untrack } from "svelte";
  import Modal from "./Modal.svelte";
  import {
    CHECK_SIZES,
    FIELD_SIZES,
    parseHex,
    type CheckKind,
    type Decoder,
    type FieldType,
    type FrameSpec,
  } from "./frames";
  import type { Tab } from "./tabs.svelte";

  let { tab, onclose }: { tab: Tab; onclose: () => void } = $props();

  const DECODERS: { value: Decoder; label: string }[] = [
    { value: "off", label: "Off" },
    { value: "layout", label: "My layout: start bytes, a length, fields and a check" },
    { value: "cobs", label: "COBS: frames ended by a 0 byte" },
    { value: "slip", label: "SLIP: frames between C0 bytes" },
    { value: "nmea", label: "NMEA sentences from a GPS receiver" },
    { value: "modbus_rtu", label: "Modbus RTU on a bus" },
    { value: "modbus_ascii", label: "Modbus ASCII on a bus" },
  ];
  const CHECKS: { value: CheckKind; label: string }[] = [
    { value: "none", label: "None" },
    { value: "sum8", label: "Sum of the bytes (8 bit)" },
    { value: "xor8", label: "XOR of the bytes (8 bit)" },
    { value: "crc8", label: "CRC-8" },
    { value: "crc16_modbus", label: "CRC-16 Modbus" },
    { value: "crc16_ccitt", label: "CRC-16 CCITT (FFFF)" },
    { value: "crc16_xmodem", label: "CRC-16 XMODEM" },
    { value: "crc32", label: "CRC-32" },
  ];
  const TYPES: FieldType[] = ["u8", "i8", "u16", "i16", "u32", "i32", "f32", "f64"];

  // A copy of the layout: nothing changes until Save.
  let draft: FrameSpec = $state(untrack(() => structuredClone($state.snapshot(tab.frameSpec))));
  let problem: string | null = $state(null);

  const framed = $derived(draft.decoder === "layout" || draft.decoder === "cobs" || draft.decoder === "slip");
  const startBytes = $derived(parseHex(draft.start));
  /** What the layout reads from what came lately, worked out as it is changed. */
  const preview = $derived.by(() => {
    if (draft.decoder === "off" || startBytes === null) return null;
    return tab.previewFrames($state.snapshot(draft));
  });

  function setLength(on: boolean) {
    draft.length = on
      ? { offset: startBytes?.length ?? 0, size: 1, endian: "le", counts: "after", adjust: 0 }
      : null;
  }

  /** A new field, after the last one. */
  function addField() {
    const last = draft.fields.at(-1);
    const offset = last
      ? last.offset + FIELD_SIZES[last.type]
      : draft.decoder === "layout"
        ? (draft.length ? draft.length.offset + draft.length.size : (startBytes?.length ?? 0))
        : 0;
    draft.fields.push({ name: `value${draft.fields.length + 1}`, offset, type: "u16", endian: "le", scale: 1 });
  }

  function submit() {
    if (startBytes === null) {
      problem = "The start bytes are not hex. Write them like AA 55.";
      return;
    }
    const names = draft.fields.map((field) => field.name.trim());
    if (names.some((name) => name === "")) {
      problem = "Every field needs a name.";
      return;
    }
    if (new Set(names).size !== names.length) {
      problem = "Two fields have the same name.";
      return;
    }
    draft.fields.forEach((field, index) => (field.name = names[index]));
    tab.setFrameSpec($state.snapshot(draft));
    onclose();
  }

  function number(value: number): string {
    return Number.isInteger(value) ? String(value) : String(Number(value.toPrecision(7)));
  }
</script>

<Modal title="Frame layout" {onclose} onsubmit={submit}>
  <label class="dlg-label wide">
    Frames
    <select class="field" bind:value={draft.decoder}>
      {#each DECODERS as { value, label } (value)}
        <option {value}>{label}</option>
      {/each}
    </select>
  </label>

  {#if draft.decoder === "nmea"}
    <p class="dlg-hint">
      Sentences such as $GPGGA and $GPRMC are checked, and lat, lon, alt, sats, speed and course read from them,
      ready for the plot. Other sentences give their numbers by position.
    </p>
  {:else if draft.decoder === "modbus_rtu" || draft.decoder === "modbus_ascii"}
    <p class="dlg-hint">
      Requests and answers are found on the bus and checked. A read's values are named by their address, hr0 for
      holding register 0, ir, c and di for the others, ready for the plot. To ask a device yourself, use the
      Modbus view.
    </p>
  {/if}

  {#if draft.decoder === "layout"}
    <div class="dlg-section">Where a frame is</div>
    <div class="dlg-grid">
      <label class="dlg-label">
        Start bytes, in hex
        <input class="field mono" class:bad={startBytes === null} placeholder="AA 55" spellcheck="false" bind:value={draft.start} />
      </label>
      <label class="dlg-label">
        Its length
        <select class="field" value={draft.length ? "field" : "fixed"} onchange={(event) => setLength(event.currentTarget.value === "field")}>
          <option value="fixed">Always the same</option>
          <option value="field">In a length byte or two</option>
        </select>
      </label>
      {#if draft.length}
        <label class="dlg-label">
          Length at offset
          <input class="field mono" type="number" min="0" bind:value={draft.length.offset} />
        </label>
        <label class="dlg-label">
          Length is
          <select class="field" bind:value={draft.length.size}>
            <option value={1}>1 byte</option>
            <option value={2}>2 bytes</option>
          </select>
        </label>
        {#if draft.length.size === 2}
          <label class="dlg-label">
            Byte order
            <select class="field" bind:value={draft.length.endian}>
              <option value="le">Little endian</option>
              <option value="be">Big endian</option>
            </select>
          </label>
        {/if}
        <label class="dlg-label">
          It counts
          <select class="field" bind:value={draft.length.counts}>
            <option value="after">The bytes after it</option>
            <option value="frame">The whole frame</option>
          </select>
        </label>
        <label class="dlg-label" title="Added to the length, for bytes it leaves out, such as a check">
          Plus
          <input class="field mono" type="number" bind:value={draft.length.adjust} />
        </label>
      {:else}
        <label class="dlg-label">
          Frame size, bytes
          <input class="field mono" type="number" min="1" max="4096" bind:value={draft.size} />
        </label>
      {/if}
    </div>
  {/if}

  {#if framed}
    <div class="dlg-section">Check at the end</div>
    <div class="dlg-grid">
      <label class="dlg-label">
        Check
        <select class="field" bind:value={draft.check.kind}>
          {#each CHECKS as { value, label } (value)}
            <option {value}>{label}</option>
          {/each}
        </select>
      </label>
      {#if draft.check.kind !== "none"}
        <label class="dlg-label" title="The check covers the bytes from here up to the check itself">
          Over the bytes from offset
          <input class="field mono" type="number" min="0" bind:value={draft.check.from} />
        </label>
        {#if CHECK_SIZES[draft.check.kind] > 1}
          <label class="dlg-label">
            Byte order
            <select class="field" bind:value={draft.check.endian}>
              <option value="le">Little endian</option>
              <option value="be">Big endian</option>
            </select>
          </label>
        {/if}
      {/if}
    </div>

    <div class="dlg-section">Fields</div>
    {#if draft.fields.length > 0}
      <div class="fields">
        <span class="head">Name</span>
        <span class="head">Offset</span>
        <span class="head">Type</span>
        <span class="head">Order</span>
        <span class="head" title="The value is the number in the bytes times this">Scale</span>
        <span></span>
        {#each draft.fields as field, index (index)}
          <input class="field mono" spellcheck="false" bind:value={field.name} aria-label="Name" />
          <input class="field mono" type="number" min="0" bind:value={field.offset} aria-label="Offset" />
          <select class="field" bind:value={field.type} aria-label="Type">
            {#each TYPES as type (type)}
              <option value={type}>{type}</option>
            {/each}
          </select>
          <select class="field" bind:value={field.endian} disabled={FIELD_SIZES[field.type] === 1} aria-label="Byte order">
            <option value="le">LE</option>
            <option value="be">BE</option>
          </select>
          <input class="field mono" type="number" step="any" bind:value={field.scale} aria-label="Scale" />
          <button class="icon" title="Remove" onclick={() => draft.fields.splice(index, 1)}>×</button>
        {/each}
      </div>
    {:else}
      <p class="dlg-hint">No fields yet: frames are found and checked, and their bytes shown.</p>
    {/if}
    <button class="btn add" onclick={addField}>Add a field</button>
    <p class="dlg-hint">
      Offsets count from the frame's first byte{draft.decoder === "layout" ? ", its start bytes included" : " after it is decoded"}.
    </p>
  {/if}

  {#if preview}
    <p class="preview" class:good={preview.good > 0}>
      {#if preview.frames === 0}
        No frames in what came lately with this layout.
      {:else}
        In what came lately: {preview.frames} frames, {preview.good} right.
        {#if preview.first}
          The first: {preview.first.values.map(([name, value]) => `${name} ${number(value)}`).join(", ") || "no fields"}.
        {:else if preview.problem}
          {preview.problem}.
        {/if}
      {/if}
    </p>
  {/if}

  {#if problem}
    <p class="dlg-error">{problem}</p>
  {/if}

  {#snippet buttons()}
    <button class="btn" onclick={onclose}>Cancel</button>
    <button class="btn primary" onclick={submit}>Save</button>
  {/snippet}
</Modal>

<style>
  .wide {
    margin-top: 4px;
  }

  .field.bad {
    border-color: var(--bad);
  }

  .fields {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 70px 72px 64px 80px 24px;
    gap: 6px 8px;
    align-items: center;
  }

  .fields .field {
    height: 28px;
    min-width: 0;
    font-size: 13px;
  }

  .head {
    font-size: 12px;
    color: var(--muted);
  }

  .icon {
    width: 24px;
    height: 24px;
    font: inherit;
    color: var(--muted);
    background: none;
    border: 0;
    border-radius: 4px;
    cursor: default;
  }

  .icon:hover {
    color: var(--text);
    background: var(--hover);
  }

  .add {
    align-self: flex-start;
    margin-top: 10px;
  }

  .preview {
    margin: 14px 0 0;
    padding: 8px 10px;
    font-size: 12px;
    color: var(--muted);
    background: var(--panel);
    border-radius: 6px;
  }

  .preview.good {
    color: var(--text);
  }
</style>
