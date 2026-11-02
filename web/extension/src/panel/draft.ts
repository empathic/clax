// Whether the side panel holds text the person has not sent or saved: a
// reply being written, a name not yet saved, a merge pattern. The worker
// does not reload the extension for a new build while any panel says so
// (sw/update.ts): a reload closes the panel and the text with it.

/** Input types whose text is a person's (a search box's is not: losing it costs nothing). */
const TEXT = new Set(["text","email", "url", "tel", "number"]);

/** Whether `root` holds a text field with something typed in it. */
export function hasDraft(root: ParentNode): boolean {
  for (const el of root.querySelectorAll<HTMLElement>("textarea, input, [contenteditable]")) {
    if (el instanceof HTMLTextAreaElement) { if (el.value.trim()) return true; continue; }
    if (el instanceof HTMLInputElement) { if (TEXT.has(el.type) && el.value.trim()) return true; continue; }
    if (el.isContentEditable && el.textContent?.trim()) return true;
  }
  return false;
}

/** How often the panel looks again without an event: a field the page
 * empties itself (a reply sent) fires none. */
export const DRAFT_CHECK_MS = 3_000;

export type DraftEnv = {
  doc: Pick<Document, "addEventListener" | "removeEventListener"> & ParentNode;
  every?(ms: number, fn: () => void): () => void;
};

/** Tells `on` whether `env.doc` holds a draft, at once and each time that
 * changes: after any input, change or focus leaving a field, and every
 * DRAFT_CHECK_MS. Returns what stops it. */
export function watchDrafts(env: DraftEnv, on: (drafting: boolean) => void): () => void {
  let last: boolean | null = null;
  const look = () => {
    const now = hasDraft(env.doc);
    if (now !== last) { last = now; on(now); }
  };
  const events = ["input", "change", "focusout"] as const;
  for (const e of events) env.doc.addEventListener(e, look, true);
  const every = env.every ?? ((ms, fn) => { const h = setInterval(fn, ms); return () => clearInterval(h); });
  const stop = every(DRAFT_CHECK_MS, look);
  look();
  return () => { stop(); for (const e of events) env.doc.removeEventListener(e, look, true); };
}
