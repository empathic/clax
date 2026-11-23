// The threads' skeleton (`.viewer > .dsk`, theme.css): where the drawer's
// list will be, drawn before the first paint by the inline `clax-dsk`
// script (`shell/artifact.html`) and shown while the drawer has no list yet
// or holds it back (the list waits for its questions, `.page.cards-wait`),
// so nothing in the drawer is blank or unsure. Its blocks are where this
// artifact's list last showed its blocks in this browser (kept in the
// shell's kept shapes, `kept-shape.ts`, under `/a/<id>`, measured as the
// list first shows), so a second visit's list lands where its placeholders
// were; else, with the bootstrap block, its open threads' count in the
// list's usual measures (`skeletonBlocks`); else a neutral two cards.
import { keepEntry, readShapes } from "./kept-shape";

/** A block of the list: its top and height in px, from the drawer's top. */
export type Block = [number, number];
/** What an artifact's list showed: its first blocks (`BLOCKS`), within the window's height. */
export type DrawerShape = { b: Block[] };

/** The list's blocks a shape keeps: the questions' cards, New comment, the
 * working strips, the Send bar, the sections' headings and the thread cards. */
export const BLOCKS = ".qcard, .new-comment, .strip, .send-unsent, .gh, .thread-card";
/** The most blocks a shape keeps, and the most artifacts kept. */
export const MAX_BLOCKS = 16;
export const MAX_DRAWERS = 40;

/** The usual measures (px from the drawer's top): New comment's line under
 * the drawer's bar, the Open heading, and its cards; the most cards drawn
 * from a count. As `clax-dsk` draws them. */
export const DEFAULT = { comment: [53, 34] as Block, heading: [99, 22] as Block, card: 131, height: 120, gap: 10, most: 6 };

const key = (id: string) => `/a/${id}`;
const ID = /^[0-9a-z]{12}$/;

/** `v` as a shape: at most `MAX_BLOCKS` blocks, each two finite, non-negative numbers; null when none. */
export function cleanDrawer(v: unknown): DrawerShape | null {
  const b = (v as DrawerShape | null)?.b;
  if (!Array.isArray(b)) return null;
  const ok = b.filter((x): x is Block => Array.isArray(x) && x.length === 2 && x.every(n => typeof n === "number" && Number.isFinite(n) && n >= 0)).slice(0, MAX_BLOCKS);
  return ok.length ? { b: ok.map(([t, h]) => [Math.round(t), Math.round(h)]) } : null;
}

/** The shape kept for artifact `id`'s list, or null. */
export function keptDrawer(id: string, store: Storage = localStorage): DrawerShape | null {
  return ID.test(id) ? cleanDrawer(readShapes(store)[key(id)]) : null;
}

/** Keeps `s` as artifact `id`'s list's shape (the newest `MAX_DRAWERS` artifacts' stay). */
export function keepDrawer(id: string, s: DrawerShape, store: Storage = localStorage): void {
  const clean = cleanDrawer(s);
  if (clean && ID.test(id)) keepEntry(key(id), clean, store, { match: /^\/a\//, max: MAX_DRAWERS });
}

/** The blocks `drawer` shows now, from its top, those that start within `height`. */
export function measureDrawer(drawer: Element, height: number): DrawerShape {
  const top = drawer.getBoundingClientRect().top;
  const b: Block[] = [];
  for (const el of drawer.querySelectorAll(BLOCKS)) {
    const r = el.getBoundingClientRect();
    if (!r.height || r.top - top >= height || b.length >= MAX_BLOCKS) continue;
    b.push([Math.round(r.top - top), Math.round(r.height)]);
  }
  return { b };
}

/** The skeleton's blocks: the kept shape's, else New comment, the Open
 * heading and `open` cards (at most `DEFAULT.most`; two where the count is
 * not known). */
export function skeletonBlocks(kept: DrawerShape | null, open: number | null): Block[] {
  if (kept) return kept.b;
  const n = open === null ? 2 : Math.min(open, DEFAULT.most);
  const cards: Block[] = Array.from({ length: n }, (_, i) => [DEFAULT.card + i * (DEFAULT.height + DEFAULT.gap), DEFAULT.height]);
  return [DEFAULT.comment, DEFAULT.heading, ...cards];
}
