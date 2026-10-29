// What local echo shows of what is typed in the terminal. It has no imports,
// so its tests run in Node as they are.

/**
 * What local echo shows of keys typed in the terminal: the text, Enter as a
 * new line and Backspace as a rubout. Not the escape sequences of keys such
 * as the arrows, Home or F1, nor the other control characters: drawn, they
 * move the cursor over what the device shows, as ↑ in a Linux shell did.
 */
export function echoed(typed: string): string {
  return (
    typed
      // CSI as the arrows and F5 send, SS3 as F1 and arrows in application mode, Alt with a key, or Escape alone.
      .replace(/\x1b(?:\[[0-?]*[ -/]*[@-~]|O.|.)?/gs, "")
      // Ctrl with a letter, such as Ctrl+N, which would switch the terminal to line drawing.
      .replace(/[\x00-\x07\x0b\x0c\x0e-\x1f]/g, "")
      .replace(/\r/g, "\r\n")
      .replace(/[\b\x7f]/g, "\b \b")
  );
}
