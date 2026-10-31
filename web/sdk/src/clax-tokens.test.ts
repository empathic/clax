// web/sdk/tokens/clax.tokens.json, Clax's one token file: its contrast
// contract holds, its two themes name the same roles, and it gives the
// colours, fonts and radii the shell's theme.css and the overlay's
// tokens.css declare by hand today (which the CSS will be generated from).
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { contrastFailures, contrastPairs, cssVariables, rgba, tokenPaths, type TokenGroup } from "./tokens";

const read = (rel: string) => readFileSync(new URL(rel, import.meta.url), "utf8");
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

  describe("matches what the CSS declares by hand", () => {
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
      const got = colours(theme(path));
      expect(want.size).toBeGreaterThan(10);
      for (const [name, c] of want) expect(got.get(name), `--${name}`).toBe(c);
    });

    it("the shell's fonts and radii", () => {
      const base = theme("base");
      for (const name of ["font", "mono", "radius", "radius-sm", "radius-xs", "gutter"]) expect(base.get(name), `--${name}`).toBe(shellLight.get(name));
    });
  });
});
