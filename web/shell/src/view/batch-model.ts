// Batch send in the sidebar (spec §8): which cards can be ticked, range
// selection, pruning, and the words shown. Framework-free.
import type { Thread } from "../threads";

export type Selection = { ids: string[]; anchor: string | null };
export const EMPTY_SELECTION: Selection = { ids: [], anchor: null };
export const selectable = (t: Thread, deleted: boolean) => !deleted && t.status === "open";

/** Ticks or unticks `id`. With `shift` and an anchor, every ID in `order`
 * between the anchor and `id` takes `id`'s new state. `id` becomes the anchor. */
export function toggle(sel: Selection, id: string, shift: boolean, order: string[]): Selection {
  const on = !sel.ids.includes(id);
  const a = sel.anchor === null ? -1 : order.indexOf(sel.anchor);
  const b = order.indexOf(id);
  const range = shift && a >= 0 && b >= 0 ? order.slice(Math.min(a, b), Math.max(a, b) + 1) : [id];
  const rest = sel.ids.filter(x => !range.includes(x));
  return { ids: on ? [...rest, ...range] : rest, anchor: id };
}

/** Keeps only IDs of threads still selectable; the same object when nothing changed. */
export function prune(sel: Selection, threads: Thread[], deleted: boolean): Selection {
  const live = new Set(threads.filter(t => selectable(t, deleted)).map(t => t.id));
  const ids = sel.ids.filter(id => live.has(id));
  const anchor = sel.anchor !== null && live.has(sel.anchor) && ids.includes(sel.anchor) ? sel.anchor : null;
  return ids.length === sel.ids.length && anchor === sel.anchor ? sel : ids.length ? { ids, anchor } : EMPTY_SELECTION;
}

export const unsent = (threads: Thread[]) => threads.filter(t => t.status === "open" && !t.sent_to_agent);
export const countLabel = (n: number) => `${n} selected`;
export const unsentLabel = (n: number, agent: string) => `Send ${n} unsent to ${agent}`;
export const sendLabel = (n: number, agent: string) => (n > 1 ? `Send ${n} to ${agent}` : `Send to ${agent}`);
