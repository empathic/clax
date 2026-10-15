import { describe, expect, it } from "vitest";
import { notSentLabel, sinceLabel } from "./link-model";

describe("sinceLabel", () => {
  it("names today's time alone, another day's date too", () => {
    const now = new Date("2026-10-15T15:00:00");
    expect(sinceLabel("2026-10-15T09:05:00", now)).toMatch(/09:05|9:05/);
    expect(sinceLabel("2026-10-14T09:05:00", now)).toMatch(/Oct 14|14 Oct/);
    expect(sinceLabel("nonsense", now)).toBe("nonsense");
  });
});

describe("notSentLabel", () => {
  it("counts the comments and the questions not sent", () => {
    expect(notSentLabel(1, 0)).toBe("1 comment");
    expect(notSentLabel(2, 0)).toBe("2 comments");
    expect(notSentLabel(0, 1)).toBe("1 question");
    expect(notSentLabel(3, 2)).toBe("3 comments and 2 questions");
  });
});
