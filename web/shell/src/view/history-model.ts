// A thread's history as version-tagged events (spec §8, "Thread sidebar"):
// "v3 alex commented · v4 Mia replied · claude replied · v5 alex resolved".
// A person's event carries the version current when it happened; an agent's
// carries one only when it came with a version (Task 18 adds those).
import type { AnchorResult } from "../../../bridge/src/protocol";
import type { Version } from "../api";
import type { Thread } from "../threads";

export type HistoryEvent = { v: number | null; who: string; agent: boolean; verb: string };

export const agentName = (h: string | null | undefined): string => h || "agent";

export function versionAt(versions: Version[], iso: string): number {
  let n = 1;
  for (const v of versions) if (v.created_at <= iso && v.n > n) n = v.n;
  return n;
}

/** `names` turns a `resolved_by` value (`viewer:<public_id>`, `agent:<harness>`) into a name. */
export function historyOf(t: Thread, versions: Version[], names: (by: string) => string): HistoryEvent[] {
  const out: HistoryEvent[] = [];
  t.comments.forEach((c, i) => {
    if (c.author_kind === "agent") out.push({ v: null, who: agentName(c.via_harness), agent: true, verb: "replied" });
    else out.push({ v: versionAt(versions, c.created_at), who: c.author_name, agent: false, verb: i === 0 ? "commented" : "replied" });
  });
  if (t.status === "resolved" && t.resolved_by && t.resolved_at) {
    const agent = t.resolved_by.startsWith("agent:");
    out.push({ v: agent ? null : versionAt(versions, t.resolved_at), who: names(t.resolved_by), agent, verb: "resolved" });
  }
  return out;
}

/** The element changed in a later version but is still there: found by
 * selector or quote while the stored hash no longer matches (spec §8). A
 * page-anchored (custom) thread is never outdated. */
export function isOutdated(t: Thread, r: AnchorResult | undefined, shown: number): boolean {
  return !!r?.found && shown > t.version_n && !!t.anchor.html_hash && (r.method === "selector" || r.method === "quote");
}
