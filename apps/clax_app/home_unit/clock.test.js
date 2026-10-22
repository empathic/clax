import { describe, expect, test } from 'vitest';
import { manualClock } from '../home/physics.js';

describe('manualClock', () => {
  test('runs timers in time order and a frame every 16 ms, only when advanced', () => {
    const c = manualClock(0, Date.UTC(2026, 9, 16, 12));
    const seen = [];
    c.after(40, () => seen.push(['t40', c.now()]));
    c.after(10, () => seen.push(['t10', c.now()]));
    const stop = c.after(20, () => seen.push(['never']));
    stop();
    const frame = (t) => { seen.push(['f', t]); if (t < 48) c.frame(frame); };
    c.frame(frame);
    expect(seen).toEqual([]);
    c.advance(50);
    expect(seen).toEqual([['t10', 10], ['f', 16], ['f', 32], ['t40', 40], ['f', 48]]);
    expect(c.now()).toBe(50);
    expect(c.date().toISOString()).toBe('2026-10-16T12:00:00.050Z');
  });
});
