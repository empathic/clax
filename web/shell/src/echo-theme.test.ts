// Echo's base layer (spec §8, "Look"): two type voices, sentence case,
// swap-only fonts, and the people/agent colours in both themes.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const css = readFileSync(join(__dirname, "theme.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
const faces = [...css.matchAll(/@font-face\s*\{([^}]*)\}/g)].map(m => m[1]);
const block = (sel: string) => [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].filter(m => m[1].split(",").map(s => s.trim()).includes(sel)).map(m => m[2]).join(";");

describe("Echo theme", () => {
  it("self-hosts exactly three faces, every one swap, the condensed one at 600 only", () => {
    const real = faces.filter(f => /url\(/.test(f));
    expect(real).toHaveLength(3);
    for (const f of real) expect(f).toMatch(/font-display:\s*swap/);
    const condensed = real.filter(f => /IBM Plex Sans Condensed/.test(f));
    expect(condensed).toHaveLength(1);
    expect(condensed[0]).toMatch(/font-weight:\s*600/);
    expect(condensed[0]).toMatch(/\/_clax\/fonts\/ibm-plex-sans-condensed-latin-600\.woff2/);
  });

  it("sets nothing in tracked capitals", () => {
    expect(css).not.toMatch(/text-transform:\s*uppercase/);
    expect(css).not.toMatch(/letter-spacing:\s*\.0[4-9]em/);
  });

  it("defines people and agent colours for light and both dark paths", () => {
    expect(block(":root")).toMatch(/--you:\s*#ed5439/);
    expect(block(":root")).toMatch(/--agent:\s*#457d26/);
    expect(block(':root[data-theme="dark"]')).toMatch(/--agent:\s*#8cc46b/);
    expect(css).toMatch(/:root:not\(\[data-theme="light"\]\)\s*\{\s*@media \(prefers-color-scheme: dark\)\s*\{[^}]*--agent:\s*#8cc46b/);
  });

  it("sets buttons in the condensed face, sentence case, and stops faux bold", () => {
    expect(block("button")).toMatch(/font:\s*600 14px\/1(\.\d+)? var\(--grot\)/);
    expect(block(":root")).toMatch(/font-synthesis:\s*none/);
  });
});
