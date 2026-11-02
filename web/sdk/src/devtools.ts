// The app dev settings hook: how a page's own development settings reach
// the Clax DevTools panel (BRIEF-stint-2026-11-02-clax-devtools-panel item 2;
// the feedback-plugins BRIEF Addendum 2), as React DevTools' global hook
// carries a page's components to its panel.
//
// The page (through this module) installs one object on `window`,
// `__CLAX_DEVTOOLS_HOOK__`, protocol 1. Each part of the page registers a
// section of fields; every field has the value it would have without the
// viewer (its `base`, from the Clax config or the site's code), and may
// carry the viewer's own override, kept in a cookie (`clax_dev_<port>`) or another
// store the host passes. The panel reads `snapshot()` and writes with
// `set()` / `reset()` through `chrome.devtools.inspectedWindow.eval`, so the
// extension needs no permission for it, sends nothing to the page but
// JSON literals, and parses what it reads with `parseSnapshot` below, the
// same validator for both sides.
//
// Only on dev/local origins (`isDevOrigin`), unless the host says the Clax
// config allows overrides here (`allow: true`); `allow: false` never
// installs. Nothing here touches the network. Resolution beyond "the
// viewer's override, else the base" (Clax config over the site's defaults)
// is the host's: it passes the winner as `base` with its `source`.
//
// Under web/sdk's license (LGPL-2.1-only once @empathic/clax-sdk lands on
// wip from main); it imports nothing, so the GPL-2 extension imports its
// types, expressions and validator, and the protocol has one definition.

export const HOOK_KEY = "__CLAX_DEVTOOLS_HOOK__";
export const PROTOCOL = 1;
/** The cookie the default override store keeps the viewer's overrides in (with `_<port>`, `cookieName`). */
export const OVERRIDE_COOKIE = "clax_dev";
/** The query parameter prefix a link sets an override with: `?clax.<section>.<field>=<value>`. */
export const OVERRIDE_PARAM = "clax.";

/** Where a field's effective value comes from: the Clax config, the site's code, or the viewer's own override. */
export type Source = "clax" | "site" | "you";
export type Kind = "radio" | "toggle" | "text";
export type Value = string | boolean;

export interface DevOption {
  value: string;
  label: string;
  description?: string;
  /** Listed but not choosable (such as a style the SDK cannot show yet). */
  disabled?: boolean;
  /** What it needs before it can be chosen (Changeling's "Needs from the Clax SDK"). */
  needs?: string;
}

/** One field as the panel sees it. */
export interface DevField {
  id: string;
  label: string;
  kind: Kind;
  description?: string;
  /** For `radio`. */
  options?: DevOption[];
  /** For `text`. */
  maxLength?: number;
  /** The effective value, and where it came from. */
  value: Value;
  source: Source;
  /** What the field would be without the viewer's override (present when `source` is "you"). */
  inherited?: { value: Value; source: Exclude<Source, "you"> };
  /** `live`: the page applies a change at once; `reload`: on the next load (the loader installs only what the config names). */
  applies: "live" | "reload";
  /** The viewer may not override it here (the Clax config says so). */
  locked?: boolean;
}

export interface DevSection {
  id: string;
  title: string;
  description?: string;
  fields: DevField[];
}

export interface Snapshot {
  v: typeof PROTOCOL;
  /** Goes up on every change the panel should see. */
  rev: number;
  origin: string;
  sections: DevSection[];
}

export type SetResult = { ok: true; field: DevField } | { ok: false; reason: string };

/** The object on `window[HOOK_KEY]`. */
export interface DevtoolsHook {
  v: typeof PROTOCOL;
  rev: number;
  snapshot(): Snapshot;
  set(section: string, field: string, value: Value): SetResult;
  reset(section: string, field: string): SetResult;
  /** Adds a section (a second copy of the SDK on the page registers into the first one's hook). */
  register(section: SectionInit, store: OverrideStore): DevSettingsHandle;
}

/** Where the viewer's overrides are kept, by key `<section>.<field>`. */
export interface OverrideStore {
  get(key: string): Value | undefined;
  set(key: string, value: Value): boolean;
  delete(key: string): void;
}

export interface FieldInit {
  id: string;
  label: string;
  kind: Kind;
  description?: string;
  options?: DevOption[];
  maxLength?: number;
  /** The value without the viewer: the Clax config's (`clax`) or the site's default (`site`). */
  base: { value: Value; source: Exclude<Source, "you"> };
  applies?: "live" | "reload";
  locked?: boolean;
}

export interface SectionInit {
  id: string;
  title: string;
  description?: string;
  fields: FieldInit[];
  /** Told the field's new effective value after the viewer (or a link) changed it. */
  onChange?(field: string, value: Value, source: Source): void;
}

export interface DevSettingsHandle {
  /** The field's effective value. */
  value(field: string): Value | undefined;
  /** Where the field's value comes from. */
  source(field: string): Source | undefined;
  /** The page's own change of a base (a new Clax config, a site default): the viewer's override still wins. */
  setBase(field: string, base: FieldInit["base"]): void;
  dispose(): void;
}

export interface RegisterOptions {
  /** true: install even off a dev/local origin (the Clax config allows overrides here); false: never. */
  allow?: boolean;
  store?: OverrideStore;
  /** The page's window (tests). */
  win?: Window & typeof globalThis;
}

// The caps every snapshot keeps, which `parseSnapshot` checks on the panel's side.
export const LIMITS = { sections: 20, fields: 50, options: 30, id: 64, label: 120, text: 600, value: 2000, json: 256 * 1024 } as const;
const ID = /^[A-Za-z0-9_-]{1,64}$/;

/** Whether `origin` is a development origin: localhost, a loopback address, or a `.localhost`, `.local` or `.test` name. */
export function isDevOrigin(origin: string): boolean {
  let u: URL;
  try { u = new URL(origin); } catch { return false; }
  if (u.protocol !== "http:" && u.protocol !== "https:") return false;
  const h = u.hostname.replace(/^\[|\]$/g, "").toLowerCase();
  return h === "localhost" || h === "::1" || /^127(\.\d{1,3}){3}$/.test(h) || /\.(localhost|local|test)$/.test(h);
}

/** The override cookie's name for a page at `loc`: `clax_dev`, plus `_<port>` when the
 * address names one. Cookies ignore ports, and every dev app on 127.0.0.1 shares one jar
 * (Changeling on 7700, the daemon on 7480): the port keeps each site's overrides its own. */
export const cookieName = (loc: Pick<Location, "port">) => (loc.port ? `${OVERRIDE_COOKIE}_${loc.port}` : OVERRIDE_COOKIE);

/** The viewer's overrides in one cookie, `cookieName(location)=<JSON>` (path /, SameSite=Lax, 30 days, at most 3 KB):
 * a cookie so the loader can read it before anything loads, and a server could too. A refused or full cookie keeps nothing. */
export function cookieStore(doc: Pick<Document, "cookie"> = document, name = cookieName(globalThis.location ?? { port: "" })): OverrideStore {
  const read = (): Record<string, Value> => {
    try {
      const raw = doc.cookie.split("; ").find(c => c.startsWith(`${name}=`))?.slice(name.length + 1);
      const o: unknown = raw ? JSON.parse(decodeURIComponent(raw)) : {};
      if (!o || typeof o !== "object" || Array.isArray(o)) return {};
      return Object.fromEntries(Object.entries(o).filter(([, v]) => typeof v === "string" || typeof v === "boolean")) as Record<string, Value>;
    } catch { return {}; }
  };
  const write = (all: Record<string, Value>): boolean => {
    const v = encodeURIComponent(JSON.stringify(all));
    if (v.length > 3000) return false;
    const empty = Object.keys(all).length === 0;
    try {
      doc.cookie = `${name}=${empty ? "" : v}; path=/; SameSite=Lax; max-age=${empty ? 0 : 30 * 86400}`;
      return empty || read()[Object.keys(all).at(-1)!] !== undefined;
    } catch { return false; }
  };
  return {
    get: key => read()[key],
    set: (key, value) => { const all = read(); delete all[key]; all[key] = value; return write(all); },
    delete: key => { const all = read(); if (key in all) { delete all[key]; write(all); } },
  };
}

/** Whether `v` is a value the field takes. */
function takes(f: FieldInit, v: unknown): v is Value {
  if (f.kind === "toggle") return typeof v === "boolean";
  if (typeof v !== "string") return false;
  if (f.kind === "radio") return !!f.options?.some(o => o.value === v && !o.disabled);
  return v.length <= Math.min(f.maxLength ?? LIMITS.value, LIMITS.value);
}

/** Reads `?clax.<section>.<field>=<value>` links: "true"/"false" for a toggle; an empty value resets. */
function linked(search: string, section: SectionInit): Map<string, string> {
  const out = new Map<string, string>();
  for (const [k, v] of new URLSearchParams(search)) {
    if (!k.startsWith(OVERRIDE_PARAM)) continue;
    const [s, f, ...rest] = k.slice(OVERRIDE_PARAM.length).split(".");
    if (s === section.id && f && !rest.length) out.set(f, v);
  }
  return out;
}

function createHook(win: Window & typeof globalThis): DevtoolsHook {
  type Entry = { init: SectionInit; fields: Map<string, FieldInit>; store: OverrideStore };
  const sections = new Map<string, Entry>();
  const bump = () => { hook.rev++; };
  const override = (e: Entry, f: FieldInit): Value | undefined => {
    if (f.locked) return undefined;
    const v = e.store.get(`${e.init.id}.${f.id}`);
    return takes(f, v) ? v : undefined;
  };
  const view = (e: Entry, f: FieldInit): DevField => {
    const mine = override(e, f);
    return {
      id: f.id, label: f.label, kind: f.kind,
      ...(f.description ? { description: f.description } : {}),
      ...(f.options ? { options: f.options.map(o => ({ ...o })) } : {}),
      ...(f.maxLength ? { maxLength: f.maxLength } : {}),
      value: mine ?? f.base.value, source: mine === undefined ? f.base.source : "you",
      ...(mine === undefined ? {} : { inherited: { ...f.base } }),
      applies: f.applies ?? "live",
      ...(f.locked ? { locked: true } : {}),
    };
  };
  const find = (section: string, field: string) => {
    const e = sections.get(section);
    const f = e?.fields.get(field);
    return e && f ? { e, f } : null;
  };
  const changed = (e: Entry, f: FieldInit) => {
    bump();
    const v = view(e, f);
    try { e.init.onChange?.(f.id, v.value, v.source); } catch { /* the page's own handler; the value stays kept */ }
    return v;
  };
  const hook: DevtoolsHook = {
    v: PROTOCOL,
    rev: 0,
    snapshot: () => ({
      v: PROTOCOL, rev: hook.rev, origin: win.location.origin,
      sections: [...sections.values()].map(e => ({
        id: e.init.id, title: e.init.title, ...(e.init.description ? { description: e.init.description } : {}),
        fields: [...e.fields.values()].map(f => view(e, f)),
      })),
    }),
    set(section, field, value) {
      const at = find(section, field);
      if (!at) return { ok: false, reason: "No such setting." };
      if (at.f.locked) return { ok: false, reason: "The Clax config does not let you change this here." };
      if (!takes(at.f, value)) return { ok: false, reason: "That value is not one this setting takes." };
      if (!at.e.store.set(`${section}.${field}`, value)) return { ok: false, reason: "This browser refused to keep it (cookies blocked or full)." };
      return { ok: true, field: changed(at.e, at.f) };
    },
    reset(section, field) {
      const at = find(section, field);
      if (!at) return { ok: false, reason: "No such setting." };
      at.e.store.delete(`${section}.${field}`);
      return { ok: true, field: changed(at.e, at.f) };
    },
    register(init, store) {
      if (!ID.test(init.id)) throw new Error(`clax devtools: bad section id ${JSON.stringify(init.id)}`);
      const fields = new Map<string, FieldInit>();
      for (const f of init.fields) {
        if (!ID.test(f.id)) throw new Error(`clax devtools: bad field id ${JSON.stringify(f.id)}`);
        fields.set(f.id, { ...f, base: { ...f.base } });
      }
      const e: Entry = { init, fields, store };
      sections.set(init.id, e);
      // A link's override is kept as if set in the panel (an empty value resets).
      for (const [id, raw] of linked(win.location.search, init)) {
        const f = fields.get(id);
        if (!f || f.locked) continue;
        const v: unknown = f.kind === "toggle" ? (raw === "true" ? true : raw === "false" ? false : raw) : raw;
        if (raw === "") store.delete(`${init.id}.${id}`);
        else if (takes(f, v)) store.set(`${init.id}.${id}`, v);
      }
      bump();
      return {
        value: id => { const f = fields.get(id); return f ? view(e, f).value : undefined; },
        source: id => { const f = fields.get(id); return f ? view(e, f).source : undefined; },
        setBase(id, base) { const f = fields.get(id); if (f) { f.base = { ...base }; bump(); } },
        dispose() { if (sections.get(init.id) === e) { sections.delete(init.id); bump(); } },
      };
    },
  };
  return hook;
}

/** A hook another copy of this module installed, if it speaks protocol 1. */
function existing(win: Window & typeof globalThis): DevtoolsHook | null {
  const h = (win as unknown as Record<string, unknown>)[HOOK_KEY] as Partial<DevtoolsHook> | undefined;
  return h && h.v === PROTOCOL && typeof h.register === "function" ? (h as DevtoolsHook) : null;
}

/** Registers `section` with the page's hook (installed on first use) for the
 * Clax DevTools panel; null when the page is not one to show them on (not a
 * dev/local origin and not `allow`ed, or `allow: false`). A section of the same
 * id registered again replaces the earlier one. */
export function registerDevSettings(section: SectionInit, opts: RegisterOptions = {}): DevSettingsHandle | null {
  const win = opts.win ?? (globalThis as Window & typeof globalThis);
  if (opts.allow === false || !win?.location) return null;
  if (!opts.allow && !isDevOrigin(win.location.origin)) return null;
  let hook = existing(win);
  if (!hook) {
    hook = createHook(win);
    Object.defineProperty(win, HOOK_KEY, { value: hook, configurable: true, enumerable: false, writable: false });
  }
  return hook.register(section, opts.store ?? cookieStore(win.document, cookieName(win.location)));
}

// The panel's side: what it reads is the page's, so it is checked whole.

const isObj = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
const str = (v: unknown, max: number): v is string => typeof v === "string" && v.length <= max;
const optStr = (v: unknown, max: number) => v === undefined || str(v, max);
const isSource = (v: unknown): v is Source => v === "clax" || v === "site" || v === "you";
const isValue = (v: unknown): v is Value => typeof v === "boolean" || str(v, LIMITS.value);

function parseField(v: unknown): DevField | null {
  if (!isObj(v) || !str(v.id, LIMITS.id) || !ID.test(v.id) || !str(v.label, LIMITS.label) || !optStr(v.description, LIMITS.text)) return null;
  if (v.kind !== "radio" && v.kind !== "toggle" && v.kind !== "text") return null;
  if (!isValue(v.value) || !isSource(v.source) || (v.applies !== "live" && v.applies !== "reload")) return null;
  if (v.locked !== undefined && typeof v.locked !== "boolean") return null;
  if (v.maxLength !== undefined && !(Number.isSafeInteger(v.maxLength) && (v.maxLength as number) > 0)) return null;
  if ((v.kind === "toggle") !== (typeof v.value === "boolean")) return null;
  let options: DevOption[] | undefined;
  if (v.kind === "radio") {
    if (!Array.isArray(v.options) || !v.options.length || v.options.length > LIMITS.options) return null;
    options = [];
    for (const o of v.options) {
      if (!isObj(o) || !str(o.value, LIMITS.value) || !str(o.label, LIMITS.label) || !optStr(o.description, LIMITS.text) || !optStr(o.needs, LIMITS.text)) return null;
      if (o.disabled !== undefined && typeof o.disabled !== "boolean") return null;
      options.push({ value: o.value, label: o.label, ...(o.description ? { description: o.description } : {}), ...(o.disabled ? { disabled: true } : {}), ...(o.needs ? { needs: o.needs } : {}) });
    }
  } else if (v.options !== undefined) return null;
  let inherited: DevField["inherited"];
  if (v.inherited !== undefined) {
    const i = v.inherited;
    if (!isObj(i) || !isValue(i.value) || (i.source !== "clax" && i.source !== "site")) return null;
    inherited = { value: i.value, source: i.source };
  }
  return {
    id: v.id, label: v.label, kind: v.kind, ...(v.description ? { description: v.description } : {}),
    ...(options ? { options } : {}), ...(v.maxLength ? { maxLength: v.maxLength as number } : {}),
    value: v.value, source: v.source, ...(inherited ? { inherited } : {}), applies: v.applies, ...(v.locked ? { locked: true } : {}),
  };
}

/** The snapshot the panel read (a JSON string, or the parsed value), checked whole; null when anything is off. */
export function parseSnapshot(raw: unknown): Snapshot | null {
  let v: unknown = raw;
  if (typeof raw === "string") {
    if (raw.length > LIMITS.json) return null;
    try { v = JSON.parse(raw); } catch { return null; }
  }
  if (!isObj(v) || v.v !== PROTOCOL || !Number.isSafeInteger(v.rev) || !str(v.origin, LIMITS.text) || !Array.isArray(v.sections) || v.sections.length > LIMITS.sections) return null;
  const sections: DevSection[] = [];
  for (const s of v.sections) {
    if (!isObj(s) || !str(s.id, LIMITS.id) || !ID.test(s.id) || !str(s.title, LIMITS.label) || !optStr(s.description, LIMITS.text)) return null;
    if (!Array.isArray(s.fields) || s.fields.length > LIMITS.fields) return null;
    const fields: DevField[] = [];
    for (const f of s.fields) { const p = parseField(f); if (!p) return null; fields.push(p); }
    sections.push({ id: s.id, title: s.title, ...(s.description ? { description: s.description } : {}), fields });
  }
  return { v: PROTOCOL, rev: v.rev as number, origin: v.origin, sections };
}

/** The result of a `set` or `reset` the panel made; null when it is not one. */
export function parseSetResult(raw: unknown): SetResult | null {
  if (typeof raw === "string") { try { raw = JSON.parse(raw); } catch { return null; } }
  if (!isObj(raw)) return null;
  if (raw.ok === false) return str(raw.reason, LIMITS.text) ? { ok: false, reason: raw.reason } : null;
  if (raw.ok !== true) return null;
  const field = parseField(raw.field);
  return field ? { ok: true, field } : null;
}

// The expressions the panel evaluates in the page. Each returns a JSON
// string (or a number), never an object, and fails closed to null.
const call = (body: string) =>
  `(() => { try { const h = window[${JSON.stringify(HOOK_KEY)}]; if (!h || h.v !== ${PROTOCOL}) return null; ${body} } catch (e) { return null; } })()`;
/** The hook's `rev`, or null when the page has no hook. */
export const REV_EXPR = call("return Number(h.rev);");
/** The hook's snapshot, as a JSON string of at most `LIMITS.json`. */
export const SNAPSHOT_EXPR = call(`const s = JSON.stringify(h.snapshot()); return s.length > ${LIMITS.json} ? null : s;`);
/** Sets `field` of `section` to `value`; every argument a JSON literal. */
export const setExpr = (section: string, field: string, value: Value) =>
  call(`return JSON.stringify(h.set(${JSON.stringify(section)}, ${JSON.stringify(field)}, ${JSON.stringify(value)}));`);
/** Drops the viewer's override of `field` of `section`. */
export const resetExpr = (section: string, field: string) =>
  call(`return JSON.stringify(h.reset(${JSON.stringify(section)}, ${JSON.stringify(field)}));`);
