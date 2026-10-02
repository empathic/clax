import { ApiError } from "./api";

export const SEND_FAILED = "Could not send to the agent";
export const RESOLVE_FAILED = "Could not resolve";
export const POST_FAILED = "Could not post";
export const NAME_FAILED = "Could not save your name";
export const NAME_LOAD_FAILED = "Could not load your name";
export const LOAD_FAILED = "Could not load comments";
export const OPEN_FAILED = "Could not open";
export const SHEET_FAILED = "Could not open the keyboard shortcuts";
/** What the shell says when a lazy part of the bridge could not load in the page. */
export const PART_FAILED: Record<"comment" | "clip" | "caps", string> = {
  comment: "Comment mode could not load in this page",
  clip: "Screenshots could not load in this page",
  caps: "This page's capabilities could not load",
};

/** `<prefix>: <message>` for an API error, a network failure, or anything thrown. */
export function failureText(prefix: string, e: unknown): string {
  const message = e instanceof ApiError || e instanceof Error ? e.message : String(e);
  return `${prefix}: ${message}`;
}

/** Awaits `p`. On success clears the notice and returns the value; on failure
 * shows `failureText(prefix, e)` and returns `undefined`. Nothing is swallowed. */
export async function report<T>(p: Promise<T>, prefix: string, setNotice: (text: string | null) => void): Promise<T | undefined> {
  try {
    const v = await p;
    setNotice(null);
    return v;
  } catch (e) {
    setNotice(failureText(prefix, e));
    return undefined;
  }
}

/** A notice setter for one kind of call: a failure text always shows, while a
 * success (`null`) clears the notice only when one of `prefixes` raised it, so
 * an unrelated success never hides a failure. `set` takes a value or updater. */
export function scopedNotice(set: (u: string | null | ((prev: string | null) => string | null)) => void, ...prefixes: string[]): (text: string | null) => void {
  return text => set(cur => (text !== null ? text : cur !== null && prefixes.some(p => cur.startsWith(`${p}:`)) ? null : cur));
}
