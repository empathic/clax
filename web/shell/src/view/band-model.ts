// The band along the sidebar's outer edge (spec 2026-11-13, "The band"):
// how many threads are pending over the timeline's lines, and the part of
// them that waits on a person, as SVG path data. Geometry only: no DOM.
// Above a row the band is as wide as that row's count, as a straight run to
// the next newer row; it changes over a straight taper 4 px each side of a
// row's `y`, and below the oldest row it tapers to nothing.

/** Rows newest first; `y` grows downward. `gap`: the day label where a new day begins below this row's predecessor. */
export type BandRow = { y: number; pending: number; waiting: number; v?: number; gap?: string };
/** `top`: where the band starts; `x0`: the edge; `dir` 1 grows toward
 * smaller x, -1 toward larger; `unit`: the width of one comment;
 * `maxUnits`: the widest it is drawn (`MAX_UNITS`); `labels`: also place
 * the version, count and day labels. */
export type BandOpts = { top: number; x0: number; dir: 1 | -1; unit: number; maxUnits?: number; labels?: boolean };
/** Labels' `y` is the vertical centre of their text; `anchor` their text
 * anchor, toward the edge. */
export type Band = {
  agent: string; waiting: string;                        // path data, "" with no rows
  spine: { x: number; y1: number; y2: number } | null;
  ticks: { x1: number; x2: number; y: number }[];        // one per version row
  versions: { x: number; y: number; text: string }[];    // with `labels`
  counts: { x: number; y: number; text: string }[];      // with `labels`
  gaps: { y: number; text: string }[];                   // with `labels`
  anchor: "start" | "end";
};

/** The widest the band is drawn, in comments. */
export const MAX_UNITS = 6;

/** Half the taper at a row, and how far below the oldest row the band ends. */
const TAPER = 4, TAIL = 12;
/** How far a tick runs past the band, and a label sits past what it labels. */
const TICK = 5, PAD = 3;
/** A label's height (the strip's 10 px labels): two closer than this overlap. */
const LABEL = 10;

const r2 = (n: number) => Math.round(n * 100) / 100 + 0;

type Pt = { y: number; hi: number; lo: number };

/** One area's outline: down its outer side (`hi`) from the first point,
 * along the last, and up its inner side (`lo`); "" where it has no width
 * anywhere. `hi` and `lo` are x coordinates. */
function area(pts: Pt[]): string {
  if (pts.every(p => p.hi === p.lo)) return "";
  const n = pts.length;
  let d = `M${r2(pts[0].hi)},${r2(pts[0].y)}`;
  for (let i = 0; i < n - 1; i++) if (pts[i].hi !== pts[i + 1].hi) d += ` V${r2(pts[i].y - TAPER)} L${r2(pts[i + 1].hi)},${r2(pts[i].y + TAPER)}`;
  d += ` V${r2(pts[n - 1].y)} H${r2(pts[n - 1].lo)}`;
  for (let i = n - 2; i >= 0; i--) if (pts[i].lo !== pts[i + 1].lo) d += ` V${r2(pts[i].y + TAPER)} L${r2(pts[i].lo)},${r2(pts[i].y - TAPER)}`;
  return `${d} V${r2(pts[0].y)} Z`;
}

/** The spine and the ticks as path data, one path each ("" for none), so a
 * drawing adds and removes no element as the band changes. */
export function strokes(b: Band): { spine: string; ticks: string } {
  return { spine: b.spine ? `M${b.spine.x},${b.spine.y1} V${b.spine.y2}` : "", ticks: b.ticks.map(k => `M${k.x1},${k.y} H${k.x2}`).join(" ") };
}

export function band(rows: BandRow[], o: BandOpts): Band {
  const anchor = o.dir > 0 ? "end" : "start";
  if (!rows.length) return { agent: "", waiting: "", spine: null, ticks: [], versions: [], counts: [], gaps: [], anchor };
  const max = o.maxUnits ?? MAX_UNITS;
  // A count past `max` is drawn `max` wide, both areas scaled alike so the
  // waiting share keeps its proportion.
  const X = (units: number) => o.x0 - o.dir * units * o.unit;
  const scale = (p: number) => (p > max ? max / p : 1);
  const W = (p: number) => Math.min(p, max);
  const end = rows[rows.length - 1].y + TAIL;
  // The top holds the newest row's width; the tail is width 0.
  const all = [{ ...rows[0], y: o.top }, ...rows, { y: end, pending: 0, waiting: 0 }];
  const split = (p: BandRow) => X((p.pending - p.waiting) * scale(p.pending));
  const agent = area(all.map(p => ({ y: p.y, hi: split(p), lo: X(0) })));
  const waiting = area(all.map(p => ({ y: p.y, hi: X(W(p.pending)), lo: split(p) })));
  const ticks: Band["ticks"] = [], versions: Band["versions"] = [], counts: Band["counts"] = [], gaps: Band["gaps"] = [];
  rows.forEach((r, i) => {
    const below = rows[i + 1]?.pending ?? 0;
    const above = i ? rows[i - 1].y : o.top;
    if (r.v !== undefined) {
      const x2 = r2(X(W(Math.max(r.pending, below))) - o.dir * TICK);
      ticks.push({ x1: r2(o.x0), x2, y: r2(r.y) });
      if (o.labels) versions.push({ x: r2(x2 - o.dir * PAD), y: r2(r.y), text: `v${r.v}` });
    }
    // A run is the rows of one count; its label is centred from the row
    // above its first to its last.
    if (o.labels && r.pending && r.pending !== below) {
      let j = i;
      while (j > 0 && rows[j - 1].pending === r.pending) j--;
      counts.push({ x: r2(X(W(r.pending)) - o.dir * PAD), y: r2((r.y + (j ? rows[j - 1].y : o.top)) / 2), text: String(r.pending) });
    }
    if (o.labels && r.gap) gaps.push({ y: r2((above + r.y) / 2), text: r.gap });
  });
  // A count whose label would overlap a version's is not placed: the
  // version names its line, and the band's width already says the count.
  const clear = counts.filter(c => versions.every(v => Math.abs(c.y - v.y) >= LABEL));
  return { agent, waiting, spine: { x: r2(o.x0), y1: r2(o.top), y2: r2(end) }, ticks, versions, counts: clear, gaps, anchor };
}
