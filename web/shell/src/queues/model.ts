// The Queues page's rules, apart from its views: a queue's origins as typed
// one per line, its reports counted by state, the page's addresses, how a
// report's reporter, context and anchor read, and a refusal as a sentence.
import type { DiagLine, FixOutcome, Report, ReportContext, ReportPage, ReportState, ResolveReason } from "../../../sdk/src/types";
import type { StateFilter } from "./api";

/** The origins typed in `text`, one per line: each as its origin
 * (`https://app.example`, no trailing slash), the first of each kept;
 * `bad` holds the lines that are not an http or https origin (a path, a
 * query or another scheme). Blank lines are skipped. */
export function parseOrigins(text: string): { origins: string[]; bad: string[] } {
  const origins: string[] = [];
  const bad: string[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line) continue;
    const o = originOf(line);
    if (!o) bad.push(line);
    else if (!origins.includes(o)) origins.push(o);
  }
  return { origins, bad };
}

function originOf(s: string): string | null {
  let u: URL;
  try { u = new URL(s); } catch { return null; }
  if (u.protocol !== "https:" && u.protocol !== "http:") return null;
  if (u.username || u.password || u.pathname !== "/" || u.search || u.hash || s.endsWith("?") || s.endsWith("#")) return null;
  return u.origin;
}

/** A queue's reports counted by state; `more` when the pages ran past the limit. */
export type Counts = Record<ReportState, number> & { more: boolean };

/** Counts the reports `page` gives, following each page's `next` for at most `limit` pages. */
export async function countReports(page: (cursor: string | null) => Promise<ReportPage>, limit = 20): Promise<Counts> {
  const c: Counts = { open: 0, claimed: 0, fix_proposed: 0, verified: 0, resolved: 0, more: false };
  let cursor: string | null = null;
  for (let i = 0; i < limit; i++) {
    const p = await page(cursor);
    for (const r of p.reports) if (r.state in c) c[r.state]++;
    cursor = p.next;
    if (!cursor) return c;
  }
  c.more = true;
  return c;
}

/** `Counts` as a line, every state in the order a report moves through
 * them: "3 open · 1 claimed · 2 fix proposed · 4 verified · 9 resolved",
 * a `+` after each when there are more. */
export function countsText(c: Counts): string {
  const plus = c.more ? "+" : "";
  return STATES.map(s => `${c[s]}${plus} ${STATE_LABEL[s].toLowerCase()}`).join(" · ");
}

/** Every state, in the order a report moves through them. */
export const STATES: readonly ReportState[] = ["open", "claimed", "fix_proposed", "verified", "resolved"];
/** A state as the views name it. */
export const STATE_LABEL: Record<ReportState, string> = {
  open: "Open", claimed: "Claimed", fix_proposed: "Fix proposed", verified: "Verified", resolved: "Resolved",
};
/** Why a report was closed, as the views say it. */
export const REASON_LABEL: Record<ResolveReason, string> = {
  fixed: "Fixed", duplicate: "Duplicate", wontfix: "Won't fix", cannot_reproduce: "Can't reproduce", other: "Other",
};
/** What became of a fix, as the views say it. */
export const OUTCOME_LABEL: Record<FixOutcome, string> = {
  pending: "Waiting for the reporter", verified: "Verified", rejected: "Still broken", superseded: "Superseded", closed: "Closed",
};

/** The queue view's tabs: the filter each lists by (`null`: every state). */
export const TABS: readonly { key: string; label: string; state: StateFilter | null }[] = [
  { key: "active", label: "Active", state: "active" },
  { key: "fix_proposed", label: "Fix proposed", state: "fix_proposed" },
  { key: "verified", label: "Verified", state: "verified" },
  { key: "resolved", label: "Resolved", state: "resolved" },
  { key: "all", label: "All", state: null },
];

/** Which view of the Queues page `pathname` is: the list (`/queues`), a
 * queue's reports (`/queues/<qid>`) or one report (`/queues/<qid>/<tid>`). */
export type QueuesRoute = { view: "list" } | { view: "queue"; qid: string } | { view: "report"; qid: string; tid: string };
export function queuesRoute(pathname: string): QueuesRoute {
  const m = /^\/queues\/([^/]+)(?:\/([^/]+))?\/?$/.exec(pathname);
  if (!m) return { view: "list" };
  const d = (s: string) => { try { return decodeURIComponent(s); } catch { return s; } };
  return m[2] ? { view: "report", qid: d(m[1]), tid: d(m[2]) } : { view: "queue", qid: d(m[1]) };
}
/** The queue view's address, and a report's. */
export const queueHref = (qid: string) => `/queues/${encodeURIComponent(qid)}`;
export const reportHref = (qid: string, tid: string) => `${queueHref(qid)}/${encodeURIComponent(tid)}`;

/** Who filed `r`, as triage knows them: the profile's name or GitHub
 * login, else the reporter ID, else "Anonymous". */
export function reporterName(r: Pick<Report, "reporter_profile" | "reporter_id">): string {
  const p = r.reporter_profile;
  return p?.name?.trim() || (p?.github ? `@${p.github}` : "") || r.reporter_id || "Anonymous";
}
/** The note `r` was filed with: its first comment. */
export const noteOf = (r: Pick<Report, "comments">): string => r.comments[0]?.body ?? "";

/** `s` as a link target when it is an http or https URL, else `null`. */
export function httpUrl(s: string | null | undefined): string | null {
  if (!s) return null;
  try {
    const u = new URL(s);
    return u.protocol === "https:" || u.protocol === "http:" ? u.href : null;
  } catch { return null; }
}

/** The repository `context.app.repo` names: an http(s) repository URL, or
 * `owner/name`, read as GitHub's. */
export function repoUrl(app: unknown): string | null {
  const repo = app && typeof app === "object" ? (app as { repo?: unknown }).repo : undefined;
  if (typeof repo !== "string") return null;
  const s = repo.trim();
  if (/^[\w.-]+\/[\w.-]+$/.test(s)) return `https://github.com/${s}`;
  const u = httpUrl(s);
  return u ? u.replace(/\/+$/, "").replace(/\.git$/, "") : null;
}

/** The release's commit as text, with a link to it when the context's
 * `app.repo` names a repository; `null` when the release names no commit. */
export function commitLink(ctx: ReportContext | undefined): { text: string; href: string | null } | null {
  const c = ctx?.release?.commit?.trim();
  if (!c) return null;
  const repo = repoUrl(ctx?.app);
  const safe = /^[0-9A-Za-z._-]+$/.test(c);
  return { text: c.length > 12 && /^[0-9a-f]+$/i.test(c) ? c.slice(0, 12) : c, href: repo && safe ? `${repo}/commit/${c}` : null };
}

/** A browser and its major version, read from a user agent: "Chrome 131",
 * "Safari 18", "Firefox 132", "Edge 131"; the agent itself when none is known. */
export function browserOf(ua: string | undefined): string | null {
  if (!ua) return null;
  const v = (re: RegExp) => re.exec(ua)?.[1];
  const known: [string, RegExp][] = [["Edge", /Edg(?:e|A|iOS)?\/(\d+)/], ["Firefox", /(?:Firefox|FxiOS)\/(\d+)/], ["Chrome", /(?:Chrome|CriOS)\/(\d+)/], ["Safari", /Version\/(\d+)[.\d]* .*Safari\//]];
  for (const [name, re] of known) { const n = v(re); if (n) return `${name} ${n}`; }
  return ua;
}

/** `iso` as a date and time in `zone` (the reader's when absent), with the
 * zone's short name; `null` for a zone the browser does not know or a time
 * it cannot read. */
export function timeIn(iso: string, zone?: string, locale?: string): string | null {
  const t = new Date(iso);
  if (Number.isNaN(t.getTime())) return null;
  try {
    return new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short", timeZone: zone }).format(t)
      + ` ${zoneName(t, zone, locale)}`;
  } catch { return null; }
}
function zoneName(t: Date, zone: string | undefined, locale: string | undefined): string {
  return new Intl.DateTimeFormat(locale, { timeZone: zone, timeZoneName: "short" }).formatToParts(t).find(p => p.type === "timeZoneName")?.value ?? zone ?? "";
}

/** What a report's anchor points at, as a line: the quote, the element's
 * name, the custom name or the selector, after the kind. */
export function anchorText(anchor: unknown): string | null {
  if (!anchor || typeof anchor !== "object") return null;
  const a = anchor as { kind?: string; quote?: string | null; label?: string; name?: string; custom_name?: string | null; selector?: string | null };
  const kind = { element: "Element", range: "Text", area: "Area", custom: "Custom", artifact: "The whole page" }[a.kind ?? ""] ?? a.kind ?? "Anchor";
  const what = a.label || (a.quote ? `“${a.quote}”` : "") || a.name || a.custom_name || a.selector || "";
  return what ? `${kind}: ${what}` : kind;
}

/** A diagnostics line's levels, most severe first. */
export const LEVELS: readonly DiagLine["level"][] = ["error", "warn", "info", "debug"];

/** What to show for a refusal `e`: the daemon's message (lower case, no
 * full stop) as a sentence. */
export function refusal(e: unknown): string {
  const s = (e instanceof Error ? e.message : String(e)).trim();
  if (!s) return "Something went wrong. Try again.";
  return `${s[0].toUpperCase()}${s.slice(1)}${/[.!?]$/.test(s) ? "" : "."}`;
}
