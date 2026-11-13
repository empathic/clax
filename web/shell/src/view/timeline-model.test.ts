import { describe, expect, it } from "vitest";
import type { Version } from "../api";
import type { Comment, FeedbackState, Thread, Viewer } from "../threads";
import { type Day, type Line, type ThreadState, type Timeline, daySummary, days, isPending, isWaiting, lineText, rowStatus, timeline } from "./timeline-model";
import type { Working } from "./working-model";

const AT = (min: number, sec = 0) => new Date(Date.UTC(2026, 10, 13, 10, min, sec)).toISOString();
const me: Viewer = { public_id: "p_me", display_name: "Alex", created_at: AT(0) };
const C = (id: string, kind: "viewer" | "agent", at: string, who = kind === "agent" ? "claude" : "Mia"): Comment => ({
  id, thread_id: id.split(".")[0], author_kind: kind, author_name: who, author_public_id: kind === "viewer" ? (who === "Alex" ? "p_me" : `p_${who}`) : null,
  via_harness: kind === "agent" ? "claude" : null, body: "x", created_at: at,
});
const thread = (id: string, o: Partial<Thread> = {}): Thread => {
  const comments = o.comments ?? [C(`${id}.1`, "viewer", AT(1))];
  return {
    id, artifact_id: "a", version_n: 1, status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: comments[0].created_at,
    resolved_at: null, resolved_by: null, feedback_state: null,
    anchor: { kind: "element", selector: "h2", quote: null, prefix: null, suffix: null, html_hash: "h", rect: null, custom_name: null, file: "index.html" },
    ...o, comments,
  };
};
const version = (n: number, at: string, addresses: string[] = [], note: string | null = null): Version =>
  ({ artifact_id: "a", n, label: null, created_at: at, files: {}, agent_harness: "claude", note, addresses });
const send = (batch_id: string, sent_at: string, size: number, note: string | null = null, sent_by = "Alex") => ({ batch_id, size, note, sent_by, sent_at });
const v1 = version(1, AT(0));
const run = (threads: Thread[], versions: Version[] = [v1], more: { working?: Working[]; claimed?: string[]; me?: Viewer | null } = {}) =>
  timeline({ threads, versions, working: more.working ?? [], claimed: more.claimed ?? [], me: "me" in more ? more.me : me });
const shape = (tl: Timeline) => tl.lines.map(l => [l.kind, l.ids, l.here, l.pending, l.waiting]);
const V1 = ["version", [], [], 0, 0];

describe("timeline", () => {
  it("reads an unsent comment as one made line that waits on a person", () => {
    const tl = run([thread("a")]);
    expect(shape(tl)).toEqual([V1, ["made", ["a"], ["a"], 1, 1]]);
    expect(tl.stateOf.get("a")).toEqual({ kind: "unsent" });
    expect(tl.pending).toEqual(["a"]);
    expect(tl.waiting).toEqual(["a"]);
  });

  it("puts a thread sent without a record with the agent from the moment it was made, with no sent line", () => {
    const tl = run([thread("a", { sent_to_agent: true })]);
    expect(shape(tl)).toEqual([V1, ["made", ["a"], ["a"], 1, 0]]);
    expect(tl.stateOf.get("a")).toEqual({ kind: "with-agent", working: false });
    expect(tl.waiting).toEqual([]);
  });

  it("does the same for a thread ClaxBot claimed", () => {
    const tl = run([thread("a")], [v1], { claimed: ["a"] });
    expect(shape(tl)).toEqual([V1, ["made", ["a"], ["a"], 1, 0]]);
    expect(tl.stateOf.get("a")).toEqual({ kind: "with-agent", working: false });
  });

  it("folds a batch of three made comments into one sent line with the batch's size and note", () => {
    const b = send("b1", AT(4), 3, "Before the demo");
    const tl = run(["a", "b", "c"].map((id, i) => thread(id, { comments: [C(`${id}.1`, "viewer", AT(i + 1))], sent_to_agent: true, sends: [b] })));
    expect(shape(tl)).toEqual([V1, ["sent", ["a", "b", "c"], ["a", "b", "c"], 3, 0]]);
    expect(tl.lines[1]).toMatchObject({ who: "Alex", mine: true, size: 3, note: "Before the demo", at: AT(4) });
  });

  it("keeps a made line of its own when another line lies between it and its send", () => {
    const tl = run([thread("a", { sent_to_agent: true, sends: [send("b1", AT(3), 1)] })], [v1, version(2, AT(2))]);
    expect(shape(tl)).toEqual([V1, ["made", ["a"], [], 1, 1], ["version", [], [], 1, 1], ["sent", ["a"], ["a"], 1, 0]]);
  });

  it("reads an agent's comment just before or just after the version that addressed the thread as that version's answer", () => {
    for (const reply of [AT(4, 58), AT(5, 2)]) {
      const a = thread("a", { comments: [C("a.1", "viewer", AT(1)), C("a.2", "agent", reply)], sent_to_agent: true, sends: [send("b1", AT(2), 1)], addressed_in: [2] });
      const tl = run([a], [v1, version(2, AT(5), ["a"])]);
      expect(shape(tl)).toEqual([V1, ["sent", ["a"], [], 1, 0], ["version", ["a"], ["a"], 0, 0]]);
      expect(tl.stateOf.get("a")).toEqual({ kind: "addressed", v: 2 });
      expect(tl.pending).toEqual([]);
    }
  });

  it("reads an agent's answer without a version as waiting on a person, a reply as back with the agent, and a version as addressed", () => {
    const comments = [C("a.1", "viewer", AT(1)), C("a.2", "agent", AT(3)), C("a.3", "agent", AT(3, 30))];
    const base = { sent_to_agent: true, sends: [send("b1", AT(2), 1)] };
    const answered = run([thread("a", { ...base, comments })]);
    expect(shape(answered)).toEqual([V1, ["sent", ["a"], [], 1, 0], ["answered", ["a"], ["a"], 1, 1]]);
    expect(answered.lines[2]).toMatchObject({ who: "claude", agent: true });
    expect(answered.stateOf.get("a")).toEqual({ kind: "answered", agent: "claude" });
    expect(answered.waiting).toEqual(["a"]);

    const replied = run([thread("a", { ...base, comments: [...comments, C("a.4", "viewer", AT(4))] })]);
    expect(shape(replied)).toEqual([V1, ["sent", ["a"], [], 1, 0], ["answered", ["a"], [], 1, 1], ["followed-up", ["a"], ["a"], 1, 0]]);
    expect(replied.stateOf.get("a")).toEqual({ kind: "with-agent", working: false });

    const addressed = run([thread("a", { ...base, comments: [...comments, C("a.4", "viewer", AT(4))], addressed_in: [2] })], [v1, version(2, AT(5), ["a"])]);
    expect(shape(addressed)).toEqual([V1, ["sent", ["a"], [], 1, 0], ["answered", ["a"], [], 1, 1], ["followed-up", ["a"], [], 1, 0], ["version", ["a"], ["a"], 0, 0]]);
    expect(addressed.stateOf.get("a")).toEqual({ kind: "addressed", v: 2 });
  });

  it("makes a thread pending again, with the agent, when a person comments after it was addressed", () => {
    const a = thread("a", { comments: [C("a.1", "viewer", AT(1)), C("a.2", "viewer", AT(4))], sent_to_agent: true, sends: [send("b1", AT(2), 1)], addressed_in: [2] });
    const tl = run([a], [v1, version(2, AT(3), ["a"])]);
    expect(shape(tl)).toEqual([V1, ["sent", ["a"], [], 1, 0], ["version", ["a"], [], 0, 0], ["followed-up", ["a"], ["a"], 1, 0]]);
    expect(tl.stateOf.get("a")).toEqual({ kind: "with-agent", working: false });
    expect(tl.pending).toEqual(["a"]);
  });

  it("writes a resolved line for a person's resolve and an agent's, the viewer's own marked mine", () => {
    const tl = run([
      thread("a", { status: "resolved", resolved_by: "viewer:p_me", resolved_at: AT(3) }),
      thread("b", { comments: [C("b.1", "viewer", AT(2))], status: "resolved", resolved_by: "agent:claude", resolved_at: AT(4) }),
    ]);
    expect(shape(tl)).toEqual([V1, ["made", ["a", "b"], [], 2, 2], ["resolved", ["a"], ["a"], 1, 1], ["resolved", ["b"], ["b"], 0, 0]]);
    expect(tl.lines.slice(2).map(l => [l.who, l.agent, l.mine])).toEqual([["Alex", false, true], ["claude", true, false]]);
    expect(tl.stateOf.get("a")).toEqual({ kind: "resolved" });
  });

  it("lists two threads one version addressed under its one line", () => {
    const ts = ["a", "b"].map((id, i) => thread(id, { comments: [C(`${id}.1`, "viewer", AT(i + 1))], sent_to_agent: true, addressed_in: [2] }));
    const tl = run(ts, [v1, version(2, AT(3), ["a", "b"], "Tighter header")]);
    expect(shape(tl)).toEqual([V1, ["made", ["a", "b"], [], 2, 0], ["version", ["a", "b"], ["a", "b"], 0, 0]]);
    expect(tl.lines[2]).toMatchObject({ who: "claude", agent: true, v: 2, note: "Tighter header" });
  });

  it("lists a thread only under its newest line", () => {
    const a = thread("a", { sent_to_agent: true, sends: [send("b1", AT(2), 1)], addressed_in: [2] });
    const tl = run([a], [v1, version(2, AT(3), ["a"])]);
    expect(tl.lines.filter(l => l.here.includes("a")).map(l => l.kind)).toEqual(["version"]);
    expect(tl.lines.filter(l => l.ids.includes("a")).map(l => l.kind)).toEqual(["sent", "version"]);
  });

  it("skips a version number no known version has", () => {
    const tl = run([thread("a", { sent_to_agent: true, addressed_in: [9] })]);
    expect(shape(tl)).toEqual([V1, ["made", ["a"], ["a"], 1, 0]]);
    expect(tl.stateOf.get("a")).toEqual({ kind: "with-agent", working: false });
  });

  it("merges two threads made at the same time by the same author, and keys every line uniquely", () => {
    const tl = run([
      thread("a"), thread("b"),
      thread("c", { comments: [C("c.1", "viewer", AT(2)), C("c.2", "viewer", AT(3)), C("c.3", "viewer", AT(3))], status: "resolved", resolved_by: "viewer:p_me", resolved_at: AT(4) }),
    ]);
    expect(shape(tl)).toEqual([V1, ["made", ["a", "b", "c"], ["a", "b"], 3, 3], ["followed-up", ["c"], [], 3, 3], ["resolved", ["c"], ["c"], 2, 2]]);
    const keys = tl.lines.map(l => l.key);
    expect(new Set(keys).size).toBe(keys.length);
  });

  it("marks a send mine only when the viewer has a display name equal to its sender, and a comment mine by public ID", () => {
    const ts = [thread("a", { comments: [C("a.1", "viewer", AT(1), "Alex")] }), thread("b", { comments: [C("b.1", "viewer", AT(2))], sent_to_agent: true, sends: [send("b1", AT(4), 1)] })];
    for (const who of [null, { ...me, display_name: null }]) {
      const tl = run(ts, [v1, version(2, AT(3))], { me: who });
      expect(tl.lines.find(l => l.kind === "sent")).toMatchObject({ who: "Alex", mine: false });
      expect(tl.lines.find(l => l.kind === "made" && l.ids.includes("a"))?.mine).toBe(who !== null);
    }
    expect(run(ts, [v1, version(2, AT(3))]).lines.find(l => l.kind === "sent")?.mine).toBe(true);
  });

  it("refines the present state of a thread with the agent by who works on it now and by a stalled delivery", () => {
    const w: Working = { key: "k", agent: "a_1", harness: "claude", message: null, thread_ids: ["a"], started_at: AT(5), last_heartbeat: AT(5) };
    expect(run([thread("a", { sent_to_agent: true })], [v1], { working: [w] }).stateOf.get("a")).toEqual({ kind: "with-agent", working: true });
    const states = ["withheld", "declined", "no_access", "agent_ended"] as const;
    const fs = (id: string, state: FeedbackState["state"]): FeedbackState => ({ thread_id: id, state, tier: null, since: AT(2), resends: 0, exhausted: false });
    const tl = run(states.map((s, i) => thread(`t${i}`, { sent_to_agent: true, feedback_state: fs(`t${i}`, s) })));
    expect(states.map((_, i) => tl.stateOf.get(`t${i}`))).toEqual(states.map(why => ({ kind: "stalled", why })));
    expect(tl.waiting.length).toBe(4);
    expect(tl.pending.length).toBe(4);
    // A delivered thread stays with the agent.
    expect(run([thread("a", { sent_to_agent: true, feedback_state: fs("a", "delivered") })]).stateOf.get("a")).toEqual({ kind: "with-agent", working: false });
  });

  it("lists pending and waiting threads newest first", () => {
    const tl = run([thread("a"), thread("b", { comments: [C("b.1", "viewer", AT(3), "Alex")] }), thread("c", { comments: [C("c.1", "viewer", AT(2))], sent_to_agent: true })]);
    expect(tl.pending).toEqual(["b", "c", "a"]);
    expect(tl.waiting).toEqual(["b", "a"]);
  });

  it("keeps every line's key when a newer thread adds a line", () => {
    const before = [
      thread("a", { sent_to_agent: true, sends: [send("b1", AT(2), 1)], addressed_in: [2] }),
      thread("b", { comments: [C("b.1", "viewer", AT(4))] }),
    ];
    const versions = [v1, version(2, AT(3), ["a"])];
    const keys = run(before, versions).lines.map(l => l.key);
    // The newer thread's made line merges into b's: the merged line keeps b's key.
    const after = run([...before, thread("c", { comments: [C("c.1", "viewer", AT(6))] })], versions).lines.map(l => l.key);
    expect(after).toEqual(expect.arrayContaining(keys));
    const later = run([...before, thread("d", { comments: [C("d.1", "viewer", AT(7), "Alex")] })], versions).lines.map(l => l.key);
    expect(later).toEqual(expect.arrayContaining(keys));
    expect(later.length).toBe(keys.length + 1);
  });

  it("agrees with the batch bar on unsent: a thread no agent has stays unsent though a version names it", () => {
    const tl = run([thread("a", { addressed_in: [2] })], [v1, version(2, AT(3), ["a"])]);
    expect(tl.stateOf.get("a")).toEqual({ kind: "unsent" });
  });
});

const L = (o: Partial<Line>): Line => ({ key: "k", at: AT(1), kind: "made", who: "Mia", agent: false, mine: false, ids: ["a"], here: [], pending: 0, waiting: 0, ...o });

describe("lineText", () => {
  const none = new Map<string, ThreadState>();
  it("words a version by its note, else by who published it", () => {
    expect(lineText(L({ kind: "version", v: 6, note: "Tighter header", who: "claude", agent: true }), none)).toEqual({ text: "v6 · Tighter header" });
    expect(lineText(L({ kind: "version", v: 6, note: null, who: "claude", agent: true }), none)).toEqual({ text: "v6 · published by claude" });
  });
  it("words a made line, and says it was sent to no one while a thread listed under it is unsent", () => {
    expect(lineText(L({ mine: true, who: "Alex" }), none).text).toBe("You commented");
    expect(lineText(L({}), none).text).toBe("Mia commented");
    expect(lineText(L({ mine: true, who: "Alex", ids: ["a", "b", "c"] }), none).text).toBe("You made 3 comments");
    const unsent = new Map<string, ThreadState>([["a", { kind: "unsent" }]]);
    expect(lineText(L({ here: ["a"] }), unsent).text).toBe("Mia commented, sent to no one");
    // Not listed under it, or no longer unsent: no clause.
    expect(lineText(L({ here: [] }), unsent).text).toBe("Mia commented");
    expect(lineText(L({ here: ["a"] }), new Map([["a", { kind: "with-agent", working: false }]])).text).toBe("Mia commented");
  });
  it("words a send by its batch's size, with its note on a second row", () => {
    expect(lineText(L({ kind: "sent", mine: true, who: "Alex", size: 4, note: "Before the demo", ids: ["a", "b"] }), none)).toEqual({ text: "You sent 4 comments", sub: "Before the demo" });
    expect(lineText(L({ kind: "sent", who: "Mia", size: 1, note: null }), none)).toEqual({ text: "Mia sent a comment" });
  });
  it("words an answer, a follow-up and a resolve", () => {
    expect(lineText(L({ kind: "answered", who: "claude", agent: true }), none).text).toBe("claude replied");
    expect(lineText(L({ kind: "followed-up", mine: true, who: "Alex" }), none).text).toBe("You replied");
    expect(lineText(L({ kind: "followed-up" }), none).text).toBe("Mia replied");
    expect(lineText(L({ kind: "followed-up", ids: ["a", "b"] }), none).text).toBe("Mia replied on 2 comments");
    expect(lineText(L({ kind: "resolved", mine: true, who: "Alex", ids: ["a", "b"] }), none).text).toBe("You resolved 2 comments");
    expect(lineText(L({ kind: "resolved", who: "claude", agent: true }), none).text).toBe("claude resolved a comment");
  });
});

describe("rowStatus", () => {
  it("words each state and asks the viewer to act only where the next move is theirs", () => {
    const cases: [ThreadState, string, boolean][] = [
      [{ kind: "addressed", v: 6 }, "✓ v6", false],
      [{ kind: "unsent" }, "Send", true],
      [{ kind: "answered", agent: "claude" }, "Reply", true],
      [{ kind: "with-agent", working: true }, "working", false],
      [{ kind: "with-agent", working: false }, "sent", false],
      [{ kind: "resolved" }, "resolved", false],
      [{ kind: "stalled", why: "withheld" }, "approve", true],
      [{ kind: "stalled", why: "declined" }, "declined", true],
      [{ kind: "stalled", why: "no_access" }, "no access", true],
      [{ kind: "stalled", why: "agent_ended" }, "no agent", true],
    ];
    expect(cases.map(([s]) => rowStatus(s))).toEqual(cases.map(([, text, act]) => ({ text, act })));
    expect(cases.map(([s]) => [isPending(s), isWaiting(s)])).toEqual([
      [false, false], [true, true], [true, true], [true, false], [true, false], [false, false], [true, true], [true, true], [true, true], [true, true],
    ]);
  });
});

describe("days", () => {
  // Local-time constructors: the cases hold in any time zone. 13 November 2026 is a Friday.
  const now = new Date(2026, 10, 13, 9, 0);
  const local = (d: number, h: number, m = 0) => new Date(2026, 10, d, h, m).toISOString();
  it("groups lines by local day, newest day and newest line first", () => {
    const lines = [
      L({ key: "1", at: local(10, 12) }), L({ key: "2", at: local(12, 8) }), L({ key: "3", at: local(12, 23, 50) }),
      L({ key: "4", at: local(13, 0, 10) }), L({ key: "5", at: local(13, 8, 59) }),
    ];
    const ds = days(lines, now);
    expect(ds.map(d => [d.label, d.date, d.lines.map(l => l.key)])).toEqual([
      ["Today", "2026-11-13", ["5", "4"]],
      ["Yesterday", "2026-11-12", ["3", "2"]],
      ["Tuesday 10 Nov", "2026-11-10", ["1"]],
    ]);
    expect(days([L({ at: new Date(2025, 11, 31, 12).toISOString() })], now)[0].label).toBe("Wednesday 31 Dec 2025");
    expect(days([], now)).toEqual([]);
  });
  it("sums a folded day up by its weekday, versions, addressed threads, comments and resolves", () => {
    const d: Day = { label: "Thursday 12 Nov", date: "2026-11-12", lines: [
      L({ kind: "version", v: 4, ids: ["a", "b"] }), L({ kind: "version", v: 3, ids: ["a", "c"] }), L({ kind: "version", v: 2, ids: [] }),
      L({ kind: "made", ids: ["d"] }), L({ kind: "resolved", ids: ["a", "b"] }),
    ] };
    expect(daySummary(d)).toBe("Thursday · 3 versions · 3 addressed · 1 comment · 2 resolved");
    expect(daySummary({ ...d, label: "Yesterday", lines: [L({ kind: "version", v: 2, ids: ["a"] })] })).toBe("Yesterday · 1 version · 1 addressed");
  });
});
