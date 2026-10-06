import { describe, expect, it } from "vitest";
import type { Version } from "../api";
import type { Comment, Thread } from "../threads";
import { addressedNote, historyOf, isOutdated, versionAt } from "./history-model";

const V = (n: number, at: string): Version => ({ artifact_id: "a", n, label: null, created_at: at, files: {} });
const vs = [V(1, "2026-09-30T10:00:00.000Z"), V(2, "2026-09-30T11:00:00.000Z"), V(3, "2026-09-30T12:00:00.000Z")];
const C = (id: string, kind: "viewer" | "agent", name: string, at: string): Comment =>
  ({ id, thread_id: "t", author_kind: kind, author_name: name, via_harness: kind === "agent" ? "claude" : null, body: "x", created_at: at });
const T = (over: Partial<Thread>): Thread => ({
  id: "t", artifact_id: "a", version_n: 1, status: "open", sent_to_agent: true, has_clip: false, clip_url: null, created_at: "2026-09-30T10:30:00.000Z",
  resolved_at: null, resolved_by: null, feedback_state: null, comments: [],
  anchor: { kind: "element", selector: "h2", quote: null, prefix: null, suffix: null, html_hash: "h1", rect: null, custom_name: null, file: "index.html" }, ...over,
});
const names = (by: string) => (by === "viewer:u_1" ? "alex" : by.startsWith("agent:") ? by.slice(6) : "someone");

describe("history-model", () => {
  it("tags an event with the version current when it happened", () => {
    expect(versionAt(vs, "2026-09-30T09:00:00.000Z")).toBe(1);
    expect(versionAt(vs, "2026-09-30T11:30:00.000Z")).toBe(2);
    expect(versionAt(vs, "2026-09-30T13:00:00.000Z")).toBe(3);
  });

  it("shows an address waiting for a live page's next snapshot", () => {
    const t = { ...T({ comments: [C("1", "viewer", "alex", "2026-10-05T10:00:00.000Z")] }), addressed_pending: { harness: "claude", at: "2026-10-05T10:05:00.000Z" } };
    const h = historyOf(t, [], by => by);
    expect(h.at(-1)).toEqual({ v: null, who: "claude", agent: true, verb: "addressed it · waiting for a snapshot" });
    expect(historyOf({ ...t, status: "resolved" }, [], by => by).some(e => e.verb.includes("waiting"))).toBe(false);
  });

  it("reads comments, replies, an agent's reply without a tag, and the resolve", () => {
    const t = T({
      comments: [C("1", "viewer", "alex", "2026-09-30T10:30:00.000Z"), C("2", "viewer", "Mia", "2026-09-30T11:10:00.000Z"), C("3", "agent", "Agent", "2026-09-30T11:20:00.000Z")],
      status: "resolved", resolved_by: "viewer:u_1", resolved_at: "2026-09-30T12:10:00.000Z",
    });
    expect(historyOf(t, vs, names)).toEqual([
      { v: 1, who: "alex", agent: false, verb: "commented" },
      { v: 2, who: "Mia", agent: false, verb: "replied" },
      { v: null, who: "claude", agent: true, verb: "replied" },
      { v: 3, who: "alex", agent: false, verb: "resolved" },
    ]);
  });

  it("puts each version that addressed the thread in time order, and labels the agent's reply with it", () => {
    const withAgent = vs.map(v => ({ ...v, agent: "a_1", agent_harness: "claude" }));
    const reply = C("2", "agent", "Agent", "2026-09-30T11:20:00.000Z");
    const t = T({ comments: [C("1", "viewer", "alex", "2026-09-30T10:30:00.000Z"), reply], addressed_in: [3] });
    expect(historyOf(t, withAgent, names)).toEqual([
      { v: 1, who: "alex", agent: false, verb: "commented" },
      { v: null, who: "claude", agent: true, verb: "replied" },
      { v: 3, who: "claude", agent: true, verb: "addressed it" },
    ]);
    expect(addressedNote(t, reply, withAgent)).toBe(3);
    expect(addressedNote(t, t.comments[0], withAgent)).toBeNull();
  });

  it("reads a batch send with how many others went with it and its note", () => {
    const t = T({
      comments: [C("c1", "viewer", "alex", "2026-09-30T10:30:00.000Z")],
      sends: [
        { batch_id: "b1", size: 3, note: "Before the demo", sent_by: "alex", sent_at: "2026-09-30T11:10:00.000Z" },
        { batch_id: "b2", size: 2, note: null, sent_by: "Mia", sent_at: "2026-09-30T12:10:00.000Z" },
        { batch_id: "b3", size: 1, note: null, sent_by: "Mia", sent_at: "2026-09-30T12:20:00.000Z" },
      ],
    });
    expect(historyOf(t, vs, names).slice(1)).toEqual([
      { v: 2, who: "alex", agent: false, verb: "sent it with 2 others · “Before the demo”" },
      { v: 3, who: "Mia", agent: false, verb: "sent it with 1 other" },
      { v: 3, who: "Mia", agent: false, verb: "sent it" },
    ]);
  });

  it("tags the opening comment with the version it was made on, even when a newer one was out", () => {
    // v2 was published at 11:00; the viewer, still on v1, commented at 11:30.
    const t = T({ version_n: 1, comments: [C("1", "viewer", "alex", "2026-09-30T11:30:00.000Z"), C("2", "viewer", "Mia", "2026-09-30T11:40:00.000Z")] });
    expect(historyOf(t, vs, names)).toEqual([
      { v: 1, who: "alex", agent: false, verb: "commented" },
      { v: 2, who: "Mia", agent: false, verb: "replied" },
    ]);
  });

  it("calls a thread outdated when a later version changed its element but still has it", () => {
    const found = (method: "exact" | "selector" | "quote" | "custom") => ({ id: "t", found: true, method, rect: null });
    expect(isOutdated(T({}), found("selector"), 2)).toBe(true);
    expect(isOutdated(T({}), found("quote"), 2)).toBe(true);
    expect(isOutdated(T({}), found("exact"), 2)).toBe(false);
    expect(isOutdated(T({}), found("selector"), 1)).toBe(false);
    expect(isOutdated(T({}), { id: "t", found: false, method: null, rect: null }, 2)).toBe(false);
    expect(isOutdated(T({ anchor: { ...T({}).anchor, html_hash: null } }), found("selector"), 2)).toBe(false);
    expect(isOutdated(T({}), found("custom"), 2)).toBe(false);
  });
});
