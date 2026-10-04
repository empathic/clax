import { describe, expect, it } from "vitest";
import type { Artifact } from "../api";
import type { Comment, Thread } from "../threads";
import { type ThreadDelta, applyPresence, applyThread, applyVersion, workingFromSummary } from "./deltas";

const c = (id: string, body = id): Comment => ({ id, thread_id: "t1", author_kind: "viewer", author_name: "Ann", via_harness: null, body, created_at: "2026-10-01T00:00:00Z" });
const thread = (comments: Comment[]): Thread => ({ id: "t1", artifact_id: "a", version_n: 1, anchor: { kind: "page" } as unknown as Thread["anchor"], status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, comments, feedback_state: null });
const delta = (t: Thread, count: number, last: Comment | null): ThreadDelta => { const { comments: _c, ...rest } = t; return { ...rest, comment_count: count, last_comment: last }; };

describe("stream deltas", () => {
  it("adds a thread's newest comment, and replaces an edited one in place", () => {
    const held = [thread([c("c1")])];
    const r = applyThread(held, delta({ ...thread([]), status: "resolved" }, 2, c("c2")));
    expect(r.complete).toBe(true);
    expect(r.thread.comments.map(x => x.id)).toEqual(["c1", "c2"]);
    expect(r.thread.status).toBe("resolved");
    expect(held[0].comments).toHaveLength(1);
    const edited = applyThread(r.threads, delta(thread([]), 2, c("c2", "edited")));
    expect(edited.thread.comments.map(x => x.body)).toEqual(["c1", "edited"]);
  });

  it("says when a thread's comments no longer add up, so it is fetched whole", () => {
    expect(applyThread([], delta(thread([]), 3, c("c3"))).complete).toBe(false);
    expect(applyThread([], delta(thread([]), 1, c("c1"))).threads).toHaveLength(1);
  });

  it("applies presence changes and departures by public ID", () => {
    const p = (id: string, state: "here" | "away" = "here") => ({ public_id: id, display_name: null, state, where: null, since: "s" });
    expect(applyPresence([p("a"), p("b")], [p("b", "away"), p("c")], ["a"])).toEqual([p("b", "away"), p("c")]);
  });

  it("updates a card's version, title and time, never backwards, and asks for an unknown card", () => {
    const a = { id: "a", title: "Old", current_version: 2, updated_at: "u" } as Artifact;
    expect(applyVersion([a], { artifact_id: "a", n: 3, title: "New", at: "t" })).toEqual([{ ...a, current_version: 3, title: "New", updated_at: "t" }]);
    const same = [a];
    expect(applyVersion(same, { artifact_id: "a", n: 1 })).toBe(same);
    expect(applyVersion([a], { artifact_id: "b", n: 1 })).toBeNull();
  });

  it("reads the gallery's working summary as working lists its chips can count", () => {
    const w = workingFromSummary([{ agent: "a_1", harness: "claude", threads: 2, started_at: "s" }]);
    expect(w).toEqual([expect.objectContaining({ agent: "a_1", harness: "claude", started_at: "s", message: null })]);
    expect(w[0].thread_ids).toHaveLength(2);
  });
});
