// The shell's base layer (spec §8, "Look"): the system's faces and nothing
// downloaded for type, sentence case, warm-neutral tokens on all three theme
// paths, the people/agent colours, and WCAG AA contrast in both themes.
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const css = readFileSync(join(__dirname, "theme.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
const block = (sel: string) => [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].filter(m => m[1].split(",").map(s => s.trim()).includes(sel)).map(m => m[2]).join(";");
const SYSTEM_DARK = /:root:not\(\[data-theme="light"\]\)\s*\{\s*@media \(prefers-color-scheme: dark\)\s*\{([^}]*)\}/;
const svelte = readdirSync(join(__dirname, "ui")).filter(f => f.endsWith(".svelte")).map(f => readFileSync(join(__dirname, "ui", f), "utf8"));

/** The custom properties a declaration block sets, by name. */
const tokens = (body: string) => Object.fromEntries([...body.matchAll(/(--[\w-]+):\s*([^;]+)/g)].map(m => [m[1], m[2].trim()]));
const light = tokens(block(":root"));
const dark = { ...light, ...tokens(block(':root[data-theme="dark"]')) };
const systemDark = tokens(SYSTEM_DARK.exec(css)![1]);

/** A token's colour, following var() aliases, as #rrggbb. */
function hex(theme: Record<string, string>, name: string): string {
  let v = theme[name];
  for (let i = 0; v?.startsWith("var("); i++) v = theme[/var\((--[\w-]+)\)/.exec(v)![1]];
  expect(v, name).toMatch(/^#[0-9a-f]{6}$/i);
  return v;
}
const lum = (h: string) => {
  const c = [1, 3, 5].map(i => parseInt(h.slice(i, i + 2), 16) / 255).map(v => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
};
const ratio = (a: string, b: string) => { const [x, y] = [lum(a), lum(b)]; return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05); };

describe("theme", () => {
  it("downloads nothing for type: no @font-face, the system sans for the interface and the system monospace for literals", () => {
    expect(css).not.toMatch(/@font-face/);
    expect(css).not.toMatch(/url\(/);
    expect(light["--font"]).toBe('ui-sans-serif, -apple-system, "Segoe UI", system-ui, sans-serif');
    expect(light["--mono"]).toBe('ui-monospace, "SF Mono", Menlo, Consolas, monospace');
    expect(block("body")).toMatch(/font:\s*14px\/1\.5 var\(--font\)/);
    for (const sel of [".kc", "code"]) expect(block(sel), sel).toMatch(/var\(--mono\)/);
    for (const src of [css, ...svelte]) {
      expect(src).not.toMatch(/Plex|--grot/);
      expect(src).not.toMatch(/\/_clax\/fonts\//);
    }
  });

  it("sets nothing in tracked capitals", () => {
    expect(css).not.toMatch(/text-transform:\s*uppercase/);
    expect(css).not.toMatch(/letter-spacing:\s*\.0[4-9]em/);
  });

  it("defines people and agent colours for light and both dark paths", () => {
    expect(light["--you"]).toBe("#e0532f");
    expect(light["--agent"]).toBe("#2f6f2a");
    for (const d of [dark, systemDark]) {
      expect(d["--you"]).toBe("#ff7a57");
      expect(d["--agent"]).toBe("#7cc36f");
    }
  });

  it("gives both dark paths the same overrides, and the accent greens alias the agent's", () => {
    expect(systemDark).toEqual(tokens(block(':root[data-theme="dark"]')));
    for (const t of ["--bg", "--card", "--raised", "--hover", "--fg", "--muted", "--border", "--border-hover", "--border-strong", "--you", "--on-you", "--you-ink", "--agent", "--agent-ink", "--accent-tint", "--comment-hl", "--danger", "--danger-tint", "--shadow", "--elev", "--elev-lg", "--pink", "--font", "--mono", "--radius", "--radius-sm"]) expect(light, t).toHaveProperty(t);
    expect(light["--accent"]).toBe("var(--agent)");
    expect(light["--accent-ink"]).toBe("var(--agent-ink)");
    expect(light["--focus"]).toBe("var(--agent)");
    // The dark paths set the green once, as --agent; the aliases follow it.
    for (const d of [systemDark, tokens(block(':root[data-theme="dark"]'))]) for (const t of ["--accent", "--accent-ink", "--focus"]) expect(d).not.toHaveProperty(t);
    // Pins sit over the artifact: one literal set, the same in both themes.
    for (const t of ["--pin", "--on-pin", "--pin-ring"]) expect(systemDark).not.toHaveProperty(t);
  });

  it.each([["light", light], ["dark", dark], ["system dark", { ...light, ...systemDark }]] as const)("meets WCAG AA in %s", (_, theme) => {
    const grounds = ["--card", "--bg", "--raised", "--hover"].map(g => hex(theme, g));
    for (const text of ["--fg", "--muted", "--agent-ink", "--you-ink", "--danger"]) for (const g of grounds) expect(ratio(hex(theme, text), g), `${text} on ${g}`).toBeGreaterThanOrEqual(4.5);
    for (const [text, ground] of [["--muted", "--comment-hl"], ["--you-ink", "--comment-hl"], ["--agent-ink", "--accent-tint"], ["--muted", "--accent-tint"], ["--danger", "--danger-tint"],
      ["--on-you", "--you"], ["--on-accent", "--accent"], ["--on-pin", "--pin"], ["--bg", "--fg"], ["--bg", "--primary-hover"], ["--on-accent", "--accent-hover"]]) {
      expect(ratio(hex(theme, text), hex(theme, ground)), `${text} on ${ground}`).toBeGreaterThanOrEqual(4.5);
    }
    for (const g of grounds) {
      expect(ratio(hex(theme, "--border-strong"), g), `--border-strong on ${g}`).toBeGreaterThanOrEqual(3);
      expect(ratio(hex(theme, "--focus"), g), `--focus on ${g}`).toBeGreaterThanOrEqual(3);
    }
  });

  it("sets buttons in the sans, rounded, sentence case, and stops faux bold", () => {
    expect(block("button")).toMatch(/font:\s*500 14px\/1(\.\d+)? var\(--font\)/);
    expect(block("button")).toMatch(/border-radius:\s*var\(--radius-sm\)/);
    expect(block(":root")).toMatch(/font-synthesis:\s*none/);
  });
});
