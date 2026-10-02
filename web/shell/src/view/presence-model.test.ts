import { describe, expect, it } from "vitest";
import { personLine, roster, stateFor } from "./presence-model";

describe("presence-model", () => {
  it("is here while visible and active, away when hidden or idle 5 minutes", () => {
    expect([stateFor(true, 1000), stateFor(false, 0), stateFor(true, 5 * 60_000)]).toEqual(["here", "away", "away"]);
  });
  it("reads a person's line", () => {
    const now = new Date("2026-09-30T10:03:00Z");
    expect(personLine({ public_id: "u", display_name: "Mia", state: "here", where: "«p95 chart»", since: "x" }, now)).toBe("here, looking at «p95 chart»");
    expect(personLine({ public_id: "u", display_name: "Mia", state: "here", where: null, since: "x" }, now)).toBe("here");
    expect(personLine({ public_id: "u", display_name: "Jun", state: "gone", where: null, since: "2026-09-30T10:00:00Z" }, now)).toBe("last here 3 min ago");
  });
  it("lists present viewers who never commented, and no gone ones", () => {
    const p = (id: string, state: "here" | "away" | "gone") => ({ public_id: id, display_name: id, state, where: null, since: "x" });
    expect(roster([], [p("u_a", "here"), p("u_b", "away")]).map(x => x.public_id)).toEqual(["u_a", "u_b"]);
    expect(roster([{ public_id: "u_a", display_name: "A", seen: 2 }], [p("u_a", "here"), p("u_c", "gone")])).toEqual([{ public_id: "u_a", display_name: "A", seen: 2 }]);
  });
});
