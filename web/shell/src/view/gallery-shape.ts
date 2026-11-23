// The shape each gallery page last showed in this browser, so the next
// load's skeleton takes it before the first paint (`gallery-frame.ts`): the
// inline `clax-shape` script (`shell/index.html`) reads it where the page
// carries no bootstrap block (the local owner's every load, and any load
// through history), and for the heights the block cannot say. Kept in
// `localStorage` under `SHAPE_KEY`, one entry per page (`topOf`'s path),
// in the shell's kept shapes (`kept-shape.ts`): small, validated on the way
// in and out, and never in the way (storage that throws keeps nothing).
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

import { keepEntry, outerHeight, readShapes } from "./kept-shape";

export { SHAPE_KEY, forgetShapes, outerHeight } from "./kept-shape";

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

/** The shape kept for the page at `path` (a `topOf` path), or null. */
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
