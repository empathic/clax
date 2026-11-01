// web/sdk/tokens/clax.tokens.json, Clax's one token file: its contrast
// contract holds in every colour profile, every profile names the same
// roles, the forced-colors one in system colours only, the shell's
// theme.css and the overlay's tokens.css hold its tokens in their generated
// regions as tokens.ts writes them, and every colour, font and radius they
// declare is the token file's. With CLAX_WRITE_TOKENS=1 the regions test
// writes the regions instead of failing:
//   CLAX_WRITE_TOKENS=1 pnpm vitest run sdk/src/clax-tokens.test.ts
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { contrastFailures, contrastPairs, cssVariables, regionSpecs, resolve, rgba, SYSTEM_COLORS, syncRegions, tokenPaths, type TokenGroup } from "./tokens";

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
  // A role that reads its public property first (`var(--clax-bg, #f7f7f5)`) declares its fallback.
  const own = (v: string) => /^var\(--clax-[\w-]+, (.*)\)$/.exec(v)?.[1] ?? v;
  return new Map([...body.matchAll(/--([\w-]+):\s*([^;]+);/g)].map(m => [m[1], own(m[2].trim())]));
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

  const PROFILES = ["light", "dark", "light-more", "dark-more", "forced"];
  const roles = (t: string) => tokenPaths((file.theme as TokenGroup)[t] as TokenGroup).sort();

  it("defines every role in every colour profile", () => {
    expect(Object.keys(file.theme as TokenGroup).filter(k => !k.startsWith("$")).sort()).toEqual([...PROFILES].sort());
    for (const t of PROFILES) expect(roles(t), t).toEqual(roles("light"));
  });

  it("holds the contrast contract in the more-contrast profiles too: each light and dark pair again", () => {
    const pairs = contrastPairs(file);
    for (const s of ["light", "dark"]) {
      const own = pairs.filter(p => p.fg.startsWith(`theme.${s}.`)).map(p => `${p.fg.slice(s.length + 7)} ${p.bg.slice(s.length + 7)} ${p.min}`);
      const more = pairs.filter(p => p.fg.startsWith(`theme.${s}-more.`)).map(p => `${p.fg.slice(s.length + 12)} ${p.bg.slice(s.length + 12)} ${p.min}`);
      expect(more, s).toEqual(own);
    }
  });

  it("makes more contrast more: no text role and no hairline is weaker than its theme's", () => {
    const ratio = (t: string, fg: string, bg: string) => {
      const [f, b] = [rgba(resolve(file, `theme.${t}.${fg}`)), rgba(resolve(file, `theme.${t}.${bg}`))];
      const l = ([r, g, bl]: number[]) => [r, g, bl].map(c => { const x = c / 255; return x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4; }).reduce((a, c, i) => a + c * [0.2126, 0.7152, 0.0722][i], 0);
      const [l1, l2] = [l(f), l(b)];
      return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
    };
    for (const s of ["light", "dark"]) for (const fg of ["fg", "muted", "border", "border-hover", "border-strong"]) {
      expect(ratio(`${s}-more`, fg, "card"), `${s} ${fg}`).toBeGreaterThanOrEqual(ratio(s, fg, "card"));
    }
    expect(ratio("light-more", "border", "card")).toBeGreaterThanOrEqual(3);
    expect(ratio("dark-more", "border", "card")).toBeGreaterThanOrEqual(3);
  });

  describe("the forced-colors profile", () => {
    const forced = (r: string) => resolve(file, `theme.forced.${r}`) as string;
    /** Pairs of system colours the platform draws to contrast: text on its own background, and an indicator on the canvas. */
    const TEXT = new Set(["CanvasText/Canvas", "LinkText/Canvas", "HighlightText/Highlight", "ButtonText/ButtonFace", "FieldText/Field", "MarkText/Mark", "Canvas/CanvasText"]);
    const UI = new Set([...TEXT, "Highlight/Canvas", "ButtonBorder/Canvas"]);

    it("is system colours only", () => {
      for (const r of roles("forced")) expect(SYSTEM_COLORS, r).toContain(forced(r));
    });

    it("pairs, for each light pair of the contract, two system colours the platform makes contrast", () => {
      for (const p of contrastPairs(file).filter(p => p.fg.startsWith("theme.light.") && p.bg.startsWith("theme.light."))) {
        const pair = `${forced(p.fg.slice(12))}/${forced(p.bg.slice(12))}`;
        expect(p.min >= 4.5 ? TEXT : UI, `${p.fg} on ${p.bg}: ${pair}`).toContain(pair);
      }
    });
  });

  it("changes only base's tokens under reduced motion, and stills them", () => {
    const base = tokenPaths(file.base as TokenGroup);
    for (const p of tokenPaths(file["reduced-motion"] as TokenGroup)) {
      expect(base).toContain(p);
      expect(resolve(file, `reduced-motion.${p}`)).toEqual({ value: 0, unit: "ms" });
    }
  });

  it.each([
    { rel: "shell/src/theme.css", specs: ["theme.light base", "theme.dark over theme.light", "theme.dark over theme.light"] },
    { rel: "overlay/src/tokens.css", specs: [
      "public theme.light base except tag pin", "public theme.dark over theme.light except tag pin",
      "public theme.light-more over theme.light except tag pin", "public theme.dark-more over theme.dark except tag pin",
      "theme.forced except tag pin", "reduced-motion",
    ] },
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

    it("the overlay: every role reads its public property first, except under forced colours and reduced motion", () => {
      const region = (spec: string) => overlay.slice(overlay.indexOf(`tokens:begin ${spec} */`), overlay.indexOf("tokens:end", overlay.indexOf(`tokens:begin ${spec} */`)));
      const decls = (spec: string) => [...region(spec).matchAll(/^\s*(--[\w-]+): (.*);$/gm)];
      for (const spec of regionSpecs(overlay)) {
        const ds = decls(spec);
        expect(ds.length, spec).toBeGreaterThan(0);
        for (const [, name, value] of ds) {
          if (spec.startsWith("public ")) expect(value, `${spec}: ${name}`).toMatch(new RegExp(`^var\\(--clax-${name.slice(2)}, `));
          else expect(value, `${spec}: ${name}`).not.toContain("--clax-");
        }
      }
    });

    it("the shell's fonts and radii", () => {
      const base = theme("base");
      for (const name of ["font", "mono", "radius", "radius-sm", "radius-xs", "gutter"]) expect(base.get(name), `--${name}`).toBe(shellLight.get(name));
    });
  });
});
