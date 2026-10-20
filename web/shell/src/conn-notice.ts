// The page's quiet notice that it cannot reach the daemon: shown while any
// reason holds it (the event stream is down, a request is stuck), with the
// latest reason's words, and gone once none does. The page keeps retrying
// meanwhile; the notice only says so. In the Clax app the host owns
// liveness and the core is in the same process, so app mode turns the
// notices off (`connNotices(false)`, `transport.ts`) and nothing shows.

export const STREAM_DOWN = "Live updates paused. Reconnecting…";
export const REQUEST_STUCK = "Clax is not answering. Retrying…";

const held = new Map<string, string>();
let el: HTMLElement | null = null;
let enabled = true;

function render(doc: Document): void {
  const text = enabled ? ([...held.values()].at(-1) ?? null) : null;
  if (text === null) {
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
  if (el.textContent !== text) el.textContent = text;
  el.hidden = false;
}

/** Holds the notice for `reason` with `text` (`on`), or lets it go. */
export function connTrouble(reason: string, on: boolean, text: string, doc: Document = document): void {
  if (on) {
    held.delete(reason);
    held.set(reason, text);
  } else if (!held.delete(reason)) return;
  render(doc);
}

/** Turns the notices on or off for this document: while off, none shows
 * whatever reasons hold. */
export function connNotices(on: boolean, doc: Document = document): void {
  enabled = on;
  render(doc);
}

/** Whether the notices are on. */
export function connNoticesOn(): boolean {
  return enabled;
}

/** Whether the notice is showing (tests). */
export function connNoticeText(): string | null {
  return el && !el.hidden ? el.textContent : null;
}
