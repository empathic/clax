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

/** `Clax.overlay`'s options. */
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

/** What sending `d` files, on the page at `url` titled `title`: the clip
 * only when it was taken. */
export function reportOf(d: Draft, page: { url: string; title: string }, opts: { externalId?: string; metadata?: unknown }): NewReport {
  const r: NewReport = { url: page.url, title: page.title.slice(0, MAX_TITLE) || page.url, anchor: d.anchor, body: d.body.trim() };
  if (opts.externalId !== undefined) r.externalId = opts.externalId;
  if (opts.metadata !== undefined) r.metadata = opts.metadata;
  if (d.clip.state === "taken") r.clip = d.clip.png;
  return r;
}

/** A failure as the reporter reads it. */
export function failureText(e: unknown): string {
  const code = (e as { code?: unknown } | null)?.code;
  if (code === "network") return "Clax could not be reached. Try again.";
  if (code === "wrong_origin") return "This page may not report into this queue.";
  if (code === "not_a_member") return "The queue was not found.";
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
