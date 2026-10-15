import { describe, expect, it } from "vitest";
import { sinceLabel } from "./link-model";

describe("sinceLabel", () => {
  it("names today's time alone, another day's date too", () => {
    const now = new Date("2026-10-15T15:00:00");
    expect(sinceLabel("2026-10-15T09:05:00", now)).toMatch(/09:05|9:05/);
    expect(sinceLabel("2026-10-14T09:05:00", now)).toMatch(/Oct 14|14 Oct/);
    expect(sinceLabel("nonsense", now)).toBe("nonsense");
  });
});
