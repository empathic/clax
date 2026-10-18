import { describe, expect, test } from 'vitest';
import { createPress, HOLD_MS } from '../home/drag.js';
import { manualClock, velocityOf } from '../home/physics.js';

function rig() {
  const clock = manualClock();
  const log = [];
  const h = (name) => (...a) => log.push([name, ...a.map((x) => (x && typeof x === 'object' && 'x' in x ? `${x.x},${x.y}` : x))]);
  const p = createPress({ lift: h('lift'), unlift: h('unlift'), menu: h('menu'), dragStart: h('dragStart'), dragMove: h('dragMove'), dragEnd: (q, s) => log.push(['dragEnd', `${q.x},${q.y}`, s.length]), dragCancel: h('dragCancel') }, clock);
  const ev = (x, y, type = 'touch', button = 0) => ({ pointerId: 1, pointerType: type, button, clientX: x, clientY: y });
  return { clock, log, p, ev };
}

describe('createPress', () => {
  test('a quick touch is a tap: nothing but the click', () => {
    const { p, ev, log, clock } = rig();
    p.down(ev(10, 10)); clock.advance(100); p.up(ev(10, 10));
    expect(log).toEqual([]);
    expect(p.takeClick()).toBe(true);
  });

  test('held 400 ms still the tile lifts; released without moving it asks for the menu and swallows the click', () => {
    const { p, ev, log, clock } = rig();
    p.down(ev(10, 10)); clock.advance(HOLD_MS - 1);
    expect(log).toEqual([]);
    clock.advance(1);
    expect(log).toEqual([['lift']]);
    p.move(ev(14, 14)); p.up(ev(14, 14));
    expect(log).toEqual([['lift'], ['menu']]);
    expect(p.takeClick()).toBe(false);
    expect(p.takeClick()).toBe(true);
  });

  test('lifted, a move past 8 px drags, and the release ends the drag with the recent samples and the release point', () => {
    const { p, ev, log, clock } = rig();
    p.down(ev(10, 10)); clock.advance(HOLD_MS);
    clock.advance(16); p.move(ev(30, 10));
    clock.advance(16); p.move(ev(50, 10));
    p.up(ev(50, 10));
    expect(log).toEqual([['lift'], ['dragStart', '30,10'], ['dragMove', '30,10'], ['dragMove', '50,10'], ['dragEnd', '50,10', 3]]);
    expect(p.takeClick()).toBe(false);
  });

  test('a touch that moves 8 px before the hold lets go: the page scrolls, nothing lifts', () => {
    const { p, ev, log, clock } = rig();
    p.down(ev(10, 10)); clock.advance(100); p.move(ev(10, 30));
    clock.advance(1000); p.up(ev(10, 30));
    expect(log).toEqual([]);
    expect(p.phase).toBe('idle');
  });

  test('a mouse drags at once on moving 8 px, with no hold', () => {
    const { p, ev, log } = rig();
    p.down(ev(10, 10, 'mouse')); p.move(ev(15, 10, 'mouse'));
    expect(log).toEqual([]);
    p.move(ev(30, 10, 'mouse'));
    expect(log[0]).toEqual(['dragStart', '30,10']);
  });

  test("a mouse's other buttons press nothing; a context menu outside a press opens at once, during one is ignored", () => {
    const { p, ev, log } = rig();
    p.down(ev(10, 10, 'mouse', 2));
    expect(p.phase).toBe('idle');
    expect(p.contextMenu()).toBe(false);
    p.down(ev(10, 10));
    expect(p.contextMenu()).toBe(true);
    expect(log).toEqual([]);
  });

  test('a cancel ends a lift or a drag', () => {
    const { p, ev, log, clock } = rig();
    p.down(ev(10, 10)); clock.advance(HOLD_MS); p.cancel(ev(10, 10));
    p.down(ev(10, 10)); clock.advance(HOLD_MS); p.move(ev(40, 10)); p.cancel(ev(40, 10));
    expect(log.map((l) => l[0])).toEqual(['lift', 'unlift', 'lift', 'dragStart', 'dragMove', 'dragCancel']);
  });
});

describe('createPress and scrolling', () => {
  test('a touch that scrolls off a tile, then a fresh tap on it, opens it', () => {
    const { p, ev, log, clock } = rig();
    p.down(ev(10, 10)); clock.advance(50); p.move(ev(10, 19)); p.move(ev(10, 60));
    clock.advance(HOLD_MS); p.up(ev(10, 60));
    expect(p.takeClick()).toBe(true);
    p.down(ev(10, 10)); clock.advance(80); p.up(ev(10, 10));
    expect(log).toEqual([]);
    expect(p.takeClick()).toBe(true);
  });

  test('moves of another pointer are not this press', () => {
    const { p, ev, log, clock } = rig();
    p.down(ev(10, 10)); p.move({ ...ev(80, 80), pointerId: 2 });
    clock.advance(HOLD_MS);
    expect(log).toEqual([['lift']]);
  });
});

describe('createPress and the release speed', () => {
  // A mouse drag of six 20 px moves 16 ms apart, then `stillMs` without a
  // move, then the release: the samples the drag ends with.
  function fling(stillMs) {
    const clock = manualClock();
    let ended = null;
    const noop = () => {};
    const p = createPress({ lift: noop, unlift: noop, menu: noop, dragStart: noop, dragMove: noop, dragCancel: noop, dragEnd: (q, s) => { ended = s; } }, clock);
    const ev = (x) => ({ pointerId: 1, pointerType: 'mouse', button: 0, clientX: x, clientY: 0 });
    p.down(ev(0));
    for (let i = 1; i <= 6; i++) { clock.advance(16); p.move(ev(20 * i)); }
    clock.advance(stillMs);
    p.up(ev(120));
    return ended;
  }

  test('a drag held still 500 ms before the release is not tossed', () => {
    expect(velocityOf(fling(500))).toEqual({ vx: 0, vy: 0 });
  });

  test('a drag released while moving keeps its speed', () => {
    expect(velocityOf(fling(0))).toEqual({ vx: 1250, vy: 0 });
  });

  test('a pause shorter than the speed window slows the release', () => {
    const v = velocityOf(fling(40));
    expect(v.vx).toBeGreaterThan(0);
    expect(v.vx).toBeLessThan(1250);
  });
});
