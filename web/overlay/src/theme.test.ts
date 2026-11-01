// A page themes the overlay with its own design tokens (`theme`): a token
// file in Clax's roles, as a host app keeps one, laid over Clax's tokens in
// the overlay's shadow root.
import { afterEach, describe, expect, it } from "vitest";
import { OVERLAY_TAG } from "../../bridge/src/anchor";
import { type TokenGroup, themeSheet } from "../../sdk/src/tokens";
import { claxTheme } from "../../sdk/tokens/dist/clax.theme";
import { overlay } from "./main";

const opts = { instance: "https://clax.example", queue: "q1", auth: async () => { throw new Error("no backend"); } };

/** A host app's token file: its own palette, and the roles it sets. */
const host: TokenGroup = {
  palette: {
    $type: "color",
    paper: { $value: "#fdf6e3" },
    ink: { $value: "#073642" },
    night: { $value: "#002b36" },
    moon: { $value: "#eee8d5" },
    violet: { $value: "#6c71c4" },
  },
  theme: {
    light: {
      bg: { $value: "{palette.paper}" },
      card: { $value: "{palette.paper}" },
      fg: { $value: "{palette.ink}" },
      agent: { $value: "{palette.violet}" },
      accent: { $value: "{theme.light.agent}" },
    },
    dark: {
      bg: { $value: "{palette.night}" },
      card: { $value: "{palette.night}" },
      fg: { $value: "{palette.moon}" },
      agent: { $value: "{palette.violet}" },
      accent: { $value: "{theme.dark.agent}" },
    },
  },
  base: {
    font: { $type: "fontFamily", $value: ["Inter", "system-ui", "sans-serif"] },
    radius: { $type: "dimension", $value: { value: 2, unit: "px" } },
  },
};

const sheetOf = (): string => {
  const root = document.querySelector(OVERLAY_TAG)?.shadowRoot;
  if (!root) throw new Error("no overlay");
  // jsdom has no constructed sheets: the overlay falls back to a <style> (bridge/src/sheet.ts).
  return [...(root.adoptedStyleSheets ?? []).flatMap(s => [...s.cssRules].map(r => r.cssText)), ...[...root.querySelectorAll("style")].map(s => s.textContent ?? "")].join("\n");
};

afterEach(() => { for (const el of document.querySelectorAll(OVERLAY_TAG)) el.remove(); });

describe("a page's theme", () => {
  it("is written as the light roles and base on :host, and the dark roles that differ under the dark scheme, none of it under forced colours", () => {
    expect(themeSheet(host)).toBe([
      "@media not all and (forced-colors: active) {",
      "  :host {",
      "    --bg: #fdf6e3;", "    --card: #fdf6e3;", "    --fg: #073642;", "    --agent: #6c71c4;", "    --accent: var(--agent);",
      "    --font: Inter, system-ui, sans-serif;", "    --radius: 2px;",
      "  }",
      "  @media (prefers-color-scheme: dark) {",
      "    :host {",
      "      --bg: #002b36;", "      --card: #002b36;", "      --fg: #eee8d5;",
      "    }",
      "  }",
      "}",
    ].join("\n"));
  });

  it("is nothing for a file with no theme or base", () => {
    expect(themeSheet({ palette: { $type: "color", paper: { $value: "#fdf6e3" } } })).toBe("");
  });

  it("is added to the overlay's sheet before its components' styles", () => {
    const handle = overlay({ ...opts, theme: host });
    const css = sheetOf();
    // Vitest gives tokens.css?inline as an empty string (it processes no
    // CSS), so here the page's theme is checked to come before the
    // components' sheet; sdk/package.test.ts checks it follows Clax's own
    // tokens in the built overlay.
    const at = css.indexOf(themeSheet(host));
    expect(at).toBeGreaterThanOrEqual(0);
    expect(css.indexOf("__CLAX_OVERLAY_COMPONENT_CSS__")).toBeGreaterThan(at);
    handle.close();
  });

  it("is checked before the overlay touches the page: a broken one throws and adds nothing", () => {
    const broken: TokenGroup = { theme: { light: { bg: { $value: "{palette.nope}" } } } };
    expect(() => overlay({ ...opts, theme: broken })).toThrow("no token palette.nope");
    expect(document.querySelectorAll(OVERLAY_TAG)).toHaveLength(0);
  });

  it("takes Clax's own template (sdk/tokens/dist/clax.theme.ts) as a theme", () => {
    const handle = overlay({ ...opts, theme: claxTheme });
    expect(sheetOf()).toContain(themeSheet(claxTheme));
    handle.close();
  });

  it("leaves the overlay as it was when there is none", () => {
    const handle = overlay(opts);
    expect(sheetOf()).not.toContain("#6c71c4");
    handle.close();
  });
});
