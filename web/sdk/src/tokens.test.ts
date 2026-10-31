import { describe, expect, it } from "vitest";
import { contrast, contrastFailures, cssValue, cssVariables, regionLines, regionSpecs, resolve, rgba, syncRegions, TokenError, type TokenGroup, typeOf } from "./tokens";

const file: TokenGroup = {
  color: {
    $type: "color",
    white: { $value: "#ffffff" },
    ink: { $value: "#1c1b19" },
    grey: { $value: "#5f5d58" },
    pale: { $value: { colorSpace: "srgb", components: [0.9, 0.9, 0.9] } },
  },
  theme: {
    light: {
      card: { $value: "{color.white}" },
      fg: { $value: "{color.ink}" },
      muted: { $value: "{color.grey}" },
      faint: { $value: "{color.pale}" },
      tag: { red: { bg: { $value: "{theme.light.card}" } } },
      radius: { $type: "dimension", $value: { value: 10, unit: "px" } },
      ease: { $type: "cubicBezier", $value: [0.4, 0, 0.2, 1] },
    },
  },
  $extensions: { "dev.empathic.clax": { contrast: [
    { fg: "theme.light.fg", bg: "theme.light.card", min: 4.5 },
    { fg: "theme.light.muted", bg: "theme.light.card", min: 4.5 },
    { fg: "theme.light.faint", bg: "theme.light.card", min: 3 },
  ] } },
};

describe("design tokens", () => {
  it("follows aliases, through more than one hop, and takes the target's type", () => {
    expect(resolve(file, "theme.light.fg")).toBe("#1c1b19");
    expect(resolve(file, "theme.light.tag.red.bg")).toBe("#ffffff");
    expect(typeOf(file, "theme.light.tag.red.bg")).toBe("color");
  });

  it("refuses a missing token, a group and an alias cycle", () => {
    expect(() => resolve(file, "theme.light.nope")).toThrow(TokenError);
    expect(() => resolve(file, "theme.light")).toThrow("is a group");
    const loop: TokenGroup = { a: { $value: "{b}" }, b: { $value: "{a}" } };
    expect(() => resolve(loop, "a")).toThrow("alias cycle: a -> b -> a");
  });

  it("reads hex colours of every length and sRGB objects", () => {
    expect(rgba("#fff")).toEqual([255, 255, 255, 1]);
    expect(rgba("#00000080")).toEqual([0, 0, 0, 128 / 255]);
    expect(rgba({ colorSpace: "srgb", components: [1, 0, 0] })).toEqual([255, 0, 0, 1]);
    expect(() => rgba("red")).toThrow(TokenError);
  });

  it("measures WCAG 2.x contrast", () => {
    expect(contrast("#000", "#fff")).toBeCloseTo(21, 5);
    expect(contrast("#fff", "#fff")).toBeCloseTo(1, 5);
    expect(contrast("#fff", "#000")).toBeCloseTo(contrast("#000", "#fff"), 10);
    // theme.css's own record: fg on --card is 17.2, muted on --card 6.6.
    expect(contrast("#1c1b19", "#ffffff")).toBeCloseTo(17.2, 1);
    expect(contrast("#5f5d58", "#ffffff")).toBeCloseTo(6.6, 1);
    // A translucent black over white is a grey.
    expect(contrast("#00000080", "#ffffff")).toBeCloseTo(contrast("#7f7f7f", "#ffffff"), 1);
  });

  it("names each pair of the contract that falls short", () => {
    const failures = contrastFailures(file);
    expect(failures.map(f => f.fg)).toEqual(["theme.light.faint"]);
    expect(failures[0].ratio).toBeLessThan(3);
  });

  it("writes a theme's roles as CSS custom properties", () => {
    expect(cssVariables(file, "theme.light")).toEqual([
      "--card: #ffffff;", "--fg: #1c1b19;", "--muted: #5f5d58;", "--faint: #e6e6e6;", "--tag-red-bg: var(--card);", "--radius: 10px;", "--ease: cubic-bezier(0.4,0,0.2,1);",
    ]);
    expect(cssValue(["ui-sans-serif", "Segoe UI"], "fontFamily")).toBe(`ui-sans-serif, "Segoe UI"`);
  });

  it("writes a dark theme as its overrides of the light one, and leaves the rest of a stylesheet alone", () => {
    const themes: TokenGroup = {
      c: { $type: "color", a: { $value: "#111111" }, b: { $value: "#eeeeee" } },
      light: { bg: { $value: "{c.b}" }, fg: { $value: "{c.a}" }, focus: { $value: "{light.fg}" } },
      dark: { bg: { $value: "{c.a}" }, fg: { $value: "{c.a}" }, focus: { $value: "{dark.fg}" } },
    };
    expect(regionLines(themes, "dark over light")).toEqual(["--bg: #111111;"]);
    const css = ":root {\n  /* tokens:begin light */\n  --bg: red;\n  /* tokens:end */\n  --other: 1px;\n}\n@media (x) {\n  :root {\n    /* tokens:begin dark over light */\n    /* tokens:end */\n  }\n}\n";
    expect(regionSpecs(css)).toEqual(["light", "dark over light"]);
    expect(syncRegions(css, themes)).toBe(":root {\n  /* tokens:begin light */\n  --bg: #eeeeee;\n  --fg: #111111;\n  --focus: var(--fg);\n  /* tokens:end */\n  --other: 1px;\n}\n@media (x) {\n  :root {\n    /* tokens:begin dark over light */\n    --bg: #111111;\n    /* tokens:end */\n  }\n}\n");
  });
});
