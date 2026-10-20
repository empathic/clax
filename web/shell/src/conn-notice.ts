// The page's quiet notice that it cannot reach the daemon: shown while any
// reason holds it (the event stream is down, a request is stuck), with the
// latest reason's words, and gone once none does. The page keeps retrying
// meanwhile; the notice only says so.

export const STREAM_DOWN = "Live updates paused. Reconnecting…";
export const REQUEST_STUCK = "Clax is not answering. Retrying…";

const held = new Map<string, string>();
let el: HTMLElement | null = null;

function render(doc: Document): void {
  const text = [...held.values()].at(-1) ?? null;
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

/** Whether the notice is showing (tests). */
export function connNoticeText(): string | null {
  return el && !el.hidden ? el.textContent : null;
}
