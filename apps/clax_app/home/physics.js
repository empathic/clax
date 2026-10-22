// The home screen's time: the page's clock, and one for tests that moves
// only when told. Every timing rule of the page (the hold, the hover, the
// springs, the toss, the store's retries) runs on one of these.

/** The page's clock: `now()` in ms (monotonic), `date()` the wall time,
 * `after(ms, fn)` and `frame(fn)` (fn gets `now()`), each returning a
 * function that cancels it. */
export const realClock = Object.freeze({
  now: () => performance.now(),
  date: () => new Date(),
  after(ms, fn) {
    const t = setTimeout(fn, ms);
    return () => clearTimeout(t);
  },
  frame(fn) {
    const r = requestAnimationFrame(() => fn(performance.now()));
    return () => cancelAnimationFrame(r);
  },
});

/** A clock that moves only on `advance(ms)`, which runs the timers that
 * fall due in time order and a frame every 16 ms. `date()` starts at
 * `dateMs` and moves with it. */
export function manualClock(startMs = 0, dateMs = Date.UTC(2026, 9, 16, 12)) {
  let t = startMs;
  let seq = 0;
  const timers = new Map();
  let frames = new Map();
  let nextFrame = startMs + 16;
  return {
    now: () => t,
    date: () => new Date(dateMs + (t - startMs)),
    after(ms, fn) {
      const id = ++seq;
      timers.set(id, { at: t + Math.max(0, ms), fn });
      return () => timers.delete(id);
    },
    frame(fn) {
      const id = ++seq;
      frames.set(id, fn);
      return () => frames.delete(id);
    },
    advance(ms) {
      const end = t + ms;
      for (;;) {
        let due = null;
        for (const [id, x] of timers) if (x.at <= end && (!due || x.at < due[1].at)) due = [id, x];
        const at = Math.min(due ? due[1].at : Infinity, nextFrame);
        if (at > end) break;
        t = at;
        if (due && due[1].at === at) {
          timers.delete(due[0]);
          due[1].fn();
          continue;
        }
        nextFrame += 16;
        const run = frames;
        frames = new Map();
        for (const fn of run.values()) fn(t);
      }
      t = end;
    },
  };
}

/** A tile's spring, per axis, unit mass: it arrives in about 300 ms and
 * overshoots once, by about 3%. */
export const SPRING = Object.freeze({ k: 380, c: 26 });
const MAX_DT = 1 / 30;

/** `s` ({x, v}) after `dt` seconds of the spring toward `target`. A step
 * longer than 1/30 s is taken in parts, so a slow frame never throws a
 * tile off. */
export function springStep(s, target, dt, { k, c } = SPRING) {
  let { x, v } = s;
  for (let left = dt; left > 1e-9; left -= MAX_DT) {
    const h = Math.min(left, MAX_DT);
    v += (-k * (x - target) - c * v) * h;
    x += v * h;
  }
  return { x, v };
}

/** Whether a spring is close enough to `target`, slowly enough, to stop. */
export const atRest = (s, target) => Math.abs(s.x - target) < 0.5 && Math.abs(s.v) < 5;

/** A tossed tile's decay time: released at v px/s it comes to rest
 * v × TOSS_TAU px on. */
export const TOSS_TAU = 0.325;
/** The release speed, in px/s, above which a release is a toss. */
export const TOSS_MIN = 600;

/** `p` ({x, y, vx, vy}) after `dt` seconds of a toss. */
export function tossStep(p, dt) {
  const f = Math.exp(-dt / TOSS_TAU);
  const g = TOSS_TAU * (1 - f);
  return { x: p.x + p.vx * g, y: p.y + p.vy * g, vx: p.vx * f, vy: p.vy * f };
}
/** Where a toss from `p` comes to rest. */
export const tossRest = (p) => ({ x: p.x + p.vx * TOSS_TAU, y: p.y + p.vy * TOSS_TAU });
/** Whether a toss is slow enough to end. */
export const tossDone = (p) => Math.hypot(p.vx, p.vy) < 20;

/** The pointer samples a release's speed is taken over. */
export const SPEED_MS = 80;

/** The pointer's velocity in px/s over the samples ({t ms, x, y}) of the
 * SPEED_MS up to the newest, which is the release: a pointer held still
 * before the release counts as still, and one with no move in the last
 * SPEED_MS has zero velocity. Zero for fewer than two samples. */
export function velocityOf(samples) {
  if (samples.length < 2) return { vx: 0, vy: 0 };
  const last = samples[samples.length - 1];
  const first = samples.find((s) => last.t - s.t <= SPEED_MS) ?? last;
  const dt = (last.t - first.t) / 1000;
  return dt > 0 ? { vx: (last.x - first.x) / dt, vy: (last.y - first.y) / dt } : { vx: 0, vy: 0 };
}
