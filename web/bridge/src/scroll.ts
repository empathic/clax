// The page's scroll position for the app shell: reported while the page
// scrolls (`clax:scroll`), and restored when the shell loads a newer version
// of the page in its place (the welcome's `restoreScroll`). Loaded with the
// caps part on the app shell's welcome, so a page outside the app loads none
// of it.

import { type Timers, windowTimers } from "./long-press";

export const SCROLL_EVERY_MS = 250;
/** How long a restore waits for the document to grow tall enough. */
export const RESTORE_WITHIN_MS = 2000;
/** How often a waiting restore measures the document again. */
const RESTORE_EVERY_MS = 100;

/** Posts `clax:scroll` with the page's file and vertical scroll position as
 * the page scrolls: at once, then at most once every `SCROLL_EVERY_MS`, the
 * last position of a burst always posted. Returns a function that stops it. */
export function reportScroll(win: Window, post: (m: { type: "clax:scroll"; file: string; y: number }) => void, file: () => string, timers: Timers = windowTimers(win)): () => void {
  let timer: number | null = null;
  let dirty = false;
  const send = () => post({ type: "clax:scroll", file: file(), y: win.scrollY });
  const tick = () => {
    timer = null;
    if (!dirty) return;
    dirty = false;
    send();
    timer = timers.set(tick, SCROLL_EVERY_MS);
  };
  const onScroll = () => {
    if (timer !== null) { dirty = true; return; }
    send();
    timer = timers.set(tick, SCROLL_EVERY_MS);
  };
  win.addEventListener("scroll", onScroll, { passive: true });
  return () => {
    win.removeEventListener("scroll", onScroll);
    if (timer !== null) timers.clear(timer);
    timer = null;
  };
}

/** Whether this document's scroll report has started. */
let reporting = false;

/** On the app shell's welcome: starts the scroll report for the page `file`
 * of `win` (once per document, whatever the number of welcomes), and
 * restores the position `y` the welcome carries, if any. */
export function appScroll(win: Window, post: (m: { type: "clax:scroll"; file: string; y: number }) => void, file: string, y?: number): void {
  if (!reporting) reportScroll(win, post, () => file);
  reporting = true;
  if (y !== undefined) restoreScroll(win, y);
}

/** Scrolls the page to `y` once the document is tall enough to reach it,
 * measuring again every `RESTORE_EVERY_MS`; after `RESTORE_WITHIN_MS` it
 * scrolls as far towards `y` as the document then reaches. A `y` that is not
 * a finite, non-negative number is ignored. */
export function restoreScroll(win: Window, y: number, timers: Timers = windowTimers(win)): void {
  if (typeof y !== "number" || !Number.isFinite(y) || y < 0) return;
  let waited = 0;
  const attempt = () => {
    const doc = win.document;
    const reach = Math.max(0, (doc.scrollingElement ?? doc.documentElement).scrollHeight - win.innerHeight);
    if (reach >= y || waited >= RESTORE_WITHIN_MS) { win.scrollTo(0, Math.min(y, reach)); return; }
    waited += RESTORE_EVERY_MS;
    timers.set(attempt, RESTORE_EVERY_MS);
  };
  attempt();
}
