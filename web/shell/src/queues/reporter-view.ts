// What a reporter's page in Clax (/reporter/<qid>[/<tid>]) reads of a
// report, apart from the DOM. (The goal/clax-reports-page branch took these
// from web/overlay/src/triage, which this lineage does not have: its owner
// pages are web/shell/src/queues, whose state names this reuses.)
import type { Anchor } from "../../../bridge/src/protocol";
import type { Report, ReportComment, ReportPage, ReportState } from "../../../sdk/src/types";
import { STATE_LABEL } from "./model";

/** A report state's words. */
export const STATE_TEXT: Record<ReportState, string> = STATE_LABEL;

/** How many pages of a queue's reports a page reads at most. */
export const MAX_PAGES = 10;

/** A report's title: its note's first line, at most 80 characters. */
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

/** The address of the page a report was filed on, its route kept. */
export function pageOf(r: Report): string | null {
  const a = asAnchor(r.anchor);
  const u = r.artifact.url;
  if (!/^https?:\/\//i.test(u)) return null;
  return typeof a?.route === "string" && a.route ? `${u.split(/[?#]/)[0]}${a.route}` : u;
}

/** Every page of a queue's reports, at most MAX_PAGES; `more` when some were left unread. */
export async function readAll(api: { reports(qid: string, cursor: string | null): Promise<ReportPage> }, qid: string): Promise<{ reports: Report[]; more: boolean }> {
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
