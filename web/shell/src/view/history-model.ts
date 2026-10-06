// A thread's history as version-tagged events (spec §8, "Thread sidebar"):
// "v3 alex commented · v4 Mia replied · claude replied · v5 alex resolved".
// The comment that opens the thread carries the version it was made on; a
// person's later event carries the version current when it happened; an
// agent's carries one only when it came with a version: "v3 claude addressed
// it", for each version that addressed the thread. A batch send reads "alex
// sent it with 2 others". On a live page, an agent's address waiting for the
// next snapshot reads "claude addressed it · waiting for a snapshot". Events
// are in time order.
import type { AnchorResult } from "../../../bridge/src/protocol";
import type { Version } from "../api";
import type { Comment, Thread } from "../threads";

export type HistoryEvent = { v: number | null; who: string; agent: boolean; verb: string };

export const agentName = (h: string | null | undefined): string => h || "agent";

export function versionAt(versions: Version[], iso: string): number {
  let n = 1;
  for (const v of versions) if (v.created_at <= iso && v.n > n) n = v.n;
  return n;
}

/** `names` turns a `resolved_by` value (`viewer:<public_id>`, `agent:<harness>`) into a name.
 * `more.working` names the agent working on the open thread now: the line ends with it. */
export function historyOf(t: Thread, versions: Version[], names: (by: string) => string, more: { working?: string | null } = {}): HistoryEvent[] {
  const out: { at: string; e: HistoryEvent }[] = [];
  t.comments.forEach((c, i) => {
    const e = c.author_kind === "agent" ? { v: null, who: agentName(c.via_harness), agent: true, verb: "replied" }
      : i === 0 ? { v: t.version_n, who: c.author_name, agent: false, verb: "commented" }
      : { v: versionAt(versions, c.created_at), who: c.author_name, agent: false, verb: "replied" };
    out.push({ at: c.created_at, e });
  });
  // A batch send (spec §8): "sent it with 2 others · “Before the demo”".
  for (const b of t.sends ?? []) {
    const others = b.size - 1;
    const verb = "sent it" + (others > 0 ? ` with ${others} other${others === 1 ? "" : "s"}` : "") + (b.note ? ` · “${b.note}”` : "");
    out.push({ at: b.sent_at, e: { v: versionAt(versions, b.sent_at), who: b.sent_by, agent: false, verb } });
  }
  // A version no agent published (a live page's snapshot) is named by the
  // agent whose reply addressed the thread: the last one before it, else the first.
  const agentReplies = t.comments.filter(c => c.author_kind === "agent" && c.via_harness);
  const replier = (at: string) => (agentReplies.filter(c => c.created_at <= at).at(-1) ?? agentReplies[0])?.via_harness;
  for (const n of t.addressed_in ?? []) {
    const v = versions.find(x => x.n === n);
    out.push({ at: v?.created_at ?? "", e: { v: n, who: agentName(v?.agent_harness ?? replier(v?.created_at ?? "~")), agent: true, verb: "addressed it" } });
  }
  // A live page's address waits for the page's next snapshot to name a version.
  if (t.addressed_pending && t.status === "open") {
    out.push({ at: t.addressed_pending.at, e: { v: null, who: agentName(t.addressed_pending.harness), agent: true, verb: "addressed it · waiting for a snapshot" } });
  }
  if (t.status === "resolved" && t.resolved_by && t.resolved_at) {
    const agent = t.resolved_by.startsWith("agent:");
    out.push({ at: t.resolved_at, e: { v: agent ? null : versionAt(versions, t.resolved_at), who: names(t.resolved_by), agent, verb: "resolved" } });
  }
  // "~" sorts after every timestamp: working ends the line.
  if (more.working && t.status === "open") out.push({ at: "~", e: { v: null, who: more.working, agent: true, verb: "working on it" } });
  // A stable sort: events at the same time keep their order.
  return out.sort((a, b) => (a.at < b.at ? -1 : a.at > b.at ? 1 : 0)).map(x => x.e);
}

/** The version an agent's reply is labelled with ("addressed in vN"): the
 * first version that addressed the thread created at or after the reply. */
export function addressedNote(t: Thread, c: Comment, versions: Version[]): number | null {
  if (c.author_kind !== "agent") return null;
  const at = (n: number) => versions.find(v => v.n === n)?.created_at ?? "";
  return (t.addressed_in ?? []).find(n => at(n) >= c.created_at) ?? null;
}

/** The element changed in a later version but is still there: found by
 * selector or quote while the stored hash no longer matches (spec §8). A
 * page-anchored (custom) thread is never outdated. */
export function isOutdated(t: Thread, r: AnchorResult | undefined, shown: number): boolean {
  return !!r?.found && shown > t.version_n && !!t.anchor.html_hash && (r.method === "selector" || r.method === "quote");
}
