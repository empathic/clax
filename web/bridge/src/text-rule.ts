// The comments contract's rule for text a page writes (comments.d.ts), shared
// by the page side (bridge) and the shell, which checks it again because a
// page can post capability calls without the bridge.

/** Most bytes of UTF-8 in a page-written comment. */
export const MAX_TEXT_BYTES = 4096;

/** Why `text` breaks the rule (non-blank, at most [`MAX_TEXT_BYTES`] of UTF-8,
 * no control characters but newline and tab), or null when it passes. */
export function textProblem(text: unknown): string | null {
  if (typeof text !== "string" || !text.trim()) return "text is a non-empty string";
  if (new TextEncoder().encode(text).length > MAX_TEXT_BYTES) return `text is at most ${MAX_TEXT_BYTES} bytes as UTF-8`;
  for (const c of text) {
    const n = c.codePointAt(0) ?? 0;
    if ((n < 0x20 && c !== "\n" && c !== "\t") || (n >= 0x7f && n <= 0x9f)) return "text has control characters other than newlines and tabs";
  }
  return null;
}

/** Most bytes of UTF-8 in a page-invented custom anchor name. */
export const MAX_NAME_BYTES = 128;

/** Why `name` is not a custom anchor name (non-empty, at most
 * [`MAX_NAME_BYTES`] of UTF-8, no control or invisible characters), or null. */
export function nameProblem(name: unknown): string | null {
  if (typeof name !== "string" || !name || new TextEncoder().encode(name).length > MAX_NAME_BYTES || /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u.test(name)) {
    return `an anchor is a non-empty name of at most ${MAX_NAME_BYTES} bytes of UTF-8 without control or invisible characters`;
  }
  return null;
}
