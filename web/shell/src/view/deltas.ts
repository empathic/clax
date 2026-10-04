// The `/api/stream` deltas applied to what a view already holds, so a view
// refetches only when it goes live, after a `resync`, or when a delta does
// not add up (`complete` false).
import type { Artifact } from "../api";
import type { Comment, Thread } from "../threads";
import type { PresenceView } from "./presence-model";
import type { Working } from "./working-model";

/** A thread as the `artifact:<id>` topic carries it: its view without its
 * comments, their count, and the newest one. */
export type ThreadDelta = Omit<Thread, "comments"> & { comment_count: number; last_comment: Comment | null };

/** `threads` with `d` applied: the newest comment added (or replaced, when
 * it was edited); `complete` is false when the comments held no longer add
 * up to the count, and the thread should be fetched whole. */
export function applyThread(threads: Thread[], d: ThreadDelta): { threads: Thread[]; thread: Thread; complete: boolean } {
  const { comment_count, last_comment, ...rest } = d;
  const i = threads.findIndex(t => t.id === d.id);
  let comments = i >= 0 ? threads[i].comments : [];
  if (last_comment) {
    const at = comments.findIndex(c => c.id === last_comment.id);
    comments = at >= 0 ? comments.map((c, k) => (k === at ? last_comment : c)) : [...comments, last_comment];
  }
  const thread: Thread = { ...(i >= 0 ? threads[i] : {}), ...rest, comments } as Thread;
  const out = threads.slice();
  if (i >= 0) out[i] = thread;
  else out.push(thread);
  return { threads: out, thread, complete: comments.length === comment_count };
}

/** `people` with the `presence:<id>` delta applied: each of `changed`
 * added or replaced by public ID, each of `gone` removed. */
export function applyPresence(people: PresenceView[], changed: PresenceView[], gone: string[]): PresenceView[] {
  const drop = new Set([...gone, ...changed.map(p => p.public_id)]);
  return [...people.filter(p => !drop.has(p.public_id)), ...changed];
}

/** One agent's work as the `gallery` topic carries it. */
export type WorkingSummary = { agent: string; harness: string; threads: number; started_at: string };

/** The gallery's working list from its summary: what its chips and rosters
 * read (the agent, its harness, how many threads, since when). */
export function workingFromSummary(list: WorkingSummary[]): Working[] {
  return list.map(w => ({
    key: w.agent, agent: w.agent, harness: w.harness, message: null,
    thread_ids: Array.from({ length: w.threads }, (_, k) => `${w.agent}#${k}`),
    started_at: w.started_at, last_heartbeat: w.started_at,
  }));
}

/** `list` with the `gallery` topic's version delta applied; null when the
 * artifact is not in the list (a new artifact: fetch its card). */
export function applyVersion(list: Artifact[], d: { artifact_id: string; n: number; title?: string | null; at?: string | null }): Artifact[] | null {
  const i = list.findIndex(a => a.id === d.artifact_id);
  if (i < 0) return null;
  const a = list[i];
  if (d.n <= a.current_version) return list;
  const out = list.slice();
  out[i] = { ...a, current_version: d.n, title: d.title ?? a.title, updated_at: d.at ?? new Date().toISOString() };
  return out;
}
