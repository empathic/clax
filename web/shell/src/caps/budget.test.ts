import { afterEach, describe, expect, it, vi } from "vitest";
import { forgetBudgets, takeSlot } from "./budget";

const LIMITS = { gapMs: 2000, perWindow: { n: 3, ms: 60_000 } };

describe("takeSlot", () => {
  afterEach(() => { sessionStorage.clear(); forgetBudgets(); vi.restoreAllMocks(); });

  it("grants slots within the gap and window and names the wait otherwise", () => {
    expect(takeSlot("k", LIMITS, 0)).toBe(0);
    expect(takeSlot("k", LIMITS, 500)).toBe(1500);
    expect(takeSlot("k", LIMITS, 2000)).toBe(0);
    expect(takeSlot("k", LIMITS, 4000)).toBe(0);
    expect(takeSlot("k", LIMITS, 6000)).toBe(54_000);
    expect(takeSlot("k", LIMITS, 60_000)).toBe(0);
    expect(takeSlot("other", LIMITS, 60_000)).toBe(0);
  });

  it("survives a reload through sessionStorage", async () => {
    expect(takeSlot("k", LIMITS, 0)).toBe(0);
    vi.resetModules();
    const fresh = await import("./budget");
    expect(fresh.takeSlot("k", LIMITS, 100)).toBe(1900);
  });

  it("keeps the budget in memory when sessionStorage throws", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("denied"); });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("denied"); });
    expect(takeSlot("k", LIMITS, 0)).toBe(0);
    expect(takeSlot("k", LIMITS, 10)).toBe(1990);
  });

  it("ignores a corrupt stored value", () => {
    sessionStorage.setItem("k", "{nope");
    expect(takeSlot("k", LIMITS, 0)).toBe(0);
  });
});
