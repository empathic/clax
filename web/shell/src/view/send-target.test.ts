import { afterEach, describe, expect, it } from "vitest";
import { ownTarget, sendsFor } from "./send-menu";
import { defaultTarget, liveAgents, rememberTarget } from "./send-target";

// As the daemon lists them: live first, most recently active first.
const agents = [{ handle: "a_cx", harness: "codex", live: true }, { handle: "a_cl", harness: "claude", live: true }, { handle: "a_old", harness: "pi", live: false }];
afterEach(() => localStorage.clear());

describe("send-target", () => {
  it("names the agent you last chose or sent to while it is live, else none", () => {
    expect(defaultTarget("x", agents)).toBeNull();
    rememberTarget("x", "a_cl");
    expect(defaultTarget("x", agents)).toBe("a_cl");
    rememberTarget("x", "a_old");
    expect(defaultTarget("x", agents)).toBeNull(); // the agent you last sent to has ended
    expect(liveAgents(agents).map(a => a.handle)).toEqual(["a_cx", "a_cl"]);
  });
  it("names no target when no agent is live, so the send goes without to", () => {
    rememberTarget("x", "a_old");
    expect(defaultTarget("x", [{ handle: "a_old", harness: "pi", live: false }])).toBeNull();
    expect(defaultTarget("x", [])).toBeNull();
  });
});

describe("ownTarget (spec 2026-10-08 §10)", () => {
  const SAM = { public_id: "u_sam", name: "Sam", owner: false };
  const ALEX = { public_id: "u_alex", name: "Alex", owner: true };
  // Sam's agent is the most recently active, so the daemon lists it first.
  const mixed = [{ handle: "a_sam", harness: "claude", live: true, person: SAM }, { handle: "a_mine", harness: "codex", live: true, person: ALEX }];
  it("is this viewer's own most recent live agent, never another person's listed first", () => {
    expect(ownTarget(mixed, "u_alex")).toBe("a_mine");
    expect(ownTarget(mixed, "u_sam")).toBe("a_sam");
    // A daemon from before people: the most recently active live agent, as before.
    expect(ownTarget(agents, "u_alex")).toBe("a_cx");
  });
  it("is none when only other people's agents are live, or before this viewer is known", () => {
    expect(ownTarget([mixed[0]], "u_alex")).toBeNull();
    expect(ownTarget(mixed, undefined)).toBeNull();
    expect(ownTarget(mixed, "u_lan")).toBeNull();
  });
});

describe("sendsFor (spec 2026-10-08 §10, O6)", () => {
  const ALEX = { public_id: "u_alex", name: "Alex", owner: true };
  const SAM = { public_id: "u_sam", name: "Sam", owner: false };
  const both = [{ handle: "a_sam", harness: "claude", live: true, person: SAM }, { handle: "a_mine", harness: "claude", live: true, person: ALEX }];
  const v = (o: object) => ({ public_id: "u_x", display_name: null, created_at: "", ...o });
  it("is the viewer's own public ID for the owner's and a connected person's browser", () => {
    expect(sendsFor(both, v({ public_id: "u_alex", owner: true }))).toBe("u_alex");
    expect(sendsFor(both, v({ public_id: "u_sam", person: true }))).toBe("u_sam");
  });
  it("is the owner's for a LAN viewer, so one press goes to the owner's newest live agent", () => {
    expect(sendsFor(both, v({ public_id: "u_lan" }))).toBe("u_alex");
    expect(ownTarget(both, sendsFor(both, v({ public_id: "u_lan" })))).toBe("a_mine");
    // With only other people's agents, none: the menu opens.
    expect(ownTarget([both[0]], sendsFor([both[0]], v({ public_id: "u_lan" })))).toBeNull();
  });
  it("is unknown before the viewer is known", () => {
    expect(sendsFor(both, null)).toBeUndefined();
    expect(ownTarget(both, sendsFor(both, undefined))).toBeNull();
  });
});
