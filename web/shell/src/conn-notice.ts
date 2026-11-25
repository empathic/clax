// The page's notice that it cannot reach Clax: shown while any reason holds
// it (the event stream is down, a request found no Clax or is stuck, a part
// could not load), with the words of the weightiest reason (an answer that
// is not Clax's, then no answer at all, then the stream alone; the latest
// among equals), and gone once none does. The page keeps retrying
// meanwhile; the notice only says so, and while a box holds text the person
// has not sent, that it is kept.
import { REASON, type Trouble } from "./trouble";

export const STREAM_DOWN = "Live updates paused. Reconnecting…";
export const REQUEST_STUCK = "Clax is not answering. Retrying…";
export const STILL_WAITING = "Clax is not answering yet. Still waiting…";
/** The reason after a prefix (trouble.ts `REASON.not_clax`), as a sentence of its own. */
export const NOT_CLAX = `T${REASON.not_clax!.slice(1)}`;
export const OFFLINE = "You are offline. Clax will catch up when you are back.";
export const UNREACHABLE = "Cannot reach Clax. Retrying…";
export const UNREACHABLE_HERE = "Clax is not answering on this computer. Retrying…";
export const PART_UNLOADED = "Part of Clax could not load. Reload the page to try again.";
export const KEPT = "What you wrote is kept.";

/** Whether `host` is this computer's, where a local daemon runs. */
const local = (host: string): boolean => /^(localhost|127\.0\.0\.1|\[::1\])$|\.localhost$/.test(host);

/** The notice's words for a request that found no Clax (`not_clax`,
 * `offline`, `unreachable`), and their weight against other reasons;
 * `host` tells a local daemon from a hosted one. */
export function noticeFor(t: Trouble, host = location.hostname): { text: string; rank: number } {
  if (t === "not_clax") return { text: NOT_CLAX, rank: 3 };
  return { text: t === "offline" ? OFFLINE : local(host) ? UNREACHABLE_HERE : UNREACHABLE, rank: 2 };
}

const held = new Map<string, { text: string; rank: number }>();
const kept = new Set<string>();
let el: HTMLElement | null = null;
let lastDoc: Document | null = null;

function render(doc: Document = lastDoc ?? document): void {
  lastDoc = doc;
  let top: { text: string; rank: number } | null = null;
  for (const h of held.values()) if (!top || h.rank >= top.rank) top = h;
  if (top === null) {
    if (el) el.hidden = true;
    return;
  }
  if (!el || el.ownerDocument !== doc || !el.isConnected) {
    el = doc.createElement("div");
    el.className = "conn-notice";
    el.setAttribute("role", "status");
    el.setAttribute("aria-live", "polite");
    doc.body.append(el);
  }
  const text = kept.size ? `${top.text} ${KEPT}` : top.text;
  if (el.textContent !== text) el.textContent = text;
  el.toggleAttribute("data-warn", top.rank >= 3);
  el.hidden = false;
}

/** Holds the notice for `reason` with `text` and its weight `rank` (`on`), or lets it go. */
export function connTrouble(reason: string, on: boolean, text: string, doc: Document = document, rank = 0): void {
  if (on) {
    held.delete(reason);
    held.set(reason, { text, rank });
  } else if (!held.delete(reason)) return;
  render(doc);
}

/** Box `key` holds text the person has not sent (`on`), or no longer does. */
export function connKept(key: string, on: boolean): void {
  if (on === kept.has(key)) return;
  if (on) kept.add(key); else kept.delete(key);
  if (el && !el.hidden) render();
}

/** A part the person asked for could not load: the notice says to reload
 * the page, until it is reloaded. Its file may be gone (a deploy or an
 * upgrade replaces them), and a browser may keep a failed import, so
 * asking again need not help. */
export function partFailed(): void {
  connTrouble("part", true, PART_UNLOADED, document, 1);
}

/** Whether the notice is showing (tests). */
export function connNoticeText(): string | null {
  return el && !el.hidden ? el.textContent : null;
}
