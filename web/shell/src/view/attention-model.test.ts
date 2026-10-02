import { describe, expect, it } from "vitest";
import type { Artifact, AttentionSummary } from "../api";
import { groups, markers, needsEyes, seenText } from "./attention-model";

const A = (id: string, over: Partial<Artifact> = {}): Artifact => ({ id, title: id, description: null, icon: null, updated_at: "2026-09-30T10:00:00Z", current_version: 5, pinned: false, ...over });
const S = (over: Partial<AttentionSummary> = {}): AttentionSummary => ({ addressed: [], addressed_v: null, new_replies: [], open_in: [], seen: 5, ...over });

describe("attention-model", () => {
  it("needs your eyes for an address, a version you have not seen, or a reply", () => {
    expect(needsEyes(A("a"), S({ addressed: ["t"], addressed_v: 5 }))).toBe(true);
    expect(needsEyes(A("a"), S({ seen: 4 }))).toBe(true);
    expect(needsEyes(A("a"), S({ new_replies: ["t"] }))).toBe(true);
    expect(needsEyes(A("a"), S({ open_in: ["t"] }))).toBe(false);
    expect(needsEyes(A("a"), S({ seen: null }))).toBe(false);
    expect(needsEyes(A("a"), undefined)).toBe(false);
  });

  it("orders markers: addressed, new version, replies, working, open", () => {
    expect(markers(A("a"), S({ addressed: ["t"], addressed_v: 5, seen: 4, new_replies: ["t", "u"], open_in: ["t", "u"] }), ["claude working on 2"])).toEqual([
      { kind: "you", text: "1 addressed in v5" }, { kind: "new", text: "v5 new" }, { kind: "rep", text: "2 new replies" },
      { kind: "ag", text: "claude working on 2" }, { kind: "oth", text: "2 open" },
    ]);
    expect(markers(A("a"), S({ new_replies: ["t"] }), [])).toEqual([{ kind: "rep", text: "1 new reply" }]);
  });

  it("groups needs first by recency, then the rest pinned first", () => {
    const list = [A("old", { updated_at: "2026-09-01" }), A("pin", { pinned: true, updated_at: "2026-08-01" }), A("eyes", { updated_at: "2026-09-02" }), A("new", { updated_at: "2026-09-30" })];
    const g = groups(list, { eyes: S({ seen: 4 }) });
    expect(g.needs.map(a => a.id)).toEqual(["eyes"]);
    expect(g.rest.map(a => a.id)).toEqual(["pin", "new", "old"]);
  });

  it("says which version you last saw", () => {
    expect([seenText(S({ seen: 4 })), seenText(S({ seen: null })), seenText(undefined)]).toEqual(["seen v4", null, null]);
  });
});
