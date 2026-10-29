// The overlay's model, apart from the DOM: its options; the context,
// metadata and recent errors a capture takes, evaluated when the capture
// starts; the draft (a new report, or a "Still broken" reopen) with its
// capture, marks and attachments, and what a send files; the reporter's
// verdict; the "Your reports" poll, which runs only while the panel is open;
// and the badge poll, which runs while the page is visible.
import type { Anchor } from "../../bridge/src/protocol";
import type { Recorder } from "../../sdk/src/diagnostics";
import { environment } from "../../sdk/src/environment";
import { type NewReport, type Reopening, type ReporterAuth, type Verification, contextJson } from "../../sdk/src/reporter";
import { scrub, wellFormed } from "../../sdk/src/scrub";
import type { Badge, CaptureInfo, DiagLine, FixAttempt, FixOutcome, FlagValue, Release, Report, ReportComment, ReportContext, ReportPage, ResolveReason } from "../../sdk/src/types";
import { type Placement, type Shape, remapShapes } from "./annotate";
import type { Captured } from "./capture";

/** How often "Your reports" asks for new replies while it is open. */
export const POLL_MS = 15_000;
/** How often the badge is asked for while the page is visible. */
export const BADGE_POLL_MS = 60_000;
/** How long the host's `context` and `metadata` functions may take before
 * the capture goes on without them. */
export const CONTEXT_TIMEOUT_MS = 2000;
/** The largest context Clax takes, as compact JSON. */
export const MAX_CONTEXT_BYTES = 16 * 1024;
/** The longest title a report carries (the page's title, cut). */
export const MAX_TITLE = 300;
/** The `<meta name>` a page names its artifact's external ID with. */
export const ARTIFACT_META = "clax-artifact";

/** What the host says about the moment of a capture. Only `release`,
 * `flags` and `app` are taken; the time, environment and capture are the
 * overlay's own. */
export type HostContext = Partial<ReportContext>;

/** A value, or a function that gives it (perhaps later). */
export type Given<T> = T | (() => T | Promise<T>);

/** `Clax.overlay`'s options. Every report carries the page's URL with its
 * query and without its fragment, and the page's title.
 *
 * A host built from configuration passes it through: clax.dev's shell, from
 * its `/_app/feedback/config` (the queue, whether this person may file, and
 * a context of release, commit, app and flags), passes the queue as
 * `queue`, whether the person may file as `canFile`, and the context as
 * `context`, its commit as `release.commit`. */
export interface OverlayOptions {
  /** The instance's base URL, such as `https://clax.corp.example`. */
  instance: string;
  /** The queue's ID. */
  queue: string;
  /** Gets a reporter token for the reporter's public key, from the deployer's backend. */
  auth: ReporterAuth;
  /** `"corner"` (the default): the Report and "Your reports" buttons in the
   * page's bottom right corner. `false`: none; the host starts a report
   * with `report()` and opens the list with `showReports()` from its own
   * controls, and shows the count `on("badge")` gives. */
  launcher?: "corner" | false;
  /** Whether this reporter may file reports (default true). False: no
   * Report button, and `report()` does nothing; "Your reports" stays. */
  canFile?: boolean;
  /** The host's part of each capture's context (spec
   * 2026-10-29-feedback-loop §5.1): `release`, `flags` and `app`, or a
   * function giving them, called when the reporter starts a capture (and
   * when they give a verdict), so it describes that moment. A function that
   * throws, or takes longer than `CONTEXT_TIMEOUT_MS`, files the report
   * with `app: {context_error}` in their place. */
  context?: Given<HostContext>;
  /** JSON the deployer attaches to each report, or a function giving it,
   * called once per report when its capture starts. One that throws or
   * takes longer than `CONTEXT_TIMEOUT_MS` files the report without. */
  metadata?: Given<unknown>;
  /** Whether "Include recent errors" starts ticked (default false). The
   * page's recent errors leave it only when the reporter keeps it ticked. */
  diagnostics?: { default?: boolean };
  /** The page's artifact. Default: `<meta name="clax-artifact">`'s content,
   * else none, and the instance files the report on the page URL without its route. */
  externalId?: string;
  /** The nonce the page's Content-Security-Policy gives its own inline
   * styles, on a page whose `style-src` refuses 'unsafe-inline' (or any
   * strict string, such as `"strict"`, where it uses hashes). The overlay
   * styles itself without inline style; given this, its page renders
   * (clips) are drawn without the page's web fonts from the first, as reading
   * them takes an inline <style> such a policy refuses, so the overlay trips
   * no violation. A policy in a `<meta http-equiv>` is read without it. */
  nonce?: string;
}

/** `given`'s value, or why it could not be had: it threw, rejected, or
 * gave nothing within `timeout` ms. */
export async function evaluate<T>(given: Given<T> | undefined, timeout: number): Promise<{ ok: true; value: T | undefined } | { ok: false; error: string }> {
  if (typeof given !== "function") return { ok: true, value: given };
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    const late = new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error(`no answer within ${timeout} ms`)), timeout); });
    const value = await Promise.race([Promise.resolve().then(() => (given as () => T | Promise<T>)()), late]);
    return { ok: true, value };
  } catch (e) {
    return { ok: false, error: wellFormed(e instanceof Error && e.message ? e.message : String(e)) };
  } finally {
    clearTimeout(timer);
  }
}

const RELEASE_KEYS = ["id", "version", "commit", "channel"] as const;

/** A release's string fields; none when it has none. */
function releaseOf(r: unknown): Release | undefined {
  if (r === null || typeof r !== "object") return undefined;
  const out: Release = {};
  for (const k of RELEASE_KEYS) {
    const v = (r as Record<string, unknown>)[k];
    if (typeof v === "string") out[k] = wellFormed(v);
  }
  return Object.keys(out).length ? out : undefined;
}

/** The flags whose values are booleans, strings or finite numbers. */
function flagsOf(f: unknown): Record<string, FlagValue> | undefined {
  if (f === null || typeof f !== "object" || Array.isArray(f)) return undefined;
  const out: Record<string, FlagValue> = {};
  for (const [k, v] of Object.entries(f)) {
    if (typeof v === "boolean" || (typeof v === "number" && Number.isFinite(v))) out[wellFormed(k)] = v;
    else if (typeof v === "string") out[wellFormed(k)] = wellFormed(v);
  }
  return out;
}

/** `v` as JSON data with every string scrubbed, as Clax scrubs `app` again
 * on receipt, so the preview shows what is kept. */
function scrubbed(v: unknown, depth = 0): unknown {
  if (depth > 32) return null;
  if (typeof v === "string") return scrub(wellFormed(v));
  if (Array.isArray(v)) return v.map(x => scrubbed(x, depth + 1));
  if (v !== null && typeof v === "object") {
    const toJSON = (v as { toJSON?: unknown }).toJSON;
    if (typeof toJSON === "function") return scrubbed(toJSON.call(v), depth + 1);
    return Object.fromEntries(Object.entries(v).filter(([, x]) => x !== undefined && typeof x !== "function" && typeof x !== "symbol").map(([k, x]) => [wellFormed(k), scrubbed(x, depth + 1)]));
  }
  if (typeof v === "number") return Number.isFinite(v) ? v : null;
  return typeof v === "boolean" || v === null ? v : null;
}

const sizeOf = (ctx: ReportContext) => new TextEncoder().encode(contextJson(ctx)).length;

/** `ctx` within `MAX_CONTEXT_BYTES`, which Clax refuses past (and the
 * report with it): past it, the host's `app` is replaced by a
 * `context_error`, then the flags are left out, then what the render left
 * out, then all of the environment but the viewport. */
export function fitContext(ctx: ReportContext): ReportContext {
  if (sizeOf(ctx) <= MAX_CONTEXT_BYTES) return ctx;
  const c: ReportContext = { ...ctx };
  if (c.app !== undefined) c.app = { context_error: `the context was larger than ${MAX_CONTEXT_BYTES} bytes, so app was left out` };
  if (sizeOf(c) > MAX_CONTEXT_BYTES) delete c.flags;
  if (sizeOf(c) > MAX_CONTEXT_BYTES && c.capture) c.capture = { mode: c.capture.mode, method: c.capture.method };
  if (sizeOf(c) > MAX_CONTEXT_BYTES && c.environment) c.environment = { viewport: c.environment.viewport };
  return c;
}

/** A capture's context (spec 2026-10-29-feedback-loop §5.1): `v`, the time
 * `now` gives, the host's `release`, `flags` and `app` (`opts.context`,
 * evaluated now), the window's environment, and `info` (none for a verdict
 * given without a capture). A host `context` that throws or gives nothing
 * within `timeout` ms gives `app: {context_error}` and never stops the
 * report. The result is within `MAX_CONTEXT_BYTES` (`fitContext`). */
export async function buildContext(opts: { context?: Given<HostContext> }, win: Window, info: CaptureInfo | null, now: () => Date, timeout = CONTEXT_TIMEOUT_MS): Promise<ReportContext> {
  const ctx: ReportContext = { v: 1, captured_at: now().toISOString() };
  const env = environment(win);
  const host = await evaluate(opts.context, timeout);
  let app: unknown;
  if (!host.ok) {
    app = { context_error: host.error };
  } else if (host.value !== null && typeof host.value === "object") {
    const h = host.value;
    const release = releaseOf(h.release);
    if (release) ctx.release = release;
    const flags = flagsOf(h.flags);
    if (flags) ctx.flags = flags;
    if (h.app !== undefined) app = scrubbed(h.app);
  }
  ctx.environment = env;
  if (info) ctx.capture = info;
  if (app !== undefined) ctx.app = app;
  return fitContext(ctx);
}

/** `ctx` with its capture `info`. */
export function withCapture(ctx: ReportContext, info: CaptureInfo): ReportContext {
  return fitContext({ ...ctx, capture: info });
}

/** What a capture takes as it starts: its context and metadata, being
 * evaluated, and the page's recent errors as they are now. */
export interface Begun {
  context: Promise<ReportContext>;
  metadata: Promise<unknown>;
  diagnostics: DiagLine[];
}

/** What the overlay evaluates per capture. */
export interface SessionOptions {
  context?: Given<HostContext>;
  metadata?: Given<unknown>;
  diagnostics?: { default?: boolean };
  /** The page's diagnostics recorder (`installDiagnostics`); without one
   * there are no recent errors. */
  recorder?: Recorder | null;
  /** Default: the current time. */
  now?: () => Date;
  /** Default: `CONTEXT_TIMEOUT_MS`. */
  timeout?: number;
}

/** The overlay's per-capture evaluation: nothing is asked of the host
 * until a capture starts (`begin`) or a verdict is given (`verdictContext`). */
export class Session {
  private readonly now: () => Date;
  private readonly timeout: number;

  constructor(private readonly opts: SessionOptions, private readonly win: Window) {
    this.now = opts.now ?? (() => new Date());
    this.timeout = opts.timeout ?? CONTEXT_TIMEOUT_MS;
  }

  /** Whether "Include recent errors" starts ticked. */
  get diagnosticsDefault(): boolean {
    return this.opts.diagnostics?.default ?? false;
  }

  /** Starts a capture `info` describes (none before there is one): the
   * host's context and metadata are asked for now, and the recent errors
   * read now. */
  begin(info: CaptureInfo | null): Begun {
    const context = buildContext(this.opts, this.win, info, this.now, this.timeout);
    const metadata = evaluate(this.opts.metadata, this.timeout).then(m => (m.ok ? m.value : undefined));
    return { context, metadata, diagnostics: this.opts.recorder?.lines() ?? [] };
  }

  /** The context of a verdict given now, without a capture. */
  verdictContext(): Promise<ReportContext> {
    return buildContext(this.opts, this.win, null, this.now, this.timeout);
  }
}

/** A draft's screenshot: being taken, taken, not taken (why), or none asked for. */
export type Clip =
  | { state: "taking" }
  | { state: "taken"; png: Blob }
  | { state: "failed"; error: string }
  | { state: "none" };

/** What a draft is for: a new report, or saying report `report` is still broken. */
export type Purpose = { kind: "new" } | { kind: "reopen"; report: Report };

/** The report being written: what it is for, where it points (none for a
 * Screen capture or a reopen), its capture, the reporter's marks on it, its
 * text, its attachments and the context they carry. */
export interface Draft {
  purpose: Purpose;
  anchor: Anchor | null;
  clip: Clip;
  /** The capture the clip came from, and how it was made. */
  capture: Captured | null;
  /** Marks on the clip, in its pixels, drawn into it when it is sent. */
  shapes: Shape[];
  /** What became of the marks when the clip was replaced, shown by Send. */
  marksNote: string | null;
  body: string;
  /** "Include screenshot": the clip is sent (default on). */
  includeClip: boolean;
  /** "Include recent errors (N)": `diagnostics` is sent. */
  includeDiagnostics: boolean;
  /** The page's recent errors when the capture started. */
  diagnostics: DiagLine[];
  /** The capture's context, once evaluated. */
  context: ReportContext | null;
  /** The deployer's metadata for this report, once evaluated. */
  metadata: unknown;
  /** A send is under way: the composer takes no second one. */
  sending: boolean;
  /** Why the last send failed; the text stays for another try. */
  error: string | null;
}

/** A new draft at `anchor` (none for the whole page), for `purpose`: a new
 * report's clip is being taken; a reopen has none until one is captured. */
export function newDraft(anchor: Anchor | null, purpose: Purpose = { kind: "new" }, includeDiagnostics = false): Draft {
  return {
    purpose, anchor, clip: purpose.kind === "new" ? { state: "taking" } : { state: "none" }, capture: null, shapes: [], marksNote: null, body: "",
    includeClip: true, includeDiagnostics, diagnostics: [], context: null, metadata: undefined, sending: false, error: null,
  };
}

/** The anchor of a Screen capture: the whole page. */
export const PAGE_ANCHOR: Anchor = { kind: "artifact", selector: null, quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "" };

/** A capture's PNG as a clip, or why there is none. */
export function clipOf(c: Captured): Clip {
  if (c.png && c.png.size !== 0) return { state: "taken", png: c.png };
  return { state: "failed", error: c.png ? "the screenshot came out empty" : c.error ?? "the screenshot could not be taken" };
}

/** The marks a capture starts with: an Element pick's outline, as a box
 * the reporter may remove. */
export function defaultShapes(c: Captured): Shape[] {
  if (!c.pick || (c.info.mode !== "element" && c.info.mode !== "range")) return [];
  const { x, y, w, h } = c.pick;
  return [{ kind: "box", from: { x, y }, to: { x: x + w, y: y + h } }];
}

/** Where a capture's image shows the page, and its size, when it has one. */
function placementOf(c: Captured): Placement | null {
  if (!c.png || !c.at) return null;
  // The decoded PNG's own size where its header gives it: the bounds marks are kept within.
  return { at: c.at, scale: c.scale, w: c.size?.w ?? c.at.w * c.scale, h: c.size?.h ?? c.at.h * c.scale };
}

/** The reporter's `shapes` on capture `from`, moved onto capture `to` (the
 * exact screenshot that replaces it), so each marks the same part of the
 * page, and what to tell the reporter: that they were moved, or how many
 * could not be and were removed. */
export function carryShapes(shapes: readonly Shape[], from: Captured, to: Captured): { shapes: Shape[]; note: string | null } {
  if (shapes.length === 0) return { shapes: [], note: null };
  const a = placementOf(from), b = placementOf(to);
  const moved = a && b ? remapShapes(shapes, a, b) : { shapes: [], dropped: shapes.length };
  if (moved.dropped === 0) return { shapes: moved.shapes, note: "Your marks were moved onto the new screenshot. Check them before you send." };
  const n = moved.dropped === 1 ? "1 of your marks" : `${moved.dropped} of your marks`;
  return { shapes: moved.shapes, note: `${n} fell outside the new screenshot and ${moved.dropped === 1 ? "was" : "were"} removed. Check it before you send.` };
}

/** Whether the draft can be sent now: it has its note and no send is under
 * way. A clip still being taken does not hold Send: the send waits for it. */
export function canSend(d: Draft): boolean {
  return !d.sending && d.body.trim() !== "";
}

/** What the composer says about the clip. */
export function clipStatus(c: Clip): string | null {
  if (c.state === "taking") return "Taking the screenshot…";
  if (c.state === "failed") return `No screenshot: ${c.error}. The report is sent without one.`;
  return null;
}

/** The recent errors a send of `d` carries: only when ticked, and some. */
export function diagnosticsSent(d: Draft): DiagLine[] | undefined {
  return d.includeDiagnostics && d.diagnostics.length > 0 ? d.diagnostics : undefined;
}

/** The clip a send of `d` carries: `flattened` (the clip with its marks
 * drawn in), else the clip; only when taken and "Include screenshot" is on. */
export function clipSent(d: Draft, flattened?: Blob): Blob | undefined {
  if (!d.includeClip || d.clip.state !== "taken") return undefined;
  return flattened ?? d.clip.png;
}

/** What the composer quotes of the pick: the quoted text (cut at 160
 * characters), else the element's name, else its selector. */
export function quoteOf(a: Anchor): string {
  const q = (a.label ?? a.quote)?.replace(/\s+/g, " ").trim();
  if (q) return `«${q.length > 160 ? `${q.slice(0, 160)}…` : q}»`;
  if (a.name) return a.name;
  if (a.kind === "area") return "An area of the page";
  if (a.kind === "artifact") return "What the page shows";
  return a.selector ?? "The page";
}

/** The page's artifact: `given`, else `<meta name="clax-artifact">`'s
 * content, else none. */
export function externalIdOf(doc: Document, given?: string): string | undefined {
  if (given !== undefined && given !== "") return given;
  const meta = doc.querySelector<HTMLMetaElement>(`meta[name="${ARTIFACT_META}"]`)?.content.trim();
  return meta ? meta : undefined;
}

/** The URL a report from `doc` carries: the page's, with its query and
 * without its fragment. */
export function pageUrl(doc: Document): string {
  const u = new URL(doc.defaultView?.location.href ?? doc.URL);
  u.hash = "";
  return u.href;
}

/** What sending `d` files, on the page at `url` titled `title`: its context
 * and metadata; the clip (`flattened` when it has marks) only when taken
 * and included; the recent errors only when ticked. Every text is
 * well-formed (`wellFormed`). */
export function reportOf(d: Draft, page: { url: string; title: string }, opts: { externalId?: string }, flattened?: Blob): NewReport {
  const url = wellFormed(page.url);
  const r: NewReport = { url, title: wellFormed(page.title).slice(0, MAX_TITLE) || url, anchor: d.anchor ?? PAGE_ANCHOR, body: wellFormed(d.body.trim()) };
  if (opts.externalId !== undefined) r.externalId = wellFormed(opts.externalId);
  if (d.metadata !== undefined) r.metadata = d.metadata;
  const clip = clipSent(d, flattened);
  if (clip) r.clip = clip;
  if (d.context) r.context = d.context;
  const diagnostics = diagnosticsSent(d);
  if (diagnostics) r.diagnostics = diagnostics;
  return r;
}

/** What sending a "Still broken" draft sends: its note, and its capture's
 * clip, context and recent errors as `reportOf` sends them. */
export function reopenOf(d: Draft, flattened?: Blob): Reopening {
  const r: Reopening = { body: wellFormed(d.body.trim()) };
  const clip = clipSent(d, flattened);
  if (clip) r.clip = clip;
  if (d.context) r.context = d.context;
  const diagnostics = diagnosticsSent(d);
  if (diagnostics) r.diagnostics = diagnostics;
  return r;
}

/** The context JSON a send carries, as "What's sent" shows it: exactly the
 * JSON sent (`contextJson`), laid out. */
export function contextPreview(ctx: ReportContext): string {
  return JSON.stringify(JSON.parse(contextJson(ctx)), null, 2);
}

/** A failure as the reporter reads it. */
export function failureText(e: unknown): string {
  const code = (e as { code?: unknown } | null)?.code;
  if (code === "network") return "Clax could not be reached. Try again.";
  if (code === "wrong_origin") return "This page may not report into this queue.";
  if (code === "not_a_member") return "The queue was not found.";
  if (code === "not_fix_proposed") return "No fix is waiting for your check any more.";
  if (code === "unsupported_browser") return "This browser can't send reports (it lacks Ed25519 signing). Use a current Chrome, Firefox or Safari.";
  const m = e instanceof Error ? e.message : String(e);
  return m || "Something went wrong. Try again.";
}

/** The replies on a report: every comment after the report's own first one. */
export function repliesOf(r: Report): ReportComment[] {
  return r.comments.slice(1);
}

/** What "Your reports" shows a report by: its first comment's text, cut. */
export function summaryOf(r: Report): string {
  const first = r.comments[0]?.body.replace(/\s+/g, " ").trim() ?? "";
  return first.length > 120 ? `${first.slice(0, 120)}…` : first || r.artifact.title;
}

const RESOLVED: Record<ResolveReason, string> = {
  fixed: "Resolved: fixed",
  duplicate: "Resolved: duplicate",
  wontfix: "Resolved: won't fix",
  cannot_reproduce: "Resolved: couldn't reproduce",
  other: "Resolved",
};

/** A report's state as the reporter reads it. */
export function stateLabel(r: Pick<Report, "state" | "resolve_reason">): string {
  switch (r.state) {
    case "open": return "Open";
    case "claimed": return "Being worked on";
    case "fix_proposed": return "Fix ready for you to check";
    case "verified": return "Fixed: you confirmed it";
    case "resolved": return r.resolve_reason ? RESOLVED[r.resolve_reason] : "Resolved";
  }
}

/** A fix attempt's outcome as the reporter reads it. */
export const OUTCOME: Record<FixOutcome, string> = {
  pending: "Waiting for your check",
  verified: "You confirmed it works",
  rejected: "You said it was still broken",
  superseded: "Replaced by a later fix",
  closed: "Closed before it was checked",
};

/** The fix waiting for the reporter's verdict: the latest attempt of a
 * `fix_proposed` report; none otherwise. */
export function waitingFix(r: Report): FixAttempt | null {
  if (r.state !== "fix_proposed") return null;
  const last = r.fixes?.at(-1);
  return last && last.outcome === "pending" ? last : null;
}

/** A link a report shows, when it is an `https:` or `http:` URL; none for
 * any other scheme (a `javascript:` URL is never made a link). */
export function safeLink(url: string | undefined): string | null {
  if (!url) return null;
  try {
    const u = new URL(url);
    return u.protocol === "https:" || u.protocol === "http:" ? u.href : null;
  } catch {
    return null;
  }
}

/** Timers the polls run on; injected in tests. */
export interface Timers {
  set(fn: () => void, ms: number): unknown;
  clear(handle: unknown): void;
}
export const realTimers: Timers = {
  set: (fn, ms) => setTimeout(fn, ms),
  clear: h => clearTimeout(h as ReturnType<typeof setTimeout>),
};

/** Calls `load` at once and then `ms` after each call settles, until
 * `stop`: never two at once, and none after `stop`. A failed load is
 * `onError`'s and the poll keeps going. */
export class Poll {
  private timer: unknown = null;
  private live = false;

  constructor(private readonly load: () => Promise<void>, private readonly timers: Timers = realTimers, private readonly ms = POLL_MS, private readonly onError: (e: unknown) => void = () => {}) {}

  start(): void {
    if (this.live) return;
    this.live = true;
    void this.tick();
  }

  /** Loads now (a reply was just sent), and restarts the wait. */
  now(): void {
    if (!this.live) return;
    this.timers.clear(this.timer);
    this.timer = null;
    void this.tick();
  }

  stop(): void {
    this.live = false;
    this.timers.clear(this.timer);
    this.timer = null;
  }

  private running = false;
  private again = false;

  private async tick(): Promise<void> {
    if (this.running) { this.again = true; return; }
    this.running = true;
    try {
      await this.load();
    } catch (e) {
      if (this.live) this.onError(e);
    } finally {
      this.running = false;
    }
    if (!this.live) return;
    if (this.again) { this.again = false; void this.tick(); return; }
    this.timer = this.timers.set(() => { this.timer = null; void this.tick(); }, this.ms);
  }
}

/** What the badge poll watches: whether the page is visible, and its
 * `focus` and `visibilitychange`. */
export interface Visibility {
  document: { visibilityState: DocumentVisibilityState; addEventListener(t: "visibilitychange", fn: () => void): void; removeEventListener(t: "visibilitychange", fn: () => void): void };
  addEventListener(t: "focus", fn: () => void): void;
  removeEventListener(t: "focus", fn: () => void): void;
}

/** What the badge poll tells: the count when it changed (the first count
 * too), and `rose` with the reports newly waiting when it rose since the
 * last count. */
export interface BadgeHooks {
  changed(n: number): void;
  rose(newly: string[]): void;
}

/** The badge, asked for every `BADGE_POLL_MS` while the page is visible,
 * at once when it becomes visible or gets focus, and on `now`; a failed ask
 * is dropped and the next one tried. */
export class BadgeWatch {
  private last: Badge | null = null;
  private readonly poll: Poll;
  private live = false;

  constructor(load: () => Promise<Badge>, private readonly win: Visibility, private readonly hooks: BadgeHooks, timers: Timers = realTimers, ms = BADGE_POLL_MS) {
    this.poll = new Poll(async () => {
      const b = await load();
      if (!this.live) return;
      this.seen(b);
      if (this.win.document.visibilityState !== "visible") this.poll.stop();
    }, timers, ms);
  }

  /** The last count, or null before the first. */
  get count(): number | null {
    return this.last?.waiting ?? null;
  }

  start(): void {
    if (this.live) return;
    this.live = true;
    this.win.document.addEventListener("visibilitychange", this.wake);
    this.win.addEventListener("focus", this.wake);
    if (this.win.document.visibilityState === "visible") this.poll.start();
  }

  /** Asks now (the reporter just read or gave a verdict on a report). */
  now(): void {
    if (!this.live) return;
    this.poll.stop();
    this.poll.start();
  }

  stop(): void {
    this.live = false;
    this.poll.stop();
    this.win.document.removeEventListener("visibilitychange", this.wake);
    this.win.removeEventListener("focus", this.wake);
  }

  private readonly wake = () => {
    if (this.win.document.visibilityState === "visible") this.now();
    else this.poll.stop();
  };

  private seen(b: Badge): void {
    const before = this.last;
    this.last = b;
    if (before?.waiting === b.waiting) return;
    this.hooks.changed(b.waiting);
    if (before && b.waiting > before.waiting) this.hooks.rose(b.report_ids.filter(id => !before.report_ids.includes(id)));
  }
}

/** What the overlay calls of the SDK's `ReporterClient`. */
export interface ReporterApi {
  fileReport(r: NewReport): Promise<Report>;
  myReports(cursor?: string): Promise<ReportPage>;
  comment(reportId: string, body: string): Promise<Report>;
  verify(reportId: string, v?: Verification): Promise<Report>;
  reopen(reportId: string, r: Reopening): Promise<Report>;
  badge(): Promise<Badge>;
  seen(reportId: string): Promise<void>;
}
