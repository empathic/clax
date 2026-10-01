import { describe, expect, it } from "vitest";
import { AnchorHandles } from "./anchor-handles";

describe("AnchorHandles", () => {
  it("gives each thread one opaque handle until forgotten, and maps it back", () => {
    const h = new AnchorHandles();
    const a = h.handle("t1");
    expect(a).toMatch(/^a[0-9a-f]{24}$/);
    expect(h.handle("t1")).toBe(a);
    expect(h.known("t1")).toBe(a);
    expect(h.thread(a)).toBe("t1");
    expect(h.known("t2")).toBeNull();
    h.forget();
    expect(h.thread(a)).toBeUndefined();
    expect(h.known("t1")).toBeNull();
    expect(h.handle("t1")).not.toBe(a);
  });
});
