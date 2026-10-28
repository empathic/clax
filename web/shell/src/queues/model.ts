// The Queues page's rules, apart from its view: a queue's origins as typed
// one per line, its reports counted by state, and a refusal as a sentence.
import type { ReportPage, ReportState } from "../../../sdk/src/types";

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
  const c: Counts = { open: 0, claimed: 0, resolved: 0, more: false };
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

/** `Counts` as a line: "3 open · 1 claimed · 9 resolved", a `+` after each when there are more. */
export function countsText(c: Counts): string {
  const plus = c.more ? "+" : "";
  return `${c.open}${plus} open · ${c.claimed}${plus} claimed · ${c.resolved}${plus} resolved`;
}

/** What to show for a refusal `e`: the daemon's message (lower case, no
 * full stop) as a sentence. */
export function refusal(e: unknown): string {
  const s = (e instanceof Error ? e.message : String(e)).trim();
  if (!s) return "Something went wrong. Try again.";
  return `${s[0].toUpperCase()}${s.slice(1)}${/[.!?]$/.test(s) ? "" : "."}`;
}
