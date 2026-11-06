import { describe, expect, it } from "vitest";
import { anchorText, browserOf, stateTone, timeIn, viewportText } from "../src/report-text";

describe("anchorText", () => {
  it("names the kind and what the anchor points at", () => {
    expect(anchorText({ kind: "range", quote: "Total" })).toBe("Text: “Total”");
    expect(anchorText({ kind: "element", name: "Save button", selector: "#save" })).toBe("Element: Save button");
    expect(anchorText({ kind: "artifact" })).toBe("The whole page");
    expect(anchorText(null)).toBeNull();
    expect(anchorText(42)).toBeNull();
  });
});

describe("browserOf", () => {
  it("names the browser and its major version", () => {
    expect(browserOf("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")).toBe("Chrome 131");
    expect(browserOf("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15")).toBe("Safari 18");
    expect(browserOf("curl/8")).toBe("curl/8");
    expect(browserOf(undefined)).toBeNull();
  });
});

describe("timeIn", () => {
  it("reads a valid time in a named zone", () => {
    expect(timeIn("2026-10-29T07:12:03.120Z", "UTC", "en-US")).toMatch(/2026.*UTC/);
  });
  it("gives null for a time it cannot read", () => {
    expect(timeIn("not a date")).toBeNull();
  });
});

describe("viewportText", () => {
  it("gives the size, the dpr only off 1, and the colour scheme", () => {
    expect(viewportText({ w: 1024, h: 768, dpr: 1 })).toBe("1024 × 768");
    expect(viewportText({ w: 390, h: 844, dpr: 3 }, "dark")).toBe("390 × 844 @3x · dark");
    expect(viewportText(undefined)).toBeNull();
  });
});

describe("stateTone", () => {
  it("maps every state to one tone", () => {
    expect(stateTone("open")).toBe("info");
    expect(stateTone("claimed")).toBe("progress");
    expect(stateTone("fix_proposed")).toBe("attention");
    expect(stateTone("verified")).toBe("done");
    expect(stateTone("resolved")).toBe("neutral");
  });
});
