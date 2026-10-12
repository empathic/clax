import { afterEach, describe, expect, it, vi } from "vitest";
import { HOLD_MS, LongPress, SLOP_PX } from "../src/long-press";

function fakeTimers() {
  let next = 1;
  const due = new Map<number, () => void>();
  return {
    t: { set: (fn: () => void, _ms: number) => { due.set(next, fn); return next++; }, clear: (id: number) => { due.delete(id); } },
    fire: () => { const fns = [...due.values()]; due.clear(); fns.forEach(f => f()); },
    pending: () => due.size,
  };
}
function touch(type: string, pts: [number, number][]) {
  const touches = pts.map(([x, y], i) => ({ identifier: i, clientX: x, clientY: y, target: document.body }) as unknown as Touch);
  const e = new TouchEvent(type, { bubbles: true, cancelable: true, touches: type === "touchend" || type === "touchcancel" ? [] : touches, changedTouches: touches });
  document.body.dispatchEvent(e);
  return e;
}
const hooks = () => ({ armed: vi.fn(), pick: vi.fn(), abandoned: vi.fn() });

let lp: LongPress | null = null;
afterEach(() => { lp?.set(false); lp = null; });

describe("LongPress", () => {
  it("arms after the hold and picks on release", () => {
    const f = fakeTimers(); const h = hooks();
    lp = new LongPress(document, h, f.t, { trustedOnly: false }); lp.set(true);
    touch("touchstart", [[20, 30]]); expect(f.pending()).toBe(1);
    f.fire(); expect(h.armed).toHaveBeenCalledWith(20, 30);
    const end = touch("touchend", [[22, 31]]);
    expect(h.pick).toHaveBeenCalledWith(22, 31); expect(end.defaultPrevented).toBe(true);
    expect(h.abandoned).not.toHaveBeenCalled();
  });

  it("a short tap neither arms nor picks", () => {
    const f = fakeTimers(); const h = hooks();
    lp = new LongPress(document, h, f.t, { trustedOnly: false }); lp.set(true);
    touch("touchstart", [[20, 30]]);
    const end = touch("touchend", [[20, 30]]);
    expect(f.pending()).toBe(0);
    expect(end.defaultPrevented).toBe(false);
    expect(h.armed).not.toHaveBeenCalled();
    expect(h.pick).not.toHaveBeenCalled();
    expect(h.abandoned).not.toHaveBeenCalled();
  });

  it("moving beyond the slop abandons", () => {
    const f = fakeTimers(); const h = hooks();
    lp = new LongPress(document, h, f.t, { trustedOnly: false }); lp.set(true);
    touch("touchstart", [[20, 30]]); f.fire();
    touch("touchmove", [[20 + SLOP_PX, 30]]);
    expect(h.abandoned).not.toHaveBeenCalled();
    touch("touchmove", [[20 + SLOP_PX + 1, 30]]);
    expect(h.abandoned).toHaveBeenCalledTimes(1);
    const end = touch("touchend", [[31, 30]]);
    expect(h.pick).not.toHaveBeenCalled();
    expect(end.defaultPrevented).toBe(false);
  });

  it("moving beyond the slop before the hold never arms", () => {
    const f = fakeTimers(); const h = hooks();
    lp = new LongPress(document, h, f.t, { trustedOnly: false }); lp.set(true);
    touch("touchstart", [[20, 30]]);
    touch("touchmove", [[20, 30 + SLOP_PX + 1]]);
    expect(f.pending()).toBe(0);
    touch("touchend", [[20, 41]]);
    expect(h.armed).not.toHaveBeenCalled();
    expect(h.abandoned).not.toHaveBeenCalled();
    expect(h.pick).not.toHaveBeenCalled();
  });

  it("a second finger abandons", () => {
    const f = fakeTimers(); const h = hooks();
    lp = new LongPress(document, h, f.t, { trustedOnly: false }); lp.set(true);
    touch("touchstart", [[20, 30]]); f.fire();
    touch("touchstart", [[20, 30], [200, 300]]);
    expect(h.abandoned).toHaveBeenCalledTimes(1);
    touch("touchend", [[20, 30]]);
    expect(h.pick).not.toHaveBeenCalled();
    // Two fingers from the start never arm.
    touch("touchstart", [[20, 30], [200, 300]]);
    expect(f.pending()).toBe(0);
  });

  it("touchcancel abandons", () => {
    const f = fakeTimers(); const h = hooks();
    lp = new LongPress(document, h, f.t, { trustedOnly: false }); lp.set(true);
    touch("touchstart", [[20, 30]]); f.fire();
    touch("touchcancel", [[20, 30]]);
    expect(h.abandoned).toHaveBeenCalledTimes(1);
    touch("touchend", [[20, 30]]);
    expect(h.pick).not.toHaveBeenCalled();
  });

  it("off stops listening", () => {
    const f = fakeTimers(); const h = hooks();
    lp = new LongPress(document, h, f.t, { trustedOnly: false }); lp.set(true);
    lp.set(false);
    touch("touchstart", [[20, 30]]);
    expect(f.pending()).toBe(0);
  });

  it("off mid-hold drops the hold", () => {
    const f = fakeTimers(); const h = hooks();
    lp = new LongPress(document, h, f.t, { trustedOnly: false }); lp.set(true);
    touch("touchstart", [[20, 30]]);
    lp.set(false);
    expect(f.pending()).toBe(0);
    lp.set(true);
    touch("touchend", [[20, 30]]);
    expect(h.pick).not.toHaveBeenCalled();
  });

  it("ignores touches the page dispatched itself unless told otherwise", () => {
    const f = fakeTimers(); const h = hooks();
    lp = new LongPress(document, h, f.t); lp.set(true);
    touch("touchstart", [[20, 30]]);
    expect(f.pending()).toBe(0);
  });

  it(`uses ${HOLD_MS} ms`, () => {
    const f = fakeTimers(); const h = hooks();
    const set = vi.spyOn(f.t, "set");
    lp = new LongPress(document, h, f.t, { trustedOnly: false }); lp.set(true);
    touch("touchstart", [[20, 30]]);
    expect(set).toHaveBeenCalledWith(expect.any(Function), HOLD_MS);
    expect(HOLD_MS).toBe(450);
    expect(SLOP_PX).toBe(10);
  });
});

describe("LongPress.holding", () => {
  it("is true from the finger's press until its release or abandonment", () => {
    const f = fakeTimers(); const h = hooks();
    lp = new LongPress(document, h, f.t, { trustedOnly: false }); lp.set(true);
    expect(lp.holding).toBe(false);
    touch("touchstart", [[20, 30]]); expect(lp.holding).toBe(true);
    f.fire(); expect(lp.holding).toBe(true);
    touch("touchend", [[20, 30]]); expect(lp.holding).toBe(false);
    touch("touchstart", [[20, 30]]);
    touch("touchmove", [[40, 30]]); expect(lp.holding).toBe(false);
    touch("touchstart", [[20, 30]]);
    lp.set(false); expect(lp.holding).toBe(false);
  });
});
