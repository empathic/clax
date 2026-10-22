import { describe, expect, test } from 'vitest';
import { layoutStore } from '../home/store.js';
import { LAYOUT_PATH } from '../home/arrange.js';
import { manualClock } from '../home/physics.js';
import { fakeDb } from '../assets/kit/fake-home.js';

const tick = () => new Promise((r) => setTimeout(r, 0));

/** `db` with `onRead(n)` run after its n-th read of a document answers. */
const racing = (db, onRead) => ({
  ...db,
  doc(path) {
    const ref = db.doc(path);
    let n = 0;
    return { ...ref, get: async () => { const snap = await ref.get(); await onRead(++n); return snap; } };
  },
});

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

  test("a change keeps a later release's v and the fields it cannot read", async () => {
    const db = fakeDb({ [LAYOUT_PATH]: { v: 2, starred: { a: 1 }, seen: [] } });
    const s = layoutStore(db, { clock: manualClock() });
    await s.change((b) => ({ ...b, v: 1, starred: [], seen: ['a'] }));
    expect(await s.load()).toEqual({ v: 2, starred: { a: 1 }, seen: ['a'] });
  });

  test('a write landing between the read and the write is read again and the change made over it', async () => {
    const db = fakeDb({ [LAYOUT_PATH]: { starred: [], order: ['a', 'b'] } });
    const s = layoutStore(racing(db, (n) => { if (n === 1) db.put(LAYOUT_PATH, { starred: [], order: ['b', 'a'] }); }), { clock: manualClock() });
    await s.change((b) => ({ ...b, starred: [...b.starred, 'a'] }));
    expect(await s.load()).toEqual({ starred: ['a'], order: ['b', 'a'] });
    expect(db.writes).toEqual([{ path: LAYOUT_PATH, data: { starred: ['a'], order: ['b', 'a'] } }]);
  });

  test('a body that changes on every read rejects busy after 10 tries and writes nothing', async () => {
    const db = fakeDb({ [LAYOUT_PATH]: { n: 0 } });
    const s = layoutStore(racing(db, (n) => db.put(LAYOUT_PATH, { n })), { clock: manualClock() });
    await expect(s.change((b) => ({ ...b, mine: true }))).rejects.toMatchObject({ code: 'busy' });
    expect(db.writes).toEqual([]);
  });

  test('the lease is renewed while a change runs', async () => {
    const clock = manualClock();
    const db = fakeDb({ [LAYOUT_PATH]: { n: 0 } }, { now: () => clock.now() });
    let answer;
    const slow = new Promise((r) => { answer = r; });
    const s = layoutStore(racing(db, (n) => (n === 1 ? slow : undefined)), { clock, holder: 'the-phone' });
    const p = s.change((b) => ({ ...b, n: b.n + 1 }));
    for (let i = 0; i < 5; i++) { await tick(); clock.advance(1000); }
    await tick();
    expect((await db.doc(LAYOUT_PATH).acquire({ holder: 'the-mac', ttlMs: 2000 })).acquired).toBe(false);
    answer();
    await p;
    expect(await s.load()).toEqual({ n: 1 });
  });

  test('a lease taken over during a change is waited for, then the body read again before writing', async () => {
    const clock = manualClock();
    const db = fakeDb({ [LAYOUT_PATH]: { order: ['a'] } }, { now: () => clock.now() });
    const s = layoutStore(racing(db, (n) => {
      if (n !== 1) return;
      db.hold(LAYOUT_PATH, 'the-mac', 3000);
      db.put(LAYOUT_PATH, { order: ['a', 'b'] });
    }), { clock, holder: 'the-phone' });
    let done = false;
    const p = s.change((b) => ({ ...b, starred: ['a'] })).then(() => { done = true; });
    await tick(); await tick();
    expect(done).toBe(false);
    expect(db.writes).toEqual([]);
    for (let i = 0; i < 5 && !done; i++) { clock.advance(1000); await tick(); }
    await p;
    expect(await s.load()).toEqual({ order: ['a', 'b'], starred: ['a'] });
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
