// A tile's press, as one state machine over pointer events and a clock.
// A tap is the click that follows. Held HOLD_MS still, a touch lifts the
// tile; a lifted press that moves more than SLOP_PX drags it, one released
// without moving asks for the menu. A mouse drags once it moves SLOP_PX
// with its main button down, with no hold. A touch that moves SLOP_PX
// before the hold is the page scrolling: the press lets go. The click
// after a menu or a drag is swallowed. A drag ends with its recent move
// samples and, last, the release point at the clock's time.
import { SPEED_MS } from './physics.js';

export const HOLD_MS = 400;
export const SLOP_PX = 8;

export function createPress(h, clock) {
  let s = null;
  let swallow = false;
  const end = () => { s?.cancel?.(); s = null; };
  const at = (e) => ({ x: e.clientX, y: e.clientY, t: clock.now() });
  return {
    get phase() { return s?.phase ?? 'idle'; },
    down(e) {
      if (e.pointerType === 'mouse' && e.button !== 0) return;
      end();
      swallow = false;
      s = { id: e.pointerId, mouse: e.pointerType === 'mouse', x0: e.clientX, y0: e.clientY, phase: 'down', samples: [] };
      if (!s.mouse) {
        const mine = s;
        s.cancel = clock.after(HOLD_MS, () => {
          if (s !== mine || s.phase !== 'down') return;
          s.phase = 'lifted';
          h.lift();
        });
      }
    },
    move(e) {
      if (!s || e.pointerId !== s.id) return;
      const p = at(e);
      const far = Math.hypot(p.x - s.x0, p.y - s.y0) > SLOP_PX;
      if (s.phase === 'down' && far && !s.mouse) { end(); return; }
      if ((s.phase === 'down' || s.phase === 'lifted') && far) {
        s.cancel?.();
        s.phase = 'drag';
        h.dragStart(p);
      }
      if (s.phase !== 'drag') return;
      s.samples.push(p);
      while (s.samples.length > 1 && p.t - s.samples[0].t > SPEED_MS) s.samples.shift();
      h.dragMove(p);
    },
    up(e) {
      if (!s || e.pointerId !== s.id) return;
      const was = s;
      end();
      if (was.phase === 'drag') {
        swallow = true;
        const p = at(e);
        h.dragEnd(p, [...was.samples, p]);
      } else if (was.phase === 'lifted') { swallow = true; h.menu(); }
    },
    cancel(e) {
      if (!s || (e && e.pointerId !== s.id)) return;
      const was = s;
      end();
      if (was.phase === 'drag') h.dragCancel();
      else if (was.phase === 'lifted') h.unlift();
    },
    contextMenu() { return s !== null; },
    takeClick() {
      if (!swallow) return true;
      swallow = false;
      return false;
    },
  };
}
