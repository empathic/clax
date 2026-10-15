import { describe, expect, it } from "vitest";
import type { Comment, Thread, ThreadLink } from "../threads";
import { anyFailed, cardLinkLines, commentLinkState, linkTag, remoteAuthor, remoteVersion, threadLinkState } from "./linked-model";

const link = (o: Partial<ThreadLink> = {}): ThreadLink => ({
  remote_id: "r_1", remote: "clax.dev", remote_origin: "https://clax.dev", url: "https://clax.dev/a/abc",
  origin: "remote", remote_version: 1, twin: true, unlinked: false, state: null, error: null, ...o,
});
const comment = (o: Partial<Comment> = {}): Comment => ({
  id: "c1", thread_id: "t1", author_kind: "viewer", author_name: "Ada", via_harness: null, body: "hi", created_at: "2026-10-15T10:00:00Z", ...o,
});
const thread = (l?: ThreadLink, comments: Comment[] = [comment()]): Thread => ({
  id: "t1", artifact_id: "a1", version_n: 2, anchor: { kind: "artifact", file: "" } as Thread["anchor"], status: "open",
  sent_to_agent: false, has_clip: false, clip_url: null, created_at: "", resolved_at: null, resolved_by: null,
  comments, feedback_state: null, linked: l,
});

describe("linked threads", () => {
  it("tags a mirrored thread with its remote, and an unlinked one as history", () => {
    expect(linkTag(thread())).toBeNull();
    expect(linkTag(thread(link()))).toEqual({ label: "clax.dev", title: "From clax.dev", unlinked: false });
    expect(linkTag(thread(link({ origin: "local" })))?.title).toBe("Also on clax.dev");
    expect(linkTag(thread(link({ unlinked: true })))).toMatchObject({ label: "clax.dev · unlinked", unlinked: true });
  });

  it("names the public version a thread with no twin is on", () => {
    expect(remoteVersion(thread(link()))).toBeNull();
    expect(remoteVersion(thread(link({ twin: false, remote_version: 4, url: "https://clax.dev/a/abc/v/4" }))))
      .toEqual({ label: "on clax.dev v4", url: "https://clax.dev/a/abc/v/4" });
  });

  it("says what waits to go up, with Retry once not sent", () => {
    expect(threadLinkState(thread(link()))).toBeNull();
    expect(threadLinkState(thread(link({ state: "pending" })))?.text).toBe("Sending to clax.dev…");
    expect(threadLinkState(thread(link({ state: "held" })))?.text).toBe("Waiting for v2 to be pushed to clax.dev");
    expect(threadLinkState(thread(link({ state: "failed" })))).toEqual({ text: "Not sent: clax.dev unreachable", retry: true, failed: true });
    expect(threadLinkState(thread(link({ state: "failed", unlinked: true })))).toBeNull();
    const c = comment({ linked: { origin: "local", remote: "clax.dev", author_kind: null, author_name: null, state: "failed" } });
    expect(commentLinkState(c)).toEqual({ text: "Not sent: clax.dev unreachable", failed: true });
    expect(anyFailed(thread(link(), [comment(), c]))).toBe(true);
    expect(anyFailed(thread(link()))).toBe(false);
  });

  it("names a remote author by their name there", () => {
    expect(remoteAuthor(comment())).toBeNull();
    const c = comment({ author_name: "Ada (not connected)", linked: { origin: "remote", remote: "clax.dev", author_kind: "viewer", author_name: "Ada", state: null } });
    expect(remoteAuthor(c)).toEqual({ name: "Ada", remote: "clax.dev" });
  });

  it("says a comment not sent once, with Retry under the first, and as sending again once retried", () => {
    const failed = (id: string) => comment({ id, linked: { origin: "local", remote: "clax.dev", author_kind: null, author_name: null, state: "failed" } });
    const t = thread(link({ origin: "local" }), [comment(), failed("c2"), failed("c3")]);
    const l = cardLinkLines(t);
    expect(l.thread).toBeNull();
    expect([...l.comments]).toEqual([
      ["c2", { text: "Not sent: clax.dev unreachable", failed: true, retry: true }],
      ["c3", { text: "Not sent: clax.dev unreachable", failed: true, retry: false }],
    ]);
    const again = cardLinkLines(t, true);
    expect([...again.comments.values()].map(c => [c.text, c.retry])).toEqual([["Sending to clax.dev…", false], ["Sending to clax.dev…", false]]);
    // The thread's own write not sent carries Retry; its comments' lines do not.
    const own = cardLinkLines(thread(link({ state: "failed" }), [failed("c2")]));
    expect(own.thread).toEqual({ text: "Not sent: clax.dev unreachable", failed: true, retry: true });
    expect(own.comments.get("c2")?.retry).toBe(false);
    expect(cardLinkLines(thread(link({ state: "failed" })), true).thread).toEqual({ text: "Sending to clax.dev…", failed: false, retry: false });
    expect(cardLinkLines(thread(link({ state: "held" }))).thread?.text).toBe("Waiting for v2 to be pushed to clax.dev");
    expect(cardLinkLines(thread(link({ unlinked: true }), [failed("c2")])).comments.size).toBe(0);
  });
});
