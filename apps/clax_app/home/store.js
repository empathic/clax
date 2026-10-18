// The arrangement in the home app's own documents (`db`), read and written
// whole at layout/main. A change is a pure function of the stored body
// (it may run more than once); changes go one at a time, each under a
// short lease on the document (`acquire`), renewed every RENEW_MS while
// the change runs, so the body a change reads is the one it replaces even
// with the home screen open on another device. Just before writing, the
// change renews the lease and reads the body again: if it changed (a
// writer that took no lease), the change is made again over it; if the
// lease went to another holder, the change waits for it and reads again.
// A busy lease is asked again when the document changes or after
// RETRY_MS, and a change is retried, at most TRIES times each, then the
// change rejects `{code: "busy"}`. The page's `set` is last-writer-wins
// with no version to pin, so a writer that takes no lease and lands
// between the last read and the write is still overwritten. Without `db`
// (a plain browser) the layout lives in memory for the page's life.
import { LAYOUT_PATH, keepNewer } from './arrange.js';

const LEASE_MS = 2000;
const RENEW_MS = LEASE_MS / 2;
const RETRY_MS = 1000;
const TRIES = 10;

const busy = () => Object.assign(new Error('the arrangement is being changed elsewhere'), { code: 'busy' });
const same = (a, b) => JSON.stringify(a ?? null) === JSON.stringify(b ?? null);

export function layoutStore(db, { clock, holder = `home-${Math.random().toString(36).slice(2, 10)}` } = {}) {
  const ref = db ? db.doc(LAYOUT_PATH) : null;
  let memory = null;
  let queue = Promise.resolve();
  const wakers = new Set();
  const read = async () => {
    const s = await ref.get();
    return s.exists ? s.data() : null;
  };
  const lease = async () => {
    for (let i = 0; i < TRIES; i++) {
      const r = await ref.acquire({ holder, ttlMs: LEASE_MS });
      if (r.acquired) return;
      await new Promise((resolve) => {
        const wake = () => { cancel(); wakers.delete(wake); resolve(); };
        const cancel = clock.after(RETRY_MS, wake);
        wakers.add(wake);
      });
    }
    throw busy();
  };
  /** Renews the lease every RENEW_MS until the returned function is called. */
  const keepLease = () => {
    let cancel = null;
    let live = true;
    const next = () => {
      cancel = clock.after(RENEW_MS, () => {
        if (!live) return;
        ref.acquire({ holder, ttlMs: LEASE_MS }).catch(() => {}).finally(() => { if (live) next(); });
      });
    };
    next();
    return () => { live = false; cancel?.(); };
  };
  const write = async (op) => {
    await lease();
    const stop = keepLease();
    try {
      for (let i = 0; i < TRIES; i++) {
        const current = await read();
        const next = keepNewer(current, op(current));
        if (same(next, current)) return next;
        const held = await ref.acquire({ holder, ttlMs: LEASE_MS });
        if (!held.acquired) {
          await lease();
          continue;
        }
        if (!same(await read(), current)) continue;
        await ref.set(next);
        return next;
      }
      throw busy();
    } finally {
      stop();
    }
  };
  return {
    load: () => (ref ? read() : Promise.resolve(memory)),
    watch(fn) {
      if (!ref) return () => {};
      return ref.onSnapshot((s) => {
        for (const wake of [...wakers]) wake();
        fn(s.exists ? s.data() : null);
      }, () => {});
    },
    change(op) {
      const run = async () => {
        if (!ref) {
          memory = op(memory);
          return memory;
        }
        return write(op);
      };
      const p = queue.then(run, run);
      queue = p.catch(() => {});
      return p;
    },
  };
}
