// Human pacing for the reel's director (spec 2026-10-27-sizzle-reel-design
// §6.2): how long the cursor takes to travel, the easing of its path (the
// cubic Bézier curves of design/tokens.json `motion.curves`), and the
// rhythm of typing. Everything here is a pure function of its arguments, so
// two renders of a vignette pace it identically.

/** `design/tokens.json` `motion.curves`, as cubic Bézier control points
 * (//design:tokens_sync checks they match: scripts/sync-tokens.py). */
export const CURVES = {
  standard: [0.4, 0, 0.2, 1],
  decelerate: [0.05, 0.7, 0.1, 1],
  accelerate: [0.3, 0, 0.8, 0.15],
} as const satisfies Record<string, readonly [number, number, number, number]>;

/** How long the cursor takes to travel `px` pixels: 250 ms and 0.6 ms a pixel, at most 900 ms. */
export const travelMs = (px: number) => Math.min(900, 250 + 0.6 * px);

/** A cubic Bézier coordinate at parameter `s`, for control values `a` and `b` (the ends are 0 and 1). */
const bezier = (s: number, a: number, b: number) => 3 * a * s * (1 - s) ** 2 + 3 * b * s ** 2 * (1 - s) + s ** 3;

/**
 * The eased progress at time `t` (0 to 1) along `curve`, as CSS's
 * `cubic-bezier(x1, y1, x2, y2)` computes it: the curve's x is solved for
 * `t` by bisection, and its y is the progress. 0 and 1 are exact.
 */
export function ease(t: number, curve: readonly [number, number, number, number]): number {
  if (t <= 0) return 0;
  if (t >= 1) return 1;
  const [x1, y1, x2, y2] = curve;
  let lo = 0;
  let hi = 1;
  for (let i = 0; i < 40; i++) {
    const mid = (lo + hi) / 2;
    if (bezier(mid, x1, x2) < t) lo = mid;
    else hi = mid;
  }
  return bezier((lo + hi) / 2, y1, y2);
}

/** Characters a second a person types at. */
export const TYPING_CPS = 14;
/** How far a keystroke's delay strays from the mean, either way. */
export const TYPING_JITTER = 0.25;

/** FNV-1a: a 32-bit hash of `s`, to seed the generator. */
function hash(s: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 0x01000193);
  return h >>> 0;
}

/** mulberry32: a small seeded generator of numbers in [0, 1). */
function generator(seed: number): () => number {
  let a = seed;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/**
 * The delay before each character of `text`, in whole milliseconds: on
 * average 1000/14 ms, each within ±25 % of that, from a generator seeded by
 * `seed` and the text, so the same text in the same vignette is typed with
 * the same rhythm every time.
 */
export function typingDelays(text: string, seed: string): number[] {
  const mean = 1000 / TYPING_CPS;
  const next = generator(hash(`${seed}\0${text}`));
  return Array.from(text, () => Math.round(mean * (1 + TYPING_JITTER * (2 * next() - 1))));
}
