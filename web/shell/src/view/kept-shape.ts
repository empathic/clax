// The shapes the shell's pages last showed in this browser, so the next
// load's placeholders take them before the first paint: one map in
// `localStorage` under `SHAPE_KEY`, keyed by page (`/`, `/tags`, … the
// gallery's, `view/gallery-shape.ts`; `/a/<id>`, an artifact's threads,
// `view/drawer-shape.ts`), which the pages' inline scripts read too. Each
// kind cleans its own entries on the way in and out, and one kind's write
// keeps every other entry. Small, and never in the way: storage that
// throws keeps nothing, and signing out of a hosted Clax forgets it all
// (`hosted/signout.ts`).

/** Where the shapes are kept. */
export const SHAPE_KEY = "clax.shape";

/** Every kept entry, as stored (each kind cleans its own); empty where none
 * parses or storage throws. */
export function readShapes(store: Storage = localStorage): Record<string, unknown> {
  try {
    const all = JSON.parse(store.getItem(SHAPE_KEY) ?? "{}") as unknown;
    return all && typeof all === "object" && !Array.isArray(all) ? { ...(all as Record<string, unknown>) } : {};
  } catch {
    return {};
  }
}

/** Keeps `value` under `key`, as the newest entry; with `bound`, of the
 * keys it matches only the newest `max` stay. */
export function keepEntry(key: string, value: unknown, store: Storage = localStorage, bound?: { match: RegExp; max: number }): void {
  const all = readShapes(store);
  delete all[key];
  all[key] = value;
  if (bound) {
    const ks = Object.keys(all).filter(k => bound.match.test(k));
    for (const k of ks.slice(0, Math.max(0, ks.length - bound.max))) delete all[k];
  }
  try { store.setItem(SHAPE_KEY, JSON.stringify(all)); } catch { /* not kept */ }
}

/** Forgets every page's shape. */
export function forgetShapes(store: Storage = localStorage): void {
  try { store.removeItem(SHAPE_KEY); } catch { /* none kept */ }
}

/** `el`'s height with its vertical margins, 0 when it is not laid out. */
export function outerHeight(el: Element | null): number {
  if (!el) return 0;
  const h = el.getBoundingClientRect().height;
  if (!h) return 0;
  const cs = getComputedStyle(el);
  return h + (parseFloat(cs.marginTop) || 0) + (parseFloat(cs.marginBottom) || 0);
}
