import { describe, expect, it } from "vitest";
import type { Attention, Version } from "../api";
import type { Thread } from "../threads";
import { decide } from "./changelog-model";
import { excerpt, versionRows } from "./version-rows";

const V = (n: number, addresses: string[] = [], note: string | null = null): Version =>
  ({ artifact_id: "a", n, label: null, created_at: `2026-09-30T1${n}:00:00.000Z`, files: {}, note, addresses, agent: "a_1", agent_harness: "claude" });
const A = (over: Partial<Attention>): Attention => ({ addressed: [], addressed_v: null, new_replies: [], open_in: [], seen: null, looked: {}, ...over });
const T = (id: string, status: "open" | "resolved" = "open", extra: Partial<Thread> = {}): Thread => ({
  id, artifact_id: "a", version_n: 1, status, sent_to_agent: true, has_clip: false, clip_url: null, created_at: "2026-09-30T10:30:00.000Z", resolved_at: null,
  resolved_by: null, feedback_state: null, comments: [{ id: `${id}c`, thread_id: id, author_kind: "viewer", author_name: "alex", author_public_id: "u_me", via_harness: null, body: "Make this  two\ncolumns", created_at: "2026-09-30T10:30:00.000Z" }],
  anchor: { kind: "element", selector: "h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }, ...extra,
});

describe("decide", () => {
  const vs = [V(1), V(2, ["t1"]), V(3, ["t1", "t2", "t3"], "Two columns")];
  it("holds the newest version's addressed threads you are in and have not looked at", () => {
    expect(decide(vs, 3, A({ addressed: ["t1", "t3", "t9"], seen: 2 }), false)).toEqual({ n: 3, ids: ["t1", "t3"], dot: true, line: "v3 addressed 2" });
  });
  it("summarises a return after several versions", () => {
    expect(decide(vs, 3, A({ addressed: ["t1", "t2"], seen: 1 }), false).line).toBe("2 new versions · 2 addressed");
  });
  it("shows no dot on a first visit or once seen, and nothing for a pinned or anonymous view", () => {
    expect(decide(vs, 3, A({ seen: null }), false).dot).toBe(false);
    expect(decide(vs, 3, A({ seen: 3 }), false)).toEqual({ n: 3, ids: [], dot: false, line: null });
    expect(decide(vs, 3, A({ addressed: ["t1"], seen: 2 }), true)).toEqual({ n: 3, ids: [], dot: false, line: null });
    expect(decide(vs, 3, null, false)).toEqual({ n: 3, ids: [], dot: false, line: null });
  });
});

describe("versionRows and excerpt", () => {
  it("lists versions newest first with who, the threads addressed, what you did, and the note", () => {
    const resolved = T("t1", "resolved", { resolved_by: "viewer:u_me", resolved_at: "2026-09-30T12:30:00.000Z" });
    const replied = T("t2", "open", { comments: [...T("t2").comments, { id: "r", thread_id: "t2", author_kind: "viewer", author_name: "alex", author_public_id: "u_me", via_harness: null, body: "not yet", created_at: "2026-09-30T12:40:00.000Z" }] });
    const rows = versionRows({ versions: [V(1), V(2, ["t1", "t2"], "Units: ms")], latest: 2, shown: 2, now: new Date("2026-09-30T12:45:00.000Z"),
      threads: [resolved, replied], numbers: new Map([["t2", 2]]), me: "u_me" });
    expect(rows[0]).toMatchObject({ n: 2, current: true, latest: true, who: "claude", chips: [{ id: "t1", n: null, open: false }, { id: "t2", n: 2, open: true }],
      did: "you resolved it; you replied on #2, still open", note: "Units: ms" });
    expect(rows[1]).toMatchObject({ n: 1, chips: [], did: null, note: "First publish" });
    expect(excerpt(T("t"))).toBe("Make this two columns");
  });
});
