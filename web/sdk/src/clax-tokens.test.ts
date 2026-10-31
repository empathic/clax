// web/sdk/tokens/clax.tokens.json, Clax's one token file: its contrast
// contract holds, its two themes name the same roles, the shell's
// theme.css and the overlay's tokens.css hold its tokens in their generated
// regions as tokens.ts writes them, and every colour, font and radius they
// declare is the token file's. With CLAX_WRITE_TOKENS=1 the regions test
// writes the regions instead of failing:
//   CLAX_WRITE_TOKENS=1 pnpm vitest run sdk/src/clax-tokens.test.ts
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { contrastFailures, contrastPairs, cssVariables, regionSpecs, rgba, syncRegions, tokenPaths, type TokenGroup } from "./tokens";

// Paths from __dirname: under jsdom, import.meta.url is no file: URL.
const read = (rel: string) => readFileSync(join(__dirname, rel), "utf8");
const file: TokenGroup = JSON.parse(read("../tokens/clax.tokens.json"));

/** The custom properties declared in the CSS block that starts at `opener`
 * and ends at the first line that is `closer`. */
function block(css: string, opener: string, closer: string): Map<string, string> {
  const start = css.indexOf(opener);
  if (start < 0) throw new Error(`no ${opener}`);
  const end = css.indexOf(`\n${closer}\n`, start);
  const body = css.slice(start + opener.length, end < 0 ? undefined : end);
  return new Map([...body.matchAll(/--([\w-]+):\s*([^;]+);/g)].map(m => [m[1], m[2].trim()]));
}

/** `decls` with each `var(--x)` replaced by what it names. */
function resolved(decls: Map<string, string>): Map<string, string> {
  const at = (v: string, depth = 0): string => {
    const ref = /^var\(--([\w-]+)\)$/.exec(v);
    if (!ref || depth > 8) return v;
    const next = decls.get(ref[1]);
    if (next === undefined) throw new Error(`${v} names nothing`);
    return at(next, depth + 1);
  };
  return new Map([...decls].map(([k, v]) => [k, at(v)]));
}

/** A CSS colour (hex or rgba()) as rounded channels and alpha, or null for
 * anything else. */
function colour(v: string): string | null {
  const m = /^rgba\(([^)]+)\)$/.exec(v);
  if (m) {
    const [r, g, b, a] = m[1].split(",").map(Number);
    return `${r},${g},${b},${a.toFixed(3)}`;
  }
  if (!/^#[0-9a-f]+$/i.test(v)) return null;
  const [r, g, b, a] = rgba(v);
  return `${r},${g},${b},${a.toFixed(3)}`;
}

/** A theme's tokens as `name -> css value`. */
function theme(path: string): Map<string, string> {
  return new Map(cssVariables(file, path).map(line => {
    const m = /^--([\w-]+): (.*);$/.exec(line)!;
    return [m[1], m[2]];
  }));
}

/** The colours of `decls` that are colours, as `colour` gives them. */
const colours = (decls: Map<string, string>) =>
  new Map([...decls].flatMap(([k, v]) => { const c = colour(v); return c ? [[k, c] as const] : []; }));

describe("clax.tokens.json", () => {
  it("meets its contrast contract, which covers both themes", () => {
    expect(contrastPairs(file).length).toBeGreaterThan(60);
    expect(contrastFailures(file)).toEqual([]);
  });

  it("gives its light and dark themes the same roles", () => {
    const roles = (t: string) => tokenPaths((file.theme as TokenGroup)[t] as TokenGroup).sort();
    expect(roles("dark")).toEqual(roles("light"));
  });

  it.each([
    { rel: "shell/src/theme.css", specs: ["theme.light base", "theme.dark over theme.light", "theme.dark over theme.light"] },
    { rel: "overlay/src/tokens.css", specs: ["theme.light base except tag pin", "theme.dark over theme.light except tag pin"] },
  ])("writes $rel's generated regions", ({ rel, specs }) => {
    const path = join(__dirname, "../..", rel);
    const css = readFileSync(path, "utf8");
    expect(regionSpecs(css)).toEqual(specs);
    const synced = syncRegions(css, file);
    if (process.env.CLAX_WRITE_TOKENS === "1") { if (synced !== css) writeFileSync(path, synced); return; }
    expect(synced, `${rel} differs from the token file: run with CLAX_WRITE_TOKENS=1`).toBe(css);
  });

  describe("matches what the CSS declares", () => {
    const shell = read("../../shell/src/theme.css");
    const shellLight = block(shell, ":root {", "}");
    const shellDark = new Map([...shellLight, ...block(shell, ':root[data-theme="dark"] {', "}")]);
    const overlay = read("../../overlay/src/tokens.css");
    const overlayLight = block(overlay, ":host {", "}");
    const overlayDark = new Map([...overlayLight, ...block(overlay, "@media (prefers-color-scheme: dark) {\n  :host {", "  }")]);

    it.each([
      ["the shell, light", shellLight, "theme.light"],
      ["the shell, dark", shellDark, "theme.dark"],
      ["the overlay, light", overlayLight, "theme.light"],
      ["the overlay, dark", overlayDark, "theme.dark"],
    ])("%s: every colour", (_, css, path) => {
      const want = colours(resolved(css));
      const got = colours(resolved(theme(path)));
      expect(want.size).toBeGreaterThan(10);
      for (const [name, c] of want) expect(got.get(name), `--${name}`).toBe(c);
    });

    it("the shell's fonts and radii", () => {
      const base = theme("base");
      for (const name of ["font", "mono", "radius", "radius-sm", "radius-xs", "gutter"]) expect(base.get(name), `--${name}`).toBe(shellLight.get(name));
    });
  });
});
