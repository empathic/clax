// The DevTools panel's link to the inspected page's Clax install (the
// loader's `window.__CLAX_INSTALL__`, web/sdk/src/config.ts `InstallInfo`):
// which container and deployment put Clax there, the config version loaded,
// and every config field with its value and where it came from. A page needs
// no code of its own for this (white label): the install itself answers.
// The viewer's override is written in the page's own cookie by the page's
// own check (`set`), and applies at the next load, as the loader reads it
// then. Read through `inspectedWindow.eval`, checked whole here.
import { type ClaxConfig, FIELDS, type FieldName, INSTALL_KEY, type InstallInfo, PRESENTATIONS, type Source } from "../../../sdk/src/config";
import type { InspectEnv } from "./settings.svelte";

const call = (body: string) =>
  `(() => { try { const h = window[${JSON.stringify(INSTALL_KEY)}]; if (!h || h.v !== 1) return null; ${body} } catch (e) { return null; } })()`;
/** The install's info, as a JSON string (at most 64 KB), or null. */
export const INFO_EXPR = call("const s = h.info(); return typeof s === 'string' && s.length <= 65536 ? s : null;");
/** Sets (or, with null, drops) the viewer's override of `field`; every argument a JSON literal. */
export const overrideExpr = (field: FieldName, value: unknown) => call(`return JSON.stringify(h.set(${JSON.stringify(field)}, ${JSON.stringify(value)}));`);

const isObj = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
const str = (v: unknown, max = 2000): v is string => typeof v === "string" && v.length <= max;
const strOrNull = (v: unknown, max = 2000) => v === null || str(v, max);
const SOURCES: readonly Source[] = ["default", "site", "clax", "you"];

/** The page's answer checked whole; null when anything is off. */
export function parseInfo(raw: unknown): InstallInfo | null {
  let v = raw;
  if (typeof raw === "string") { try { v = JSON.parse(raw); } catch { return null; } }
  if (!isObj(v) || v.v !== 1) return null;
  if (!str(v.origin) || !str(v.instance) || !str(v.container, 200) || !strOrNull(v.error) || !strOrNull(v.deployment, 100) || !strOrNull(v.script)) return null;
  if (!(v.version === null || (Number.isSafeInteger(v.version) && (v.version as number) >= 0))) return null;
  if (v.carrier !== "site" && v.carrier !== "extension") return null;
  if (typeof v.applies !== "boolean" || !isObj(v.config) || !isObj(v.source)) return null;
  for (const k of Object.keys(FIELDS)) if (!(k in v.config) || !SOURCES.includes(v.source[k] as Source)) return null;
  if (!Array.isArray(v.plugins) || !v.plugins.every(p => str(p, 64))) return null;
  if (!(v.built === null || (Array.isArray(v.built) && v.built.every(p => str(p, 64))))) return null;
  if (!Array.isArray(v.ignored) || !v.ignored.every(i => isObj(i) && SOURCES.includes(i.layer as Source) && str(i.field, 200) && str(i.reason, 500))) return null;
  return v as unknown as InstallInfo;
}

export type SetAnswer = { ok: true } | { ok: false; reason: string };
function parseSet(raw: unknown): SetAnswer | null {
  if (typeof raw === "string") { try { raw = JSON.parse(raw); } catch { return null; } }
  if (!isObj(raw)) return null;
  if (raw.ok === true) return { ok: true };
  return raw.ok === false && str(raw.reason, 500) ? { ok: false, reason: raw.reason } : null;
}

/** Where a value came from, in the panel's words. */
export const SOURCE_TEXT: Record<Source, string> = { default: "Clax default", site: "Site default", clax: "Container", you: "You" };

/** One choice of a row: `built` false when the overlay has no such thing yet. */
export interface Choice { value: string; label: string; built: boolean }
/** One row of the Integration view: a config field, or a choice the overlay has not built. */
export interface Row {
  field: FieldName;
  label: string;
  kind: "bool" | "enum" | "string" | "tokens" | "targets";
  value: unknown;
  source: Source;
  choices?: Choice[];
  /** Not built at all (a row, not a dead control). */
  notBuilt?: string;
}
export interface Group { id: string; title: string; rows: Row[] }

const PRESENTATION_TEXT: Record<(typeof PRESENTATIONS)[number], string> = {
  floating: "Floating in the corner", dialog: "Dialog", drawer: "Side drawer", sheet: "Bottom sheet", window: "Its own window", extension: "The Clax extension",
};
const LABEL: Partial<Record<FieldName, string>> = {
  enabled: "Clax on this site", "entry.launcher": "Floating launcher", "entry.targets": "Host triggers (data-clax, selectors)", "entry.key": "Keyboard shortcut",
  "entry.palette": "Command palette entry", "entry.link": "?clax= links", targets: "Host trigger selectors", presentation: "Presentation", start: "Start mode",
  "attachments.screenshot": "Screenshot", annotate: "Annotate", "filters.code-blur": "Blur code", "pickers.code-line": "Code-line picker",
  "attachments.errors": "Recent errors", "attachments.logs": "App logs", "attachments.version": "App version", "attachments.browser": "Browser",
  pins: "Inline pins", theme: "Theme tokens", render: "Clax pages drawn", "filters.code-selector": "What counts as code", "overrides.viewer": "Viewer overrides",
};
const START_TEXT: Record<ClaxConfig["start"], string> = { note: "Note", screenshot: "Screenshot then annotate", element: "Element picker first" };

/** The fields as groups (presentation, entry, start, plugins, theme), each row with its value and source. */
export function groups(i: InstallInfo): Group[] {
  const row = (field: FieldName, extra: Partial<Row> = {}): Row => ({
    field, label: LABEL[field] ?? field, kind: FIELDS[field].kind, value: i.config[field], source: i.source[field], ...extra,
  });
  const has = (id: string) => i.built === null || i.built.includes(id);
  const presentation = row("presentation", {
    choices: PRESENTATIONS.map(p => ({ value: p, label: PRESENTATION_TEXT[p], built: has(p) })),
  });
  const pins = row("pins", has("pins") ? {} : { notBuilt: "Clax has no pins plugin yet." });
  return [
    { id: "presentation", title: "Presentation", rows: [presentation, pins] },
    { id: "entry", title: "Entry", rows: [row("enabled"), row("entry.launcher"), row("entry.key"), row("entry.targets"), row("targets"), row("entry.palette"), row("entry.link")] },
    { id: "start", title: "Start mode", rows: [row("start", { choices: (FIELDS.start.values as readonly ClaxConfig["start"][]).map(v => ({ value: v, label: START_TEXT[v], built: true })) })] },
    {
      id: "plugins", title: "Plugins",
      rows: (["attachments.screenshot", "annotate", "filters.code-blur", "pickers.code-line", "attachments.errors", "attachments.logs", "attachments.version", "attachments.browser"] as FieldName[]).map(f => row(f)),
    },
    { id: "theme", title: "Theme", rows: [row("theme")] },
  ];
}

/** A value as the panel shows it. */
export function shownValue(r: Pick<Row, "kind" | "value" | "choices">): string {
  const v = r.value;
  if (typeof v === "boolean") return v ? "On" : "Off";
  if (r.choices) return r.choices.find(c => c.value === v)?.label ?? String(v);
  if (r.kind === "tokens") { const n = Object.keys((v as object) ?? {}).length; return n ? `${n} token${n === 1 ? "" : "s"}` : "none set"; }
  if (r.kind === "targets") { const n = (v as unknown[] | null)?.length ?? 0; return n ? `${n} selector${n === 1 ? "" : "s"}` : "none"; }
  return v === "" ? "none" : `“${String(v)}”`;
}

/** Changeling's on-page "Dev · Feedback style" options, each with the
 * Integration view row that replaces it: the removal checklist. */
export const CHANGELING_PANEL: readonly { option: string; field: FieldName; value: unknown }[] = [
  { option: "Send feedback (prod)", field: "presentation", value: "dialog" },
  { option: "Header button → Clax overlay", field: "targets", value: [{ selector: "<the header button>", action: "report" }] },
  { option: "Floating launcher", field: "entry.launcher", value: true },
  { option: "Clax off", field: "enabled", value: false },
  { option: "Side drawer", field: "presentation", value: "drawer" },
  { option: "Bottom sheet", field: "presentation", value: "sheet" },
  { option: "Inline pins", field: "pins", value: true },
  { option: "Screenshot then annotate", field: "start", value: "screenshot" },
  { option: "Element picker first", field: "start", value: "element" },
  { option: "Command palette entry", field: "entry.palette", value: true },
];

export class IntegrationLink {
  /** `loading` until the first read; `none` when the page has no Clax install (or answered something off). */
  status = $state<"loading" | "none" | "ready">("loading");
  info = $state<InstallInfo | null>(null);
  private readonly off: () => void;

  constructor(private readonly env: InspectEnv) {
    this.off = env.onNavigated(() => void this.refresh());
  }

  async refresh(): Promise<void> {
    const i = parseInfo(await this.env.eval(INFO_EXPR));
    this.info = i;
    this.status = i ? "ready" : "none";
  }

  /** Sets the viewer's override of `field` to `value` (null drops it); applies at the next load. */
  async set(field: FieldName, value: unknown): Promise<SetAnswer> {
    const r = parseSet(await this.env.eval(overrideExpr(field, value))) ?? { ok: false as const, reason: "The page did not answer. Reload it and try again." };
    return r;
  }

  close(): void { this.off(); }
}
