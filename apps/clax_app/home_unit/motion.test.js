import { describe, expect, test } from 'vitest';
import { createMotion } from '../home/motion.js';
import { manualClock } from '../home/physics.js';

const el = () => ({ style: { transform: '' } });

describe('createMotion', () => {
  test('a moved tile springs from where it was to its new place, then settles once', () => {
    const clock = manualClock();
    let settled = 0;
    const m = createMotion(clock, { onSettle: () => settled++ });
    const a = el();
    m.from('a', a, 100, 0);
    expect(a.style.transform).toBe('translate(100px, 0px)');
    expect(m.moving).toBe(true);
    clock.advance(1000);
    expect(a.style.transform).toBe('');
    expect(m.moving).toBe(false);
    expect(settled).toBe(1);
  });

  test('a held tile stays where it is put, and springs home once let go', () => {
    const clock = manualClock();
    const m = createMotion(clock);
    const a = el();
    m.hold('a', a, 40, 30);
    clock.advance(500);
    expect(a.style.transform).toBe('translate(40px, 30px)');
    m.let('a');
    clock.advance(1000);
    expect(a.style.transform).toBe('');
  });

  test('a bump pushes a tile aside and it settles back', () => {
    const clock = manualClock();
    const m = createMotion(clock);
    const a = el();
    m.bump('a', a, 800, 0);
    clock.advance(48);
    expect(a.style.transform).toMatch(/^translate\([1-9]/);
    clock.advance(1000);
    expect(a.style.transform).toBe('');
  });

  test('with reduced motion nothing springs', () => {
    const clock = manualClock();
    let settled = 0;
    const m = createMotion(clock, { reduced: true, onSettle: () => settled++ });
    const a = el();
    m.from('a', a, 100, 0);
    m.bump('a', a, 800, 0);
    expect(a.style.transform).toBe('');
    expect(m.moving).toBe(false);
    expect(settled).toBe(1);
  });
});

describe('createMotion timing and stop', () => {
  test('a tile added while another springs does not slow the first', () => {
    const run = (late) => {
      const clock = manualClock();
      const m = createMotion(clock);
      const a = el();
      m.from('a', a, 100, 0);
      clock.advance(40);
      if (late) m.from('b', el(), 50, 0);
      clock.advance(40);
      return a.style.transform;
    };
    expect(run(true)).toBe(run(false));
  });

  test('stop puts every tile, held or moving, at its place', () => {
    const clock = manualClock();
    const m = createMotion(clock);
    const a = el();
    const b = el();
    m.from('a', a, 100, 0);
    m.hold('b', b, 20, 20);
    clock.advance(16);
    m.stop();
    expect([a.style.transform, b.style.transform]).toEqual(['', '']);
    expect(m.moving).toBe(false);
    clock.advance(1000);
    expect(a.style.transform).toBe('');
  });

  test('rest puts every tile in place at once, held ones too, and settles', () => {
    const clock = manualClock();
    let settled = 0;
    const m = createMotion(clock, { onSettle: () => settled++ });
    const a = el();
    const b = el();
    m.hold('a', a, 40, 30);
    m.from('b', b, 100, 0);
    m.rest();
    expect([a.style.transform, b.style.transform]).toEqual(['', '']);
    expect(m.moving).toBe(false);
    expect(settled).toBe(1);
  });
});
