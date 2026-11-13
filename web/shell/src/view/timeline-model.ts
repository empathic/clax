// The artifact's timeline (spec 2026-11-13, "The model"): every thread read
// as a sequence of facts, walked by one state machine, and the facts of all
// threads and versions merged into lines, oldest first. A thread is listed
// once, under the newest line that concerns it (`here`); each line carries
// how many threads are pending, and how many of those wait on a person,
// just after it. Pure: no DOM, no clock of its own.
import type { Version } from "../api";
import { type Comment, type Thread, type Viewer, resolvedByLabel } from "../threads";
import { unsent } from "./batch-model";
import { agentName } from "./history-model";
import type { Working } from "./working-model";

/** A thread's state. Pending: unsent, with-agent, answered, stalled; of
 * those, unsent, answered and stalled wait on a person. Addressed is not
 * pending though the thread is still open. */
export type ThreadState =
  | { kind: "unsent" } | { kind: "with-agent"; working: boolean }
  | { kind: "answered"; agent: string } | { kind: "stalled"; why: "withheld" | "declined" | "no_access" | "agent_ended" }
  | { kind: "addressed"; v: number } | { kind: "resolved" };
export type LineKind = "version" | "made" | "sent" | "answered" | "followed-up" | "resolved";
export type Line = {
  /** Stable across recomputes: its kind and the first thread, comment, batch or version it stands for. */
  key: string; at: string; kind: LineKind;
  /** A display name, or the agent's name. */
  who: string;
  /** `mine`: the viewer did it. */
  agent: boolean; mine: boolean;
  /** A version's number and note; a batch's size and note. */
  v?: number; note?: string | null; size?: number;
  /** `ids`: the threads the line is about; `here`: of those, the ones whose newest line this is. */
  ids: string[]; here: string[];
  /** The counts over all threads just after this line. */
  pending: number; waiting: number;
};
/** `lines` oldest first; `pending` and `waiting` (thread IDs, now) newest first. */
export type Timeline = { lines: Line[]; pending: string[]; waiting: string[]; stateOf: Map<string, ThreadState> };
/** `claimed`: the threads ClaxBot took; `me`: the viewer, for "You". */
export type TimelineInput = { threads: Thread[]; versions: Version[]; working: Working[]; claimed: readonly string[]; me?: Viewer | null };

export const isPending = (s: ThreadState): boolean => s.kind === "unsent" || s.kind === "with-agent" || s.kind === "answered" || s.kind === "stalled";
export const isWaiting = (s: ThreadState): boolean => s.kind === "unsent" || s.kind === "answered" || s.kind === "stalled";

/** The delivery states in which the card asks a person to act. */
const STALLED = new Set(["withheld", "declined", "no_access", "agent_ended"]);

type Fact =
  | { at: string; kind: "made"; c: Comment | undefined }
  | { at: string; kind: "sent"; batch: string; first: boolean }
  | { at: string; kind: "person"; c: Comment }
  | { at: string; kind: "agent"; c: Comment }
  | { at: string; kind: "addressed"; v: number }
  | { at: string; kind: "resolved"; by: string };
/** A line before folding and merging. */
type Raw = Omit<Line, "here" | "pending" | "waiting">;

/** `list` sorted by `at` (ISO strings), equal times keeping their order. */
const byAt = <T extends { at: string }>(list: T[]): T[] => [...list].sort((a, b) => (a.at < b.at ? -1 : a.at > b.at ? 1 : 0));

/** A thread's facts in time order; at equal times: made, sent, a person's
 * comment, an agent's, addressed, resolved. A version number no known
 * version has is skipped. */
function factsOf(t: Thread, versionAt: Map<number, string>): Fact[] {
  const [first, ...rest] = t.comments;
  const out: Fact[] = [{ at: first?.created_at ?? t.created_at, kind: "made", c: first }];
  (t.sends ?? []).forEach((s, i) => out.push({ at: s.sent_at, kind: "sent", batch: s.batch_id, first: i === 0 }));
  for (const c of rest) if (c.author_kind !== "agent") out.push({ at: c.created_at, kind: "person", c });
  for (const c of rest) if (c.author_kind === "agent") out.push({ at: c.created_at, kind: "agent", c });
  for (const n of t.addressed_in ?? []) { const at = versionAt.get(n); if (at !== undefined) out.push({ at, kind: "addressed", v: n }); }
  if (t.status === "resolved" && t.resolved_at) out.push({ at: t.resolved_at, kind: "resolved", by: t.resolved_by ?? "" });
  return byAt(out);
}

/** A thread's state after each of its facts, and the lines its facts write
 * (sends and addresses are written by their batch's and version's lines). */
function walk(t: Thread, facts: Fact[], implicit: boolean, me: Viewer | null | undefined): { steps: { at: string; s: ThreadState }[]; lines: Raw[] } {
  // A stretch runs from one person's comment to the next; an agent's comment
  // in a stretch that holds an address is that version's answer.
  const stretchAddressed: boolean[] = [];
  let seg = -1;
  for (const f of facts) {
    if (f.kind === "made" || f.kind === "person") stretchAddressed[++seg] = false;
    else if (f.kind === "addressed") stretchAddressed[seg] = true;
  }
  const mine = (c: Comment | undefined) => !!me && !!c?.author_public_id && c.author_public_id === me.public_id;
  const steps: { at: string; s: ThreadState }[] = [];
  const lines: Raw[] = [];
  let s: ThreadState = { kind: "unsent" };
  let sent = false, answered = false;
  seg = -1;
  for (const f of facts) {
    switch (f.kind) {
      case "made":
        seg++; answered = false; sent = implicit;
        s = implicit ? { kind: "with-agent", working: false } : { kind: "unsent" };
        lines.push({ key: `made:${t.id}`, at: f.at, kind: "made", who: f.c?.author_name ?? "", agent: f.c?.author_kind === "agent", mine: mine(f.c), ids: [t.id] });
        break;
      case "sent":
        sent = true; s = { kind: "with-agent", working: false };
        break;
      case "person":
        seg++; answered = false;
        s = sent ? { kind: "with-agent", working: false } : { kind: "unsent" };
        lines.push({ key: `followed-up:${f.c.id}`, at: f.at, kind: "followed-up", who: f.c.author_name, agent: false, mine: mine(f.c), ids: [t.id] });
        break;
      case "agent":
        if (stretchAddressed[seg] || answered) break;
        answered = true;
        s = { kind: "answered", agent: agentName(f.c.via_harness) };
        lines.push({ key: `answered:${f.c.id}`, at: f.at, kind: "answered", who: s.agent, agent: true, mine: false, ids: [t.id] });
        break;
      case "addressed":
        s = { kind: "addressed", v: f.v };
        break;
      case "resolved":
        s = { kind: "resolved" };
        lines.push({ key: `resolved:${t.id}`, at: f.at, kind: "resolved", who: resolvedByLabel(f.by, me, t.resolved_by_name), agent: f.by.startsWith("agent:"), mine: !!me && f.by === `viewer:${me.public_id}`, ids: [t.id] });
        break;
    }
    steps.push({ at: f.at, s });
  }
  return { steps, lines };
}

/** The thread's state now: its history's last state, refined. A resolved
 * thread is resolved; one `unsent()` lists is unsent (and only those are);
 * one with the agent is `working` while a working record lists it, and
 * stalled while its delivery waits on a person. */
function present(t: Thread, last: ThreadState, isUnsent: boolean, working: Set<string>): ThreadState {
  if (t.status === "resolved") return { kind: "resolved" };
  if (isUnsent) return { kind: "unsent" };
  const s: ThreadState = last.kind === "unsent" ? { kind: "with-agent", working: false } : last;
  if (s.kind !== "with-agent") return s;
  const fs = t.feedback_state?.state;
  if (fs && STALLED.has(fs)) return { kind: "stalled", why: fs as "withheld" | "declined" | "no_access" | "agent_ended" };
  return { kind: "with-agent", working: working.has(t.id) };
}

const MERGES = new Set<LineKind>(["made", "followed-up", "resolved"]);

export function timeline(i: TimelineInput): Timeline {
  const known = new Set(i.threads.map(t => t.id));
  const versionAt = new Map(i.versions.map(v => [v.n, v.created_at]));
  const unsentIds = new Set(unsent(i.threads, i.claimed).map(t => t.id));
  const working = new Set(i.working.flatMap(w => w.thread_ids));

  // Versions first, so a thread's line at a version's very time follows it.
  const raw: Raw[] = [];
  const versionLine = new Map<number, Raw>();
  for (const v of [...i.versions].sort((a, b) => a.n - b.n)) {
    const line: Raw = { key: `version:${v.n}`, at: v.created_at, kind: "version", who: agentName(v.agent_harness), agent: true, mine: false, v: v.n, note: v.note ?? null, ids: (v.addresses ?? []).filter(id => known.has(id)) };
    versionLine.set(v.n, line);
    raw.push(line);
  }
  const batchLine = new Map<string, Raw>();
  // By a sent line's key: the threads whose first send it is.
  const firstSends = new Map<string, Set<string>>();
  const steps = new Map<string, { at: string; s: ThreadState }[]>();
  const stateOf = new Map<string, ThreadState>();
  for (const t of i.threads) {
    const facts = factsOf(t, versionAt);
    const implicit = (t.sent_to_agent && !t.sends?.length) || i.claimed.includes(t.id);
    const w = walk(t, facts, implicit, i.me);
    steps.set(t.id, w.steps);
    raw.push(...w.lines);
    for (const f of facts) {
      if (f.kind === "addressed") {
        const line = versionLine.get(f.v);
        if (line && !line.ids.includes(t.id)) line.ids.push(t.id);
      } else if (f.kind === "sent") {
        const b = t.sends?.find(x => x.batch_id === f.batch);
        let line = batchLine.get(f.batch);
        if (!line && b) {
          line = { key: `sent:${f.batch}`, at: b.sent_at, kind: "sent", who: b.sent_by, agent: false, mine: !!i.me?.display_name && i.me.display_name === b.sent_by, size: b.size, note: b.note, ids: [] };
          batchLine.set(f.batch, line);
          raw.push(line);
        }
        if (!line) continue;
        if (f.at > line.at) line.at = f.at;
        if (!line.ids.includes(t.id)) line.ids.push(t.id);
        if (f.first) firstSends.set(line.key, (firstSends.get(line.key) ?? new Set()).add(t.id));
      }
    }
    stateOf.set(t.id, present(t, w.steps.at(-1)?.s ?? { kind: "unsent" }, unsentIds.has(t.id), working));
  }

  // A made line directly before its thread's first send (with only other
  // such made lines between) is folded into the sent line.
  const sorted = byAt(raw);
  const dropped = new Set<Raw>();
  sorted.forEach((line, k) => {
    if (line.kind !== "sent") return;
    for (let j = k - 1; j >= 0 && sorted[j].kind === "made" && firstSends.get(line.key)?.has(sorted[j].ids[0]); j--) dropped.add(sorted[j]);
  });
  // Adjacent made, followed-up and resolved lines of one author merge; the
  // merged line keeps the first one's key.
  const merged: Raw[] = [];
  for (const line of sorted) {
    if (dropped.has(line)) continue;
    const prev = merged.at(-1);
    if (prev && MERGES.has(line.kind) && prev.kind === line.kind && prev.who === line.who && prev.agent === line.agent && prev.mine === line.mine) {
      for (const id of line.ids) if (!prev.ids.includes(id)) prev.ids.push(id);
      if (line.at > prev.at) prev.at = line.at;
      continue;
    }
    merged.push({ ...line, ids: [...line.ids] });
  }

  const newest = new Map<string, number>();
  merged.forEach((line, k) => { for (const id of line.ids) newest.set(id, k); });
  const stateAt = (id: string, at: string): ThreadState | null => {
    let s: ThreadState | null = null;
    for (const st of steps.get(id) ?? []) { if (st.at > at) break; s = st.s; }
    return s;
  };
  const lines: Line[] = merged.map((line, k) => {
    let pending = 0, waiting = 0;
    for (const t of i.threads) {
      const s = stateAt(t.id, line.at);
      if (s && isPending(s)) pending++;
      if (s && isWaiting(s)) waiting++;
    }
    return { ...line, here: line.ids.filter(id => newest.get(id) === k), pending, waiting };
  });

  // Newest first: by the thread's newest line, then its newest fact.
  const lastAt = (id: string) => steps.get(id)?.at(-1)?.at ?? "";
  const order = i.threads.map(t => t.id).sort((a, b) => (newest.get(b) ?? -1) - (newest.get(a) ?? -1) || (lastAt(a) < lastAt(b) ? 1 : lastAt(a) > lastAt(b) ? -1 : 0));
  return {
    lines,
    pending: order.filter(id => isPending(stateOf.get(id)!)),
    waiting: order.filter(id => isWaiting(stateOf.get(id)!)),
    stateOf,
  };
}

const count = (n: number, what: string) => (n === 1 ? `a ${what}` : `${n} ${what}s`);

/** The line's words, and a second row when it has one (a batch's note). */
export function lineText(l: Line, stateOf: Map<string, ThreadState>): { text: string; sub?: string } {
  const who = l.mine ? "You" : l.who;
  switch (l.kind) {
    case "version": return { text: `v${l.v} · ${l.note || `published by ${l.who}`}` };
    case "made": {
      const nobody = l.here.some(id => stateOf.get(id)?.kind === "unsent") ? ", sent to no one" : "";
      return { text: (l.ids.length > 1 ? `${who} made ${l.ids.length} comments` : `${who} commented`) + nobody };
    }
    case "sent": {
      const text = `${who} sent ${count(l.size ?? l.ids.length, "comment")}`;
      return l.note ? { text, sub: l.note } : { text };
    }
    case "answered": return { text: `${who} replied` };
    case "followed-up": return { text: l.ids.length > 1 ? `${who} replied on ${l.ids.length} comments` : `${who} replied` };
    case "resolved": return { text: `${who} resolved ${count(l.ids.length, "comment")}` };
  }
}

const STALLED_WORDS = { withheld: "approve", declined: "declined", no_access: "no access", agent_ended: "no agent" } as const;

/** The row's state at the right: its words and whether it asks the viewer to act. */
export function rowStatus(s: ThreadState): { text: string; act: boolean } {
  switch (s.kind) {
    case "addressed": return { text: `✓ v${s.v}`, act: false };
    case "unsent": return { text: "Send", act: true };
    case "answered": return { text: "Reply", act: true };
    case "with-agent": return { text: s.working ? "working" : "sent", act: false };
    case "stalled": return { text: STALLED_WORDS[s.why], act: true };
    case "resolved": return { text: "resolved", act: false };
  }
}

/** `date`: the local day, `YYYY-MM-DD`. */
export type Day = { label: string; date: string; lines: Line[] };   // lines newest first

const WEEKDAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const two = (n: number) => String(n).padStart(2, "0");
const dayKey = (d: Date) => `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())}`;

/** The lines grouped by local day, newest day first: "Today", "Yesterday",
 * else weekday and date ("Tuesday 10 Nov", with the year when it is not
 * `now`'s). */
export function days(lines: Line[], now: Date): Day[] {
  const today = dayKey(now);
  const yesterday = dayKey(new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1));
  const out: Day[] = [];
  for (const l of byAt(lines).reverse()) {
    const d = new Date(l.at);
    const date = dayKey(d);
    let day = out.at(-1);
    if (day?.date !== date) {
      const label = date === today ? "Today" : date === yesterday ? "Yesterday"
        : `${WEEKDAYS[d.getDay()]} ${d.getDate()} ${MONTHS[d.getMonth()]}${d.getFullYear() === now.getFullYear() ? "" : ` ${d.getFullYear()}`}`;
      day = { label, date, lines: [] };
      out.push(day);
    }
    day.lines.push(l);
  }
  return out;
}

/** A folded day's summary: "Thursday · 3 versions · 7 addressed", then its
 * comments and resolves; a part that would be 0 is left out. */
export function daySummary(d: Day): string {
  const name = d.label === "Today" || d.label === "Yesterday" ? d.label : d.label.split(" ")[0];
  const of = (k: LineKind) => d.lines.filter(l => l.kind === k);
  const versions = of("version");
  const addressed = new Set(versions.flatMap(l => l.ids)).size;
  const comments = of("made").reduce((n, l) => n + l.ids.length, 0);
  const resolved = of("resolved").reduce((n, l) => n + l.ids.length, 0);
  const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;
  const parts = [versions.length ? plural(versions.length, "version") : "", addressed ? `${addressed} addressed` : "", comments ? plural(comments, "comment") : "", resolved ? `${resolved} resolved` : ""];
  return [name, ...parts.filter(Boolean)].join(" · ");
}
