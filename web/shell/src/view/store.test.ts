import { describe, expect, it, vi } from "vitest";
import { Store } from "./store";

describe("Store", () => {
  it("calls a subscriber at once and after each real change, never for an equal patch", () => {
    const s = new Store({ a: 1, b: [1] });
    const seen = vi.fn();
    const off = s.subscribe(seen);
    expect(seen).toHaveBeenCalledTimes(1);
    s.set({ a: 1 });
    expect(seen).toHaveBeenCalledTimes(1);
    const before = s.get();
    s.set(x => ({ a: x.a + 1 }));
    expect(seen).toHaveBeenCalledTimes(2);
    expect(s.get()).not.toBe(before);
    expect(s.get()).toEqual({ a: 2, b: [1] });
    off();
    s.set({ a: 3 });
    expect(seen).toHaveBeenCalledTimes(2);
  });
});
