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

/** A queue's reports counted by state; `more` when the pages ran past the
 * limit; `lapses` the soonest a claim counted ends (its lease's
 * `expires_at`, in ms), when one does: no event says a lease lapsed. */
export type Counts = Record<ReportState, number> & { more: boolean; lapses: number | null };

/** Counts the reports `page` gives, following each page's `next` for at most `limit` pages. */
export async function countReports(page: (cursor: string | null) => Promise<ReportPage>, limit = 20): Promise<Counts> {
  const c: Counts = { open: 0, claimed: 0, resolved: 0, more: false, lapses: null };
  let cursor: string | null = null;
  for (let i = 0; i < limit; i++) {
    const p = await page(cursor);
    for (const r of p.reports) {
      if (r.state in c) c[r.state]++;
      const end = r.state === "claimed" && r.claim ? Date.parse(r.claim.expires_at) : NaN;
      if (!Number.isNaN(end) && (c.lapses === null || end < c.lapses)) c.lapses = end;
    }
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

/** What changed in queue `name` from counts `a` to `b`, as a sentence for
 * a screen reader: "1 report claimed in Bugs.", "2 reports filed, 1
 * resolved in Bugs."; `null` when nothing rose or either count is cut
 * short (`more`). A report back in `open` (released, or reopened by a
 * comment) is "open again". */
export function changeText(name: string, a: Counts, b: Counts): string | null {
  if (a.more || b.more) return null;
  const filed = Math.max(0, b.open + b.claimed + b.resolved - (a.open + a.claimed + a.resolved));
  const parts: [number, string][] = [
    [filed, "filed"],
    [b.claimed - a.claimed, "claimed"],
    [b.resolved - a.resolved, "resolved"],
    [b.open - a.open - filed, "open again"],
  ];
  const said = parts.filter(([n]) => n > 0).map(([n, what], i) => (i === 0 ? `${n} report${n === 1 ? "" : "s"} ${what}` : `${n} ${what}`));
  if (!said.length) return null;
  return `${said.join(", ")} in ${name}.`;
}

/** What to show for a refusal `e`: the daemon's message (lower case, no
 * full stop) as a sentence. */
export function refusal(e: unknown): string {
  const s = (e instanceof Error ? e.message : String(e)).trim();
  if (!s) return "Something went wrong. Try again.";
  return `${s[0].toUpperCase()}${s.slice(1)}${/[.!?]$/.test(s) ? "" : "."}`;
}
