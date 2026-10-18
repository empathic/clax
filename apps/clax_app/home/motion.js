// The tiles' motion: each moving tile has a spring per axis that carries
// its offset (a transform from where the page lays it out) to zero, on one
// frame loop over the page's clock. A dragged tile is held where the
// pointer puts it; one sent `to` an offset springs there and stays there.
// `onSettle` runs each time every tile has come to rest. With reduced
// motion, tiles go straight to their places. `stop()` ends
// every motion and puts each tile at its place; `rest()` does too, and
// settles.
import { springStep, atRest } from './physics.js';

export function createMotion(clock, { reduced = false, onSettle = () => {} } = {}) {
  const tiles = new Map();
  let stopFrame = null;
  let last = 0;
  const draw = (t) => {
    t.el.style.transform = t.x.x === 0 && t.y.x === 0 && !t.held ? '' : `translate(${Math.round(t.x.x * 10) / 10}px, ${Math.round(t.y.x * 10) / 10}px)`;
  };
  // A tile's target offset: 0, its place, unless it was sent elsewhere.
  const tx = (t) => t.to?.x ?? 0;
  const ty = (t) => t.to?.y ?? 0;
  const settle = () => { if (tiles.size === 0) onSettle(); };
  const loop = (now) => {
    const dt = Math.min(0.1, (now - last) / 1000);
    last = now;
    const arrived = [];
    for (const [key, t] of tiles) {
      if (t.held) continue;
      t.x = springStep(t.x, tx(t), dt);
      t.y = springStep(t.y, ty(t), dt);
      if (atRest(t.x, tx(t)) && atRest(t.y, ty(t))) {
        t.x = { x: tx(t), v: 0 };
        t.y = { x: ty(t), v: 0 };
        tiles.delete(key);
        if (t.to?.done) arrived.push(t.to.done);
      }
      draw(t);
    }
    stopFrame = null;
    for (const done of arrived) done();
    if ([...tiles.values()].some((t) => !t.held)) run();
    else settle();
  };
  // Starts the frame loop unless it runs; time is counted from now.
  const run = () => {
    if (stopFrame) return;
    last = clock.now();
    stopFrame = clock.frame(loop);
  };
  const put = (key, el, x, vx, y, vy, held = false) => {
    const t = { el, x: { x, v: vx }, y: { x: y, v: vy }, held };
    tiles.set(key, t);
    draw(t);
    if (!held) run();
  };
  const place = (key, el) => {
    tiles.delete(key);
    el.style.transform = '';
    settle();
  };
  return {
    from(key, el, dx, dy) {
      if (reduced) return place(key, el);
      const t = tiles.get(key);
      put(key, el, dx, t?.x.v ?? 0, dy, t?.y.v ?? 0);
    },
    hold(key, el, dx, dy) { put(key, el, dx, 0, dy, 0, true); },
    /** Springs tile `key` from where the motion has it to the offset (dx,
     * dy) from its place, leaves it there, and calls `done` once it
     * arrives; any later move of the tile drops `done`. */
    to(key, el, dx, dy, done) {
      const t = tiles.get(key);
      if (reduced) {
        tiles.delete(key);
        draw({ el, x: { x: dx }, y: { x: dy } });
        done?.();
        settle();
        return;
      }
      tiles.set(key, { el, x: t?.x ?? { x: 0, v: 0 }, y: t?.y ?? { x: 0, v: 0 }, held: false, to: { x: dx, y: dy, done } });
      run();
    },
    let(key) {
      const t = tiles.get(key);
      if (!t) return;
      if (reduced) return place(key, t.el);
      t.held = false;
      run();
    },
    bump(key, el, vx, vy) {
      if (reduced) return;
      const t = tiles.get(key);
      put(key, el, t?.x.x ?? 0, (t?.x.v ?? 0) + vx, t?.y.x ?? 0, (t?.y.v ?? 0) + vy);
    },
    get moving() { return tiles.size > 0; },
    stop() {
      stopFrame?.();
      stopFrame = null;
      for (const t of tiles.values()) t.el.style.transform = '';
      tiles.clear();
    },
    /** Every tile at its place at once, held ones too, then `onSettle`:
     * for a page that may run no frames (hidden) and must still settle. */
    rest() {
      this.stop();
      onSettle();
    },
  };
}
