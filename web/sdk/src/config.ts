// A Clax install's configuration (`clax.config.json`): separate axes, each
// its own field, resolved from three layers, the later winning:
//
//   site   the defaults the site gives in code (the snippet's `site`)
//   clax   the container Clax serves for this site (edited and published in Clax)
//   you    a per-viewer override in a cookie, written by the DevTools panel's
//          settings or a `?clax.<field>=` link; only on a local or tailnet
//          origin unless the resolved `overrides.viewer` says "always"
//
// and Clax's defaults under all three. Every field reports its effective
// value's source, so a settings view can show "Clax", "site" or "you".
// The loader (overlay/src/loader.ts) reads the resolved config first and
// installs only what it names.

/** Where a field's effective value came from. */
export type Source = "default" | "site" | "clax" | "you";

type Field =
  | { kind: "bool"; default: boolean; doc: string }
  | { kind: "enum"; values: readonly string[]; default: string; doc: string }
  | { kind: "string"; default: string; doc: string; max?: number }
  | { kind: "targets"; default: Target[]; doc: string }
  | { kind: "tokens"; default: Record<string, string>; doc: string };

/** An element on the site that starts something: a selector, and what it does. */
export interface Target {
  selector: string;
  action: Action;
}

/** What a target, key, link or `clax.push` does. */
export type Action = "report" | "reports" | "close";
export const ACTIONS: readonly Action[] = ["report", "reports", "close"];

export const PRESENTATIONS = ["floating", "dialog", "drawer", "sheet", "window", "extension"] as const;
export type PresentationId = (typeof PRESENTATIONS)[number];
export const START_MODES = ["note", "screenshot", "element"] as const;
export type StartMode = (typeof START_MODES)[number];

/** Every field, by its dotted path in `clax.config.json`. */
export const FIELDS = {
  enabled: { kind: "bool", default: true, doc: "Clax on or off on this site." },
  // Entry points: each on or off.
  "entry.launcher": { kind: "bool", default: true, doc: "The floating Report and Your reports buttons in the corner." },
  "entry.targets": { kind: "bool", default: true, doc: "Elements marked data-clax=\"report\" (or \"reports\"), and the `targets` selectors, start Clax." },
  "entry.key": { kind: "string", default: "", max: 1, doc: "A key that starts a report (such as \"f\"); empty for none. Never heard in a text field or with a modifier." },
  "entry.palette": { kind: "bool", default: false, doc: "\"Send feedback\" and \"Your reports\" are offered to the site's command palette (handle.actions())." },
  "entry.link": { kind: "bool", default: true, doc: "?clax=report (or reports) in the page's address starts Clax on load." },
  targets: { kind: "targets", default: [], doc: "More elements that start Clax: a selector each, and its action." },
  presentation: { kind: "enum", values: PRESENTATIONS, default: "floating", doc: "How the composer is shown: floating in the corner, a dialog, a side drawer, a bottom sheet, its own window, or the installed Clax extension (a dialog when it is not installed)." },
  render: { kind: "enum", values: ["native", "embed"], default: "native", doc: "Clax pages (a report, its thread, Your reports) drawn by the SDK in the site's theme, or embedded from Clax." },
  start: { kind: "enum", values: START_MODES, default: "element", doc: "What a report starts with: the note, a screenshot of the page, or picking an element." },
  // What a report may carry, and whether each starts ticked.
  "attachments.screenshot": { kind: "bool", default: true, doc: "A screenshot can be attached (off: capture is never downloaded)." },
  "attachments.errors": { kind: "bool", default: false, doc: "The page's recent errors start ticked." },
  "attachments.logs": { kind: "bool", default: false, doc: "The app's log lines (from the site's log source) start ticked." },
  "attachments.version": { kind: "bool", default: true, doc: "The app's version is sent." },
  "attachments.browser": { kind: "bool", default: true, doc: "The browser, platform, language and time zone are sent." },
  annotate: { kind: "bool", default: true, doc: "The reporter can draw on the screenshot." },
  "filters.code-blur": { kind: "bool", default: true, doc: "Code on the page is blurred in screenshots." },
  "filters.code-selector": { kind: "string", default: "pre, code, .cm-content, [data-clax-code]", max: 500, doc: "What counts as code, for the blur." },
  "pickers.code-line": { kind: "bool", default: false, doc: "A pick in code records its file and lines (from the site's code locator)." },
  pins: { kind: "bool", default: false, doc: "Reports are pinned on what they are about." },
  // The host's data, read from what the page already exposes (no host code for Clax): a global by its dotted path from `window`.
  "hooks.logs": { kind: "string", default: "", max: 200, doc: "The page's global (a dotted path) giving its recent log lines." },
  "hooks.release": { kind: "string", default: "", max: 200, doc: "The page's global giving the app version: {version, commit?} or a string." },
  "hooks.user": { kind: "string", default: "", max: 200, doc: "The page's global naming the signed-in person." },
  theme: { kind: "tokens", default: {}, doc: "--clax-* custom properties: the site's look." },
  "overrides.viewer": { kind: "enum", values: ["local", "always", "never"], default: "local", doc: "Whether a viewer's cookie override applies: on local and tailnet origins only, everywhere, or nowhere. Never itself overridden by the cookie." },
} as const satisfies Record<string, Field>;

export type FieldName = keyof typeof FIELDS;
type ValueOf<F> = F extends { kind: "bool" } ? boolean : F extends { kind: "enum"; values: readonly (infer V)[] } ? V : F extends { kind: "targets" } ? Target[] : F extends { kind: "tokens" } ? Record<string, string> : string;
/** The resolved configuration: every field, by dotted path. */
export type ClaxConfig = { -readonly [K in FieldName]: ValueOf<(typeof FIELDS)[K]> };
/** A layer: some fields, flat or nested. */
export type ConfigLayer = Record<string, unknown>;

/** Fields the viewer's cookie never sets. */
const NOT_OVERRIDABLE: readonly FieldName[] = ["overrides.viewer"];

/** `doc`'s nested objects as dotted paths, down to the fields (`theme` and
 * `targets` are values, not objects to walk). */
export function flatten(doc: unknown, prefix = ""): ConfigLayer {
  const out: ConfigLayer = {};
  if (!doc || typeof doc !== "object" || Array.isArray(doc)) return out;
  for (const [k, v] of Object.entries(doc as Record<string, unknown>)) {
    const path = prefix ? `${prefix}.${k}` : k;
    if (path in FIELDS || !v || typeof v !== "object" || Array.isArray(v)) out[path] = v;
    else Object.assign(out, flatten(v, path));
  }
  return out;
}

/** A field's value checked, or why it is not one. Strings for booleans
 * ("1", "true", "on", "0", "false", "off") are taken, as links carry them. */
export function check(name: FieldName, v: unknown): { ok: true; value: unknown } | { ok: false; reason: string } {
  const f = FIELDS[name] as Field;
  switch (f.kind) {
    case "bool": {
      if (typeof v === "boolean") return { ok: true, value: v };
      if (typeof v === "string" && /^(1|true|on|yes)$/i.test(v)) return { ok: true, value: true };
      if (typeof v === "string" && /^(0|false|off|no)$/i.test(v)) return { ok: true, value: false };
      return { ok: false, reason: "not true or false" };
    }
    case "enum":
      return typeof v === "string" && f.values.includes(v) ? { ok: true, value: v } : { ok: false, reason: `not one of ${f.values.join(", ")}` };
    case "string":
      return typeof v === "string" && v.length <= (f.max ?? 1000) ? { ok: true, value: v } : { ok: false, reason: `not a string of at most ${f.max ?? 1000} characters` };
    case "targets":
      return Array.isArray(v) && v.every(t => t && typeof t.selector === "string" && t.selector && (ACTIONS as readonly string[]).includes(t.action))
        ? { ok: true, value: v.map(t => ({ selector: t.selector, action: t.action })) }
        : { ok: false, reason: "not a list of {selector, action}" };
    case "tokens":
      return v && typeof v === "object" && !Array.isArray(v) && Object.entries(v).every(([k, x]) => /^--clax-[a-z0-9-]+$/.test(k) && typeof x === "string")
        ? { ok: true, value: { ...(v as Record<string, string>) } }
        : { ok: false, reason: "not a map of --clax-* names to strings" };
  }
}

/** Whether `origin` is local or on a tailnet: where a viewer's override applies by default. */
export function isDevOrigin(origin: string): boolean {
  let host: string;
  try { host = new URL(origin).hostname; } catch { return false; }
  // A tailnet's MagicDNS names and its 100.64.0.0/10 addresses.
  return host === "localhost" || host === "[::1]" || /^127\./.test(host) || /\.(localhost|local|ts\.net)$/.test(host) || /^100\.(6[4-9]|[7-9]\d|1[01]\d|12[0-7])\.\d+\.\d+$/.test(host);
}

/** What the page exposes at `path` (a dotted path from `win`, such as
 * "app.release"): a function is called (with no arguments) when read.
 * A method is called on its object. Undefined for an empty or malformed
 * path, a missing global, or one that throws; never `__proto__`,
 * `prototype` or `constructor`. */
export function pageHook(win: object, path: string): (() => unknown) | undefined {
  if (!/^[A-Za-z_$][\w$]*(\.[A-Za-z_$][\w$]*)*$/.test(path)) return undefined;
  const keys = path.split(".");
  if (keys.some(k => k === "__proto__" || k === "prototype" || k === "constructor")) return undefined;
  return () => {
    try {
      let at: unknown = win, of: unknown = undefined;
      for (const k of keys) {
        if (at === null || (typeof at !== "object" && typeof at !== "function")) return undefined;
        of = at;
        at = (at as Record<string, unknown>)[k];
      }
      return typeof at === "function" ? (at as () => unknown).call(of) : at;
    } catch { return undefined; }
  };
}

/** The global an install exposes for the Clax DevTools panel's Integration
 * view (`IntegrationHook`): what put Clax on the page and how each field
 * resolved, and the viewer's override to set. No host code is involved. */
export const INSTALL_KEY = "__CLAX_INSTALL__";

/** What an install tells the DevTools panel, as JSON (protocol 1). */
export interface InstallInfo {
  v: 1;
  origin: string;
  instance: string;
  container: string;
  /** The container version read (from Clax or kept in this browser); null when there was none. */
  version: number | null;
  error: string | null;
  carrier: "site" | "extension";
  /** The deployment the tag names (`data-deployment`), when it names one. */
  deployment: string | null;
  /** The tag's script address (its `?v=` pin), for a tag install. */
  script: string | null;
  /** Whether a viewer's override applies on this page now, and the rule that decided. */
  applies: boolean;
  config: ClaxConfig;
  source: Record<FieldName, Source>;
  ignored: Resolved["ignored"];
  plugins: string[];
  /** The presentations (and "pins") the overlay has; null when it loaded none to ask. */
  built: string[] | null;
}

/** The object on `window[INSTALL_KEY]`. */
export interface IntegrationHook {
  v: 1;
  /** `InstallInfo` as a JSON string. */
  info(): string;
  /** Sets the viewer's override of `field` (null drops it); applies at the next load. */
  set(field: string, value: unknown): { ok: true } | { ok: false; reason: string };
}

/** The hook `install` puts on the page (non-enumerable, replaced by a later install). */
export function integrationHook(info: () => InstallInfo, doc: Document): IntegrationHook {
  return {
    v: 1,
    info: () => JSON.stringify(info()),
    set(field, value) {
      const i = info();
      if (!i.applies) return { ok: false, reason: i.config["overrides.viewer"] === "never" ? "Viewer overrides are off for this site." : "Viewer overrides apply on local and tailnet origins only." };
      if (!(field in FIELDS) || NOT_OVERRIDABLE.includes(field as FieldName)) return { ok: false, reason: "That setting can't be overridden." };
      const layer = { ...readOverride(doc.cookie ?? "") };
      if (value === null) delete layer[field];
      else {
        const c = check(field as FieldName, value);
        if (!c.ok) return { ok: false, reason: `Not a value it takes: ${c.reason}.` };
        layer[field] = c.value;
      }
      writeOverride(doc, layer);
      return { ok: true };
    },
  };
}

/** The resolved configuration, each field's source, and the values left out and why. */
export interface Resolved {
  config: ClaxConfig;
  source: Record<FieldName, Source>;
  ignored: { layer: Source; field: string; reason: string }[];
}

/** Resolves the layers over Clax's defaults: site, then Clax, then the
 * viewer's override where `overrides.viewer` (from the other layers) lets it. */
export function resolve(layers: { site?: unknown; clax?: unknown; you?: unknown }, origin: string): Resolved {
  const config = {} as Record<FieldName, unknown>;
  const source = {} as Record<FieldName, Source>;
  for (const [k, f] of Object.entries(FIELDS)) {
    config[k as FieldName] = structuredClone(f.default);
    source[k as FieldName] = "default";
  }
  const ignored: Resolved["ignored"] = [];
  const apply = (layer: Exclude<Source, "default">, doc: unknown) => {
    for (const [k, v] of Object.entries(flatten(doc))) {
      if (!(k in FIELDS)) { ignored.push({ layer, field: k, reason: "no such field" }); continue; }
      const name = k as FieldName;
      if (layer === "you" && NOT_OVERRIDABLE.includes(name)) { ignored.push({ layer, field: k, reason: "a viewer cannot override it" }); continue; }
      const c = check(name, v);
      if (!c.ok) { ignored.push({ layer, field: k, reason: c.reason }); continue; }
      config[name] = c.value;
      source[name] = layer;
    }
  };
  apply("site", layers.site);
  apply("clax", layers.clax);
  if (layers.you !== undefined) {
    const mode = config["overrides.viewer"];
    if (mode === "always" || (mode === "local" && isDevOrigin(origin))) apply("you", layers.you);
    else for (const k of Object.keys(flatten(layers.you))) ignored.push({ layer: "you", field: k, reason: mode === "never" ? "viewer overrides are off" : "viewer overrides apply on local and tailnet origins only" });
  }
  return { config: config as ClaxConfig, source, ignored };
}

/** The cookie a viewer's override is kept in. */
export const OVERRIDE_COOKIE = "clax_override";
/** How long it is kept: a week. */
const OVERRIDE_MAX_AGE = 7 * 24 * 3600;

/** The viewer's override in `cookie` (a document's `cookie` string), or none. */
export function readOverride(cookie: string): ConfigLayer | undefined {
  for (const part of cookie.split(/;\s*/)) {
    const eq = part.indexOf("=");
    if (eq < 0 || part.slice(0, eq) !== OVERRIDE_COOKIE) continue;
    try {
      const v = JSON.parse(decodeURIComponent(part.slice(eq + 1)));
      return v && typeof v === "object" && !Array.isArray(v) ? flatten(v) : undefined;
    } catch {
      return undefined;
    }
  }
  return undefined;
}

/** Writes `layer` as the viewer's override on `doc` (null clears it). */
export function writeOverride(doc: Document, layer: ConfigLayer | null): void {
  const value = layer && Object.keys(layer).length ? encodeURIComponent(JSON.stringify(layer)) : "";
  const secure = doc.location?.protocol === "https:" ? "; Secure" : "";
  doc.cookie = `${OVERRIDE_COOKIE}=${value}; Path=/; SameSite=Lax; Max-Age=${value ? OVERRIDE_MAX_AGE : 0}${secure}`;
}

/** The `?clax.<field>=` values in `search`, as a layer (unknown fields are kept, for `resolve` to report). */
export function linkOverride(search: string): ConfigLayer | undefined {
  const out: ConfigLayer = {};
  for (const [k, v] of new URLSearchParams(search)) {
    if (!k.startsWith("clax.")) continue;
    const name = k.slice(5);
    const f = (FIELDS as Record<string, Field>)[name];
    if (f?.kind === "targets" || f?.kind === "tokens") {
      try { out[name] = JSON.parse(v); } catch { out[name] = v; }
    } else out[name] = v;
  }
  return Object.keys(out).length ? out : undefined;
}

/** The JSON Schema of `clax.config.json` (draft 2020-12): nested objects by
 * the fields' dotted paths, every field optional, nothing else allowed. */
export function configSchema(): Record<string, unknown> {
  const root: Record<string, unknown> = { type: "object", additionalProperties: false, properties: {} };
  for (const [path, f] of Object.entries(FIELDS) as [string, Field][]) {
    const parts = path.split(".");
    let node = root;
    for (const p of parts.slice(0, -1)) {
      const props = node.properties as Record<string, Record<string, unknown>>;
      node = props[p] ??= { type: "object", additionalProperties: false, properties: {} };
    }
    (node.properties as Record<string, unknown>)[parts.at(-1)!] = fieldSchema(f);
  }
  return { $schema: "https://json-schema.org/draft/2020-12/schema", $id: "https://clax.dev/schemas/clax.config.schema.json", title: "Clax install configuration", ...root };
}

function fieldSchema(f: Field): Record<string, unknown> {
  const base = { description: f.doc, default: f.default };
  switch (f.kind) {
    case "bool": return { type: "boolean", ...base };
    case "enum": return { enum: [...f.values], ...base };
    case "string": return { type: "string", maxLength: f.max ?? 1000, ...base };
    case "targets": return { type: "array", items: { type: "object", additionalProperties: false, required: ["selector", "action"], properties: { selector: { type: "string", minLength: 1 }, action: { enum: [...ACTIONS] } } }, ...base };
    case "tokens": return { type: "object", propertyNames: { pattern: "^--clax-[a-z0-9-]+$" }, additionalProperties: { type: "string" }, ...base };
  }
}

/** A container as Clax serves it: a published version of the site's config. */
export interface Container {
  /** The container's id: the site in Clax (its queue, until containers have their own ids). */
  id: string;
  /** The published version's number; the loader caches by it. */
  version: number;
  config: ConfigLayer;
}

/** Reads a container answer, or why it is not one. */
export function parseContainer(v: unknown): Container {
  const c = v as Partial<Container> | null;
  if (!c || typeof c !== "object") throw new TypeError("a container is an object");
  if (typeof c.id !== "string" || !c.id) throw new TypeError("a container has an id");
  if (!Number.isInteger(c.version) || (c.version as number) < 0) throw new TypeError("a container has a version number");
  if (!c.config || typeof c.config !== "object" || Array.isArray(c.config)) throw new TypeError("a container has a config object");
  return { id: c.id, version: c.version as number, config: c.config };
}
