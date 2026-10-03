import { afterEach, describe, expect, it } from "vitest";
import { rallyOnce } from "./rally";

afterEach(() => localStorage.clear());

describe("rally", () => {
  it("fires once per artifact, on v10 only", () => {
    expect(rallyOnce("a", 9)).toBe(false);
    expect(rallyOnce("a", 10)).toBe(true);
    expect(rallyOnce("a", 10)).toBe(false);
    expect(rallyOnce("b", 10)).toBe(true);
  });
});
