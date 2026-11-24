// The shape each gallery page last showed in this browser, so the next
// load's skeleton takes it before the first paint (`gallery-frame.ts`): the
// inline `clax-shape` script (`shell/index.html`) reads it where the page
// carries no bootstrap block (the local owner's every load, and any load
// through history), and for the heights the block cannot say. Kept in
// `localStorage` under `SHAPE_KEY`, one entry per page (`topOf`'s path),
// as the artifact page keeps its drawer: small, validated on the way in
// and out, and never in the way (storage that throws keeps nothing).
//
// An entry holds the window's width it was measured at (`w`: its heights
// apply at that width only), the header links' width (`links`), the
// heights the usage banner, the inbox summary and the tag bar took with
// their margins (`ban`, `sum`, `tb`: 0 when not shown), and, on the
// gallery's list, its cards: under Needs your eyes (`need`) and in the
// rest (`rest`), each counting the New tile where it leads that group,
// the height the needs group took without its margin (`nh`: its cards'
// heights follow their titles, so the rest group starts where it did),
// whether the tile showed (`tile`), and whether the gallery was empty.
// Who the browser is matters to none of it beyond a count, and signing
// out of a hosted Clax forgets it all (`hosted/signout.ts`).

/** Where the shapes are kept. */
export const SHAPE_KEY = "clax.shape";

export type Shape = {
  w: number;
  links: number;
  ban: number;
  sum: number;
  tb: number;
  need?: number;
  nh?: number;
  rest?: number;
  tile?: number;
  empty?: boolean;
};

const NUMBERS = ["w", "links", "ban", "sum", "tb", "need", "nh", "rest", "tile"] as const;
const PAGE = /^\/[a-z]*$/;

/** `s` with only its finite, non-negative numbers and a boolean `empty`; null when no `w`. */
export function cleanShape(s: unknown): Shape | null {
  if (!s || typeof s !== "object") return null;
  const out: Record<string, number | boolean> = {};
  for (const k of NUMBERS) {
    const v = (s as Record<string, unknown>)[k];
    if (typeof v === "number" && Number.isFinite(v) && v >= 0) out[k] = v;
  }
  if (typeof (s as Shape).empty === "boolean") out.empty = (s as Shape).empty!;
  if (!out.w) return null;
  return { links: 0, ban: 0, sum: 0, tb: 0, ...out } as Shape;
}

function readAll(store: Storage): Record<string, Shape> {
  try {
    const all = JSON.parse(store.getItem(SHAPE_KEY) ?? "{}") as unknown;
    if (!all || typeof all !== "object") return {};
    const out: Record<string, Shape> = {};
    for (const [k, v] of Object.entries(all)) {
      const s = PAGE.test(k) ? cleanShape(v) : null;
      if (s) out[k] = s;
    }
    return out;
  } catch {
    return {};
  }
}

/** The shape kept for the page at `path` (a `topOf` path), or null. */
export function keptShape(path: string, store: Storage = localStorage): Shape | null {
  return readAll(store)[path] ?? null;
}

/** Keeps `s` as the shape of the page at `path`. */
export function keepShape(path: string, s: Shape, store: Storage = localStorage): void {
  const clean = cleanShape(s);
  if (!clean || !PAGE.test(path)) return;
  try {
    store.setItem(SHAPE_KEY, JSON.stringify({ ...readAll(store), [path]: clean }));
  } catch { /* not kept */ }
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

/** The cards the gallery's list showed: under Needs your eyes, in the rest, the New tile, and none at all. */
export type Cards = { need: number; rest: number; tile: boolean; empty: boolean };

/** The shape of the page now in `doc` (the header's links, and in `main`
 * the banner, the summary and the tag bar), with the list's `cards`. */
export function measureShape(doc: Document, cards?: Cards): Shape {
  const main = doc.querySelector("main.gal");
  const s: Shape = {
    w: doc.defaultView?.innerWidth ?? 0,
    links: doc.querySelector(".gbar > .glinks")?.getBoundingClientRect().width ?? 0,
    ban: outerHeight(main?.querySelector(":scope > .bbanner") ?? null),
    sum: outerHeight(main?.querySelector(":scope > .inbox-sum:not([hidden])") ?? null),
    tb: outerHeight(main?.querySelector(":scope > .tagbar") ?? null),
  };
  if (!cards) return s;
  const tile = cards.tile ? 1 : 0;
  const nh = main?.querySelector(":scope > .grp.needs")?.getBoundingClientRect().height ?? 0;
  return { ...s, need: cards.need ? cards.need + tile : 0, nh, rest: cards.rest + (cards.need ? 0 : tile), tile, empty: cards.empty };
}
