// The overlay's :host declares every custom property the overlay reads.
// `all: initial` (tokens.css) resets every property but the custom ones,
// which inherit through the shadow root from the deployer's page: a
// var(--x) the overlay reads and :host does not declare takes the page's
// --x, so a page's own --muted or --fg would restyle Clax's interface.
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const strip = (css: string) => css.replace(/\/\*[\s\S]*?\*\//g, "");
const tokens = strip(readFileSync(join(__dirname, "tokens.css"), "utf8"));

/** The custom properties every `:host` block of tokens.css declares, media rules' included. */
const declared = new Set([...tokens.matchAll(/:host\s*\{([^{}]*)\}/g)].flatMap(m => [...m[1].matchAll(/(--[\w-]+)\s*:/g)].map(d => d[1])));

/** The overlay's sources: its components, modules and stylesheet, not its tests. */
const sources = readdirSync(__dirname)
  .filter(f => /\.(svelte|ts|css)$/.test(f) && !f.endsWith(".test.ts") && f !== "dts-check.ts")
  .map(f => ({ f, text: f.endsWith(".css") ? strip(readFileSync(join(__dirname, f), "utf8")) : readFileSync(join(__dirname, f), "utf8") }));

/** Custom properties a source sets for its own use (a `style:--x`, or `--x:` outside tokens.css's :host). */
const local = new Set(sources.filter(s => s.f !== "tokens.css").flatMap(s => [...s.text.matchAll(/(?:style:|[\s{;"'`])(--[\w-]+)\s*[:=]/g)].map(m => m[1])));

describe("the overlay's tokens", () => {
  it("declares on :host every custom property the overlay reads, so none inherits from the page", () => {
    const missing = sources.flatMap(s => [...s.text.matchAll(/var\(\s*(--[\w-]+)/g)].map(m => m[1]).filter(n => !declared.has(n) && !local.has(n)).map(n => `${s.f}: ${n}`));
    expect(missing).toEqual([]);
  });

  it("reads tokens at all, and declares each in the light and the dark :host", () => {
    expect(declared.size).toBeGreaterThan(10);
    for (const n of ["--fg", "--muted", "--bg", "--you", "--focus"]) expect(declared, n).toContain(n);
    const dark = [...tokens.matchAll(/@media \(prefers-color-scheme: dark\)\s*\{\s*:host\s*\{([^{}]*)\}/g)].map(m => m[1]).join(";");
    expect(dark).toMatch(/--fg:/);
  });

  it("resets every inherited property on :host", () => {
    expect(tokens).toMatch(/:host\s*\{[^}]*all:\s*initial/);
  });
});
