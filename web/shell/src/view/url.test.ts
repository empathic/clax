import { afterEach, describe, expect, it, vi } from "vitest";
import { setUrl, validHash } from "./url";

describe("url", () => {
  afterEach(() => { vi.restoreAllMocks(); history.replaceState(null, "", "/"); });
  it("accepts an empty or #-led fragment of at most 512 characters", () => {
    expect(validHash("")).toBe(true);
    expect(validHash("#a")).toBe(true);
    expect(validHash("a")).toBe(false);
    expect(validHash(`#${"x".repeat(512)}`)).toBe(false);
    expect(validHash(3)).toBe(false);
  });
  it("pushes or replaces, and reports a refusal instead of throwing", () => {
    const n = history.length;
    expect(setUrl("/a/x", true)).toBe(true);
    expect(history.length).toBe(n + 1);
    expect(setUrl("/a/y")).toBe(true);
    expect(location.pathname).toBe("/a/y");
    vi.spyOn(history, "pushState").mockImplementation(() => { throw new DOMException("too many", "SecurityError"); });
    expect(setUrl("/a/z", true)).toBe(false);
    expect(location.pathname).toBe("/a/y");
  });
});
