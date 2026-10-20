import { afterEach, describe, expect, it, vi } from "vitest";
import { Lifecycle, backoff, onPageCache, retrying } from "./lifecycle";

afterEach(() => { vi.useRealTimers(); });

describe("Lifecycle", () => {
  it("ends everything it owns at once, latest first, and only once", () => {
    const life = new Lifecycle();
    const order: string[] = [];
    life.defer(() => order.push("first"));
    life.defer(() => order.push("second"));
    expect(life.disposed).toBe(false);
    life.dispose();
    life.dispose();
    expect(order).toEqual(["second", "first"]);
    expect(life.disposed).toBe(true);
    expect(life.signal.aborted).toBe(true);
  });

  it("runs a teardown deferred after the end at once", () => {
    const life = new Lifecycle();
    life.dispose();
    const fn = vi.fn();
    life.defer(fn);
    expect(fn).toHaveBeenCalledOnce();
  });

  it("forgets a teardown whose remover ran", () => {
    const life = new Lifecycle();
    const fn = vi.fn();
    const forget = life.defer(fn);
    forget();
    life.dispose();
    expect(fn).not.toHaveBeenCalled();
  });

  it("keeps ending the rest when one teardown throws, then rethrows", () => {
    const life = new Lifecycle();
    const after = vi.fn();
    life.defer(after);
    life.defer(() => { throw new Error("broken teardown"); });
    expect(() => life.dispose()).toThrow("broken teardown");
    expect(after).toHaveBeenCalledOnce();
    expect(life.disposed).toBe(true);
  });

  it("removes its listeners when it ends", () => {
    const life = new Lifecycle();
    const target = new EventTarget();
    const heard = vi.fn();
    life.listen(target, "ping", heard);
    target.dispatchEvent(new Event("ping"));
    life.dispose();
    target.dispatchEvent(new Event("ping"));
    expect(heard).toHaveBeenCalledOnce();
    // A listener added after the end is never added.
    life.listen(target, "ping", heard);
    target.dispatchEvent(new Event("ping"));
    expect(heard).toHaveBeenCalledOnce();
  });

  it("clears its timeouts and intervals when it ends", () => {
    vi.useFakeTimers();
    const life = new Lifecycle();
    const once = vi.fn();
    const tick = vi.fn();
    life.timeout(once, 1000);
    life.interval(tick, 100);
    vi.advanceTimersByTime(250);
    expect(tick).toHaveBeenCalledTimes(2);
    life.dispose();
    vi.advanceTimersByTime(5000);
    expect(once).not.toHaveBeenCalled();
    expect(tick).toHaveBeenCalledTimes(2);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("lets a timer be cleared early, and forgets a timeout that fired", () => {
    vi.useFakeTimers();
    const life = new Lifecycle();
    const tick = vi.fn();
    const stop = life.interval(tick, 100);
    vi.advanceTimersByTime(100);
    stop();
    vi.advanceTimersByTime(500);
    expect(tick).toHaveBeenCalledOnce();
    const fired = vi.fn();
    life.timeout(fired, 10);
    vi.advanceTimersByTime(10);
    expect(fired).toHaveBeenCalledOnce();
    life.dispose();
    expect(fired).toHaveBeenCalledOnce();
  });

  it("ends a child with its parent, and a child alone without the parent", () => {
    const parent = new Lifecycle();
    const a = parent.child();
    const b = parent.child();
    const parentEnd = vi.fn();
    parent.defer(parentEnd);
    a.dispose();
    expect(parent.disposed).toBe(false);
    expect(parentEnd).not.toHaveBeenCalled();
    parent.dispose();
    expect(b.disposed).toBe(true);
    expect(b.signal.aborted).toBe(true);
    // A child of an ended parent starts ended.
    expect(parent.child().disposed).toBe(true);
  });

  it("aborts requests that took its signal", async () => {
    const life = new Lifecycle();
    const req = new Promise((_, reject) => life.signal.addEventListener("abort", () => reject(life.signal.reason)));
    life.dispose();
    await expect(req).rejects.toMatchObject({ name: "AbortError" });
  });
});

describe("onPageCache", () => {
  it("hears the page hide and a restore from the back/forward cache, until the lifecycle ends", () => {
    const life = new Lifecycle();
    const hide = vi.fn();
    const show = vi.fn();
    onPageCache(life, hide, show);
    const page = (type: string, persisted: boolean) => { const e = new Event(type); Object.defineProperty(e, "persisted", { value: persisted }); dispatchEvent(e); };
    page("pagehide", true);
    page("pageshow", false);
    page("pageshow", true);
    page("pagehide", false);
    expect(hide.mock.calls).toEqual([[true], [false]]);
    expect(show).toHaveBeenCalledOnce();
    life.dispose();
    page("pagehide", true);
    page("pageshow", true);
    expect(hide).toHaveBeenCalledTimes(2);
    expect(show).toHaveBeenCalledOnce();
  });
});

describe("backoff", () => {
  it("doubles from half a second up to thirty, spread by a fifth either way", () => {
    expect([0, 1, 2, 3].map(n => backoff(n, () => 0.5))).toEqual([500, 1000, 2000, 4000]);
    expect(backoff(20, () => 0.5)).toBe(30_000);
    expect(backoff(0, () => 0)).toBe(400);
    expect(backoff(0, () => 1)).toBe(600);
  });
});

describe("retrying", () => {
  it("retries a failure after a backoff, holding the trouble flag meanwhile", async () => {
    vi.useFakeTimers();
    const life = new Lifecycle();
    const trouble: boolean[] = [];
    let n = 0;
    const p = retrying(life, async () => { if (n++ < 2) throw new TypeError("network"); return "ok"; }, { trouble: on => trouble.push(on) });
    await vi.advanceTimersByTimeAsync(5000);
    await expect(p).resolves.toBe("ok");
    expect(n).toBe(3);
    expect(trouble).toEqual([true, true, false]);
    life.dispose();
  });

  it("abandons a stuck attempt through its signal and tries again", async () => {
    vi.useFakeTimers();
    const life = new Lifecycle();
    const signals: AbortSignal[] = [];
    const p = retrying(life, signal => {
      signals.push(signal);
      if (signals.length === 1) return new Promise((_, reject) => signal.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError"))));
      return Promise.resolve(7);
    }, { timeoutMs: 1000 });
    await vi.advanceTimersByTimeAsync(1000 + 600);
    await expect(p).resolves.toBe(7);
    expect(signals[0].aborted).toBe(true);
    expect(signals[1].aborted).toBe(true);
    life.dispose();
  });

  it("gives up on a failure it may not retry, and when the lifecycle ends", async () => {
    vi.useFakeTimers();
    const life = new Lifecycle();
    const refused = retrying(life, () => Promise.reject(new Error("404")), { retry: () => false });
    await expect(refused).rejects.toThrow("404");
    const forever = retrying(life, () => Promise.reject(new TypeError("down")));
    const settled = expect(forever).rejects.toMatchObject({ name: "AbortError" });
    await vi.advanceTimersByTimeAsync(100);
    life.dispose();
    await settled;
    expect(vi.getTimerCount()).toBe(0);
  });
});
