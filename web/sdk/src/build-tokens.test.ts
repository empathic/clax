// The public token build (web/scripts/build-tokens.ts, Style Dictionary):
// its outputs are what a build writes and every build writes the same; every
// profile block of clax.css declares every role; the page's data-theme and
// class choose the profile over the system's scheme in both directions, more
// contrast combines with either, and forced colours win over all; and the
// contrast contract holds on the built values.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { buildTokens, OUTPUTS, PROFILES, type Profile } from "../../scripts/build-tokens";
import { contrast, contrastPairs, SYSTEM_COLORS, type TokenGroup } from "./tokens";

const web = join(__dirname, "../..");
const read = (rel: string) => readFileSync(join(web, rel), "utf8");
const file: TokenGroup = JSON.parse(read("sdk/tokens/clax.tokens.json"));

interface Block { media: string[]; selector: string; decls: Map<string, string> }

/** clax.css's rules, each with the media conditions it sits in. */
function blocks(css: string): Block[] {
  const out: Block[] = [];
  const media: string[] = [];
  let current: Block | null = null;
  for (const line of css.split("\n")) {
    const t = line.trim();
    if (t.startsWith("@media ")) media.push(t.slice(7, -2));
    else if (t.endsWith("{")) { current = { media: [...media], selector: t.slice(0, -2), decls: new Map() }; out.push(current); }
    else if (t === "}") { if (current) current = null; else media.pop(); }
    else if (current) { const m = /^(--[\w-]+): (.*);$/.exec(t); if (m) current.decls.set(m[1], m[2]); }
  }
  return out;
}

interface Scene { dark: boolean; more: boolean; forced: boolean; theme?: "light" | "dark" }

/** The --clax-* properties the root ends with in `scene`: every matching block in order, the later winning. */
function cascade(css: string, scene: Scene): Map<string, string> {
  document.documentElement.removeAttribute("data-theme");
  if (scene.theme) document.documentElement.setAttribute("data-theme", scene.theme);
  const holds = (cond: string) => cond.split(" and ").every(c => ({
    "(prefers-color-scheme: dark)": scene.dark, "(prefers-color-scheme: light)": !scene.dark,
    "(prefers-contrast: more)": scene.more, "(forced-colors: active)": scene.forced, "(prefers-reduced-motion: reduce)": false,
  } as Record<string, boolean>)[c] ?? (() => { throw new Error(`unknown condition ${c}`); })());
  const got = new Map<string, string>();
  for (const b of blocks(css)) {
    if (!b.media.every(holds) || !document.documentElement.matches(b.selector)) continue;
    for (const [k, v] of b.decls) got.set(k, v);
  }
  const at = (v: string, depth = 0): string => { const r = /^var\((--[\w-]+)\)$/.exec(v); return r && depth < 8 ? at(got.get(r[1]) ?? v, depth + 1) : v; };
  return new Map([...got].map(([k, v]) => [k, at(v)]));
}

const built = await buildTokens(file);
const template = JSON.parse(built.json) as { theme: Record<Profile, TokenGroup> };
/** A profile's built value of a role. */
const valueIn = (p: Profile, role: string): string => {
  let node: unknown = template.theme[p];
  for (const part of role.split(".")) node = (node as Record<string, unknown>)[part];
  return (node as { $value: string }).$value;
};

describe("the public token build", () => {
  it("writes what is committed, and the same on every build", async () => {
    const again = await buildTokens(JSON.parse(read("sdk/tokens/clax.tokens.json")));
    expect(again).toEqual(built);
    for (const [k, rel] of Object.entries(OUTPUTS)) expect(built[k as keyof typeof built], `${rel} differs from a build: run node scripts/build-tokens.ts`).toBe(read(rel));
  });

  it("declares every role in every profile's block, with the --clax- prefix", () => {
    const bs = blocks(built.css);
    const count = Object.keys(template.theme.light).length;
    expect(count).toBeGreaterThan(30);
    const roles = (b: Block) => [...b.decls.keys()].filter(k => !["font", "mono", "radius", "space", "text", "leading", "duration", "ease", "gutter"].some(n => k === `--clax-${n}` || k.startsWith(`--clax-${n}-`)));
    const colourBlocks = bs.filter(b => !b.media.includes("(prefers-reduced-motion: reduce)"));
    expect(colourBlocks).toHaveLength(7);
    const first = roles(colourBlocks[0]).sort();
    for (const b of colourBlocks) expect(roles(b).sort(), `${b.media.join(" ")} ${b.selector}`).toEqual(first);
    for (const b of bs) for (const k of b.decls.keys()) expect(k).toMatch(/^--clax-[\w-]+$/);
  });

  it("writes forced colours in system colours only", () => {
    const forced = blocks(built.css).find(b => b.media.includes("(forced-colors: active)"))!;
    for (const [k, v] of forced.decls) expect(SYSTEM_COLORS, k).toContain(v);
  });

  it.each([
    [{ dark: false, more: false, forced: false }, "light"],
    [{ dark: true, more: false, forced: false }, "dark"],
    [{ dark: true, more: false, forced: false, theme: "light" }, "light"],
    [{ dark: false, more: false, forced: false, theme: "dark" }, "dark"],
    [{ dark: false, more: true, forced: false }, "light-more"],
    [{ dark: true, more: true, forced: false }, "dark-more"],
    [{ dark: true, more: true, forced: false, theme: "light" }, "light-more"],
    [{ dark: false, more: true, forced: false, theme: "dark" }, "dark-more"],
    [{ dark: true, more: true, forced: true, theme: "dark" }, "forced"],
    [{ dark: false, more: false, forced: true, theme: "light" }, "forced"],
  ] as [Scene, Profile][])("chooses the profile for %o: %s", (scene, want) => {
    const got = cascade(built.css, scene);
    for (const role of ["bg", "fg", "muted", "border", "accent", "focus"]) expect(got.get(`--clax-${role}`), role).toBe(valueIn(want, role));
  });

  it.each(PROFILES.filter(p => p !== "forced"))("holds the contrast contract on the built values of %s", p => {
    const pairs = contrastPairs(file).filter(c => c.fg.startsWith(`theme.${p}.`) && c.bg.startsWith(`theme.${p}.`));
    expect(pairs.length).toBeGreaterThan(30);
    const role = (path: string) => path.slice(`theme.${p}.`.length);
    for (const c of pairs) expect(contrast(valueIn(p, role(c.fg)), valueIn(p, role(c.bg))), `${c.fg} on ${c.bg}`).toBeGreaterThanOrEqual(c.min);
  });

  it("stills motion under reduced motion", () => {
    const b = blocks(built.css).find(x => x.media.includes("(prefers-reduced-motion: reduce)"))!;
    expect(Object.fromEntries(b.decls)).toEqual({ "--clax-duration": "0ms" });
  });
});
