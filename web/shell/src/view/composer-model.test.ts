import { describe, expect, it } from "vitest";
import { type Draft, composerQuote } from "./composer-model";

const d = (anchor: Record<string, unknown>, label?: string) => ({ pickId: "p", version: 1, clip: null, label, anchor: { file: "index.html", ...anchor } }) as unknown as Draft;

describe("composerQuote", () => {
  it("prefers the page's label, then the quote in guillemets cut at 160 characters, then the custom name, area label or selector", () => {
    expect(composerQuote(d({ quote: "x" }, "Chart"))).toBe("Chart");
    expect(composerQuote(d({ quote: "  a\n  b " }))).toBe("«a b»");
    expect(composerQuote(d({ quote: "q".repeat(200) }))).toBe(`«${"q".repeat(160)}…»`);
    expect(composerQuote(d({ kind: "custom", custom_name: "Row 3" }))).toBe("Row 3");
    expect(composerQuote(d({ kind: "element", selector: "body > h2" }))).toBe("body > h2");
  });
});
