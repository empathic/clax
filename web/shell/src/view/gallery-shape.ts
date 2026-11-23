// The shape each gallery page last showed in this browser, so the next
// load's skeleton takes it before the first paint (`gallery-frame.ts`): the
// inline `clax-shape` script (`shell/index.html`) reads it where the page
// carries no bootstrap block (the local owner's every load, and any load
// through history), and for the heights the block cannot say. Kept in
// `localStorage` under `SHAPE_KEY`, one entry per page (`shapeKeyOf`'s
// key: each Settings screen its own, a queue's view and a report's each
// one for every queue), in the shell's kept shapes (`kept-shape.ts`): small, validated on the way
// in and out, and never in the way (storage that throws keeps nothing).
//
// An entry holds the window's width it was measured at (`w`: its heights
// apply at that width only), the header links' width (`links`), the
// heights the usage banner, the inbox summary and the tag bar took with
// their margins (`ban`, `sum`, `tb`: 0 when not shown), on a page beside
// the gallery the height its content took in `main` (`ph`), and, on the
// gallery's list, its cards: under Needs your eyes (`need`) and in the
// rest (`rest`), each counting the New tile where it leads that group,
// the height the needs group took without its margin (`nh`: its cards'
// heights follow their titles, so the rest group starts where it did),
// whether the tile showed (`tile`), and whether the gallery was empty.
// Who the browser is matters to none of it beyond a count, and signing
// out of a hosted Clax forgets it all (`hosted/signout.ts`).

import { keepEntry, outerHeight, readShapes } from "./kept-shape";

export { SHAPE_KEY, forgetShapes, outerHeight } from "./kept-shape";

export type Shape = {
  w: number;
  links: number;
  ban: number;
  sum: number;
  tb: number;
  ph?: number;
  need?: number;
  nh?: number;
  rest?: number;
  tile?: number;
  empty?: boolean;
};

const NUMBERS = ["w", "links", "ban", "sum", "tb", "ph", "need", "nh", "rest", "tile"] as const;
const PAGE = /^\/([a-z]*|settings\/[a-z-]+|queues\/\*(\/\*)?)$/;

/** The key the page at `path` keeps its shape under: its path, with a
 * queue's view and a report's each under one key for every queue (the
 * inline `clax-shape` script reads it the same way). */
export const shapeKeyOf = (path: string): string => path.replace(/^\/queues\/[^/]+\/[^/]+$/, "/queues/*/*").replace(/^\/queues\/[^/]+$/, "/queues/*");

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

/** The shape kept under `path` (a `shapeKeyOf` key), or null. */
export function keptShape(path: string, store: Storage = localStorage): Shape | null {
  return PAGE.test(path) ? cleanShape(readShapes(store)[path]) : null;
}

/** Keeps `s` as the shape of the page at `path`. */
export function keepShape(path: string, s: Shape, store: Storage = localStorage): void {
  const clean = cleanShape(s);
  if (clean && PAGE.test(path)) keepEntry(path, clean, store);
}

/** The cards the gallery's list showed: under Needs your eyes, in the rest, the New tile, and none at all. */
export type Cards = { need: number; rest: number; tile: boolean; empty: boolean };

/** The shape of the page now in `doc` (the header's links, and in `main`
 * the banner, the summary and the tag bar, and a page beside the gallery's
 * height from `main`'s content top), with the list's `cards`. */
export function measureShape(doc: Document, cards?: Cards): Shape {
  const main = doc.querySelector("main.gal");
  // A page beside the gallery: what `main` shows in the skeleton's place
  // (the page, or the line saying this browser has no inbox).
  const shown = main && doc.documentElement.dataset.page === "page" ? [...main.children].filter(c => !c.matches(".gskel, .gpage.pending")) : [];
  const s: Shape = {
    w: doc.defaultView?.innerWidth ?? 0,
    links: doc.querySelector(".gbar > .glinks")?.getBoundingClientRect().width ?? 0,
    ban: outerHeight(main?.querySelector(":scope > .bbanner") ?? null),
    sum: outerHeight(main?.querySelector(":scope > .inbox-sum:not([hidden])") ?? null),
    tb: outerHeight(main?.querySelector(":scope > .tagbar") ?? null),
  };
  if (main && shown.length) {
    const top = main.getBoundingClientRect().top + (parseFloat(getComputedStyle(main).paddingTop) || 0);
    s.ph = Math.max(0, ...shown.map(c => c.getBoundingClientRect().bottom - top));
  }
  if (!cards) return s;
  const tile = cards.tile ? 1 : 0;
  const nh = main?.querySelector(":scope > .grp.needs")?.getBoundingClientRect().height ?? 0;
  return { ...s, need: cards.need ? cards.need + tile : 0, nh, rest: cards.rest + (cards.need ? 0 : tile), tile, empty: cards.empty };
}
