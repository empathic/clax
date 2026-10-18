import { describe, expect, test } from 'vitest';
import { pack, geometry, cellBox, slotAt } from '../home/grid.js';

const at = (r) => r.cells.map((c) => `${c.key}@${c.col},${c.row}${c.span === 2 ? '×2' : ''}`);

describe('pack', () => {
  test('1×1 items fill rows in order', () => {
    expect(at(pack(['a', 'b', 'c', 'd', 'e'].map((key) => ({ key })), 4))).toEqual(['a@0,0', 'b@1,0', 'c@2,0', 'd@3,0', 'e@0,1']);
  });

  test('a 2×2 takes the first block at or after its place; the next 1×1 items fill the gaps it leaves', () => {
    const r = pack([{ key: 'a' }, { key: 'B', span: 2 }, { key: 'c' }, { key: 'd' }, { key: 'e' }, { key: 'f' }], 4);
    expect(at(r)).toEqual(['a@0,0', 'B@1,0×2', 'c@3,0', 'd@0,1', 'e@3,1', 'f@0,2']);
    expect(r.rows).toBe(3);
  });

  test('a 2×2 that does not fit at the row end starts the next row; earlier items keep their places', () => {
    expect(at(pack([{ key: 'a' }, { key: 'b' }, { key: 'c' }, { key: 'D', span: 2 }, { key: 'e' }], 4))).toEqual(['a@0,0', 'b@1,0', 'c@2,0', 'D@0,1×2', 'e@3,0']);
  });

  test('one column holds a 2×2 as a 1×1', () => {
    expect(at(pack([{ key: 'A', span: 2 }, { key: 'b' }], 1))).toEqual(['A@0,0', 'b@0,1']);
  });
});

describe('geometry and slotAt', () => {
  const g = geometry(4 * 100 + 3 * 4, 4);
  test('cells are 100 wide, 116 high, 4 and 8 apart; a 2×2 spans the gap', () => {
    expect(g.cellW).toBe(100);
    expect(cellBox({ col: 1, row: 1, span: 1 }, g)).toEqual({ x: 104, y: 124, w: 100, h: 116 });
    expect(cellBox({ col: 1, row: 0, span: 2 }, g)).toEqual({ x: 104, y: 0, w: 204, h: 240 });
  });

  test('a point lands on the nearest cell', () => {
    const { cells } = pack(['a', 'b', 'c', 'd', 'e'].map((key) => ({ key })), 4);
    expect(slotAt(cells, g, 10, 10)).toBe(0);
    expect(slotAt(cells, g, 260, 60)).toBe(2);
    expect(slotAt(cells, g, 50, 900)).toBe(4);
    expect(slotAt([], g, 0, 0)).toBe(-1);
  });
});
