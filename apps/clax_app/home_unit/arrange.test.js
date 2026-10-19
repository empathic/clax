import { describe, expect, test } from 'vitest';
import { normalize, reconcile, sections, moveTo, star, unstar, opened, isNew, groupOf, groupKey, groupNameOf, makeGroup, addToGroup, removeFromGroup, moveInGroup, renameGroup, setLook, ungroup, keepNewer } from '../home/arrange.js';

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
    const n = normalize({ starred: ['a', 'a', 3, ''], order: 'b', groups: { x: { apps: ['c'], look: 7 }, '': {} }, opens: { a: { n: -1, days: { '2026-10-16': 2, nope: 1 } } } });
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

describe('groups', () => {
  const apps = [A, B, C, D];
  const base = () => reconcile({ starred: ['d'], order: ['a', 'b', 'c'], seen: ['a', 'b', 'c', 'd'] }, apps);

  test('a group is named from the common icon word, else Group', () => {
    expect(groupNameOf(app('x', 0, { word: 'Money' }), app('y', 0, { word: 'money ' }))).toBe('Money');
    expect(groupNameOf(app('x', 0, { word: 'money' }), app('y', 0, { word: 'chart' }))).toBe('Group');
    expect(groupNameOf(app('x', 0, { emoji: '🍲' }), app('y', 0, {}))).toBe('Group');
  });

  test("dropping an app on another makes a folder at the target's place, target first", () => {
    const l = reconcile(makeGroup(base(), 'b', 'a', 'g1', 'Group'), apps);
    expect(l.order).toEqual(['g:g1', 'c']);
    expect(l.groups.g1).toEqual({ name: 'Group', look: 'folder', apps: ['b', 'a'] });
    expect(reconcile(makeGroup(base(), 'c', 'a', 'g1', 'G'), apps).order).toEqual(['b', 'g:g1']);
  });

  test('a starred target leaves Starred for the new group, first in All apps; a group never goes in a group', () => {
    const l = reconcile(makeGroup(base(), 'd', 'c', 'g1', 'G'), apps);
    expect(l.starred).toEqual([]);
    expect(l.order).toEqual(['g:g1', 'a', 'b']);
    expect(l.groups.g1.apps).toEqual(['d', 'c']);
    const g = reconcile(makeGroup(base(), 'a', 'b', 'g1', 'G'), apps);
    expect(makeGroup(g, 'c', 'g:g1', 'g2', 'H')).toEqual(g);
    expect(addToGroup(g, 'g1', 'g:g1')).toEqual(g);
  });

  test('add, move inside, remove, rename, look, ungroup', () => {
    let l = reconcile(makeGroup(base(), 'a', 'b', 'g1', 'G'), apps);
    l = reconcile(addToGroup(l, 'g1', 'c'), apps);
    expect(l.groups.g1.apps).toEqual(['a', 'b', 'c']);
    expect(l.order).toEqual(['g:g1']);
    l = moveInGroup(l, 'g1', 'c', 0);
    expect(l.groups.g1.apps).toEqual(['c', 'a', 'b']);
    l = reconcile(removeFromGroup(l, 'a'), apps);
    expect(l.order).toEqual(['g:g1', 'a']);
    expect(renameGroup(l, 'g1', '  Tools  ').groups.g1.name).toBe('Tools');
    expect(renameGroup(l, 'g1', '   ').groups.g1.name).toBe('G');
    expect(renameGroup(l, 'g1', 'x'.repeat(60)).groups.g1.name).toHaveLength(40);
    expect(setLook(l, 'g1', 'pile').groups.g1.look).toBe('pile');
    const u = reconcile(ungroup(l, 'g1'), apps);
    expect(u.order).toEqual(['c', 'b', 'a']);
    expect(u.groups).toEqual({});
  });

  test('removing the second-last app dissolves the group into the one left', () => {
    let l = reconcile(makeGroup(base(), 'a', 'b', 'g1', 'G'), apps);
    l = reconcile(removeFromGroup(l, 'b'), apps);
    expect(l.order).toEqual(['a', 'b', 'c']);
    expect(l.groups).toEqual({});
  });
});

describe('bigTiles and big, which earlier releases wrote', () => {
  test('are kept as they were and change nothing', () => {
    const old = { v: 1, order: ['a', 'b', 'c'], seen: ['a'], opens: { a: { n: 9, last: '2026-10-16T10:00:00.000Z', days: { '2026-10-16': 9 } } }, bigTiles: true, big: ['a', 'a', 7] };
    const l = opened(reconcile(old, [A, B, C]), 'b', new Date(Date.UTC(2026, 9, 16, 12)));
    expect(l.bigTiles).toBe(true);
    expect(l.big).toEqual(['a', 'a', 7]);
    expect(sections(l, [A, B, C]).all.map((e) => e.key)).toEqual(['a', 'b', 'c']);
  });
});

describe('a layout a later release wrote', () => {
  const now = new Date(Date.UTC(2026, 9, 16, 12));

  test('keeps a look it does not know, shown as a folder until a known look is set', () => {
    const later = { order: ['g:x'], groups: { x: { name: 'Tools', look: 'stack', apps: ['a', 'b'] } }, seen: ['a', 'b'] };
    const l = reconcile(later, [A, B]);
    expect(l.groups.x.look).toBe('stack');
    expect(sections(l, [A, B]).all[0].group.look).toBe('folder');
    expect(star(l, 'c').groups.x.look).toBe('stack');
    expect(setLook(l, 'x', 'pile').groups.x.look).toBe('pile');
  });

  test('keeps order entries of kinds it does not know in place, and does not show them', () => {
    const spacer = { kind: 'spacer', size: 2 };
    const later = { order: ['a', 's:1', 'b', spacer, 'g:x', 's:1'], groups: { x: { name: 'X', apps: ['c', 'd'] } }, seen: ['a', 'b', 'c', 'd'] };
    expect(normalize(later).order).toEqual(['a', 's:1', 'b', spacer, 'g:x']);
    const l = reconcile(later, [A, B, C, D]);
    expect(l.order).toEqual(['a', 's:1', 'b', spacer, 'g:x']);
    expect(sections(l, [A, B, C, D]).all.map((e) => e.key)).toEqual(['a', 'b', 'g:x']);
    // A gone app goes; the entries around it stay.
    expect(reconcile(later, [A, C, D]).order).toEqual(['a', 's:1', spacer, 'g:x']);
  });

  test('indexes into All apps count only the entries shown', () => {
    const later = { order: ['s:1', 'a', 's:2', 'b', 'g:x'], groups: { x: { name: 'X', apps: ['c', 'd'] } } };
    const l = reconcile(later, [A, B, C, D]);
    expect(moveTo(l, 'b', 'all', 0).order).toEqual(['s:1', 'b', 'a', 's:2', 'g:x']);
    expect(moveTo(l, 'a', 'all', 1).order).toEqual(['s:1', 's:2', 'b', 'a', 'g:x']);
    expect(moveTo(l, 'a', 'all', 9).order).toEqual(['s:1', 's:2', 'b', 'g:x', 'a']);
    expect(removeFromGroup(l, 'c').order).toEqual(['s:1', 'a', 's:2', 'b', 'g:x', 'c']);
    expect(ungroup(l, 'x').order).toEqual(['s:1', 'a', 's:2', 'b', 'c', 'd']);
    expect(makeGroup(l, 'a', 'b', 'y', 'Y').order).toEqual(['s:1', 'g:y', 's:2', 'g:x']);
  });

  test('keeps fields it does not know through every change', () => {
    const later = { v: 1, order: ['a', 'b', 'c'], seen: ['a'], wallpaper: { kind: 'dots' }, pinnedRow: ['a'] };
    const changes = [
      (l) => reconcile(l, [A, B, C]),
      (l) => star(l, 'a'),
      (l) => unstar(star(l, 'a'), 'a'),
      (l) => opened(l, 'b', now),
      (l) => makeGroup(l, 'a', 'b', 'y', 'Y'),
      (l) => ungroup(makeGroup(l, 'a', 'b', 'y', 'Y'), 'y'),
    ];
    for (const change of changes) {
      const l = change(later);
      expect(l.wallpaper).toEqual({ kind: 'dots' });
      expect(l.pinnedRow).toEqual(['a']);
    }
  });

  test('keeps a v above 1 through every change and never lowers it', () => {
    expect(normalize({ v: 2 }).v).toBe(2);
    expect(normalize({ v: 2.5 }).v).toBe(2.5);
    expect(reconcile({ v: 3, order: ['a'] }, [A]).v).toBe(3);
    expect(opened(star({ v: 2 }, 'a'), 'a', now).v).toBe(2);
    expect(keepNewer({ v: 2 }, { v: 1, starred: [] }).v).toBe(2);
  });

  test('a v above 1 writes back unchanged the known fields it cannot read', () => {
    const stored = { v: 2, starred: { a: 1 }, order: 'by-use', groups: ['x'], seen: ['a'], opens: 5, bigTiles: 'auto', note: 'n' };
    const next = reconcile(stored, [A, B]);
    expect(keepNewer(stored, next)).toEqual({ ...next, starred: { a: 1 }, order: 'by-use', groups: ['x'], opens: 5, bigTiles: 'auto' });
    // A v 1 layout's malformed fields are its own: they are replaced.
    const old = { ...stored, v: 1 };
    expect(keepNewer(old, reconcile(old, [A, B]))).toEqual(reconcile(old, [A, B]));
    expect(keepNewer(null, { v: 1 })).toEqual({ v: 1 });
  });
});
