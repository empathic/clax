import { describe, expect, test } from 'vitest';
import { normalize, reconcile, sections, moveTo, star, unstar, opened, isNew, groupOf, groupKey } from '../home/arrange.js';

const app = (id, hoursAgo = 0, icon = {}) => ({ id, title: id.toUpperCase(), updatedAt: new Date(Date.UTC(2026, 9, 16, 12) - hoursAgo * 3600e3).toISOString(), build: null, icon });
const A = app('a', 1), B = app('b', 2), C = app('c', 3), D = app('d', 4);

describe('normalize', () => {
  test('fills what is missing and keeps fields it does not know', () => {
    expect(normalize(null)).toEqual({ v: 1, starred: [], order: [], groups: {}, seen: [], opens: {} });
    const later = { v: 1, starred: ['a'], order: ['b', 'g:x', 'zzz'], groups: { x: { name: 'Tools', look: 'pile', apps: ['c', 'd'], colour: 'teal' } }, seen: ['a'], opens: {}, bigTiles: false, wallpaper: { kind: 'dots' } };
    const n = normalize(later);
    expect(n.bigTiles).toBe(false);
    expect(n.wallpaper).toEqual({ kind: 'dots' });
    expect(n.groups.x).toEqual({ name: 'Tools', look: 'pile', apps: ['c', 'd'], colour: 'teal' });
  });

  test('drops malformed entries and duplicates', () => {
    const n = normalize({ starred: ['a', 'a', 3, ''], order: 'b', groups: { x: { apps: ['c'], look: 'cube' }, '': {} }, opens: { a: { n: -1, days: { '2026-10-16': 2, nope: 1 } } } });
    expect(n.starred).toEqual(['a']);
    expect(n.order).toEqual([]);
    expect(n.groups).toEqual({ x: { name: 'Group', look: 'folder', apps: ['c'] } });
    expect(n.opens.a).toEqual({ n: 0, last: null, days: { '2026-10-16': 2 } });
  });
});

describe('reconcile', () => {
  test('a first load lists the apps newest first, every one seen', () => {
    const l = reconcile(null, [B, A, C], { first: true });
    expect(l.order).toEqual(['a', 'b', 'c']);
    expect(l.seen.sort()).toEqual(['a', 'b', 'c']);
    expect(['a', 'b', 'c'].some((id) => isNew(l, id))).toBe(false);
  });

  test('a listed app in no place goes first, unseen; an app no longer listed goes everywhere', () => {
    const stored = { starred: ['a', 'gone'], order: ['b', 'gone', 'c'], seen: ['a', 'b', 'c', 'gone'], opens: { gone: { n: 3, last: null, days: {} } } };
    const l = reconcile(stored, [D, A, B, C]);
    expect(l.starred).toEqual(['a']);
    expect(l.order).toEqual(['d', 'b', 'c']);
    expect(isNew(l, 'd')).toBe(true);
    expect(l.seen).not.toContain('gone');
    expect(l.opens.gone).toBeUndefined();
  });

  test('an app in two places keeps Starred, then its group, then All apps', () => {
    const l = reconcile({ starred: ['a'], order: ['a', 'g:x', 'b', 'c'], groups: { x: { name: 'X', apps: ['a', 'b', 'c', 'd'] } }, seen: ['a', 'b', 'c', 'd'] }, [A, B, C, D]);
    expect(l.starred).toEqual(['a']);
    expect(l.groups.x.apps).toEqual(['b', 'c', 'd']);
    expect(l.order).toEqual(['g:x']);
  });

  test('a group left with one app gives way to it in place; one left with none goes', () => {
    const stored = { order: ['a', 'g:x', 'g:y', 'b'], groups: { x: { name: 'X', apps: ['c', 'gone'] }, y: { name: 'Y', apps: ['gone2'] } }, seen: ['a', 'b', 'c'] };
    const l = reconcile(stored, [A, B, C]);
    expect(l.order).toEqual(['a', 'c', 'b']);
    expect(l.groups).toEqual({});
  });

  test('a group no entry names is put at the end of All apps; a g: entry with no group goes', () => {
    const l = reconcile({ order: ['a', 'g:missing'], groups: { x: { name: 'X', apps: ['b', 'c'] } }, seen: ['a', 'b', 'c'] }, [A, B, C]);
    expect(l.order).toEqual(['a', 'g:x']);
  });

  test('keeps fields and groups an earlier release does not show', () => {
    const later = { v: 1, order: ['g:x', 'a'], groups: { x: { name: 'Tools', look: 'pile', apps: ['b', 'c'] } }, seen: ['a', 'b', 'c'], bigTiles: false, wallpaper: 'dots' };
    const l = reconcile(later, [A, B, C]);
    expect(l.order).toEqual(['g:x', 'a']);
    expect(l.groups.x.look).toBe('pile');
    expect(l.bigTiles).toBe(false);
    expect(l.wallpaper).toBe('dots');
    expect(isNew(l, 'b')).toBe(false);
  });
});

describe('sections', () => {
  test('gives Starred and All apps, a group as one entry with its apps', () => {
    const l = reconcile({ starred: ['d'], order: ['a', 'g:x'], groups: { x: { name: 'Tools', look: 'folder', apps: ['b', 'c'] } }, seen: ['a', 'b', 'c', 'd'] }, [A, B, C, D]);
    const s = sections(l, [A, B, C, D]);
    expect(s.starred.map((e) => e.key)).toEqual(['d']);
    expect(s.all.map((e) => e.key)).toEqual(['a', 'g:x']);
    expect(s.all[1].group).toEqual({ id: 'x', name: 'Tools', look: 'folder' });
    expect(s.all[1].apps.map((a) => a.id)).toEqual(['b', 'c']);
  });
});

describe('moving and starring', () => {
  const base = () => reconcile({ starred: ['a'], order: ['b', 'g:x', 'c'], groups: { x: { name: 'X', apps: ['d', 'e'] } }, seen: ['a', 'b', 'c', 'd', 'e'] }, [A, B, C, D, app('e')]);

  test('moveTo moves within and between sections, clamped', () => {
    expect(moveTo(base(), 'c', 'all', 0).order).toEqual(['c', 'b', 'g:x']);
    expect(moveTo(base(), 'b', 'all', 99).order).toEqual(['g:x', 'c', 'b']);
    const s = moveTo(base(), 'c', 'starred', 0);
    expect(s.starred).toEqual(['c', 'a']);
    expect(s.order).toEqual(['b', 'g:x']);
    expect(moveTo(base(), 'a', 'all', 1).order).toEqual(['b', 'a', 'g:x', 'c']);
  });

  test('a group moves like an app but is never starred', () => {
    expect(moveTo(base(), groupKey('x'), 'all', 0).order).toEqual(['g:x', 'b', 'c']);
    expect(moveTo(base(), groupKey('x'), 'starred', 0)).toEqual(base());
  });

  test('star puts an app last in Starred, out of its group; unstar puts it first in All apps', () => {
    const s = star(base(), 'd');
    expect(s.starred).toEqual(['a', 'd']);
    expect(s.groups.x.apps).toEqual(['e']);
    expect(groupOf(s, 'd')).toBeNull();
    const u = unstar(base(), 'a');
    expect(u.starred).toEqual([]);
    expect(u.order).toEqual(['a', 'b', 'g:x', 'c']);
    expect(groupOf(base(), 'e')).toBe('x');
  });

  test('changes return new layouts and leave the old one as it was', () => {
    const l = base();
    const copy = JSON.parse(JSON.stringify(l));
    star(l, 'b'); unstar(l, 'a'); moveTo(l, 'c', 'all', 0); opened(l, 'b', new Date());
    expect(l).toEqual(copy);
  });
});

describe('opened', () => {
  test('marks the app seen and counts the open per UTC day, keeping the last 14 days', () => {
    let l = reconcile({ order: ['a'], seen: [], opens: { a: { n: 4, last: '2026-09-01T00:00:00.000Z', days: { '2026-09-01': 4, '2026-10-03': 1 } } } }, [A]);
    expect(isNew(l, 'a')).toBe(true);
    l = opened(l, 'a', new Date('2026-10-16T12:00:00Z'));
    l = opened(l, 'a', new Date('2026-10-16T13:00:00Z'));
    expect(isNew(l, 'a')).toBe(false);
    expect(l.opens.a).toEqual({ n: 6, last: '2026-10-16T13:00:00.000Z', days: { '2026-10-03': 1, '2026-10-16': 2 } });
  });
});
