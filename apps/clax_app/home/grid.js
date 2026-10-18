// The home screen's cells: where each entry goes in a grid of a given
// width, and which cell a point is over.

/** Where `items` ({key, span}: 1, or 2 for a 2×2) go in a grid `cols`
 * wide, keeping their order as nearly as the cells allow: a 1×1 takes the
 * first free cell, so it fills a gap a 2×2 left before it; a 2×2 takes
 * the first free block at or after the place the item before it took. */
export function pack(items, cols) {
  const used = new Set();
  const free = (c, r, n) => {
    if (c + n > cols) return false;
    for (let i = 0; i < n; i++) for (let j = 0; j < n; j++) if (used.has(`${c + i},${r + j}`)) return false;
    return true;
  };
  const cells = [];
  let cursor = 0;
  let rows = 0;
  for (const it of items) {
    const n = Math.min(it.span === 2 ? 2 : 1, cols);
    let i = n === 1 ? 0 : cursor;
    while (!free(i % cols, Math.floor(i / cols), n)) i++;
    const col = i % cols;
    const row = Math.floor(i / cols);
    for (let a = 0; a < n; a++) for (let b = 0; b < n; b++) used.add(`${col + a},${row + b}`);
    cells.push({ key: it.key, col, row, span: n });
    cursor = Math.max(cursor, i);
    rows = Math.max(rows, row + n);
  }
  return { cells, rows };
}

/** The measures of a grid `width` CSS px wide in `cols` columns. */
export function geometry(width, cols, { gapX = 4, gapY = 8, cellH = 116 } = {}) {
  return { cols, cellW: (width - gapX * (cols - 1)) / cols, cellH, gapX, gapY };
}

/** `cell`'s box, from the grid's top left. */
export function cellBox(cell, g) {
  return {
    x: cell.col * (g.cellW + g.gapX),
    y: cell.row * (g.cellH + g.gapY),
    w: cell.span * g.cellW + (cell.span - 1) * g.gapX,
    h: cell.span * g.cellH + (cell.span - 1) * g.gapY,
  };
}

/** The index of the cell whose centre is nearest (x, y), from the grid's
 * top left; -1 with no cells. */
export function slotAt(cells, g, x, y) {
  let best = -1;
  let dist = Infinity;
  cells.forEach((cell, i) => {
    const b = cellBox(cell, g);
    const d = Math.hypot(b.x + b.w / 2 - x, b.y + b.h / 2 - y);
    if (d < dist) { dist = d; best = i; }
  });
  return best;
}

/** The index of the cell whose centre is within `radius` px of (x, y),
 * from the grid's top left, or -1. */
export function centreUnder(cells, g, x, y, radius = 24) {
  const i = slotAt(cells, g, x, y);
  if (i < 0) return -1;
  const b = cellBox(cells[i], g);
  return Math.hypot(b.x + b.w / 2 - x, b.y + b.h / 2 - y) <= radius ? i : -1;
}

/** The tilt, in degrees from −8 to 8, of the `i`th icon of group `gid`'s
 * pile: from a hash of both (FNV-1a), so a pile looks the same each time. */
export function tiltOf(gid, i) {
  const s = `${gid}:${i}`;
  let h = 0x811c9dc5;
  for (let k = 0; k < s.length; k++) h = Math.imul(h ^ s.charCodeAt(k), 0x01000193) >>> 0;
  return (h % 1601) / 100 - 8;
}
