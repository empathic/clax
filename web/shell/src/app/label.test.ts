import { describe, expect, it } from "vitest";
import type { Anchor } from "../../../bridge/src/protocol";
import { anchorLabel, anchorQuote } from "./label";

const anchor = (over: Partial<Anchor> = {}): Anchor => ({ kind: "element", selector: "body > main > div.run:nth-of-type(2)", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html", ...over });

describe("anchorLabel", () => {
  it("is the quoted text, trimmed and cut short", () => {
    expect(anchorLabel(anchor({ quote: "  Mon   5 km \n run " }))).toBe("“Mon 5 km run”");
    expect(anchorLabel(anchor({ quote: "x".repeat(100) }))).toBe(`“${"x".repeat(60)}…”`);
  });
  it("is what the element is, from its role, tag and name, when it quotes nothing", () => {
    expect(anchorLabel(anchor(), { tag: "button", role: null, name: "Save" })).toBe("the button “Save”");
    expect(anchorLabel(anchor({ quote: "Mon5 km" }), { tag: "div", role: null, name: null, text: " Mon  5 km" })).toBe("“Mon 5 km”");
    expect(anchorLabel(anchor({ quote: "Mon5 km" }), { text: "Mon 5 km" })).toBe("“Mon 5 km”");
    expect(anchorLabel(anchor({ quote: "Mon5 km" }), { text: "\u200B" })).toBe("“Mon5 km”");
    expect(anchorLabel(anchor({ quote: "Mon5 km", label: "Mon 5 km" }))).toBe("“Mon 5 km”");
    expect(anchorLabel(anchor({ quote: "Mon5 km", label: "Mon 6 km" }))).toBe("“Mon5 km”");
    expect(anchorLabel(anchor(), { tag: "div", role: "button", name: null })).toBe("a button");
    expect(anchorLabel(anchor(), { tag: "img", role: null, name: null })).toBe("an image");
    expect(anchorLabel(anchor(), { tag: "img", role: null, name: "A cat" })).toBe("the image “A cat”");
    expect(anchorLabel(anchor({ selector: "body > h2" }))).toBe("a heading");
    expect(anchorLabel(anchor(), { tag: "div", role: null, name: "Weekly plan" })).toBe("“Weekly plan”");
  });
  it("is never a selector", () => {
    for (const a of [anchor(), anchor({ selector: "#run-3 > span.km" }), anchor({ selector: "div[data-x='1'] > div" })]) {
      const l = anchorLabel(a);
      expect(l).toBe("this spot");
    }
    expect(anchorLabel(anchor(), { tag: "<b>", role: "x y", name: null })).toBe("this spot");
  });
  it("names the whole app, a custom anchor, and a drawn area", () => {
    expect(anchorLabel(anchor({ kind: "artifact", selector: null, file: "" }))).toBe("Whole app");
    expect(anchorLabel(anchor({ kind: "custom", selector: null, custom_name: "Week 3" }))).toBe("“Week 3”");
    expect(anchorLabel(anchor({ kind: "area", area: { x: 0, y: 0, w: 1, h: 1, tag: "img" } }))).toBe("part of an image");
    expect(anchorLabel(anchor({ kind: "area", area: { x: 0, y: 0, w: 1, h: 1, tag: "div" } }))).toBe("this area");
  });
  it("fences the page's words so they never read as the app's own", () => {
    expect(anchorLabel(anchor({ kind: "custom", selector: null, custom_name: "Whole app" }))).toBe("“Whole app”");
    expect(anchorLabel(anchor({ quote: "this spot" }))).toBe("“this spot”");
    expect(anchorLabel(anchor(), { tag: "button", role: null, name: "Save” and “Delete" })).toBe("the button “Save' and 'Delete”");
    expect(anchorLabel(anchor({ quote: 'say "hi" «there»' }))).toBe("“say 'hi' 'there'”");
    expect(anchorLabel(anchor({ quote: "a＂b〝c〞d〟e‚f" }))).toBe("“a'b'c'd'e'f”");
    // Bidi, format and control characters are dropped.
    expect(anchorLabel(anchor({ quote: "abc\u202Edef\u200B\u0007" }))).toBe("“abcdef”");
    expect(anchorLabel(anchor(), { tag: "img", role: null, name: "\u202E\u2066" })).toBe("an image");
  });
});

describe("anchorQuote", () => {
  it("is the quoted text as a reader sees it, tidied, fenced-safe and cut at 280 characters", () => {
    expect(anchorQuote(anchor({ quote: "Mon5 km", label: "Mon 5 km" }))).toBe("Mon 5 km");
    expect(anchorQuote(anchor({ quote: "Mon5 km" }), { text: "Mon 5 km" })).toBe("Mon 5 km");
    expect(anchorQuote(anchor({ kind: "range", quote: "  say \"hi\"\n\u202Ethere " }))).toBe("say 'hi' there");
    expect(anchorQuote(anchor({ kind: "range", quote: "x".repeat(300) }))).toBe(`${"x".repeat(280)}…`);
    expect(anchorQuote(anchor({ quote: "x".repeat(100) }))).toBe("x".repeat(100));
  });
  it("is null for an anchor that quotes nothing", () => {
    expect(anchorQuote(anchor())).toBeNull();
    expect(anchorQuote(anchor({ quote: " \u200B " }))).toBeNull();
    expect(anchorQuote(anchor({ kind: "artifact", selector: null, file: "" }))).toBeNull();
  });
});
