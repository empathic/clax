// Design tokens in the W3C Design Tokens Community Group format
// (https://www.designtokens.org/tr/drafts/format/), written for Clax from
// the spec: no token library at run time (the build of the public
// stylesheet, web/scripts/build-tokens.mjs, uses Style Dictionary and this
// module's cssValue). A token file is nested groups; a token is an
// object with a `$value` (and an optional `$type`, inherited from its
// groups). A value of the form "{group.token}" is an alias for another
// token's value. Clax's contrast contract rides in the spec's
// `$extensions`, under `dev.empathic.clax`:
//
//   "$extensions": { "dev.empathic.clax": { "contrast": [
//     { "fg": "theme.light.fg", "bg": "theme.light.card", "min": 4.5 }
//   ] } }
//
// `contrastFailures` checks each pair with WCAG 2.x's contrast ratio, and
// `cssVariables` writes one theme's roles as CSS custom properties. A
// colour token may also be a CSS system colour (`SYSTEM_COLORS`), which the
// spec does not allow: Clax's forced-colors profile (`theme.forced`) is
// written in them, and no contrast is computed for one.

/** A colour as the spec writes it: a hex string, or an sRGB object. */
export type ColorValue = string | { colorSpace: "srgb"; components: [number, number, number]; alpha?: number; hex?: string };

export interface Token { $value: unknown; $type?: string; $description?: string; $extensions?: Record<string, unknown> }
export interface TokenGroup { [name: string]: TokenGroup | Token | unknown }

export interface ContrastPair { fg: string; bg: string; min: number }
export interface ContrastFailure extends ContrastPair { ratio: number }

export class TokenError extends Error {}

const EXTENSION = "dev.empathic.clax";

/** CSS Color 4's system colours (https://www.w3.org/TR/css-color-4/#css-system-colors):
 * the user's palette under forced colours. */
export const SYSTEM_COLORS: readonly string[] = [
  "AccentColor", "AccentColorText", "ActiveText", "ButtonBorder", "ButtonFace", "ButtonText", "Canvas", "CanvasText",
  "Field", "FieldText", "GrayText", "Highlight", "HighlightText", "LinkText", "Mark", "MarkText", "SelectedItem",
  "SelectedItemText", "VisitedText",
];
const ALIAS = /^\{([^{}]+)\}$/;

const isToken = (v: unknown): v is Token => typeof v === "object" && v !== null && "$value" in v;
const isGroup = (v: unknown): v is TokenGroup => typeof v === "object" && v !== null && !Array.isArray(v) && !isToken(v);

/** The token at `path` (dot-separated), with the `$type` it has or
 * inherits from its groups. */
export function tokenAt(file: TokenGroup, path: string): { token: Token; type: string | undefined } {
  let node: unknown = file;
  let type = typeof file.$type === "string" ? file.$type : undefined;
  for (const part of path.split(".")) {
    if (!isGroup(node) || part.startsWith("$") || !(part in node)) throw new TokenError(`no token ${path}`);
    node = node[part];
    if ((isGroup(node) || isToken(node)) && typeof (node as Token).$type === "string") type = (node as Token).$type;
  }
  if (!isToken(node)) throw new TokenError(`${path} is a group, not a token`);
  return { token: node, type };
}

/** The value of the token at `path`, every alias followed. Fails on a
 * missing token and on an alias that leads back to itself. */
export function resolve(file: TokenGroup, path: string, seen: string[] = []): unknown {
  if (seen.includes(path)) throw new TokenError(`alias cycle: ${[...seen, path].join(" -> ")}`);
  const { token } = tokenAt(file, path);
  const alias = typeof token.$value === "string" ? ALIAS.exec(token.$value) : null;
  return alias ? resolve(file, alias[1], [...seen, path]) : token.$value;
}

/** The `$type` of the token at `path`: its own or its groups', else that of
 * the token its alias names (the spec lets an alias take its target's). */
export function typeOf(file: TokenGroup, path: string, seen: string[] = []): string | undefined {
  if (seen.includes(path)) throw new TokenError(`alias cycle: ${[...seen, path].join(" -> ")}`);
  const { token, type } = tokenAt(file, path);
  if (type) return type;
  const alias = typeof token.$value === "string" ? ALIAS.exec(token.$value) : null;
  return alias ? typeOf(file, alias[1], [...seen, path]) : undefined;
}

/** Every token's path under `group` (`prefix` the group's own path). */
export function tokenPaths(group: TokenGroup, prefix = ""): string[] {
  return Object.entries(group).flatMap(([name, v]) => {
    if (name.startsWith("$")) return [];
    const path = prefix ? `${prefix}.${name}` : name;
    if (isToken(v)) return [path];
    return isGroup(v) ? tokenPaths(v, path) : [];
  });
}

/** sRGB channels 0-255 and alpha 0-1 of a colour value. */
export function rgba(value: unknown): [number, number, number, number] {
  if (typeof value === "string") {
    const hex = /^#([0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i.exec(value)?.[1];
    if (!hex) throw new TokenError(`not a hex colour: ${value}`);
    const full = hex.length <= 4 ? [...hex].map(c => c + c).join("") : hex;
    const n = (i: number) => parseInt(full.slice(i, i + 2), 16);
    return [n(0), n(2), n(4), full.length === 8 ? n(6) / 255 : 1];
  }
  if (typeof value === "object" && value !== null && (value as { colorSpace?: unknown }).colorSpace === "srgb") {
    const { components, alpha } = value as { components: number[]; alpha?: number };
    if (components.length !== 3 || components.some(c => typeof c !== "number" || c < 0 || c > 1)) throw new TokenError("sRGB components are three numbers from 0 to 1");
    return [components[0] * 255, components[1] * 255, components[2] * 255, alpha ?? 1];
  }
  throw new TokenError(`not a colour: ${JSON.stringify(value)}`);
}

/** WCAG 2.x relative luminance of an opaque sRGB colour. */
export function luminance([r, g, b]: [number, number, number, number]): number {
  const lin = (c: number) => { const s = c / 255; return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4; };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

/** WCAG 2.x contrast ratio of two colours, from 1 to 21. A translucent
 * foreground is composited over the background first; the background must
 * be opaque. */
export function contrast(fg: unknown, bg: unknown): number {
  const b = rgba(bg);
  if (b[3] !== 1) throw new TokenError("a contrast background must be opaque");
  const f = rgba(fg);
  const over = (i: 0 | 1 | 2) => f[i] * f[3] + b[i] * (1 - f[3]);
  const [l1, l2] = [luminance([over(0), over(1), over(2), 1]), luminance(b)];
  return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
}

/** The file's contrast contract (`$extensions["dev.empathic.clax"].contrast`). */
export function contrastPairs(file: TokenGroup): ContrastPair[] {
  const ext = (file.$extensions as Record<string, { contrast?: unknown }> | undefined)?.[EXTENSION];
  const pairs = ext?.contrast ?? [];
  if (!Array.isArray(pairs)) throw new TokenError("contrast is a list of { fg, bg, min }");
  for (const p of pairs) {
    if (typeof p?.fg !== "string" || typeof p?.bg !== "string" || typeof p?.min !== "number") throw new TokenError(`a contrast pair needs fg, bg and min: ${JSON.stringify(p)}`);
  }
  return pairs as ContrastPair[];
}

/** Each pair of the contract whose colours fall below its minimum. */
export function contrastFailures(file: TokenGroup): ContrastFailure[] {
  return contrastPairs(file)
    .map(p => ({ ...p, ratio: contrast(resolve(file, p.fg), resolve(file, p.bg)) }))
    .filter(p => p.ratio < p.min);
}

/** A resolved value as CSS: a colour as hex, a dimension object as its
 * number and unit, a duration likewise, a cubic Bézier as the function, a
 * font family list joined, and strings and numbers as they are. */
export function cssValue(value: unknown, type: string | undefined): string {
  if (type === "color") {
    if (typeof value === "string" && SYSTEM_COLORS.includes(value)) return value;
    const [r, g, b, a] = rgba(value);
    const h = (n: number) => Math.round(n).toString(16).padStart(2, "0");
    return a === 1 ? `#${h(r)}${h(g)}${h(b)}` : `rgba(${Math.round(r)},${Math.round(g)},${Math.round(b)},${+a.toFixed(3)})`;
  }
  if (typeof value === "object" && value !== null && "value" in value && "unit" in value) return `${(value as { value: number }).value}${(value as { unit: string }).unit}`;
  if (type === "cubicBezier" && Array.isArray(value)) return `cubic-bezier(${value.join(",")})`;
  if (type === "fontFamily" && Array.isArray(value)) return value.map(f => (/^[\w-]+$/.test(f) ? f : JSON.stringify(f))).join(", ");
  if (typeof value === "string" || typeof value === "number") return String(value);
  throw new TokenError(`no CSS for ${JSON.stringify(value)}`);
}

/** The tokens under the group at `path` as CSS custom properties, one
 * `--name: value;` per token, named by their path under the group joined
 * with `-` (`tag.red.bg` is `--tag-red-bg`). A token that aliases another
 * of the same group is `var()` of it, so a page that sets the one (a host's
 * `--agent`) moves the other (`--accent`) too. */
export function cssVariables(file: TokenGroup, path: string): string[] {
  let group: unknown = file;
  for (const part of path.split(".")) group = isGroup(group) ? group[part] : undefined;
  if (!isGroup(group)) throw new TokenError(`no group ${path}`);
  const name = (p: string) => `--${p.replace(/\./g, "-")}`;
  return tokenPaths(group).map(p => {
    const full = `${path}.${p}`;
    const { token } = tokenAt(file, full);
    const alias = typeof token.$value === "string" ? ALIAS.exec(token.$value)?.[1] : undefined;
    if (alias?.startsWith(`${path}.`)) return `${name(p)}: var(${name(alias.slice(path.length + 1))});`;
    return `${name(p)}: ${cssValue(resolve(file, full), typeOf(file, full))};`;
  });
}

/** The declarations a region of a stylesheet holds (`regionLines`): the
 * groups `spec` names, space-separated, as `cssVariables` writes them; with
 * ` over ` and more groups after it, only the lines that differ from
 * theirs (a dark theme's overrides of the light one); with ` except ` and
 * names after it, none of the properties under those names (`except tag`
 * leaves out `--tag-*`, for a stylesheet that draws no tags); with
 * `public ` before it all, each value as the fallback of the role's public
 * property (`--bg: var(--clax-bg, #f7f7f5);`), so a page that sets
 * `--clax-bg` on an ancestor of the overlay's host draws the overlay in it. */
export function regionLines(file: TokenGroup, spec: string): string[] {
  if (spec.startsWith("public ")) return regionLines(file, spec.slice("public ".length)).map(publicLine);
  const [main, except = ""] = spec.split(" except ");
  const [groups, base] = main.split(" over ");
  const left = except.trim().split(/\s+/).filter(Boolean);
  const kept = (l: string) => !left.some(n => l.startsWith(`--${n}:`) || l.startsWith(`--${n}-`));
  const lines = groups.trim().split(/\s+/).flatMap(p => cssVariables(file, p)).filter(kept);
  if (!base) return lines;
  const under = new Set(base.trim().split(/\s+/).flatMap(p => cssVariables(file, p)));
  return lines.filter(l => !under.has(l));
}

/** `--name: value;` as `--name: var(--clax-name, value);`. */
const publicLine = (line: string): string => line.replace(/^--([\w-]+): (.*);$/, (_, n: string, v: string) => `--${n}: var(${PUBLIC_PREFIX}${n}, ${v});`);

/** The prefix of Clax's public custom properties, the ones a page sets. */
export const PUBLIC_PREFIX = "--clax-";

const REGION = /^([ \t]*)\/\* tokens:begin ([^*]+?) \*\/\n[\s\S]*?^[ \t]*\/\* tokens:end \*\//gm;

/** `css` with every generated region written again from `file`. A region
 * runs from a line `/* tokens:begin <spec> *\/` to a line
 * `/* tokens:end *\/`; its declarations are `regionLines(file, spec)`, one
 * per line, at the begin line's indent. Everything outside the regions is
 * left as it is. */
export function syncRegions(css: string, file: TokenGroup): string {
  return css.replace(REGION, (_, indent: string, spec: string) =>
    [`${indent}/* tokens:begin ${spec} */`, ...regionLines(file, spec).map(l => indent + l), `${indent}/* tokens:end */`].join("\n"));
}

const hasGroup = (file: TokenGroup, path: string) => {
  let node: unknown = file;
  for (const part of path.split(".")) node = isGroup(node) ? node[part] : undefined;
  return isGroup(node);
};

/** A stylesheet that gives `selector` the themes of a token file in Clax's
 * shape (`theme.light`, `theme.dark` and `base`, each optional, with the
 * role names of web/sdk/tokens/clax.tokens.json): the light theme and the
 * base on `selector`, and the dark theme's overrides of the light one under
 * the system's dark scheme. A host's theme for the overlay, laid over its
 * own tokens: a role it leaves out keeps Clax's. All of it sits in
 * `@media not all and (forced-colors: active)`: under forced colours the
 * overlay keeps its system colours (`theme.forced`), whatever the page's. */
export function themeSheet(file: TokenGroup, selector = ":host"): string {
  const light = ["theme.light", "base"].filter(p => hasGroup(file, p)).flatMap(p => cssVariables(file, p));
  const dark = !hasGroup(file, "theme.dark") ? [] : hasGroup(file, "theme.light") ? regionLines(file, "theme.dark over theme.light") : cssVariables(file, "theme.dark");
  const out: string[] = [];
  if (light.length) out.push(`  ${selector} {`, ...light.map(l => `    ${l}`), "  }");
  if (dark.length) out.push("  @media (prefers-color-scheme: dark) {", `    ${selector} {`, ...dark.map(l => `      ${l}`), "    }", "  }");
  return out.length ? ["@media not all and (forced-colors: active) {", ...out, "}"].join("\n") : "";
}

/** The specs of `css`'s generated regions, in order. */
export function regionSpecs(css: string): string[] {
  return [...css.matchAll(REGION)].map(m => m[2]);
}
