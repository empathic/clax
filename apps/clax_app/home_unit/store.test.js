import { describe, expect, test } from 'vitest';
import { layoutStore } from '../home/store.js';
import { LAYOUT_PATH } from '../home/arrange.js';
import { manualClock } from '../home/physics.js';
import { fakeDb } from '../assets/kit/fake-home.js';

const tick = () => new Promise((r) => setTimeout(r, 0));

describe('layoutStore', () => {
  test('loads null with no document, then each change writes the whole body', async () => {
    const db = fakeDb();
    const s = layoutStore(db, { clock: manualClock() });
    expect(await s.load()).toBeNull();
    await s.change((b) => ({ ...(b ?? {}), starred: ['a'] }));
    expect(await s.load()).toEqual({ starred: ['a'] });
    expect(db.writes).toEqual([{ path: LAYOUT_PATH, data: { starred: ['a'] } }]);
  });

  test('a change that changes nothing writes nothing', async () => {
    const db = fakeDb({ [LAYOUT_PATH]: { starred: ['a'] } });
    const s = layoutStore(db, { clock: manualClock() });
    await s.change((b) => ({ ...b }));
    expect(db.writes).toEqual([]);
  });

  test('changes go one at a time, each over the body the one before wrote', async () => {
    const db = fakeDb({ [LAYOUT_PATH]: { order: [] } });
    const s = layoutStore(db, { clock: manualClock() });
    await Promise.all([
      s.change((b) => ({ ...b, order: [...b.order, 'a'] })),
      s.change((b) => ({ ...b, order: [...b.order, 'b'] })),
    ]);
    expect((await s.load()).order).toEqual(['a', 'b']);
  });

  test("another device's write between two changes is kept", async () => {
    const db = fakeDb({ [LAYOUT_PATH]: { starred: [], order: ['a', 'b'] } });
    const s = layoutStore(db, { clock: manualClock() });
    await s.change((b) => ({ ...b, starred: ['a'] }));
    db.put(LAYOUT_PATH, { ...(await s.load()), order: ['b', 'a'] });
    await s.change((b) => ({ ...b, starred: [...b.starred, 'b'] }));
    expect(await s.load()).toEqual({ starred: ['a', 'b'], order: ['b', 'a'] });
  });

  test('a busy lease is tried again when the document changes or after 1 s', async () => {
    const clock = manualClock();
    const db = fakeDb({ [LAYOUT_PATH]: { n: 0 } }, { now: () => clock.now() });
    db.hold(LAYOUT_PATH, 'the-mac', 3000);
    const s = layoutStore(db, { clock, holder: 'the-phone' });
    const stop = s.watch(() => {});
    let done = false;
    const p = s.change((b) => ({ ...b, n: b.n + 1 })).then(() => { done = true; });
    await tick();
    clock.advance(1000); await tick();
    expect(done).toBe(false);
    db.put(LAYOUT_PATH, { n: 10 });
    await tick();
    expect(done).toBe(false);
    // Each retry registers its next wait only after the one before ran, so
    // the clock moves a second at a time until the lease ends at 3 s.
    for (let i = 0; i < 4 && !done; i++) { clock.advance(1000); await tick(); }
    await p;
    expect(await s.load()).toEqual({ n: 11 });
    stop();
  });

  test('gives up after 10 busy tries and rejects', async () => {
    const clock = manualClock();
    const db = fakeDb({}, { now: () => clock.now() });
    db.hold(LAYOUT_PATH, 'the-mac', 60_000);
    const s = layoutStore(db, { clock, holder: 'the-phone' });
    const p = s.change(() => ({ n: 1 }));
    let failed = null;
    p.catch((e) => { failed = e; });
    for (let i = 0; i < 12; i++) { await tick(); clock.advance(1000); }
    await tick();
    expect(failed?.code).toBe('busy');
    expect(db.writes).toEqual([]);
  });

  test('without db it keeps the layout in memory', async () => {
    const s = layoutStore(null, { clock: manualClock() });
    expect(await s.load()).toBeNull();
    await s.change(() => ({ starred: ['a'] }));
    expect(await s.load()).toEqual({ starred: ['a'] });
    expect(typeof s.watch(() => {})).toBe('function');
  });
});

describe('fakeDb', () => {
  test('keeps documents in sessionStorage under `persist`', async () => {
    sessionStorage.clear();
    await fakeDb({}, { persist: 'k' }).doc('layout/main').set({ a: 1 });
    expect((await fakeDb({}, { persist: 'k' }).doc('layout/main').get()).data()).toEqual({ a: 1 });
  });

  test('a snapshot listener hears the current state, then each write', async () => {
    const db = fakeDb({ 'layout/main': { a: 1 } });
    const heard = [];
    const stop = db.doc('layout/main').onSnapshot((s) => heard.push(s.data()));
    await tick();
    db.put('layout/main', { a: 2 });
    stop();
    db.put('layout/main', { a: 3 });
    expect(heard).toEqual([{ a: 1 }, { a: 2 }]);
  });
});
