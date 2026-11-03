// Report triage, apart from the DOM: what a queue's page and a report's page
// (Clax's /queues/<qid> and /queues/<qid>/<tid>, and the Clax DevTools
// panel's Queues view) read and do, over one interface each host supplies:
// the shell with the browser's own session, the DevTools panel through the
// extension's worker. Written from the queue routes' contract
// (docs/contract.md; the routes in crates/clax-server/src/routes/queue_agent.rs)
// and web/sdk's types, under web/overlay's license (LGPL-2.1-only), so the
// AGPL shell and the GPL-2 extension can both use it.
//
// A person replies and resolves; a claim is an agent's alone (the claim
// route refuses a person, `no_session`), so a claim is shown, never taken.
import type { Anchor } from "../../../bridge/src/protocol";
import type { Queue, Report, ReportComment, ReportPage, ReportState } from "../../../sdk/src/types";

export type { Queue, Report, ReportComment, ReportState };

/** What a host gives the triage views. Every method rejects with an `Error` whose message can be shown. */
export interface TriageApi {
  queues(): Promise<Queue[]>;
  /** One page of the queue's reports, every state, oldest first, from `cursor`. */
  reports(qid: string, cursor: string | null): Promise<ReportPage>;
  report(qid: string, tid: string): Promise<Report>;
  /** The report's screenshot as a URL an `<img>` can show (an object or data URL); null when it has none. */
  clip(qid: string, tid: string): Promise<string | null>;
  reply(qid: string, tid: string, body: string): Promise<Report>;
  resolve(qid: string, tid: string): Promise<Report>;
  /** Hears that some queue's reports may have changed (the `queues` stream ping, or the stream coming back), until the result runs. */
  watch(changed: () => void): () => void;
}

/** How many pages of a queue's reports a view reads at most (the rest behind "Show older"). */
export const MAX_PAGES = 10;

export const STATE_TEXT: Record<ReportState, string> = { open: "Open", claimed: "Claimed", fix_proposed: "Fix proposed", verified: "Verified", resolved: "Resolved" };
export const FILTERS: { value: ReportState | "all"; label: string }[] = [
  { value: "all", label: "All" }, { value: "open", label: "Open" }, { value: "claimed", label: "Claimed" }, { value: "fix_proposed", label: "Fix proposed" }, { value: "verified", label: "Verified" }, { value: "resolved", label: "Resolved" },
];

export const queuePath = (qid: string) => `/queues/${encodeURIComponent(qid)}`;
export const reportPath = (qid: string, tid: string) => `${queuePath(qid)}/${encodeURIComponent(tid)}`;

/** `/queues/<qid>[/<tid>]` read back; null for any other path. */
export function routeOf(path: string): { qid: string; tid: string | null } | null {
  const m = /^\/queues\/([^/]+)(?:\/([^/]+))?\/?$/.exec(path);
  if (!m) return null;
  try { return { qid: decodeURIComponent(m[1]), tid: m[2] ? decodeURIComponent(m[2]) : null }; } catch { return null; }
}

/** The queues that take reports from `origin`. */
export const queuesFor = (queues: Queue[], origin: string) => queues.filter(q => q.origins.includes(origin));

/** A report's name: the first line of its first comment, cut at 80 characters. */
export function titleOf(r: Report): string {
  const line = (r.comments[0]?.body ?? "").split("\n").find(l => l.trim())?.trim() ?? "";
  if (!line) return "Report without a note";
  return line.length > 80 ? `${line.slice(0, 79)}…` : line;
}

/** The replies to a report: every comment after the reporter's first. */
export const repliesOf = (r: Report): ReportComment[] => r.comments.slice(1);

const asAnchor = (a: unknown): Partial<Anchor> | null => (a && typeof a === "object" ? (a as Partial<Anchor>) : null);

/** What a report is about, in words: the picked element's name, its quote, or the page. */
export function aboutOf(r: Report): string {
  const a = asAnchor(r.anchor);
  if (!a || a.kind === "artifact") return "The whole page";
  const name = a.label ?? a.custom_name ?? a.quote;
  if (a.kind === "area") return name ? `An area: ${name}` : "An area of the page";
  if (a.kind === "range") return name ? `The text “${name}”` : "Some text";
  return name ? `“${name}”` : a.selector ? `The element ${a.selector}` : "An element";
}

/** The CSS selector of the report's element, when it names one (the panel highlights it in the page). */
export function selectorOf(r: Report): string | null {
  const a = asAnchor(r.anchor);
  return a && a.kind !== "artifact" && typeof a.selector === "string" && a.selector ? a.selector : null;
}

/** What the site filed with a report (its `metadata`, such as `{"build": "1"}`), as text pairs: flat scalars only, at most 20, each cut at 200 characters. */
export function metaOf(r: Report): [string, string][] {
  const m = r.metadata;
  if (!m || typeof m !== "object" || Array.isArray(m)) return [];
  return Object.entries(m as Record<string, unknown>)
    .filter(([, v]) => typeof v === "string" || typeof v === "number" || typeof v === "boolean")
    .slice(0, 20)
    .map(([k, v]) => [k.slice(0, 60), String(v).slice(0, 200)]);
}

/** The address of the page a report was filed on, its route kept: what "Open on the page" opens. */
export function pageOf(r: Report): string | null {
  const a = asAnchor(r.anchor);
  const u = r.artifact.url;
  if (!/^https?:\/\//i.test(u)) return null;
  return typeof a?.route === "string" && a.route ? `${u.split(/[?#]/)[0]}${a.route}` : u;
}

/** Whether `a` and `b` are the same page: origin and path, the query and fragment aside. */
export function samePage(a: string, b: string): boolean {
  try { const x = new URL(a), y = new URL(b); return x.origin === y.origin && x.pathname === y.pathname; } catch { return false; }
}

/** The reports in `state` (all for "all"), newest first, those on `page` (when given) apart and first. */
export function arrange(reports: Report[], state: ReportState | "all", page: string | null): { here: Report[]; rest: Report[] } {
  const shown = reports.filter(r => state === "all" || r.state === state).sort((a, b) => b.created_at.localeCompare(a.created_at));
  if (!page) return { here: [], rest: shown };
  return { here: shown.filter(r => samePage(r.artifact.url, page)), rest: shown.filter(r => !samePage(r.artifact.url, page)) };
}

/** A claim's words: who holds it and until when. */
export const claimText = (r: Report, now = Date.now()) => {
  if (!r.claim) return null;
  const mins = Math.max(0, Math.round((Date.parse(r.claim.expires_at) - now) / 60_000));
  return `Claimed by ${r.claim.holder_name}${Number.isFinite(mins) ? `, ${mins} min left` : ""}`;
};

/** What changed between two reads of a queue, in words ("1 new report. 1 report resolved."); "" when nothing did. */
export function changes(before: Report[], after: Report[]): string {
  const was = new Map(before.map(r => [r.id, r]));
  const count = (n: number, what: string) => (n ? `${n} ${n === 1 ? "report" : "reports"} ${what}.` : "");
  const fresh = after.filter(r => !was.has(r.id)).length;
  const moved = (s: ReportState) => after.filter(r => was.has(r.id) && was.get(r.id)!.state !== s && r.state === s).length;
  const replied = after.filter(r => was.has(r.id) && r.comments.length > was.get(r.id)!.comments.length).length;
  return [
    fresh ? `${fresh} new ${fresh === 1 ? "report" : "reports"}.` : "",
    count(moved("claimed"), "claimed"), count(moved("resolved"), "resolved"), count(moved("open"), "open again"),
    replied ? `New ${replied === 1 ? "reply" : "replies"} on ${replied} ${replied === 1 ? "report" : "reports"}.` : "",
  ].filter(Boolean).join(" ");
}

/** Every page of a queue's reports, at most MAX_PAGES; `more` when some were left unread. */
export async function readAll(api: Pick<TriageApi, "reports">, qid: string): Promise<{ reports: Report[]; more: boolean }> {
  const out: Report[] = [];
  let cursor: string | null = null;
  for (let i = 0; i < MAX_PAGES; i++) {
    const p: ReportPage = await api.reports(qid, cursor);
    out.push(...p.reports);
    cursor = p.next;
    if (!cursor) return { reports: out, more: false };
  }
  return { reports: out, more: true };
}

/** Runs `f` one at a time: a call while one runs makes it run once more after, however many calls came. */
export function serial(f: () => Promise<void>): () => Promise<void> {
  let running: Promise<void> | null = null;
  let again = false;
  return () => {
    if (running) { again = true; return running; }
    running = (async () => {
      try { do { again = false; await f(); } while (again); } finally { running = null; }
    })();
    return running;
  };
}

/** An error's words for the status line. */
export const failureOf = (e: unknown) => (e instanceof Error && e.message ? e.message : "Clax did not answer.");
