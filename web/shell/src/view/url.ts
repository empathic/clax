/** A fragment the frame may report or a link may carry: "" or `#…`, at most 512 characters. */
export const validHash = (h: unknown): h is string => typeof h === "string" && (h === "" || h.startsWith("#")) && h.length <= 512;

/** Replaces (or, with `push`, adds) the shell's history entry for `url`;
 * false when the browser refuses (Safari and Firefox throw a SecurityError
 * past their rate limits), leaving the address bar as it was. */
export function setUrl(url: string, push = false): boolean {
  try {
    if (push) history.pushState(null, "", url);
    else history.replaceState(history.state, "", url);
    return true;
  } catch {
    return false;
  }
}
