import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SCROLL_EVERY_MS, reportScroll, restoreScroll } from "../src/scroll";

function fakeTimers() {
  let next = 1;
  const due = new Map<number, { fn: () => void; ms: number }>();
  return {
    t: { set: (fn: () => void, ms: number) => { due.set(next, { fn, ms }); return next++; }, clear: (id: number) => { due.delete(id); } },
    /** Runs the timers due now, returning the delays they were set with. */
    fire: () => { const all = [...due.values()]; due.clear(); all.forEach(d => d.fn()); return all.map(d => d.ms); },
    pending: () => due.size,
    delays: () => [...due.values()].map(d => d.ms),
  };
}
const setScrollY = (y: number) => Object.defineProperty(window, "scrollY", { value: y, configurable: true });
const setHeight = (h: number) => Object.defineProperty(document.documentElement, "scrollHeight", { value: h, configurable: true });

describe("reportScroll", () => {
  let stop: (() => void) | null = null;
  afterEach(() => { stop?.(); stop = null; setScrollY(0); });

  it("posts at most once per interval while the page scrolls, then the last position", () => {
    const f = fakeTimers();
    const post = vi.fn();
    stop = reportScroll(window, post, () => "index.html", f.t);
    setScrollY(10); window.dispatchEvent(new Event("scroll"));
    setScrollY(20); window.dispatchEvent(new Event("scroll"));
    setScrollY(30); window.dispatchEvent(new Event("scroll"));
    expect(post.mock.calls).toEqual([[{ type: "clax:scroll", file: "index.html", y: 10 }]]);
    expect(f.delays()).toEqual([SCROLL_EVERY_MS]);
    f.fire();
    expect(post.mock.calls.at(-1)).toEqual([{ type: "clax:scroll", file: "index.html", y: 30 }]);
    expect(post).toHaveBeenCalledTimes(2);
    // Quiet through the next interval: nothing more, and no timer left.
    f.fire();
    expect(post).toHaveBeenCalledTimes(2);
    expect(f.pending()).toBe(0);
  });

  it("reports the file the page is on when it posts", () => {
    const f = fakeTimers();
    const post = vi.fn();
    let file = "a.html";
    stop = reportScroll(window, post, () => file, f.t);
    window.dispatchEvent(new Event("scroll"));
    file = "b.html";
    f.fire();
    window.dispatchEvent(new Event("scroll"));
    expect(post.mock.calls.map(c => c[0].file)).toEqual(["a.html", "b.html"]);
  });

  it("stops listening and drops its timer when stopped", () => {
    const f = fakeTimers();
    const post = vi.fn();
    const s = reportScroll(window, post, () => "index.html", f.t);
    window.dispatchEvent(new Event("scroll"));
    window.dispatchEvent(new Event("scroll"));
    s();
    expect(f.pending()).toBe(0);
    window.dispatchEvent(new Event("scroll"));
    expect(post).toHaveBeenCalledTimes(1);
  });
});

describe("restoreScroll", () => {
  let scrollTo: ReturnType<typeof vi.spyOn>;
  beforeEach(() => {
    scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    Object.defineProperty(window, "innerHeight", { value: 600, configurable: true });
  });
  afterEach(() => { scrollTo.mockRestore(); setHeight(0); });

  it("scrolls at once when the document is tall enough", () => {
    const f = fakeTimers();
    setHeight(2000);
    restoreScroll(window, 1400, f.t);
    expect(scrollTo).toHaveBeenCalledWith(0, 1400);
    expect(f.pending()).toBe(0);
  });

  it("waits for the document to grow, retrying on the timer", () => {
    const f = fakeTimers();
    setHeight(800);
    restoreScroll(window, 1400, f.t);
    expect(scrollTo).not.toHaveBeenCalled();
    expect(f.pending()).toBe(1);
    f.fire();
    expect(scrollTo).not.toHaveBeenCalled();
    setHeight(2000);
    f.fire();
    expect(scrollTo).toHaveBeenCalledWith(0, 1400);
    expect(f.pending()).toBe(0);
  });

  it("gives up after 2000 ms in all", () => {
    const f = fakeTimers();
    setHeight(800);
    restoreScroll(window, 1400, f.t);
    let waited = 0;
    for (let i = 0; i < 1000 && f.pending(); i++) waited += f.fire().reduce((a, b) => a + b, 0);
    expect(waited).toBe(2000);
    expect(f.pending()).toBe(0);
    expect(scrollTo).not.toHaveBeenCalled();
  });

  it("scrolls to the top at once, and ignores a position that is not a number", () => {
    const f = fakeTimers();
    setHeight(0);
    restoreScroll(window, 0, f.t);
    expect(scrollTo).toHaveBeenCalledWith(0, 0);
    restoreScroll(window, Number.NaN, f.t);
    restoreScroll(window, -5, f.t);
    expect(scrollTo).toHaveBeenCalledTimes(1);
    expect(f.pending()).toBe(0);
  });
});
