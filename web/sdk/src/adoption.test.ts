// A host design system adopts Clax's tokens (web/sdk/tokens/README.md): the
// two worked examples in web/sdk/tokens/examples/ name only Clax's roles,
// and Clax's contrast contract holds in light and dark with the host's
// values in place of Clax's for the roles it maps.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { contrastFailures, resolve, themeSheet, type TokenGroup, tokenPaths } from "./tokens";

const read = (rel: string) => readFileSync(join(__dirname, "..", rel), "utf8");
const clax: TokenGroup = JSON.parse(read("tokens/clax.tokens.json"));
const roles = tokenPaths((clax.theme as TokenGroup).light as TokenGroup);
const BASE = tokenPaths(clax.base as TokenGroup);

/** Clax's token file with `values` (role -> colour, per theme) in place of
 * its own, and its contract cut to the light and dark pairs. */
function withHost(values: Record<"light" | "dark", Record<string, string>>): TokenGroup {
  const file = structuredClone(clax);
  const theme = file.theme as Record<string, TokenGroup>;
  for (const s of ["light", "dark"] as const) {
    for (const [role, v] of Object.entries(values[s])) {
      let node = theme[s] as Record<string, unknown>;
      const parts = role.split(".");
      for (const p of parts.slice(0, -1)) node = node[p] as Record<string, unknown>;
      node[parts.at(-1)!] = { $type: "color", $value: v };
    }
  }
  const ext = (file.$extensions as Record<string, { contrast: { fg: string; bg: string }[] }>)["dev.empathic.clax"];
  ext.contrast = ext.contrast.filter(p => /^theme\.(light|dark)\./.test(p.fg));
  return file;
}

describe("Changeling's mapping (examples/changeling.css)", () => {
  const css = read("tokens/examples/changeling.css");
  const mapping = new Map([...css.matchAll(/--clax-([\w-]+): var\(--([\w-]+)\);/g)].map(m => [m[1], m[2]]));
  // Changeling's own tokens, light (:root) and dark ([data-theme="dark"]), from
  // empathic/changelist web/src/styles/tokens.css at c3f3c056.
  const changeling = {
    light: { bg: "#f4f2ec", surface: "#fffefa", fg: "#242922", muted: "#687065", line: "#dcded3", accent: "#a54120", soft: "#f7e7da", good: "#246347", goodbg: "#e6efe6", bad: "#963d37", badbg: "#f9e8e3", code: "#f0f1ea", gutter: "#646d61", "review-color": "#327755", sel: "#f4dcc8" },
    dark: { bg: "#1b201d", surface: "#272d27", fg: "#dddfd6", muted: "#bac2b6", line: "#424c43", accent: "#f2af88", soft: "#413228", good: "#9fd0af", goodbg: "#2c4033", bad: "#efaea5", badbg: "#47302e", code: "#1f2520", gutter: "#99a89c", "review-color": "#a3d5b7", sel: "#523d2e" },
  } as const;

  it("sets only Clax's public properties, each from one of Changeling's tokens", () => {
    expect(mapping.size).toBeGreaterThan(15);
    for (const role of mapping.keys()) expect([...roles.map(r => r.replace(/\./g, "-")), ...BASE], role).toContain(role);
    const decls = [...css.matchAll(/^\s*(--[\w-]+):/gm)].map(m => m[1]);
    expect(decls).toHaveLength(mapping.size);
  });

  it("meets Clax's contrast contract in light and dark", () => {
    const values = { light: {} as Record<string, string>, dark: {} as Record<string, string> };
    for (const s of ["light", "dark"] as const) {
      for (const [role, token] of mapping) {
        if (BASE.includes(role)) continue;
        const v = (changeling[s] as Record<string, string>)[token];
        if (!v) throw new Error(`no ${token} in Changeling's ${s} tokens`);
        values[s][role] = v;
      }
    }
    expect(contrastFailures(withHost(values))).toEqual([]);
  });
});

describe("a viewer's theme file (examples/fold-viewer.theme.tokens.json)", () => {
  const host: TokenGroup = JSON.parse(read("tokens/examples/fold-viewer.theme.tokens.json"));

  it("names only Clax's roles, and is a theme the overlay takes", () => {
    for (const s of ["light", "dark"]) for (const r of tokenPaths((host.theme as TokenGroup)[s] as TokenGroup)) expect(roles, `${s}.${r}`).toContain(r);
    const sheet = themeSheet(host);
    expect(sheet).toContain("--agent: #2d5f8a;");
    expect(sheet).toContain("--accent: var(--agent);");
    expect(sheet.startsWith("@media not all and (forced-colors: active) {")).toBe(true);
  });

  it("meets Clax's contrast contract in light and dark", () => {
    const values = { light: {} as Record<string, string>, dark: {} as Record<string, string> };
    for (const s of ["light", "dark"] as const) {
      for (const r of tokenPaths((host.theme as TokenGroup)[s] as TokenGroup)) values[s][r] = resolve(host, `theme.${s}.${r}`) as string;
    }
    expect(contrastFailures(withHost(values))).toEqual([]);
  });
});
