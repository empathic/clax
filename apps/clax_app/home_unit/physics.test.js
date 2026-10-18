import { describe, expect, test } from 'vitest';
import { springStep, atRest, tossStep, tossRest, tossDone, velocityOf, TOSS_TAU } from '../home/physics.js';

describe('springStep', () => {
  test('carries a tile to its cell with one small overshoot, at rest within 500 ms', () => {
    let s = { x: 100, v: 0 };
    let low = 0; let crossings = 0; let side = 1; let restAt = null;
    for (let f = 1; f <= 60; f++) {
      s = springStep(s, 0, 1 / 60);
      low = Math.min(low, s.x);
      if (Math.abs(s.x) > 0.5 && Math.sign(s.x) !== side) { crossings++; side = Math.sign(s.x); }
      if (restAt === null && atRest(s, 0)) restAt = f;
    }
    expect(crossings).toBe(1);
    expect(-low).toBeGreaterThan(1);
    expect(-low).toBeLessThan(10);
    expect(restAt).toBeLessThanOrEqual(30);
  });

  test('one long step equals the same time in short steps, and never blows up', () => {
    const a = springStep({ x: 100, v: 0 }, 0, 0.5);
    let b = { x: 100, v: 0 };
    for (let i = 0; i < 15; i++) b = springStep(b, 0, 1 / 30);
    expect(a.x).toBeCloseTo(b.x, 6);
    expect(Math.abs(springStep({ x: 100, v: 0 }, 0, 5).x)).toBeLessThan(1);
  });
});

describe('toss', () => {
  test('decays with the release speed and comes to rest v·τ on', () => {
    let p = { x: 0, y: 0, vx: 2000, vy: -1000 };
    expect(tossRest(p)).toEqual({ x: 2000 * TOSS_TAU, y: -1000 * TOSS_TAU });
    for (let f = 0; f < 180 && !tossDone(p); f++) p = tossStep(p, 1 / 60);
    expect(tossDone(p)).toBe(true);
    expect(Math.abs(p.x - 2000 * TOSS_TAU)).toBeLessThan(10);
    expect(Math.abs(p.y + 1000 * TOSS_TAU)).toBeLessThan(10);
  });
});

describe('velocityOf', () => {
  test('takes the speed over the last 80 ms only', () => {
    expect(velocityOf([])).toEqual({ vx: 0, vy: 0 });
    expect(velocityOf([{ t: 0, x: 0, y: 0 }])).toEqual({ vx: 0, vy: 0 });
    const samples = [{ t: 0, x: 0, y: 0 }, { t: 100, x: 0, y: 0 }, { t: 140, x: 40, y: 0 }, { t: 180, x: 80, y: -20 }];
    expect(velocityOf(samples)).toEqual({ vx: 1000, vy: -250 });
  });
});
