// The hex view's text: every byte sent and received, with the direction, the
// offset and the bytes as characters.
//
//   RX 00000000  48 65 6C 6C 6F 0D 0A 75  61 72 74 3A 7E 24 20 1B  Hello..uart:~$ .
//   TX 00000000  68 65 6C 70 0D                                     help.
//
// It only ever adds to the end, so it is right in a narrow pane where long
// lines wrap. A line ends when it is full, when the direction changes, and,
// as the tab's settings say, after a pause or after a chosen byte, so each
// packet can start on a line of its own.

import type { HexView } from "./serial";

const RESET = "\x1b[0m";
const DIM = "\x1b[2m";
const RX_TAG = `\x1b[32mRX${RESET}`;
const TX_TAG = `\x1b[33mTX${RESET}`;

const HEX = Array.from({ length: 256 }, (_, byte) => byte.toString(16).toUpperCase().padStart(2, "0"));

export class HexDump {
  private readonly width: number;
  private readonly pauseMs: number;
  private readonly endByte: number | null;
  /** Whether a line is started and not finished yet. */
  private open = false;
  private tx = false;
  /** The bytes on the line so far. */
  private line: number[] = [];
  /** Bytes so far in each direction, for the offsets. */
  private received = 0;
  private sent = 0;
  private lastAt = 0;

  /** `pauseMs` is the tab's pause that starts a new line, or 0 for none. */
  constructor(view: HexView, pauseMs: number) {
    this.width = view.width;
    this.pauseMs = pauseMs;
    this.endByte = view.end_byte ?? null;
  }

  /** Takes bytes that were received or sent at `at` (ms), and gives the text to add. */
  feed(tx: boolean, at: number, bytes: Uint8Array): string {
    let out = "";
    const paused = this.pauseMs > 0 && at - this.lastAt >= this.pauseMs;
    if (this.open && (tx !== this.tx || paused)) {
      out += this.finish();
    }
    for (const byte of bytes) {
      if (!this.open) {
        out += this.start(tx);
      }
      this.line.push(byte);
      const count = this.line.length;
      // A wider space after every eighth byte, inside the line.
      out += count % 8 === 0 && count < this.width ? `${HEX[byte]}  ` : `${HEX[byte]} `;
      if (tx) {
        this.sent++;
      } else {
        this.received++;
      }
      if (count === this.width || byte === this.endByte) {
        out += this.finish();
      }
    }
    this.lastAt = at;
    return out;
  }

  /** Finishes the open line, if there is one: the bytes as characters, then a new line. */
  finish(): string {
    if (!this.open) {
      return "";
    }
    const count = this.line.length;
    const pad = this.columns(this.width) - this.columns(count);
    const text = this.line.map((byte) => (byte >= 0x20 && byte < 0x7f ? String.fromCharCode(byte) : ".")).join("");
    this.open = false;
    this.line = [];
    return `${" ".repeat(pad)} ${text}\r\n`;
  }

  /** How wide `count` bytes are written: three columns each, and the wider spaces. */
  private columns(count: number): number {
    const gaps = Math.min(Math.floor(count / 8), Math.max(0, Math.ceil(this.width / 8) - 1));
    return count * 3 + gaps;
  }

  private start(tx: boolean): string {
    this.open = true;
    this.tx = tx;
    const offset = (tx ? this.sent : this.received).toString(16).toUpperCase().padStart(8, "0");
    return `${tx ? TX_TAG : RX_TAG} ${DIM}${offset}${RESET}  `;
  }
}
