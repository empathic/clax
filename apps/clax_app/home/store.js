// The arrangement in the home app's own documents (`db`), read and written
// whole at layout/main. A change is a function of the stored body; changes
// go one at a time, each under a short lease on the document (`acquire`),
// so the body a change reads is the one it replaces even with the home
// screen open on another device. A busy lease is asked again when the
// document changes or after RETRY_MS, at most TRIES times, then the change
// rejects `{code: "busy"}`. Without `db` (a plain browser) the layout
// lives in memory for the page's life.
import { LAYOUT_PATH } from './arrange.js';

const LEASE_MS = 2000;
const RETRY_MS = 1000;
const TRIES = 10;

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
    throw Object.assign(new Error('the arrangement is being changed elsewhere'), { code: 'busy' });
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
        await lease();
        const current = await read();
        const next = op(current);
        if (!same(next, current)) await ref.set(next);
        return next;
      };
      const p = queue.then(run, run);
      queue = p.catch(() => {});
      return p;
    },
  };
}
