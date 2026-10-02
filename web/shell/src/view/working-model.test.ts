import { describe, expect, it } from "vitest";
import { agentNames, chips, clock, stripText, summary, threadMarker, type Working } from "./working-model";

const w = (over: Partial<Working>): Working => ({
  key: "k", agent: "a_1111aaaa", harness: "claude", message: null, thread_ids: [], started_at: "2026-09-30T10:00:00.000Z", last_heartbeat: "2026-09-30T10:00:00.000Z", ...over,
});
const now = new Date("2026-09-30T10:00:42.000Z");
const agents = [{ handle: "a_1111aaaa", harness: "claude", live: true }, { handle: "a_2222bbbb", harness: "codex", live: true }];

describe("working-model", () => {
  it("names agents by harness, and tells two of one harness apart", () => {
    expect(agentNames([w({})], agents).get("a_1111aaaa")).toBe("claude");
    const two = agentNames([w({}), w({ agent: "a_3333cccc" })], [...agents, { handle: "a_3333cccc", harness: "claude", live: true }]);
    expect([two.get("a_1111aaaa"), two.get("a_3333cccc")]).toEqual(["claude 1111", "claude 3333"]);
  });

  it("reads the summary for one agent, all yours, some yours, a message, and nobody", () => {
    const names = agentNames([w({})], agents);
    const base = { names, open: 3, idle: [], addressed: null, published: null, now };
    expect(summary({ ...base, working: [w({ thread_ids: ["a", "b"] })], mine: new Set(["a", "b"]) }))
      .toEqual({ line1: "claude working on 2", agent: true, line2: "all yours", elapsed: "0:42" });
    expect(summary({ ...base, working: [w({ thread_ids: ["a", "b"] })], mine: new Set(["a"]) }).line2).toBe("1 yours");
    expect(summary({ ...base, working: [w({ message: "Rebuilding the chart" })], mine: new Set() }).line1).toBe("claude: Rebuilding the chart");
    expect(summary({ ...base, working: [], mine: new Set(), idle: ["codex"] })).toEqual({ line1: "Nobody working", agent: false, line2: "3 open threads · codex idle", elapsed: null });
    expect(summary({ ...base, working: [], mine: new Set(), addressed: "v5 addressed 3" })).toEqual({ line1: "v5 addressed 3", agent: false, line2: "yours, not looked at yet", elapsed: null });
    expect(summary({ ...base, working: [], mine: new Set(), published: 3 })).toEqual({ line1: "v3 published", agent: false, line2: "reload to see it", elapsed: null });
  });

  it("lists several agents and counts distinct threads", () => {
    const list = [w({ thread_ids: ["a", "b"] }), w({ key: "k2", agent: "a_2222bbbb", harness: "codex", thread_ids: ["b", "c"], started_at: "2026-09-30T10:00:10.000Z" })];
    expect(summary({ working: list, names: agentNames(list, agents), mine: new Set(), open: 3, idle: [], addressed: null, published: null, now }).line1).toBe("codex, claude working on 3");
    expect(chips(list, agentNames(list, agents))).toEqual(["codex working on 2", "claude working on 2"]);
  });

  it("marks a thread and writes the strip", () => {
    const list = [w({ thread_ids: ["t1", "t3"] })];
    const names = agentNames(list, agents);
    expect(threadMarker(list, "t1", names)).toEqual({ text: "claude is working on it", since: "2026-09-30T10:00:00.000Z" });
    expect(threadMarker(list, "t2", names)).toBeNull();
    expect(stripText(list[0], names, new Map([["t1", 1], ["t3", 3]]), new Set(["t1"]))).toBe("claude is working on #1 (yours) and #3");
    expect(stripText(w({ thread_ids: ["x", "y", "z", "q"] }), names, new Map(), new Set())).toBe("claude is working on 4 threads");
  });

  it("formats the clock", () => {
    expect([clock("2026-09-30T10:00:00.000Z", now), clock("2026-09-30T08:59:00.000Z", now)]).toEqual(["0:42", "1:01:42"]);
  });
});
