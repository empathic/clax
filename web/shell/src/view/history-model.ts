// A thread's history as version-tagged events (spec §8, "Thread sidebar"):
// "v4 alex sent it with 2 others · v5 claude addressed it · v5 alex
// resolved". Its comments are not repeated there: each shows its author,
// its version (`commentVersion`) and its time on its own line (owner ruling
// 2026-10-07). A person's event carries the version current when it
// happened; an agent's carries one only when it came with a version: "v3
// claude addressed it", for each version that addressed the thread. On a
// live page, an agent's address waiting for the next snapshot reads "claude
// addressed it · waiting for a snapshot". Events are in time order.
import type { AnchorResult } from "../../../bridge/src/protocol";
import type { Version } from "../api";
import type { Comment, Thread } from "../threads";

export type HistoryEvent = { v: number | null; who: string; agent: boolean; verb: string };

export const agentName = (h: string | null | undefined): string => h || "agent";

/** What a version tag needs of a version (all the side panel has of another page's). */
export type VersionTag = Pick<Version, "n" | "created_at" | "agent_harness">;

export function versionAt(versions: VersionTag[], iso: string): number {
  let n = 1;
  for (const v of versions) if (v.created_at <= iso && v.n > n) n = v.n;
  return n;
}

/** `names` turns a `resolved_by` value (`viewer:<public_id>`, `agent:<harness>`) into a name.
 * `more.working` names the agent working on the open thread now: the line ends with it. */
export function historyOf(t: Thread, versions: VersionTag[], names: (by: string) => string, more: { working?: string | null } = {}): HistoryEvent[] {
  const out: { at: string; e: HistoryEvent }[] = [];
  // A person's event is tagged with the version current then; with no
  // versions known (another page's, not loaded yet), with none.
  const current = (iso: string) => (versions.length ? versionAt(versions, iso) : null);
  // A batch send (spec §8): "sent it with 2 others · “Before the demo”".
  for (const b of t.sends ?? []) {
    const others = b.size - 1;
    const verb = "sent it" + (others > 0 ? ` with ${others} other${others === 1 ? "" : "s"}` : "") + (b.note ? ` · “${b.note}”` : "");
    out.push({ at: b.sent_at, e: { v: current(b.sent_at), who: b.sent_by, agent: false, verb } });
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
    out.push({ at: t.resolved_at, e: { v: agent ? null : current(t.resolved_at), who: names(t.resolved_by), agent, verb: "resolved" } });
  }
  // "~" sorts after every timestamp: working ends the line.
  if (more.working && t.status === "open") out.push({ at: "~", e: { v: null, who: more.working, agent: true, verb: "working on it" } });
  // A stable sort: events at the same time keep their order.
  return out.sort((a, b) => (a.at < b.at ? -1 : a.at > b.at ? 1 : 0)).map(x => x.e);
}

/** The version a comment is tagged with beside its time: the thread's
 * first comment, the version it was made on; a person's reply, the version
 * current then (none while `versions` is not known); an agent's reply, none
 * (its address names one: `addressedNote`). */
export function commentVersion(t: Thread, c: Comment, versions: VersionTag[]): number | null {
  if (c.author_kind === "agent") return null;
  if (c.id === t.comments[0]?.id) return t.version_n;
  return versions.length ? versionAt(versions, c.created_at) : null;
}

/** The version an agent's reply is labelled with ("addressed in vN"): the
 * first version that addressed the thread created at or after the reply. */
export function addressedNote(t: Thread, c: Comment, versions: VersionTag[]): number | null {
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
