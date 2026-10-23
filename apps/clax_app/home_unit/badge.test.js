import { describe, expect, test } from 'vitest';
import { SYNC_BADGES, syncBadgeOf, syncSvgOf } from '../home/sync.js';

describe('syncBadgeOf', () => {
  test('tile_badges_per_state', () => {
    expect(syncBadgeOf('live')).toEqual({ glyph: '☁', warm: false, words: 'live' });
    expect(syncBadgeOf('ahead')).toEqual({ glyph: '↑', warm: false, words: 'newer here' });
    expect(syncBadgeOf('update')).toEqual({ glyph: '↓', warm: true, words: 'update waiting' });
    expect(syncBadgeOf('diverged')).toEqual({ glyph: '⑂', warm: false, words: 'changed in both places' });
    // A copy installed from someone else and changed here is a fork.
    expect(syncBadgeOf('fork')).toEqual({ glyph: '⑂', warm: false, words: 'changed here' });
    // Only here and kept mine have none (apps() gives null), nor does a
    // value this page does not know.
    for (const none of [null, undefined, '', 'onlyHere', 'keptMine', 'toString', 'constructor', 7]) {
      expect(syncBadgeOf(none)).toBeNull();
    }
  });

  test('the update is the only warm badge', () => {
    expect(Object.entries(SYNC_BADGES).filter(([, b]) => b.warm).map(([k]) => k)).toEqual(['update']);
  });
});

describe('syncSvgOf', () => {
  test('draws each badge, and nothing for none', () => {
    for (const k of Object.keys(SYNC_BADGES)) expect(syncSvgOf(k)).toMatch(/^<svg [^>]*viewBox="0 0 24 24"[^>]*><path [^>]*\/><\/svg>$/);
    // A fork is drawn as diverged is.
    expect(syncSvgOf('fork')).toBe(syncSvgOf('diverged'));
    expect(new Set(Object.keys(SYNC_BADGES).map(syncSvgOf)).size).toBe(4);
    for (const none of [null, 'keptMine', 'toString']) expect(syncSvgOf(none)).toBe('');
  });
});
