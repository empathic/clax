// The SDK's plugin contract and its host (RECOMMEND-consultant-2026-11-01-
// clax-integration-package Q5): a light core (the report model, transport,
// identity, config, lifecycle and an event bus) and named slots every feedback
// feature ties into. Capture, annotate, privacy filters, attachments,
// pickers, presentations and themes are plugins: the overlay renders none of
// them unless a plugin contributes it.
//
// The contract is framework-free: a plugin is plain data and functions. A
// slot that shows UI takes a `mount(el, ctx) → dispose` (any framework, or
// none); Clax's own plugins may also hand the overlay one of its Svelte
// components (`ui`), which only the overlay reads.
//
// Every plugin that adds data to a report says so with a sent-row, which
// "What will be sent" lists; the host refuses data that comes without one.

import type { DiagLine, Environment, Release } from "./types";

/** The contract's version. A plugin built for another major is refused. */
export const PLUGIN_API_VERSION = 1;

/** What a plugin may ask the core for: a closed list. A plugin asking for
 * anything else, or for what the host does not grant, is refused. */
export const CAPABILITIES = ["dom:read", "dom:render", "media:display", "storage:local", "net:host"] as const;
export type Capability = (typeof CAPABILITIES)[number];

/** The slots a plugin contributes to. */
export const SLOTS = ["capture", "annotate", "filters", "attachments", "pickers", "presentations", "theme", "adapter", "self"] as const;
export type SlotName = (typeof SLOTS)[number];

/** One line of "What will be sent": what a plugin adds to the report, shown
 * to the reporter before they send it. `optIn`: the reporter ticks it to
 * send it; `default` is whether it starts ticked. */
export interface SentRow {
  /** The contribution's id (the attachment's, filter's or capture's). */
  id: string;
  label: string;
  /** What is sent, as the reporter reads it (text; a list's lines joined by newlines). */
  preview: string;
  /** About how many bytes it adds, when known. */
  bytes?: number;
  optIn: boolean;
  default: boolean;
  /** How it is kept private (what is left out or scrubbed). */
  privacyNote?: string;
}

/** A rectangle in CSS or image pixels. */
export interface Box { x: number; y: number; w: number; h: number }

/** A capture method: how the screenshot is made. */
export interface CaptureContribution {
  id: string;
  label: string;
  /** `dom`: a private render of the page's DOM, no permission asked.
   * `display`: the browser's own screenshot of the tab, after the reporter allows it. */
  method: "dom" | "display";
  /** Whether this browser can make it. */
  supported?(win: Window): boolean;
  /** Makes the capture. The request and result are the overlay's
   * (`overlay/src/capture.ts`): a plugin for another host uses its own. */
  take(req: unknown): Promise<unknown>;
}

/** A drawing tool over a captured image. */
export interface AnnotateTool {
  /** `box`, `arrow`, `highlight`, `redact` or `text` for Clax's own Annotator. */
  id: string;
  label: string;
  /** The key that picks the tool in the annotator. */
  key?: string;
}

/** The annotate slot: the tools the reporter draws with. Undo and Clear go
 * with any tool. */
export interface AnnotateContribution {
  tools: AnnotateTool[];
  /** A host-specific UI component (Clax's overlay: its Annotator). */
  ui?: unknown;
}

/** What a filter is given: a captured PNG, the part of the page it shows
 * (`at`, page CSS pixels) and its pixels per CSS pixel. */
export interface FilterInput {
  png: Blob;
  at: Box;
  scale: number;
  doc: Document;
}

/** What a filter gives back: the PNG (the same one when it changed
 * nothing) and a line for "What will be sent", such as "Code blurred (2)". */
export interface FilterOutput {
  png: Blob;
  /** What it did, as the reporter reads it; null when it did nothing. */
  note: string | null;
}

/** A privacy filter: runs on every capture before the reporter sees it. */
export interface FilterContribution {
  id: string;
  label: string;
  /** Whether it runs unless the reporter turns it off. Privacy filters default on. */
  default: boolean;
  apply(input: FilterInput): Promise<FilterOutput>;
}

/** What an attachment adds to a report, on the wire fields a report already
 * has (no new field): log lines as `diagnostics`, the app's version as
 * `context.release`, the browser as `context.environment`, anything else of
 * the host's as `context.app` (merged by key). */
export interface AttachmentData {
  diagnostics?: DiagLine[];
  release?: Release;
  environment?: Partial<Environment>;
  app?: Record<string, unknown>;
}

/** What an attachment's `collect` gives: its data and its sent-row. */
export interface Collected {
  data: AttachmentData;
  row: SentRow | null;
}

/** Data added at send time behind an opt-in box. */
export interface AttachmentContribution {
  id: string;
  label: string;
  /** Whether its box starts ticked. */
  default: boolean;
  /** Reads what it would send now (when the report starts, and again before it is sent). */
  collect(ctx: { doc: Document }): Promise<Collected> | Collected;
}

/** How a report points at the page. `element` (pointer and keyboard; a text
 * selection is a `text` pick), `area`, `page` are the overlay's comment
 * mode; a refinement (`refine`) adds what it knows of a pick, as a code line
 * picker adds the file and lines. */
export interface PickerContribution {
  id: string;
  label: string;
  /** The key that chooses it in the mode bar. */
  key?: string;
  /** Which of the overlay's modes it is, if it is one. */
  mode?: "element" | "area" | "screen";
  /** What this picker adds to a pick on `target` (an element or a text
   * range), as `context.app` keys; null when the pick is not its kind. */
  refine?(target: Element | Range | null, doc: Document): Record<string, unknown> | null;
}

/** How the composer is laid out. */
export type Layout = "dock" | "dialog" | "drawer" | "sheet" | "window";

/** What a presentation is asked to do when a report starts. */
export interface HandoffRequest {
  mode: "element" | "area" | "screen" | "note";
  url: string;
  title: string;
}

/** How the composer is shown. */
export interface PresentationContribution {
  /** `floating`, `dialog`, `drawer`, `sheet`, `window`, `extension`. */
  id: string;
  label: string;
  layout: Layout;
  /** Whether it can be used here (an extension that is installed, a window that may open). */
  available?(win: Window): boolean;
  /** Its stand-in when it is not available. */
  fallback?: string;
  /** Hands the report to something outside the page (the Clax extension);
   * true when it took it, so the overlay shows nothing. */
  handoff?(req: HandoffRequest, win: Window): boolean;
}

/** A theme: `--clax-*` custom properties, which reach into the overlay's shadow root. */
export interface ThemeContribution {
  tokens: Record<string, string>;
}

/** A host framework's lifecycle (React, Svelte, a tag manager): outside the overlay. */
export interface AdapterContribution {
  name: string;
}

/** A second target: the overlay's own queue, for feedback on the feedback tool. */
export interface SelfContribution {
  instance: string;
  queue: string;
}

/** What a plugin contributes, by slot. */
export interface Contributions {
  capture?: CaptureContribution[];
  annotate?: AnnotateContribution;
  filters?: FilterContribution[];
  attachments?: AttachmentContribution[];
  pickers?: PickerContribution[];
  presentations?: PresentationContribution[];
  theme?: ThemeContribution;
  adapter?: AdapterContribution;
  self?: SelfContribution;
}

/** The events on the bus, and what each carries. */
export interface BusEvents {
  pick: { mode: string; target: Element | Range | null };
  compose: { body: string };
  "before-send": { body: string; rows: SentRow[] };
  sent: { id: string };
  reply: { id: string };
  error: { message: string };
}
export type BusEvent = keyof BusEvents;

/** Events a listener may cancel (with a reason the reporter reads). */
export const CANCELLABLE: readonly BusEvent[] = ["pick", "before-send"];

/** What a listener is handed: the event's payload, and `cancel` on a cancellable one. */
export type BusMessage<E extends BusEvent> = BusEvents[E] & { readonly event: E; cancel(reason: string): void };

/** What a plugin's `setup` is given: the bus and the capabilities it was granted. */
export interface PluginContext {
  on<E extends BusEvent>(event: E, fn: (m: BusMessage<E>) => void): () => void;
  granted: readonly Capability[];
}

/** A plugin, as `definePlugin` makes it. */
export interface ClaxPlugin {
  name: string;
  version: string;
  apiVersion: number;
  capabilities: Capability[];
  contributes: Contributions;
  setup?(ctx: PluginContext): void | (() => void);
}

const NAME = /^[a-z0-9][a-z0-9-]*(\/[a-z0-9][a-z0-9-]*)?$/;

/** Checks `p` and gives it back: a plugin with no name, a name that is not
 * lower-case kebab (`code-blur`, or scoped `acme/logs`), an unknown
 * capability or slot is a TypeError at the author's desk, not at a reporter's. */
export function definePlugin<P extends ClaxPlugin>(p: P): P {
  if (!p || typeof p !== "object") throw new TypeError("definePlugin: a plugin is an object");
  if (typeof p.name !== "string" || !NAME.test(p.name)) throw new TypeError(`definePlugin: name ${JSON.stringify(p.name)} is not lower-case kebab`);
  if (typeof p.version !== "string" || !p.version) throw new TypeError(`definePlugin(${p.name}): version must be a non-empty string`);
  if (!Number.isInteger(p.apiVersion)) throw new TypeError(`definePlugin(${p.name}): apiVersion must be an integer`);
  for (const c of p.capabilities ?? []) if (!(CAPABILITIES as readonly string[]).includes(c)) throw new TypeError(`definePlugin(${p.name}): no capability ${JSON.stringify(c)}`);
  for (const k of Object.keys(p.contributes ?? {})) if (!(SLOTS as readonly string[]).includes(k)) throw new TypeError(`definePlugin(${p.name}): no slot ${JSON.stringify(k)}`);
  return p;
}

/** A plugin the host would not run, and why. */
export interface Refusal {
  name: string;
  reason: string;
}

/** What the attachments a reporter ticked add to a report, merged, with the rows they showed. */
export interface Attached {
  data: AttachmentData;
  rows: SentRow[];
  /** Attachments whose data was dropped: they gave no sent-row, or failed. */
  refused: Refusal[];
}

/** The plugins of one overlay: checks each, runs their setups, keeps their
 * contributions by slot, runs the bus. */
export class PluginHost {
  readonly plugins: ClaxPlugin[] = [];
  readonly refused: Refusal[] = [];
  private listeners = new Map<BusEvent, Set<(m: BusMessage<BusEvent>) => void>>();
  private teardown: (() => void)[] = [];

  /** `grant`: the capabilities this host gives (default all). */
  constructor(plugins: readonly ClaxPlugin[] = [], private grant: readonly Capability[] = CAPABILITIES) {
    for (const p of plugins) this.add(p);
  }

  /** Adds `p`, unless it is refused: another major of the contract, a
   * capability not granted, or a name already taken. */
  add(p: ClaxPlugin): boolean {
    const name = typeof p?.name === "string" ? p.name : "(unnamed)";
    const refuse = (reason: string) => { this.refused.push({ name, reason }); return false; };
    try { definePlugin(p); } catch (e) { return refuse((e as Error).message); }
    if (p.apiVersion !== PLUGIN_API_VERSION) return refuse(`built for plugin API ${p.apiVersion}, this is ${PLUGIN_API_VERSION}`);
    const denied = p.capabilities.filter(c => !this.grant.includes(c));
    if (denied.length) return refuse(`asks for ${denied.join(", ")}, not granted`);
    if (this.plugins.some(q => q.name === p.name)) return refuse("a plugin of that name is already in");
    this.plugins.push(p);
    try {
      const off = p.setup?.({ on: (e, fn) => this.on(e, fn), granted: p.capabilities.slice() });
      if (typeof off === "function") this.teardown.push(off);
    } catch (e) {
      this.plugins.pop();
      return refuse(`setup threw: ${(e as Error)?.message ?? String(e)}`);
    }
    return true;
  }

  /** Whether a plugin named `name` is in. */
  has(name: string): boolean {
    return this.plugins.some(p => p.name === name);
  }

  /** Every contribution to a list slot, in plugin order. */
  list<K extends "capture" | "filters" | "attachments" | "pickers" | "presentations">(slot: K): NonNullable<Contributions[K]> {
    return this.plugins.flatMap(p => (p.contributes[slot] ?? []) as never[]) as NonNullable<Contributions[K]>;
  }

  /** The annotate tools of every plugin, in order, each id once. */
  tools(): AnnotateTool[] {
    const out: AnnotateTool[] = [];
    for (const p of this.plugins) for (const t of p.contributes.annotate?.tools ?? []) if (!out.some(o => o.id === t.id)) out.push(t);
    return out;
  }

  /** The annotate UI the first annotating plugin gives. */
  annotateUi(): unknown {
    return this.plugins.find(p => p.contributes.annotate?.ui)?.contributes.annotate?.ui;
  }

  /** The theme tokens, later plugins' over earlier ones'. Only `--clax-*` names are kept. */
  theme(): Record<string, string> {
    const out: Record<string, string> = {};
    for (const p of this.plugins) for (const [k, v] of Object.entries(p.contributes.theme?.tokens ?? {})) if (/^--clax-[a-z0-9-]+$/.test(k) && typeof v === "string") out[k] = v;
    return out;
  }

  /** The presentation `id`, else its fallbacks in turn, else the first one
   * available, else null (the overlay's own dock). */
  presentation(id: string | undefined, win: Window): PresentationContribution | null {
    const all = this.list("presentations");
    const seen = new Set<string>();
    let want = id;
    while (want && !seen.has(want)) {
      seen.add(want);
      const p = all.find(x => x.id === want);
      if (!p) break;
      if (!p.available || p.available(win)) return p;
      want = p.fallback;
    }
    return all.find(p => !p.available || p.available(win)) ?? null;
  }

  /** What the pickers' refinements add to a pick on `target`, as `context.app` keys. */
  refine(target: Element | Range | null, doc: Document): Record<string, unknown> {
    const out: Record<string, unknown> = {};
    for (const p of this.list("pickers")) {
      if (!p.refine) continue;
      try { Object.assign(out, p.refine(target, doc) ?? {}); } catch (e) { console.error(`Clax: picker ${p.id} threw`, e); }
    }
    return out;
  }

  /** Runs the filters `on` (ids) over `input` in plugin order; each one's
   * note is kept. A filter that throws stops the capture: a privacy filter
   * that cannot run must not let the unfiltered image through. */
  async filter(input: FilterInput, on: readonly string[]): Promise<{ png: Blob; notes: { id: string; note: string }[] }> {
    let png = input.png;
    const notes: { id: string; note: string }[] = [];
    for (const f of this.list("filters")) {
      if (!on.includes(f.id)) continue;
      let out: FilterOutput;
      try {
        out = await f.apply({ ...input, png });
      } catch (e) {
        throw new Error(`the ${f.label} filter could not run (${(e as Error)?.message ?? String(e)}), so the screenshot is left out`, { cause: e });
      }
      png = out.png;
      if (out.note) notes.push({ id: f.id, note: out.note });
    }
    return { png, notes };
  }

  /** Collects the attachments `on` (ids). Data with no sent-row is not
   * sent: the reporter never sees it, so it is refused. */
  async attach(on: readonly string[], doc: Document): Promise<Attached> {
    const rows: SentRow[] = [];
    const refused: Refusal[] = [];
    const data: AttachmentData = {};
    for (const a of this.list("attachments")) {
      if (!on.includes(a.id)) continue;
      let c: Collected;
      try {
        c = await a.collect({ doc });
      } catch (e) {
        refused.push({ name: a.id, reason: `failed: ${(e as Error)?.message ?? String(e)}` });
        continue;
      }
      if (!c?.row) {
        refused.push({ name: a.id, reason: "no sent-row: the reporter would not see it" });
        continue;
      }
      rows.push(c.row);
      mergeData(data, c.data ?? {});
    }
    return { data, rows, refused };
  }

  /** Listens for `event`; the result stops listening. */
  on<E extends BusEvent>(event: E, fn: (m: BusMessage<E>) => void): () => void {
    let set = this.listeners.get(event);
    if (!set) this.listeners.set(event, (set = new Set()));
    set.add(fn as (m: BusMessage<BusEvent>) => void);
    return () => { set!.delete(fn as (m: BusMessage<BusEvent>) => void); };
  }

  /** Tells every listener; a cancellable event comes back with why the
   * first listener cancelled it (null: it goes ahead). A listener that
   * throws is logged and skipped. */
  emit<E extends BusEvent>(event: E, payload: BusEvents[E]): { cancelled: string | null } {
    let cancelled: string | null = null;
    const can = CANCELLABLE.includes(event);
    for (const fn of [...(this.listeners.get(event) ?? [])]) {
      const m = { ...payload, event, cancel: (reason: string) => { if (can && cancelled === null) cancelled = String(reason || "cancelled"); } } as BusMessage<BusEvent>;
      try { fn(m); } catch (e) { console.error(`Clax: a ${event} listener threw`, e); }
    }
    return { cancelled };
  }

  /** Ends every plugin's setup and drops the listeners. */
  destroy(): void {
    for (const off of this.teardown.splice(0)) {
      try { off(); } catch (e) { console.error("Clax: a plugin's teardown threw", e); }
    }
    this.listeners.clear();
  }
}

/** Merges `b` into `a`: lines appended, objects merged by key (later wins). */
function mergeData(a: AttachmentData, b: AttachmentData): void {
  if (b.diagnostics?.length) a.diagnostics = [...(a.diagnostics ?? []), ...b.diagnostics];
  if (b.release) a.release = { ...a.release, ...b.release };
  if (b.environment) a.environment = { ...a.environment, ...b.environment };
  if (b.app) a.app = { ...a.app, ...b.app };
}
