// The side panel's view of the tab's site (spec 2026-10-05 §6.4, §7.1;
// owner decision 2026-10-06): the threads of the site's other pages grouped
// by page, newest activity first, with each page's counts; the status
// filter and the search, both run here on the listing; and merge rule
// patterns, checked and matched as the daemon does, for the preview.
import type { Thread } from "../../../shell/src/threads";
import type { PageView, SiteView } from "../messages";

export type Status = "open" | "addressed" | "resolved";
export type Filter = Status | "all";
export const FILTERS: { value: Filter; label: string }[] = [
  { value: "open", label: "Open" }, { value: "addressed", label: "Addressed" }, { value: "resolved", label: "Resolved" }, { value: "all", label: "All" },
];

/** As the daemon counts a page's threads: resolved; addressed (open, and an
 * agent's address is listed or waits for a snapshot); else open. */
export const statusOf = (t: Thread): Status =>
  t.status === "resolved" ? "resolved" : (t.addressed_in?.length || t.addressed_pending) ? "addressed" : "open";

/** The thread's newest creation, comment or resolve. */
export function lastActivity(t: Thread): string {
  let last = t.created_at;
  for (const c of t.comments) if (c.created_at > last) last = c.created_at;
  if (t.resolved_at && t.resolved_at > last) last = t.resolved_at;
  return last;
}

/** Whether `t` passes the filter, and every word of the search is in its
 * comments' text or authors, its quote, or the path it was made at. */
export function matches(t: Thread, filter: Filter, search: string, path = t.page_path ?? ""): boolean {
  if (filter !== "all" && statusOf(t) !== filter) return false;
  const words = search.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return true;
  const hay = [path, t.anchor.quote ?? "", ...t.comments.flatMap(c => [c.body, c.author_name])].join("\n").toLowerCase();
  return words.every(w => hay.includes(w));
}

export type Counts = Record<Status, number>;
export type Group = {
  page: PageView;
  /** What the group is called: its path, or a merged page's pattern. */
  label: string;
  counts: Counts;
  last: string;
  /** Its threads that pass the filter and the search, newest activity first. */
  threads: Thread[];
};

export function counts(threads: Thread[]): Counts {
  const c: Counts = { open: 0, addressed: 0, resolved: 0 };
  for (const t of threads) c[statusOf(t)]++;
  return c;
}

const newestFirst = (a: Thread, b: Thread) => (lastActivity(a) < lastActivity(b) ? 1 : lastActivity(a) > lastActivity(b) ? -1 : 0);

/** A page's name in the panel: a merged page's pattern, else its path. */
export const pageLabel = (p: PageView): string => (p.merged && p.pattern) || p.path;

/** The site's pages other than `here` (the tab's page), each with its
 * threads that pass the filter and the search; a page with none left out;
 * newest activity first. */
export function groups(site: SiteView | null, here: string | null, filter: Filter, search: string): Group[] {
  return (site?.pages ?? []).filter(p => p.page.artifact_id !== here).map(({ page, threads }) => ({
    page, label: pageLabel(page), counts: counts(threads),
    last: threads.reduce((m, t) => (lastActivity(t) > m ? lastActivity(t) : m), ""),
    threads: threads.filter(t => matches(t, filter, search, t.page_path ?? page.path)).sort(newestFirst),
  })).filter(g => g.threads.length).sort((a, b) => (a.last < b.last ? 1 : a.last > b.last ? -1 : 0));
}

const LITERAL = /^(?:[A-Za-z0-9\-._~!$&'()+,;=@:]|%[0-9A-Fa-f]{2})+$/;
const NAME = /^:[A-Za-z0-9_]{1,32}$/;

/** Why the daemon would refuse `pattern` as a merge rule, in words; null when it would take it. */
export function checkPattern(pattern: string): string | null {
  if (!pattern.startsWith("/")) return "A pattern starts with /.";
  if (new TextEncoder().encode(pattern).length > 256) return "A pattern is at most 256 bytes.";
  const segs = pattern.slice(1).split("/");
  if (segs.length > 16) return "A pattern has at most 16 segments.";
  let literal = false, wild = false;
  for (const [i, s] of segs.entries()) {
    if (s === "") return "A pattern has no empty segment and no trailing /.";
    if (s === "*") {
      if (i !== segs.length - 1) return "* can only be the last segment.";
      wild = true;
    } else if (s.startsWith(":")) {
      if (!NAME.test(s)) return `${s} is not a name: use 1 to 32 letters, digits or _ after the colon.`;
      wild = true;
    } else {
      if (s === "." || s === ".." || !LITERAL.test(s)) return `${s} is not a path segment Clax can match.`;
      literal = true;
    }
  }
  if (!literal) return "A pattern needs at least one fixed segment, such as /users/:id.";
  if (!wild) return "A pattern needs a :name or a last *, such as /users/:id.";
  return null;
}

/** Whether `pattern` (as `checkPattern` takes it) matches `path`: a literal
 * the same text, `:name` one non-empty segment, a last `*` one or more. */
export function matchPattern(pattern: string, path: string): boolean {
  const ps = pattern.slice(1).split("/");
  const xs = path.slice(1).split("/");
  for (const [i, p] of ps.entries()) {
    if (p === "*") return i < xs.length && xs[i] !== "";
    if (i >= xs.length || (p.startsWith(":") ? xs[i] === "" : xs[i] !== p)) return false;
  }
  return xs.length === ps.length;
}

/** The paths of the site's pages a new rule of `pattern` would merge (not
 * merged pages, nor the pattern's own page), in order. The daemon decides between
 * rules that match the same path; this preview does not. */
export function preview(site: SiteView | null, pattern: string): string[] {
  if (checkPattern(pattern)) return [];
  return (site?.pages ?? []).map(p => p.page).filter(p => !p.merged && p.path !== pattern && matchPattern(pattern, p.path)).map(p => p.path).sort();
}
