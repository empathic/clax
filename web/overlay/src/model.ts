// The overlay's composer and its "Your reports" polling, apart from the DOM:
// a draft's clip as it is taken, what a send files, and a poll that runs only
// while the reports panel is open.
import type { Anchor } from "../../bridge/src/protocol";
import type { NewReport, ReporterAuth } from "../../sdk/src/reporter";
import type { Report, ReportComment, ReportPage } from "../../sdk/src/types";

/** How often "Your reports" asks for new replies while it is open. */
export const POLL_MS = 15_000;
/** The longest title a report carries (the page's title, cut). */
export const MAX_TITLE = 300;
/** The `<meta name>` a page names its artifact's external ID with. */
export const ARTIFACT_META = "clax-artifact";

/** `Clax.overlay`'s options. Every report carries the page's URL with its
 * query and without its fragment, and the page's title. */
export interface OverlayOptions {
  /** The instance's base URL, such as `https://clax.corp.example`. */
  instance: string;
  /** The queue's ID. */
  queue: string;
  /** Gets a reporter token for the reporter's public key, from the deployer's backend. */
  auth: ReporterAuth;
  /** JSON the deployer attaches to every report filed from this page. */
  metadata?: unknown;
  /** The page's artifact. Default: `<meta name="clax-artifact">`'s content,
   * else none, and the instance files the report on the page URL without its route. */
  externalId?: string;
}

/** A draft's screenshot: being taken, taken, or not taken (why). */
export type Clip =
  | { state: "taking" }
  | { state: "taken"; png: Blob }
  | { state: "failed"; error: string };

/** The report being written: where it points, its clip and its text. */
export interface Draft {
  anchor: Anchor;
  clip: Clip;
  body: string;
  /** A send is under way: the composer takes no second one. */
  sending: boolean;
  /** Why the last send failed; the text stays for another try. */
  error: string | null;
}

/** A new draft at `anchor`, its clip being taken. */
export function newDraft(anchor: Anchor): Draft {
  return { anchor, clip: { state: "taking" }, body: "", sending: false, error: null };
}

/** Runs `render` and gives its PNG as a clip, or the reason it failed: a
 * clip that cannot be drawn never stops a report. */
export async function takeClip(render: () => Promise<ArrayBuffer>): Promise<Clip> {
  try {
    const bytes = await render();
    if (bytes.byteLength === 0) return { state: "failed", error: "the screenshot came out empty" };
    return { state: "taken", png: new Blob([bytes], { type: "image/png" }) };
  } catch (e) {
    return { state: "failed", error: e instanceof Error && e.message ? e.message : String(e) };
  }
}

/** Whether the draft can be sent now: it has text and no send is under way.
 * A clip still being taken does not hold Send: the send waits for it. */
export function canSend(d: Draft): boolean {
  return !d.sending && d.body.trim() !== "";
}

/** What the composer says about the clip. */
export function clipStatus(c: Clip): string | null {
  if (c.state === "taking") return "Taking the screenshot…";
  if (c.state === "failed") return `No screenshot: ${c.error}. The report is sent without one.`;
  return null;
}

/** What the composer quotes of the pick: the quoted text (cut at 160
 * characters), else the element's name, else its selector. */
export function quoteOf(a: Anchor): string {
  const q = (a.label ?? a.quote)?.replace(/\s+/g, " ").trim();
  if (q) return `«${q.length > 160 ? `${q.slice(0, 160)}…` : q}»`;
  if (a.name) return a.name;
  if (a.kind === "area") return "An area of the page";
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

/** What sending `d` files, on the page at `url` titled `title`: the clip
 * only when it was taken. */
export function reportOf(d: Draft, page: { url: string; title: string }, opts: { externalId?: string; metadata?: unknown }): NewReport {
  const r: NewReport = { url: page.url, title: page.title.slice(0, MAX_TITLE) || page.url, anchor: d.anchor, body: d.body.trim() };
  if (opts.externalId !== undefined) r.externalId = opts.externalId;
  if (opts.metadata !== undefined) r.metadata = opts.metadata;
  if (d.clip.state === "taken") r.clip = d.clip.png;
  return r;
}

/** The origin of the page `doc`, for naming it in a failure. */
export function pageOrigin(doc: Document): string {
  return new URL(pageUrl(doc)).origin;
}

/** A failure as the reporter reads it; `origin`, the page's, is named
 * when the queue refuses it. */
export function failureText(e: unknown, origin?: string): string {
  const code = (e as { code?: unknown } | null)?.code;
  if (code === "network") return "Clax could not be reached. Try again.";
  if (code === "wrong_origin") {
    return origin ? `This page (${origin}) may not report into this queue.` : "This page may not report into this queue.";
  }
  if (code === "not_a_member") return "The queue was not found.";
  if (code === "unsupported_browser") return "This browser can't send reports (it lacks Ed25519 signing). Use a current Chrome, Firefox or Safari.";
  const m = e instanceof Error ? e.message : String(e);
  return m || "Something went wrong. Try again.";
}

/** A refusal of the overlay's calls by the queue or the instance, as the
 * overlay's notice says it, with what fixes it. */
export interface QueueProblem {
  /** The refusal's code: the notice shows each at most once a page load
   * unless the person's own action meets it again. */
  code: string;
  /** What is wrong, in words, naming the page's origin. */
  text: string;
  /** Who can fix it, and how, when no button here does. */
  fix: string | null;
  /** Trying again may fix it (Clax could not be reached). */
  retry: boolean;
  /** The page's origin may be missing from the queue's origins: a queue
   * admin can add it here. */
  addOrigin: boolean;
}

/** `e`, a failure of a call from the page of `origin`, as a problem of the
 * queue; `null` for a failure that is not one (such as a draft's own). */
export function problemOf(e: unknown, origin: string): QueueProblem | null {
  const code = (e as { code?: unknown } | null)?.code;
  const p = (text: string, fix: string | null, more: Partial<QueueProblem> = {}): QueueProblem =>
    ({ code: String(code), text, fix, retry: false, addOrigin: false, ...more });
  switch (code) {
    case "network":
      return p("Clax could not be reached, so this page can't send reports right now.", null, { retry: true });
    case "wrong_origin":
      // The refusal of a read files a note for the instance's owner.
      return p(`This page (${origin}) may not report into this queue.`, "The owner of this Clax has been told, and can allow this page's origin.", { addOrigin: true });
    case "invalid_token":
    case "token_expired":
    case "invalid_signature":
      return p("Clax refused this page's sign-in, so it can't send reports.", "Whoever runs this site has to fix how its page gets a Clax reporter token.");
    case "not_a_member":
      return p("The queue this page reports into was not found.", "Whoever runs this site has to point it at a queue that exists.");
    case "unsupported_browser":
      return p(failureText(e), null);
    case "check_failed":
      return p(`Clax could not check this page's queue: ${(e as { message?: unknown }).message}`, "Whoever runs this site has to fix how its page signs in to Clax.", { retry: true });
    default:
      return null;
  }
}

/** `origin` as `URL` writes an origin, or `null` when it is none. */
export function originOf(origin: string): string | null {
  try {
    const o = new URL(origin).origin;
    return o === "null" ? null : o;
  } catch {
    return null;
  }
}

/** Whether queue `origins` lists `origin`, both read as `URL` writes them. */
export function listsOrigin(origins: readonly string[], origin: string): boolean {
  const want = originOf(origin);
  return want !== null && origins.some(o => originOf(o) === want);
}

/** `origins` with `origin` added, unless it is listed. */
export function withOrigin(origins: readonly string[], origin: string): string[] {
  return listsOrigin(origins, origin) ? [...origins] : [...origins, originOf(origin) ?? origin];
}

/** A queue as its admin reads it. */
export interface AdminQueue {
  id: string;
  name: string;
  origins: string[];
}

/** The queue's admin routes, called with the viewer's own Clax session
 * (never the reporter token); only on a page of the instance's own origin,
 * where the browser sends that session. */
export interface QueueAdminApi {
  /** The queue, when this viewer is its admin; else `null`. */
  queue(): Promise<AdminQueue | null>;
  /** Sets the queue's origins. */
  setOrigins(origins: string[]): Promise<AdminQueue>;
}

/** The admin API of queue `queue` on `instance`, through `fetch`: `null`
 * unless the page `doc` is of the instance's own origin. */
export function queueAdminApi(instance: string, queue: string, doc: Document, f: typeof fetch = (i, init) => globalThis.fetch(i, init)): QueueAdminApi | null {
  const base = originOf(instance);
  if (base === null || base !== pageOrigin(doc)) return null;
  const root = instance.replace(/\/+$/, "");
  const call = async <T>(path: string, init?: RequestInit): Promise<T> => {
    const res = await f(`${root}${path}`, { credentials: "same-origin", ...init });
    let body: { error?: { code?: string; message?: string } } & T = {} as never;
    try { body = await res.json(); } catch { /* no JSON */ }
    if (!res.ok) throw Object.assign(new Error(body.error?.message || `HTTP ${res.status}`), { code: body.error?.code ?? `http_${res.status}` });
    return body;
  };
  const pick = (q: AdminQueue): AdminQueue => ({ id: q.id, name: q.name, origins: q.origins });
  return {
    async queue() {
      const { queues } = await call<{ queues?: (AdminQueue & { role?: string })[] }>("/api/queues");
      const q = queues?.find(x => x.id === queue && x.role === "admin");
      return q ? pick(q) : null;
    },
    async setOrigins(origins) {
      const { queue: q } = await call<{ queue: AdminQueue }>(`/api/queues/${encodeURIComponent(queue)}`, {
        method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify({ origins }),
      });
      return pick(q);
    },
  };
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

/** Timers the poll runs on; injected in tests. */
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

/** What the overlay calls of the SDK's `ReporterClient`. */
export interface ReporterApi {
  fileReport(r: NewReport): Promise<Report>;
  myReports(cursor?: string): Promise<ReportPage>;
  comment(reportId: string, body: string): Promise<Report>;
}
